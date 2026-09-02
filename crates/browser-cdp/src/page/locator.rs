use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;
use webtest_browser::{BrowserError, CandidateEvidence, Locator, LocatorState};

use super::{
    CdpPage,
    evaluation::{evaluate_expression, evaluation_value, invalid_evaluation},
};

pub(super) async fn resolve(
    page: &CdpPage,
    locator: &Locator,
) -> Result<ResolveSnapshot, BrowserError> {
    let expression = resolver_expression(locator, &page.test_id_attribute)?;
    let result = evaluate_expression(page, expression).await?;
    let value = evaluation_value(&result)
        .ok_or_else(|| invalid_evaluation("locator result was missing"))?;
    let snapshot: ResolveSnapshot = serde_json::from_value(value.clone())
        .map_err(|error| invalid_evaluation(&error.to_string()))?;
    if let Some(message) = snapshot.invalid.clone() {
        return Err(BrowserError::LocatorInvalid {
            locator: locator.clone(),
            message,
        });
    }
    Ok(snapshot)
}

pub(super) async fn scroll_into_view(
    page: &CdpPage,
    locator: &Locator,
) -> Result<(), BrowserError> {
    let expression = scroll_expression(locator, &page.test_id_attribute)?;
    evaluate_expression(page, expression).await.map(|_| ())
}

pub(super) async fn wait_for_locator(
    page: &CdpPage,
    locator: &Locator,
    state: LocatorState,
    deadline: Instant,
) -> Result<(), BrowserError> {
    let mut backoff = Duration::from_millis(20);
    let mut last_snapshot = None;
    loop {
        if Instant::now() >= deadline {
            return Err(locator_deadline_failure(
                locator,
                state,
                last_snapshot.take(),
            ));
        }
        let resolution =
            match super::complete_before_deadline(deadline, resolve(page, locator)).await {
                Ok(resolution) => resolution,
                Err(()) => {
                    return Err(locator_deadline_failure(
                        locator,
                        state,
                        last_snapshot.take(),
                    ));
                }
            };
        match resolution {
            Err(BrowserError::LocatorInvalid { locator, message }) => {
                return Err(BrowserError::LocatorInvalid { locator, message });
            }
            Err(error) => return Err(error),
            Ok(snapshot) => {
                if state_satisfied(&snapshot, state) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(locator_state_failure(locator, state, snapshot));
                }
                last_snapshot = Some(snapshot);
            }
        }
        tokio::time::sleep_until(deadline.min(Instant::now() + backoff)).await;
        backoff = (backoff * 2).min(Duration::from_millis(100));
    }
}

fn locator_deadline_failure(
    locator: &Locator,
    state: LocatorState,
    last_snapshot: Option<ResolveSnapshot>,
) -> BrowserError {
    last_snapshot.map_or_else(
        || BrowserError::AssertionFailed {
            locator: locator.clone(),
            expected: state,
            actual: "the deadline expired before the locator state could be observed".into(),
        },
        |snapshot| locator_state_failure(locator, state, snapshot),
    )
}

fn locator_state_failure(
    locator: &Locator,
    state: LocatorState,
    snapshot: ResolveSnapshot,
) -> BrowserError {
    if snapshot.matches > 1 {
        return BrowserError::LocatorAmbiguous {
            locator: locator.clone(),
            matches: snapshot.matches,
        };
    }
    if snapshot.matches == 0 && !matches!(state, LocatorState::Hidden | LocatorState::Detached) {
        return BrowserError::LocatorNotFound {
            locator: locator.clone(),
        };
    }
    if state == LocatorState::Visible && snapshot.matches == 1 && !snapshot.visible {
        return BrowserError::LocatorNotVisible {
            locator: locator.clone(),
        };
    }
    BrowserError::AssertionFailed {
        locator: locator.clone(),
        expected: state,
        actual: snapshot.state_summary(),
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct ResolveSnapshot {
    pub(super) matches: usize,
    pub(super) invalid: Option<String>,
    pub(super) visible: bool,
    pub(super) disabled: bool,
    pub(super) editable: bool,
    pub(super) checked: Option<bool>,
    pub(super) obscured: bool,
    pub(super) tag: Option<String>,
    pub(super) rect: Option<ElementRect>,
    pub(super) candidates: Vec<CandidateEvidence>,
    #[serde(default, rename = "documentIndex")]
    pub(super) document_index: Option<usize>,
}

impl ResolveSnapshot {
    pub(super) fn center(&self) -> Option<(f64, f64)> {
        self.rect
            .as_ref()
            .map(|rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0))
    }
    fn state_summary(&self) -> String {
        if self.matches == 0 {
            return "detached".into();
        }
        if self.matches > 1 {
            return format!("{} matches", self.matches);
        }
        format!(
            "visible={}, enabled={}, checked={:?}",
            self.visible, !self.disabled, self.checked
        )
    }
    pub(super) fn actionability_facts(&self) -> Vec<String> {
        vec![
            format!("attached={}", self.matches == 1),
            format!("visible={}", self.visible),
            format!("enabled={}", !self.disabled),
            format!("editable={}", self.editable),
            format!("obscured={}", self.obscured),
        ]
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct ElementRect {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

pub(super) fn rect_stable(first: Option<&ElementRect>, second: Option<&ElementRect>) -> bool {
    let (Some(first), Some(second)) = (first, second) else {
        return false;
    };
    (first.x - second.x).abs() < 0.25
        && (first.y - second.y).abs() < 0.25
        && (first.width - second.width).abs() < 0.25
        && (first.height - second.height).abs() < 0.25
}

fn state_satisfied(snapshot: &ResolveSnapshot, state: LocatorState) -> bool {
    match state {
        LocatorState::Hidden => {
            snapshot.matches == 0 || (snapshot.matches == 1 && !snapshot.visible)
        }
        LocatorState::Detached => snapshot.matches == 0,
        LocatorState::Attached => snapshot.matches == 1,
        LocatorState::Visible => snapshot.matches == 1 && snapshot.visible,
        LocatorState::Enabled => snapshot.matches == 1 && !snapshot.disabled,
        LocatorState::Disabled => snapshot.matches == 1 && snapshot.disabled,
        LocatorState::Checked => snapshot.matches == 1 && snapshot.checked == Some(true),
        LocatorState::Unchecked => snapshot.matches == 1 && snapshot.checked == Some(false),
    }
}

fn resolver_expression(locator: &Locator, test_id_attribute: &str) -> Result<String, BrowserError> {
    let elements = locator_array_expression(locator, test_id_attribute)?;
    Ok(format!(
        r#"(() => {{
        const norm = value => String(value || '').replace(/\s+/g, ' ').trim();
        const implicitRole = element => {{
            const tag = element.tagName.toLowerCase();
            if (tag === 'button') return 'button';
            if (tag === 'a' && element.hasAttribute('href')) return 'link';
            if (tag === 'textarea') return 'textbox';
            if (tag === 'select') return 'combobox';
            if (tag === 'input') {{
                const type = (element.type || 'text').toLowerCase();
                if (['button','submit','reset'].includes(type)) return 'button';
                if (type === 'checkbox') return 'checkbox';
                if (type === 'radio') return 'radio';
                if (['text','email','password','search','tel','url'].includes(type)) return 'textbox';
            }}
            return null;
        }};
        const accessibleName = element => {{
            const labelledby = element.getAttribute('aria-labelledby');
            if (labelledby) return norm(labelledby.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' '));
            if (element.hasAttribute('aria-label')) return norm(element.getAttribute('aria-label'));
            if (element.labels?.length) return norm(Array.from(element.labels).map(label => {{
                const copy = label.cloneNode(true);
                copy.querySelectorAll('input,textarea,select,button').forEach(control => control.remove());
                return copy.innerText || copy.textContent;
            }}).join(' '));
            if (element.tagName === 'IMG') return norm(element.alt);
            if (element.tagName === 'INPUT' && ['button','submit','reset'].includes(element.type)) return norm(element.value);
            return norm(element.innerText || element.textContent || element.title);
        }};
        try {{
            const elements = {elements};
            const candidates = elements.slice(0, 5).map(element => {{
                const password = element.tagName === 'INPUT' && element.type === 'password';
                return {{
                    tag: element.tagName.toLowerCase(), id: element.id || null,
                    role: element.getAttribute('role') || implicitRole(element),
                    name: accessibleName(element).slice(0, 120) || null,
                    text: password ? null : norm(element.innerText || '').slice(0, 120) || null
                }};
            }});
            if (elements.length !== 1) return {{matches: elements.length, candidates}};
            const element = elements[0];
            const rect = element.getBoundingClientRect();
            const style = getComputedStyle(element);
            const visible = style.display !== 'none' && style.visibility !== 'hidden'
                && Number(style.opacity) !== 0 && rect.width > 0 && rect.height > 0;
            const disabled = element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true';
            const editable = !disabled && !element.readOnly && (
                element.tagName === 'TEXTAREA' || element.isContentEditable
                || (element.tagName === 'INPUT' && !['button','submit','reset','checkbox','radio','file','hidden'].includes(element.type))
            );
            const checkable = element.matches('input[type=checkbox],input[type=radio],[role=checkbox],[role=radio],[role=switch]');
            const checked = checkable ? (('checked' in element) ? Boolean(element.checked)
                : element.getAttribute('aria-checked') === 'true') : null;
            const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
            const hit = visible ? document.elementFromPoint(x, y) : null;
            const obscured = visible && !(hit === element || element.contains(hit));
            const documentIndex = Array.from(document.querySelectorAll('body *')).indexOf(element);
            return {{matches:1, candidates, visible, disabled, editable, checked, obscured, documentIndex,
                tag: element.tagName.toLowerCase(), rect: {{x:rect.x,y:rect.y,width:rect.width,height:rect.height}}}};
        }} catch (error) {{ return {{matches:0, invalid:String(error)}}; }}
    }})()"#
    ))
}

fn scroll_expression(locator: &Locator, test_id_attribute: &str) -> Result<String, BrowserError> {
    let elements = locator_array_expression(locator, test_id_attribute)?;
    Ok(format!(
        r#"(() => {{
        const norm = value => String(value || '').replace(/\s+/g, ' ').trim();
        const implicitRole = element => {{
            const tag = element.tagName.toLowerCase();
            if (tag === 'button') return 'button';
            if (tag === 'a' && element.hasAttribute('href')) return 'link';
            if (tag === 'textarea') return 'textbox';
            if (tag === 'select') return 'combobox';
            if (tag === 'input') {{
                const type = (element.type || 'text').toLowerCase();
                if (['button','submit','reset'].includes(type)) return 'button';
                if (type === 'checkbox') return 'checkbox';
                if (type === 'radio') return 'radio';
                if (['text','email','password','search','tel','url'].includes(type)) return 'textbox';
            }}
            return null;
        }};
        const accessibleName = element => {{
            const labelledby = element.getAttribute('aria-labelledby');
            if (labelledby) return norm(labelledby.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' '));
            if (element.hasAttribute('aria-label')) return norm(element.getAttribute('aria-label'));
            if (element.labels?.length) return norm(Array.from(element.labels).map(label => {{
                const copy = label.cloneNode(true);
                copy.querySelectorAll('input,textarea,select,button').forEach(control => control.remove());
                return copy.innerText || copy.textContent;
            }}).join(' '));
            if (element.tagName === 'IMG') return norm(element.alt);
            if (element.tagName === 'INPUT' && ['button','submit','reset'].includes(element.type)) return norm(element.value);
            return norm(element.innerText || element.textContent || element.title);
        }};
        const elements = {elements};
        if (elements.length !== 1) return false;
        elements[0].scrollIntoView({{block:'center', inline:'center', behavior:'instant'}});
        return true;
    }})()"#
    ))
}

pub(super) fn locator_array_expression(
    locator: &Locator,
    test_id_attribute: &str,
) -> Result<String, BrowserError> {
    let json = |value: &str| {
        serde_json::to_string(value).map_err(|error| invalid_evaluation(&error.to_string()))
    };
    let expression = match locator {
        Locator::Id(value) => format!(
            "(() => {{ const e = document.getElementById({}); return e ? [e] : []; }})()",
            json(value)?
        ),
        Locator::Role { role, name } => {
            let role = json(role)?;
            let name = name
                .as_deref()
                .map(json)
                .transpose()?
                .unwrap_or_else(|| "null".into());
            format!(
                "Array.from(document.querySelectorAll('body *')).filter(element => (element.getAttribute('role') || implicitRole(element)) === {role} && ({name} === null || accessibleName(element) === {name}))"
            )
        }
        Locator::Label(value) => {
            let value = json(value)?;
            format!(
                "Array.from(document.querySelectorAll('input,textarea,select,button,[contenteditable=true]')).filter(element => accessibleName(element) === {value})"
            )
        }
        Locator::Text(value) => {
            let value = json(value)?;
            format!(
                "(() => {{ const all = Array.from(document.querySelectorAll('body *')).filter(element => !['SCRIPT','STYLE','NOSCRIPT'].includes(element.tagName) && norm(element.innerText) === {value}); const actionable = all.filter(element => element.matches('button,a[href],input,textarea,select,[role],[contenteditable=true]')); const pool = actionable.length ? actionable : all; return pool.filter(element => !pool.some(other => other !== element && element.contains(other))); }})()"
            )
        }
        Locator::Placeholder(value) => format!(
            "Array.from(document.querySelectorAll('input[placeholder],textarea[placeholder]')).filter(element => element.getAttribute('placeholder') === {})",
            json(value)?
        ),
        Locator::TestId(value) => format!(
            "Array.from(document.querySelectorAll('[{}]')).filter(element => element.getAttribute({}) === {})",
            css_attribute(test_id_attribute)?,
            json(test_id_attribute)?,
            json(value)?
        ),
        Locator::Css(value) => format!("Array.from(document.querySelectorAll({}))", json(value)?),
        Locator::XPath(value) => format!(
            "(() => {{ const result = document.evaluate({}, document, null, XPathResult.ORDERED_NODE_ITERATOR_TYPE, null); const values=[]; let item; while ((item=result.iterateNext())) {{ if (item.nodeType === Node.ELEMENT_NODE) values.push(item); }} return values; }})()",
            json(value)?
        ),
    };
    Ok(expression)
}

fn css_attribute(value: &str) -> Result<String, BrowserError> {
    if !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
    {
        Ok(value.into())
    } else {
        Err(BrowserError::Protocol {
            method: "Runtime.evaluate".into(),
            message: "test-ID attribute is not a valid attribute name".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_values_are_json_encoded_and_test_id_attributes_are_validated() {
        let hostile = "\\\"\\n</script>é";
        for locator in [
            Locator::Id(hostile.into()),
            Locator::Role {
                role: hostile.into(),
                name: Some(hostile.into()),
            },
            Locator::Label(hostile.into()),
            Locator::Text(hostile.into()),
            Locator::Placeholder(hostile.into()),
            Locator::TestId(hostile.into()),
            Locator::Css(hostile.into()),
            Locator::XPath(hostile.into()),
        ] {
            let expression = locator_array_expression(&locator, "data-testid").expect("expression");
            assert!(expression.contains(&serde_json::to_string(hostile).expect("JSON string")));
        }
        assert!(css_attribute("data-testid").is_ok());
        assert!(css_attribute("data testid").is_err());
        assert!(css_attribute("").is_err());
    }

    #[test]
    fn shared_resolution_is_passive_and_scrolling_is_action_only() {
        let resolver =
            resolver_expression(&Locator::Id("target".into()), "data-testid").expect("resolver");
        assert!(!resolver.contains("scrollIntoView"));
        assert!(!resolver.contains(".focus("));
        assert!(!resolver.contains(".click("));
        assert!(!resolver.contains("dispatchEvent"));

        let scroll =
            scroll_expression(&Locator::Id("target".into()), "data-testid").expect("scroll helper");
        assert!(scroll.contains("scrollIntoView"));
    }

    #[test]
    fn locator_states_preserve_zero_and_single_match_semantics() {
        let detached = ResolveSnapshot::default();
        assert!(state_satisfied(&detached, LocatorState::Hidden));
        assert!(state_satisfied(&detached, LocatorState::Detached));
        assert!(!state_satisfied(&detached, LocatorState::Visible));

        let visible = ResolveSnapshot {
            matches: 1,
            visible: true,
            ..ResolveSnapshot::default()
        };
        assert!(state_satisfied(&visible, LocatorState::Attached));
        assert!(state_satisfied(&visible, LocatorState::Visible));
        assert!(state_satisfied(&visible, LocatorState::Enabled));
    }

    #[test]
    fn rectangle_stability_uses_strict_quarter_pixel_tolerance() {
        let first = ElementRect {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        let stable = ElementRect {
            x: 1.249,
            ..first.clone()
        };
        let unstable = ElementRect {
            x: 1.25,
            ..first.clone()
        };
        assert!(rect_stable(Some(&first), Some(&stable)));
        assert!(!rect_stable(Some(&first), Some(&unstable)));
        assert!(!rect_stable(Some(&first), None));
    }
}
