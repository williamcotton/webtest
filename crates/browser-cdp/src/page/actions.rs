use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;
use webtest_browser::{Action, BrowserError, Locator, LocatorState};

use crate::wire::bounded_text;

use super::{
    CdpPage,
    evaluation::{self, evaluation_value, invalid_evaluation},
    locator::{self, ResolveSnapshot, locator_array_expression, rect_stable},
    navigation,
};

pub(super) async fn perform(
    page: &CdpPage,
    action: &Action,
    timeout: Duration,
    deadline: Instant,
) -> Result<(), BrowserError> {
    let snapshot = wait_for_actionability(page, action, timeout, deadline).await?;
    match action {
        Action::Click { .. } => {
            let mut navigation = navigation::watch(page);
            physical_click(page, &snapshot).await?;
            if super::navigation::wait_for_navigation_after_action(page, &mut navigation, deadline)
                .await?
            {
                Ok(())
            } else {
                Err(BrowserError::ActionTimeout {
                    locator: action.locator().clone(),
                    timeout_ms: duration_millis(timeout),
                })
            }
        }
        Action::Hover { .. } => mouse_move(page, &snapshot).await,
        Action::Fill { value, .. } => {
            physical_click(page, &snapshot).await?;
            select_all(page).await?;
            key_event(page, "Backspace", "Backspace", 0, None).await?;
            insert_text(page, value).await
        }
        Action::Type { value, .. } => {
            physical_click(page, &snapshot).await?;
            insert_text(page, value).await
        }
        Action::Press { key, .. } => {
            physical_click(page, &snapshot).await?;
            let key =
                parse_key(key).ok_or_else(|| BrowserError::InvalidKey { key: key.clone() })?;
            key_event(
                page,
                &key.key,
                &key.code,
                key.modifiers,
                key.text.as_deref(),
            )
            .await
        }
        Action::Check { locator, checked } => {
            if snapshot.checked == Some(*checked) {
                Ok(())
            } else {
                physical_click(page, &snapshot).await?;
                let after = locator::resolve(page, locator).await?;
                if after.checked == Some(*checked) {
                    Ok(())
                } else {
                    Err(BrowserError::AssertionFailed {
                        locator: locator.clone(),
                        expected: if *checked {
                            LocatorState::Checked
                        } else {
                            LocatorState::Unchecked
                        },
                        actual: format!("checked={:?}", after.checked),
                    })
                }
            }
        }
        Action::Select { locator, option } => select_option(page, locator, option).await,
    }
}

async fn wait_for_actionability(
    page: &CdpPage,
    action: &Action,
    timeout: Duration,
    deadline: Instant,
) -> Result<ResolveSnapshot, BrowserError> {
    let locator = action.locator();
    let mut backoff = Duration::from_millis(20);
    let mut last_failure = None;
    let mut failures_changed = false;
    loop {
        if Instant::now() >= deadline {
            return terminal_actionability_failure(
                locator,
                timeout,
                failures_changed,
                last_failure.take(),
            );
        }
        let initial = match super::complete_before_deadline(
            deadline,
            locator::resolve(page, locator),
        )
        .await
        {
            Ok(result) => result?,
            Err(()) => {
                return terminal_actionability_failure(
                    locator,
                    timeout,
                    failures_changed,
                    last_failure.take(),
                );
            }
        };
        let last_error = match preparation_failure(locator, action, &initial) {
            Some(error) => error,
            _ => {
                match super::complete_before_deadline(
                    deadline,
                    locator::scroll_into_view(page, locator),
                )
                .await
                {
                    Ok(result) => result?,
                    Err(()) => {
                        return terminal_actionability_failure(
                            locator,
                            timeout,
                            failures_changed,
                            last_failure.take(),
                        );
                    }
                }
                let first = match super::complete_before_deadline(
                    deadline,
                    locator::resolve(page, locator),
                )
                .await
                {
                    Ok(result) => result?,
                    Err(()) => {
                        return terminal_actionability_failure(
                            locator,
                            timeout,
                            failures_changed,
                            last_failure.take(),
                        );
                    }
                };
                if let Some(error) = post_scroll_failure(locator, action, &first) {
                    error
                } else {
                    tokio::time::sleep_until(
                        deadline.min(Instant::now() + Duration::from_millis(50)),
                    )
                    .await;
                    if Instant::now() >= deadline {
                        return terminal_actionability_failure(
                            locator,
                            timeout,
                            failures_changed,
                            last_failure.take(),
                        );
                    }
                    let second = match super::complete_before_deadline(
                        deadline,
                        locator::resolve(page, locator),
                    )
                    .await
                    {
                        Ok(result) => result?,
                        Err(()) => {
                            return terminal_actionability_failure(
                                locator,
                                timeout,
                                failures_changed,
                                last_failure.take(),
                            );
                        }
                    };
                    if second.matches == 0 {
                        BrowserError::ElementDetached {
                            locator: locator.clone(),
                        }
                    } else if second.matches > 1 {
                        BrowserError::LocatorAmbiguous {
                            locator: locator.clone(),
                            matches: second.matches,
                        }
                    } else if !rect_stable(first.rect.as_ref(), second.rect.as_ref()) {
                        BrowserError::ElementUnstable {
                            locator: locator.clone(),
                        }
                    } else if let Some(error) = post_scroll_failure(locator, action, &second) {
                        error
                    } else {
                        return Ok(second);
                    }
                }
            }
        };
        if let Some(previous) = last_failure.as_ref() {
            failures_changed |=
                std::mem::discriminant(previous) != std::mem::discriminant(&last_error);
        }
        if Instant::now() >= deadline {
            return terminal_actionability_failure(
                locator,
                timeout,
                failures_changed,
                Some(last_error),
            );
        }
        last_failure = Some(last_error);
        tokio::time::sleep_until(deadline.min(Instant::now() + backoff)).await;
        backoff = (backoff * 2).min(Duration::from_millis(100));
    }
}

fn preparation_failure(
    locator: &Locator,
    action: &Action,
    snapshot: &ResolveSnapshot,
) -> Option<BrowserError> {
    if snapshot.matches == 0 {
        return Some(BrowserError::LocatorNotFound {
            locator: locator.clone(),
        });
    }
    common_actionability_failure(locator, action, snapshot)
}

fn post_scroll_failure(
    locator: &Locator,
    action: &Action,
    snapshot: &ResolveSnapshot,
) -> Option<BrowserError> {
    if snapshot.matches == 0 {
        return Some(BrowserError::ElementDetached {
            locator: locator.clone(),
        });
    }
    common_actionability_failure(locator, action, snapshot).or_else(|| {
        (requires_physical_pointer(action) && snapshot.obscured).then(|| {
            BrowserError::ElementObscured {
                locator: locator.clone(),
            }
        })
    })
}

fn common_actionability_failure(
    locator: &Locator,
    action: &Action,
    snapshot: &ResolveSnapshot,
) -> Option<BrowserError> {
    if snapshot.matches > 1 {
        Some(BrowserError::LocatorAmbiguous {
            locator: locator.clone(),
            matches: snapshot.matches,
        })
    } else if !snapshot.visible {
        Some(BrowserError::LocatorNotVisible {
            locator: locator.clone(),
        })
    } else if snapshot.disabled {
        Some(BrowserError::ElementDisabled {
            locator: locator.clone(),
        })
    } else if matches!(action, Action::Fill { .. } | Action::Type { .. }) && !snapshot.editable
        || matches!(action, Action::Select { .. }) && snapshot.tag.as_deref() != Some("select")
        || matches!(action, Action::Check { .. }) && snapshot.checked.is_none()
    {
        Some(BrowserError::ElementNotEditable {
            locator: locator.clone(),
        })
    } else {
        None
    }
}

fn requires_physical_pointer(action: &Action) -> bool {
    matches!(
        action,
        Action::Click { .. }
            | Action::Hover { .. }
            | Action::Fill { .. }
            | Action::Type { .. }
            | Action::Press { .. }
            | Action::Check { .. }
    )
}

fn terminal_actionability_failure(
    locator: &Locator,
    timeout: Duration,
    failures_changed: bool,
    last_failure: Option<BrowserError>,
) -> Result<ResolveSnapshot, BrowserError> {
    match (failures_changed, last_failure) {
        (false, Some(last_failure)) => Err(last_failure),
        _ => Err(BrowserError::ActionTimeout {
            locator: locator.clone(),
            timeout_ms: duration_millis(timeout),
        }),
    }
}

async fn mouse_move(page: &CdpPage, snapshot: &ResolveSnapshot) -> Result<(), BrowserError> {
    let (x, y) = snapshot
        .center()
        .ok_or_else(|| invalid_evaluation("element had no interaction point"))?;
    page.connection
        .command(
            "Input.dispatchMouseEvent",
            Some(json!({
                "type": "mouseMoved", "x": x, "y": y
            })),
            Some(&page.session_id),
        )
        .await
        .map(|_| ())
}

async fn physical_click(page: &CdpPage, snapshot: &ResolveSnapshot) -> Result<(), BrowserError> {
    let (x, y) = snapshot
        .center()
        .ok_or_else(|| invalid_evaluation("element had no interaction point"))?;
    mouse_move(page, snapshot).await?;
    page.connection
        .command(
            "Input.dispatchMouseEvent",
            Some(json!({
                "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1
            })),
            Some(&page.session_id),
        )
        .await?;
    page.connection
        .command(
            "Input.dispatchMouseEvent",
            Some(json!({
                "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1
            })),
            Some(&page.session_id),
        )
        .await
        .map(|_| ())
}

async fn insert_text(page: &CdpPage, value: &str) -> Result<(), BrowserError> {
    page.connection
        .command(
            "Input.insertText",
            Some(json!({ "text": value })),
            Some(&page.session_id),
        )
        .await
        .map(|_| ())
}

async fn select_all(page: &CdpPage) -> Result<(), BrowserError> {
    let modifiers = if cfg!(target_os = "macos") { 4 } else { 2 };
    page.connection
        .command(
            "Input.dispatchKeyEvent",
            Some(json!({
                "type": "rawKeyDown", "key": "a", "code": "KeyA", "modifiers": modifiers,
                "commands": ["selectAll"]
            })),
            Some(&page.session_id),
        )
        .await?;
    page.connection
        .command(
            "Input.dispatchKeyEvent",
            Some(json!({
                "type": "keyUp", "key": "a", "code": "KeyA", "modifiers": modifiers
            })),
            Some(&page.session_id),
        )
        .await
        .map(|_| ())
}

async fn key_event(
    page: &CdpPage,
    key: &str,
    code: &str,
    modifiers: i32,
    text: Option<&str>,
) -> Result<(), BrowserError> {
    let mut down = json!({ "type": "keyDown", "key": key, "code": code, "modifiers": modifiers });
    if let Some(text) = text {
        down["text"] = Value::String(text.into());
    }
    page.connection
        .command("Input.dispatchKeyEvent", Some(down), Some(&page.session_id))
        .await?;
    page.connection
        .command(
            "Input.dispatchKeyEvent",
            Some(json!({
                "type": "keyUp", "key": key, "code": code, "modifiers": modifiers
            })),
            Some(&page.session_id),
        )
        .await
        .map(|_| ())
}

async fn select_option(
    page: &CdpPage,
    locator: &Locator,
    option: &str,
) -> Result<(), BrowserError> {
    let elements = locator_array_expression(locator, &page.test_id_attribute)?;
    let option_json =
        serde_json::to_string(option).map_err(|error| invalid_evaluation(&error.to_string()))?;
    let expression = format!(
        r#"(() => {{
            const norm = value => String(value || '').replace(/\s+/g, ' ').trim();
            const implicitRole = element => {{
                const tag = element.tagName.toLowerCase();
                if (tag === 'button') return 'button';
                if (tag === 'textarea') return 'textbox';
                if (tag === 'select') return 'combobox';
                if (tag === 'input') {{
                    if (element.type === 'checkbox') return 'checkbox';
                    if (element.type === 'radio') return 'radio';
                    return 'textbox';
                }}
                return element.getAttribute('role');
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
                return norm(element.innerText || element.textContent || element.title);
            }};
            try {{ const elements = {elements}; if (elements.length !== 1) return {{matches: elements.length}};
            const select = elements[0]; const wanted = {option_json};
            const options = Array.from(select.options || []).filter(o => o.value === wanted || norm(o.text) === wanted);
            if (options.length !== 1) return {{matches: 1, options: options.length}};
            select.value = options[0].value; select.dispatchEvent(new Event('input', {{bubbles:true}}));
            select.dispatchEvent(new Event('change', {{bubbles:true}})); return {{matches:1, options:1}};
            }} catch (error) {{ return {{invalid:String(error)}}; }} }})()"#
    );
    let result = evaluation::evaluate_expression(page, expression).await?;
    let value =
        evaluation_value(&result).ok_or_else(|| invalid_evaluation("select result missing"))?;
    if let Some(message) = value.get("invalid").and_then(Value::as_str) {
        return Err(BrowserError::LocatorInvalid {
            locator: locator.clone(),
            message: bounded_text(message),
        });
    }
    match value.get("matches").and_then(Value::as_u64) {
        Some(0) => Err(BrowserError::LocatorNotFound {
            locator: locator.clone(),
        }),
        Some(1) if value.get("options").and_then(Value::as_u64) == Some(1) => Ok(()),
        Some(1) if value.get("options").and_then(Value::as_u64).unwrap_or(0) > 1 => {
            Err(BrowserError::OptionAmbiguous {
                locator: locator.clone(),
                option: option.into(),
                matches: value.get("options").and_then(Value::as_u64).unwrap_or(0) as usize,
            })
        }
        Some(1) => Err(BrowserError::OptionNotFound {
            locator: locator.clone(),
            option: option.into(),
        }),
        Some(count) => Err(BrowserError::LocatorAmbiguous {
            locator: locator.clone(),
            matches: count as usize,
        }),
        None => Err(invalid_evaluation(
            "select result did not contain match count",
        )),
    }
}

struct KeySpec {
    key: String,
    code: String,
    modifiers: i32,
    text: Option<String>,
}

fn parse_key(value: &str) -> Option<KeySpec> {
    let mut modifiers = 0;
    let mut main = None;
    for part in value.split('+') {
        match part {
            "Alt" => modifiers |= 1,
            "Control" | "Ctrl" => modifiers |= 2,
            "Meta" | "Command" => modifiers |= 4,
            "Shift" => modifiers |= 8,
            _ if main.is_none() && !part.is_empty() => main = Some(part),
            _ => return None,
        }
    }
    let main = main?;
    let (key, code, text) = match main {
        "Enter" => ("Enter".into(), "Enter".into(), None),
        "Tab" => ("Tab".into(), "Tab".into(), None),
        "Escape" | "Esc" => ("Escape".into(), "Escape".into(), None),
        "Backspace" => ("Backspace".into(), "Backspace".into(), None),
        "Delete" => ("Delete".into(), "Delete".into(), None),
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp"
        | "PageDown" => (main.into(), main.into(), None),
        "Space" => (" ".into(), "Space".into(), Some(" ".into())),
        value if value.chars().count() == 1 => {
            let character = value.chars().next()?;
            let code = if character.is_ascii_alphabetic() {
                format!("Key{}", character.to_ascii_uppercase())
            } else if character.is_ascii_digit() {
                format!("Digit{character}")
            } else {
                "Unidentified".into()
            };
            (value.into(), code, Some(value.into()))
        }
        _ => return None,
    };
    Some(KeySpec {
        key,
        code,
        modifiers,
        text,
    })
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_modifier_and_unicode_keys() {
        let enter = parse_key("Control+Shift+Enter").expect("Enter chord");
        assert_eq!(enter.key, "Enter");
        assert_eq!(enter.code, "Enter");
        assert_eq!(enter.modifiers, 10);
        assert!(enter.text.is_none());

        let unicode = parse_key("é").expect("Unicode scalar");
        assert_eq!(unicode.key, "é");
        assert_eq!(unicode.code, "Unidentified");
        assert_eq!(unicode.text.as_deref(), Some("é"));
        assert!(parse_key("").is_none());
        assert!(parse_key("Enter+Tab").is_none());
        assert!(parse_key("Unsupported").is_none());
    }

    #[test]
    fn physical_pointer_requirement_covers_every_click_dependent_action() {
        let locator = Locator::Id("target".into());
        for action in [
            Action::Click {
                locator: locator.clone(),
            },
            Action::Hover {
                locator: locator.clone(),
            },
            Action::Fill {
                locator: locator.clone(),
                value: "value".into(),
            },
            Action::Type {
                locator: locator.clone(),
                value: "value".into(),
            },
            Action::Press {
                locator: locator.clone(),
                key: "Enter".into(),
            },
            Action::Check {
                locator: locator.clone(),
                checked: true,
            },
        ] {
            assert!(requires_physical_pointer(&action), "{action:?}");
        }
        assert!(!requires_physical_pointer(&Action::Select {
            locator,
            option: "one".into(),
        }));
    }
}
