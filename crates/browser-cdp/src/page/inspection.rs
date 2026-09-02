use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::Value;
use webtest_browser::{
    BrowserError, ElementStates, INSPECTION_SCHEMA_VERSION, InspectableElement, InspectionOptions,
    InspectionTruncation, Locator, LocatorCandidate, LocatorCandidateKind, PageInspection,
    PageSummary, SupportedAction,
};

use super::evaluation::{evaluation_value, invalid_evaluation};
use super::{CdpPage, evaluation, locator, redaction};

static NEXT_SNAPSHOT_ID: AtomicU64 = AtomicU64::new(1);

pub(super) async fn inspect(
    page: &CdpPage,
    options: &InspectionOptions,
) -> Result<PageInspection, BrowserError> {
    let options = options.bounded();
    let expression = inspection_expression(
        &page.test_id_attribute,
        options.include_hidden,
        options.max_elements,
    )?;
    let result = evaluation::evaluate_expression(page, expression).await?;
    let value = evaluation_value(&result)
        .ok_or_else(|| invalid_evaluation("inspection result was missing"))?;
    let raw: RawInspection = serde_json::from_value(value.clone())
        .map_err(|error| invalid_evaluation(&error.to_string()))?;
    let returned_elements = raw.elements.len();
    let version = page
        .connection
        .command("Browser.getVersion", None, None)
        .await
        .ok()
        .and_then(|value| {
            value
                .get("product")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".into());
    let mut text_truncated = false;
    let mut candidates_truncated = false;
    let mut options_truncated = false;
    let mut elements = Vec::new();
    for raw_element in raw.elements {
        let built = inspectable_element(
            page,
            raw_element,
            &options,
            &mut text_truncated,
            &mut candidates_truncated,
            &mut options_truncated,
        )
        .await?;
        if let Some(element) = built {
            elements.push(element);
        }
    }
    let omitted_elements = raw.total.saturating_sub(returned_elements);
    Ok(PageInspection {
        kind: "inspection".into(),
        inspection_schema_version: INSPECTION_SCHEMA_VERSION,
        snapshot_id: format!(
            "snapshot-{}",
            NEXT_SNAPSHOT_ID.fetch_add(1, Ordering::Relaxed)
        ),
        browser_version: redaction::truncate_utf8(&version, 128),
        page: PageSummary {
            url: redaction::redact_url_query(&raw.url, &options.redacted_query_parameters),
            title: redaction::redact_and_truncate(
                &raw.title,
                &options.redacted_values,
                options.max_text_bytes,
            ),
        },
        elements,
        truncation: InspectionTruncation {
            elements_truncated: omitted_elements > 0,
            omitted_elements,
            candidates_truncated,
            text_truncated,
            options_truncated,
        },
    })
}

async fn inspectable_element(
    page: &CdpPage,
    raw: RawInspectableElement,
    options: &InspectionOptions,
    text_truncated: &mut bool,
    candidates_truncated: &mut bool,
    options_truncated: &mut bool,
) -> Result<Option<InspectableElement>, BrowserError> {
    let (role, role_source) = bounded_field(raw.role, options, text_truncated);
    let (name, name_source) = bounded_field(raw.name, options, text_truncated);
    let (label, label_source) = bounded_field(raw.label, options, text_truncated);
    let (placeholder, placeholder_source) = bounded_field(raw.placeholder, options, text_truncated);
    let (test_id, test_id_source) = bounded_field(raw.test_id, options, text_truncated);
    let (dom_id, dom_id_source) = bounded_field(raw.dom_id, options, text_truncated);
    let (_text, text_source) = bounded_field(raw.text, options, text_truncated);

    let mut locators = Vec::new();
    if let Some(value) = label_source {
        locators.push((
            Locator::Label(value),
            LocatorCandidateKind::Label,
            "unique associated label",
        ));
    }
    if let Some(role_value) = role_source {
        locators.push((
            Locator::Role {
                role: role_value,
                name: name_source,
            },
            LocatorCandidateKind::Role,
            "unique accessible role and name",
        ));
    }
    if let Some(value) = test_id_source {
        locators.push((
            Locator::TestId(value),
            LocatorCandidateKind::TestId,
            "unique configured test ID",
        ));
    }
    if let Some(value) = dom_id_source {
        locators.push((
            Locator::Id(value),
            LocatorCandidateKind::Id,
            "unique DOM ID",
        ));
    }
    if let Some(value) = placeholder_source {
        locators.push((
            Locator::Placeholder(value),
            LocatorCandidateKind::Placeholder,
            "unique placeholder",
        ));
    }
    if let Some(value) = text_source {
        locators.push((
            Locator::Text(value),
            LocatorCandidateKind::Text,
            "unique exact user-facing text",
        ));
    }

    let mut validated = Vec::new();
    for (locator, kind, reason) in locators {
        let snapshot = locator::resolve(page, &locator).await?;
        if snapshot.matches == 1 && snapshot.document_index == Some(raw.document_index) {
            validated.push(LocatorCandidate {
                source: locator.to_string(),
                kind,
                reason: reason.into(),
            });
        }
    }
    validated.dedup_by(|left, right| left.source == right.source);
    if validated.is_empty() {
        return Ok(None);
    }
    if validated.len() > options.max_candidates_per_element {
        validated.truncate(options.max_candidates_per_element);
        *candidates_truncated = true;
    }
    let preferred_locator = validated.remove(0);
    let interactive = raw.interactive || role.is_some();
    let mut supported_actions = Vec::new();
    if raw.editable && raw.visible && !raw.disabled && !raw.obscured {
        supported_actions.extend([
            SupportedAction::Fill,
            SupportedAction::Type,
            SupportedAction::Press,
        ]);
    }
    if raw.clickable && raw.visible && !raw.disabled && !raw.obscured {
        supported_actions.push(SupportedAction::Click);
    }
    if raw.checkable && raw.visible && !raw.disabled && !raw.obscured {
        supported_actions.extend([SupportedAction::Check, SupportedAction::Uncheck]);
    }
    if raw.selectable && raw.visible && !raw.disabled {
        supported_actions.push(SupportedAction::Select);
    }
    if raw.hoverable && raw.visible && !raw.obscured {
        supported_actions.push(SupportedAction::Hover);
    }
    if raw.options.len() > 50 {
        *options_truncated = true;
    }
    Ok(Some(InspectableElement {
        role,
        accessible_name: name,
        label,
        placeholder,
        test_id,
        dom_id,
        states: ElementStates {
            visible: raw.visible,
            enabled: interactive.then_some(!raw.disabled),
            editable: raw.editable_applicable.then_some(raw.editable),
            checked: raw.checked,
            selected: raw.selected,
            receives_pointer_input: raw.hoverable.then_some(raw.visible && !raw.obscured),
        },
        supported_actions,
        preferred_locator,
        alternate_locators: validated,
        options: raw
            .options
            .into_iter()
            .take(50)
            .map(|option| {
                redaction::redact_and_truncate(
                    &option,
                    &options.redacted_values,
                    options.max_text_bytes,
                )
            })
            .collect(),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInspection {
    url: String,
    title: String,
    total: usize,
    elements: Vec<RawInspectableElement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInspectableElement {
    document_index: usize,
    role: Option<String>,
    name: Option<String>,
    label: Option<String>,
    placeholder: Option<String>,
    test_id: Option<String>,
    dom_id: Option<String>,
    text: Option<String>,
    visible: bool,
    disabled: bool,
    editable: bool,
    editable_applicable: bool,
    checked: Option<bool>,
    selected: Option<bool>,
    obscured: bool,
    interactive: bool,
    clickable: bool,
    checkable: bool,
    selectable: bool,
    hoverable: bool,
    #[serde(default)]
    options: Vec<String>,
}

pub(super) fn bounded_field(
    value: Option<String>,
    options: &InspectionOptions,
    truncated: &mut bool,
) -> (Option<String>, Option<String>) {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return (None, None);
    };
    if options
        .redacted_values
        .iter()
        .any(|secret| !secret.is_empty() && value.contains(secret))
    {
        return (
            Some(redaction::redact_and_truncate(
                &value,
                &options.redacted_values,
                options.max_text_bytes,
            )),
            None,
        );
    }
    if value.len() <= options.max_text_bytes {
        return (Some(value.clone()), Some(value));
    }
    *truncated = true;
    (
        Some(redaction::truncate_utf8(&value, options.max_text_bytes)),
        None,
    )
}

fn inspection_expression(
    test_id_attribute: &str,
    include_hidden: bool,
    max_elements: usize,
) -> Result<String, BrowserError> {
    let test_id_attribute = serde_json::to_string(test_id_attribute)
        .map_err(|error| invalid_evaluation(&error.to_string()))?;
    Ok(format!(
        r#"(() => {{
        const norm = value => String(value || '').replace(/\s+/g, ' ').trim();
        const bounded = value => {{ const text = norm(value); return text ? text.slice(0, 4097) : null; }};
        const implicitRole = element => {{
            const tag = element.tagName.toLowerCase();
            if (tag === 'button') return 'button';
            if (tag === 'a' && element.hasAttribute('href')) return 'link';
            if (tag === 'textarea') return 'textbox';
            if (tag === 'select') return 'combobox';
            if (tag === 'img') return 'img';
            if (tag === 'input') {{
                const type = (element.type || 'text').toLowerCase();
                if (['button','submit','reset'].includes(type)) return 'button';
                if (type === 'checkbox') return 'checkbox';
                if (type === 'radio') return 'radio';
                if (['text','email','password','search','tel','url'].includes(type)) return 'textbox';
            }}
            return null;
        }};
        const labelText = element => element.labels?.length ? norm(Array.from(element.labels).map(label => {{
            const copy = label.cloneNode(true);
            copy.querySelectorAll('input,textarea,select,button').forEach(control => control.remove());
            return copy.innerText || copy.textContent;
        }}).join(' ')) : '';
        const accessibleName = element => {{
            const labelledby = element.getAttribute('aria-labelledby');
            if (labelledby) return norm(labelledby.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' '));
            if (element.hasAttribute('aria-label')) return norm(element.getAttribute('aria-label'));
            const label = labelText(element); if (label) return label;
            if (element.tagName === 'IMG') return norm(element.alt);
            if (element.tagName === 'INPUT' && ['button','submit','reset'].includes(element.type)) return norm(element.value);
            return norm(element.innerText || element.textContent || element.title);
        }};
        const all = Array.from(document.querySelectorAll('body *'));
        const inspected = all.map((element, documentIndex) => {{
            if (['SCRIPT','STYLE','NOSCRIPT','TEMPLATE'].includes(element.tagName)) return null;
            const rect = element.getBoundingClientRect(), style = getComputedStyle(element);
            const visible = style.display !== 'none' && style.visibility !== 'hidden'
                && Number(style.opacity) !== 0 && rect.width > 0 && rect.height > 0;
            if (!{include_hidden} && !visible) return null;
            const tag = element.tagName.toLowerCase();
            const role = element.getAttribute('role') || implicitRole(element);
            const name = accessibleName(element);
            const label = labelText(element);
            const placeholder = element.getAttribute('placeholder') || '';
            const testId = element.getAttribute({test_id_attribute}) || '';
            const text = element.tagName === 'INPUT' && element.type === 'password'
                ? '' : norm(element.innerText || '');
            const leafText = text && !Array.from(element.children).some(child => norm(child.innerText) === text);
            const interactive = element.matches('button,a[href],input,textarea,select,[contenteditable=true],[role]');
            if (!(interactive || role || name || label || testId || leafText)) return null;
            const disabled = element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true';
            const editableApplicable = element.tagName === 'TEXTAREA' || element.isContentEditable
                || (element.tagName === 'INPUT' && !['button','submit','reset','checkbox','radio','file','hidden'].includes(element.type));
            const editable = editableApplicable && !disabled && !element.readOnly;
            const checkable = element.matches('input[type=checkbox],input[type=radio],[role=checkbox],[role=radio],[role=switch]');
            const checked = checkable ? (('checked' in element) ? Boolean(element.checked)
                : element.getAttribute('aria-checked') === 'true') : null;
            const selected = element.matches('option,[role=option]')
                ? (element.selected ?? element.getAttribute('aria-selected') === 'true') : null;
            const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
            const hit = visible ? document.elementFromPoint(x, y) : null;
            const obscured = visible && !(hit === element || element.contains(hit));
            const clickable = element.matches('button,a[href],input[type=button],input[type=submit],input[type=reset],[role=button],[role=link]');
            const selectable = element.tagName === 'SELECT';
            const hoverable = clickable || checkable;
            return {{
                documentIndex, role: bounded(role), name: bounded(name), label: bounded(label),
                placeholder: bounded(placeholder), testId: bounded(testId), domId: bounded(element.id),
                text: leafText ? bounded(text) : null, visible, disabled, editable, editableApplicable,
                checked, selected, obscured, interactive, clickable, checkable, selectable, hoverable,
                options: selectable ? Array.from(element.options).slice(0, 51).map(option => bounded(option.label || option.value)).filter(Boolean) : []
            }};
        }}).filter(Boolean);
        return {{url: location.href, title: document.title, total: inspected.length,
            elements: inspected.slice(0, {max_elements})}};
    }})()"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_script_encodes_the_test_id_attribute() {
        let hostile = "\\\"\\n</script>é";
        let expression = inspection_expression(hostile, false, 10).expect("inspection script");
        assert!(expression.contains(&serde_json::to_string(hostile).expect("JSON string")));
    }

    #[test]
    fn redacted_and_truncated_fields_are_never_locator_sources() {
        let mut truncated = false;
        let options = InspectionOptions {
            redacted_values: vec!["password-value".into()],
            ..InspectionOptions::default()
        };
        let (display, source) = bounded_field(
            Some("prefix password-value suffix".into()),
            &options,
            &mut truncated,
        );
        assert_eq!(display.as_deref(), Some("prefix [redacted] suffix"));
        assert!(source.is_none());

        let options = InspectionOptions {
            max_text_bytes: 2,
            ..InspectionOptions::default()
        };
        let (display, source) = bounded_field(Some("éé".into()), &options, &mut truncated);
        assert_eq!(display.as_deref(), Some("é"));
        assert!(source.is_none());
        assert!(truncated);
    }
}
