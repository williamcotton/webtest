use std::collections::{BTreeMap, HashMap};

use webtest_browser::{BrowserError, Locator as BrowserLocator};
use webtest_hir::BindingId;
use webtest_plan::{PlannedStep, ServerProviderCall, TestOperation};
use webtest_provider::Value;

use crate::{
    AssertionFailure, StepError,
    assertions::{assertion_message, value_diff},
    evaluation::{display_value, evaluate},
};

pub(crate) fn redact_step_error(
    error: StepError,
    fields: &[String],
    secrets: &[String],
    query_parameters: &[String],
) -> StepError {
    match error {
        StepError::Assertion(error) => {
            let expected = error
                .expected
                .map(|value| value.redacted_with_secrets(fields, secrets));
            let actual = error.actual.redacted_with_secrets(fields, secrets);
            StepError::Assertion(Box::new(AssertionFailure {
                matcher: error.matcher,
                message: assertion_message(error.matcher, &actual, expected.as_ref()),
                diff: value_diff(error.matcher, &actual, expected.as_ref()),
                expected,
                actual,
            }))
        }
        StepError::Provider(error) => StepError::Provider(error.redacted(secrets)),
        StepError::Browser(error) => {
            StepError::Browser(redact_browser_error(error, secrets, query_parameters))
        }
        error => error,
    }
}

fn redact_browser_error(
    error: BrowserError,
    secrets: &[String],
    query_parameters: &[String],
) -> BrowserError {
    let locator = |locator| redact_locator(locator, secrets);
    let text = |value: String| redact_text(value, secrets);
    let url = |value: String| redact_url(value, secrets, query_parameters);
    match error {
        BrowserError::LocatorNotFound { locator: value } => BrowserError::LocatorNotFound {
            locator: locator(value),
        },
        BrowserError::LocatorAmbiguous {
            locator: value,
            matches,
        } => BrowserError::LocatorAmbiguous {
            locator: locator(value),
            matches,
        },
        BrowserError::LocatorInvalid {
            locator: value,
            message,
        } => BrowserError::LocatorInvalid {
            locator: locator(value),
            message: text(message),
        },
        BrowserError::ElementDetached { locator: value } => BrowserError::ElementDetached {
            locator: locator(value),
        },
        BrowserError::LocatorNotVisible { locator: value } => BrowserError::LocatorNotVisible {
            locator: locator(value),
        },
        BrowserError::ElementUnstable { locator: value } => BrowserError::ElementUnstable {
            locator: locator(value),
        },
        BrowserError::ElementDisabled { locator: value } => BrowserError::ElementDisabled {
            locator: locator(value),
        },
        BrowserError::ElementObscured { locator: value } => BrowserError::ElementObscured {
            locator: locator(value),
        },
        BrowserError::ElementNotEditable { locator: value } => BrowserError::ElementNotEditable {
            locator: locator(value),
        },
        BrowserError::OptionNotFound {
            locator: value,
            option,
        } => BrowserError::OptionNotFound {
            locator: locator(value),
            option: text(option),
        },
        BrowserError::OptionAmbiguous {
            locator: value,
            option,
            matches,
        } => BrowserError::OptionAmbiguous {
            locator: locator(value),
            option: text(option),
            matches,
        },
        BrowserError::InvalidKey { key } => BrowserError::InvalidKey { key: text(key) },
        BrowserError::ActionTimeout {
            locator: value,
            timeout_ms,
        } => BrowserError::ActionTimeout {
            locator: locator(value),
            timeout_ms,
        },
        BrowserError::AssertionFailed {
            locator: value,
            expected,
            actual,
        } => BrowserError::AssertionFailed {
            locator: locator(value),
            expected,
            actual: text(actual),
        },
        BrowserError::UrlMismatch { expected, actual } => BrowserError::UrlMismatch {
            expected: url(expected),
            actual: url(actual),
        },
        BrowserError::NavigationFailed { url: value, reason } => BrowserError::NavigationFailed {
            url: url(value),
            reason: text(reason),
        },
        BrowserError::NavigationTimeout {
            url: value,
            timeout_ms,
        } => BrowserError::NavigationTimeout {
            url: url(value),
            timeout_ms,
        },
        BrowserError::CommandTimeout { method, timeout_ms } => BrowserError::CommandTimeout {
            method: text(method),
            timeout_ms,
        },
        BrowserError::BrowserDisconnected => BrowserError::BrowserDisconnected,
        BrowserError::BrowserCrashed { status } => BrowserError::BrowserCrashed {
            status: text(status),
        },
        BrowserError::MalformedProtocol { message } => BrowserError::MalformedProtocol {
            message: text(message),
        },
        BrowserError::Protocol { method, message } => BrowserError::Protocol {
            method: text(method),
            message: text(message),
        },
        BrowserError::Launch(message) => BrowserError::Launch(text(message)),
        BrowserError::EvaluationFailed {
            expression,
            message,
        } => BrowserError::EvaluationFailed {
            expression: text(expression),
            message: text(message),
        },
        BrowserError::UnsupportedCapability { capability } => BrowserError::UnsupportedCapability {
            capability: text(capability),
        },
    }
}

fn redact_locator(locator: BrowserLocator, secrets: &[String]) -> BrowserLocator {
    match locator {
        BrowserLocator::Id(value) => BrowserLocator::Id(redact_text(value, secrets)),
        BrowserLocator::Role { role, name } => BrowserLocator::Role {
            role: redact_text(role, secrets),
            name: name.map(|value| redact_text(value, secrets)),
        },
        BrowserLocator::Label(value) => BrowserLocator::Label(redact_text(value, secrets)),
        BrowserLocator::Text(value) => BrowserLocator::Text(redact_text(value, secrets)),
        BrowserLocator::Placeholder(value) => {
            BrowserLocator::Placeholder(redact_text(value, secrets))
        }
        BrowserLocator::TestId(value) => BrowserLocator::TestId(redact_text(value, secrets)),
        BrowserLocator::Css(value) => BrowserLocator::Css(redact_text(value, secrets)),
        BrowserLocator::XPath(value) => BrowserLocator::XPath(redact_text(value, secrets)),
    }
}

fn redact_text(mut value: String, secrets: &[String]) -> String {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        value = value.replace(secret, "[redacted]");
    }
    value
}

fn redact_url(value: String, secrets: &[String], query_parameters: &[String]) -> String {
    let mut value = if let Ok(mut parsed) = url::Url::parse(&value) {
        let pairs = parsed
            .query_pairs()
            .map(|(name, value)| {
                let value = if query_parameters
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&name))
                {
                    "[redacted]".into()
                } else {
                    value.into_owned()
                };
                (name.into_owned(), value)
            })
            .collect::<Vec<_>>();
        if !pairs.is_empty() {
            parsed.query_pairs_mut().clear().extend_pairs(pairs);
        }
        parsed.to_string()
    } else {
        value
    };
    value = redact_text(value, secrets);
    value
}

pub(crate) fn visible_bindings(
    environment: &HashMap<BindingId, Value>,
    names: &HashMap<BindingId, String>,
    redacted_fields: &[String],
    secrets: &[String],
) -> BTreeMap<String, Value> {
    names
        .iter()
        .filter_map(|(id, name)| {
            // Debugger snapshots may inspect server-only values. This does not make those values
            // transferable to browser expressions or change the final transferable bindings.
            environment.get(id).map(|value| {
                (
                    name.clone(),
                    debugger_value(value, redacted_fields, secrets),
                )
            })
        })
        .collect()
}

const DEBUGGER_SNAPSHOT_BYTES: usize = 16 * 1024;
const DEBUGGER_SNAPSHOT_ITEMS: usize = 256;

struct DebuggerSnapshotBudget {
    bytes: usize,
    items: usize,
}

fn debugger_value(value: &Value, redacted_fields: &[String], secrets: &[String]) -> Value {
    // Keep enough overlap to redact a secret crossing the final snapshot boundary, then apply the
    // hard bound a second time. Debugger presentation must never copy an unbounded response body.
    let overlap = secrets.iter().map(String::len).max().unwrap_or_default();
    let mut before_redaction = DebuggerSnapshotBudget {
        bytes: DEBUGGER_SNAPSHOT_BYTES.saturating_add(overlap),
        items: DEBUGGER_SNAPSHOT_ITEMS,
    };
    let value = debugger_snapshot(value, &mut before_redaction)
        .redacted_with_secrets(redacted_fields, secrets);
    let mut final_budget = DebuggerSnapshotBudget {
        bytes: DEBUGGER_SNAPSHOT_BYTES,
        items: DEBUGGER_SNAPSHOT_ITEMS,
    };
    debugger_snapshot(&value, &mut final_budget)
}

fn debugger_snapshot(value: &Value, budget: &mut DebuggerSnapshotBudget) -> Value {
    if budget.items == 0 {
        return Value::String("<debugger value truncated>".into());
    }
    budget.items -= 1;
    match value {
        Value::String(value) => Value::String(debugger_text(value, budget)),
        Value::Bytes(value) => Value::Bytes(debugger_bytes(value, budget)),
        Value::List(values) => Value::List(
            values
                .iter()
                .take(budget.items)
                .map(|value| debugger_snapshot(value, budget))
                .collect(),
        ),
        Value::Record(values) => Value::Record(
            values
                .iter()
                .take(budget.items)
                .map(|(name, value)| (name.clone(), debugger_snapshot(value, budget)))
                .collect(),
        ),
        Value::Headers(values) => Value::Headers(
            values
                .iter()
                .take(budget.items)
                .map(|(name, value)| (name.clone(), debugger_text(value, budget)))
                .collect(),
        ),
        Value::Response(value) => Value::Response(webtest_provider::ResponseValue {
            status: value.status,
            headers: value
                .headers
                .iter()
                .take(budget.items)
                .map(|(name, value)| (name.clone(), debugger_text(value, budget)))
                .collect(),
            body: debugger_bytes(&value.body, budget),
            json: value
                .json
                .as_deref()
                .map(|value| Box::new(debugger_snapshot(value, budget))),
        }),
        Value::ProcessResult(value) => Value::ProcessResult(webtest_provider::ProcessResultValue {
            exit_code: value.exit_code,
            stdout: debugger_text(&value.stdout, budget),
            stderr: debugger_text(&value.stderr, budget),
            stdout_bytes: debugger_bytes(&value.stdout_bytes, budget),
            stderr_bytes: debugger_bytes(&value.stderr_bytes, budget),
        }),
        value => value.clone(),
    }
}

fn debugger_text(value: &str, budget: &mut DebuggerSnapshotBudget) -> String {
    let mut end = value.len().min(budget.bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    budget.bytes -= end;
    value[..end].to_owned()
}

fn debugger_bytes(value: &[u8], budget: &mut DebuggerSnapshotBudget) -> Vec<u8> {
    let end = value.len().min(budget.bytes);
    budget.bytes -= end;
    value[..end].to_vec()
}

pub(crate) fn collect_provider_secrets(
    call: &ServerProviderCall,
    environment: &HashMap<BindingId, Value>,
    redacted_fields: &[String],
    secrets: &mut Vec<String>,
) {
    for (name, expression) in &call.arguments {
        let Ok(value) = evaluate(expression, environment) else {
            continue;
        };
        collect_sensitive_values(
            &value,
            redacted_fields,
            call.redacted_arguments
                .iter()
                .any(|argument| argument == name),
            secrets,
        );
    }
    secrets.sort();
    secrets.dedup();
}

pub(crate) fn collect_provider_result_secrets(
    call: &ServerProviderCall,
    environment: &HashMap<BindingId, Value>,
    redacted_fields: &[String],
    secrets: &mut Vec<String>,
) {
    let Some(value) = call
        .result_binding
        .and_then(|binding| environment.get(&binding))
    else {
        return;
    };
    let fields = redacted_fields
        .iter()
        .chain(&call.redacted_result_fields)
        .cloned()
        .collect::<Vec<_>>();
    collect_sensitive_values(value, &fields, false, secrets);
    secrets.sort();
    secrets.dedup();
}

pub(crate) fn provider_argument_summaries(
    call: &ServerProviderCall,
    environment: &HashMap<BindingId, Value>,
    redacted_fields: &[String],
    secrets: &[String],
) -> BTreeMap<String, String> {
    call.arguments
        .iter()
        .map(|(name, expression)| {
            let summary = if call.redacted_arguments.contains(name) {
                "[redacted]".into()
            } else {
                evaluate(expression, environment)
                    .map(|value| {
                        bounded_value_summary(
                            &value.redacted_with_secrets(redacted_fields, secrets),
                        )
                    })
                    .unwrap_or_else(|_| "<unavailable>".into())
            };
            (name.clone(), summary)
        })
        .collect()
}

pub(crate) fn visible_step_bindings(
    step: &PlannedStep,
    environment: &HashMap<BindingId, Value>,
    binding_names: &HashMap<BindingId, String>,
    redacted_fields: &[String],
    secrets: &[String],
) -> BTreeMap<String, Value> {
    let mut visible = visible_bindings(environment, binding_names, redacted_fields, secrets);
    let TestOperation::ServerProviderCall(call) = &step.operation else {
        return visible;
    };
    for (name, expression) in &call.arguments {
        let value = if call.redacted_arguments.contains(name) {
            Value::String("[redacted]".into())
        } else {
            evaluate(expression, environment)
                .map(|value| debugger_value(&value, redacted_fields, secrets))
                .unwrap_or_else(|_| Value::String("<unavailable>".into()))
        };
        visible.insert(format!("argument.{name}"), value);
    }
    visible
}

pub(crate) fn bounded_value_summary(value: &Value) -> String {
    let mut summary = display_value(value);
    if summary.len() > 256 {
        let mut end = 256;
        while !summary.is_char_boundary(end) {
            end -= 1;
        }
        summary.truncate(end);
        summary.push_str("...");
    }
    summary
}

fn collect_sensitive_values(
    value: &Value,
    redacted_fields: &[String],
    sensitive: bool,
    secrets: &mut Vec<String>,
) {
    match value {
        Value::String(value) if sensitive && !value.is_empty() => secrets.push(value.clone()),
        Value::Record(values) => {
            for (name, value) in values {
                let sensitive = sensitive
                    || redacted_fields
                        .iter()
                        .any(|field| field.eq_ignore_ascii_case(name));
                collect_sensitive_values(value, redacted_fields, sensitive, secrets);
            }
        }
        Value::List(values) => {
            for value in values {
                collect_sensitive_values(value, redacted_fields, sensitive, secrets);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
