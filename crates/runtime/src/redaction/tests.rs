use webtest_browser::{BrowserError, Locator, LocatorState};
use webtest_observation::ValueDiff;
use webtest_plan::ValueMatcher;
use webtest_provider::Value;

use crate::{AssertionFailure, StepError};

use super::{DEBUGGER_SNAPSHOT_BYTES, debugger_value, redact_step_error};

fn secret_locator() -> Locator {
    Locator::Role {
        role: "secret".into(),
        name: Some("secret".into()),
    }
}

#[test]
fn every_browser_error_string_surface_is_redacted() {
    let locator = secret_locator();
    let errors = vec![
        BrowserError::LocatorNotFound {
            locator: locator.clone(),
        },
        BrowserError::LocatorAmbiguous {
            locator: locator.clone(),
            matches: 2,
        },
        BrowserError::LocatorInvalid {
            locator: locator.clone(),
            message: "secret".into(),
        },
        BrowserError::ElementDetached {
            locator: locator.clone(),
        },
        BrowserError::LocatorNotVisible {
            locator: locator.clone(),
        },
        BrowserError::ElementUnstable {
            locator: locator.clone(),
        },
        BrowserError::ElementDisabled {
            locator: locator.clone(),
        },
        BrowserError::ElementObscured {
            locator: locator.clone(),
        },
        BrowserError::ElementNotEditable {
            locator: locator.clone(),
        },
        BrowserError::OptionNotFound {
            locator: locator.clone(),
            option: "secret".into(),
        },
        BrowserError::OptionAmbiguous {
            locator: locator.clone(),
            option: "secret".into(),
            matches: 2,
        },
        BrowserError::InvalidKey {
            key: "secret".into(),
        },
        BrowserError::ActionTimeout {
            locator: locator.clone(),
            timeout_ms: 1,
        },
        BrowserError::AssertionFailed {
            locator,
            expected: LocatorState::Visible,
            actual: "secret".into(),
        },
        BrowserError::UrlMismatch {
            expected: "https://example.test/?TOKEN=secret".into(),
            actual: "https://example.test/?token=secret&password=secret".into(),
        },
        BrowserError::NavigationFailed {
            url: "https://example.test/?token=secret".into(),
            reason: "secret".into(),
        },
        BrowserError::NavigationTimeout {
            url: "https://example.test/?token=secret".into(),
            timeout_ms: 1,
        },
        BrowserError::CommandTimeout {
            method: "secret".into(),
            timeout_ms: 1,
        },
        BrowserError::BrowserDisconnected,
        BrowserError::BrowserCrashed {
            status: "secret".into(),
        },
        BrowserError::MalformedProtocol {
            message: "secret".into(),
        },
        BrowserError::Protocol {
            method: "secret".into(),
            message: "secret".into(),
        },
        BrowserError::Launch("secret".into()),
        BrowserError::EvaluationFailed {
            expression: "secret".into(),
            message: "secret".into(),
        },
        BrowserError::UnsupportedCapability {
            capability: "secret".into(),
        },
    ];

    for error in errors {
        let redacted = redact_step_error(
            StepError::Browser(error),
            &[],
            &["secret".into()],
            &["token".into(), "password".into()],
        );
        let reachable = format!("{redacted:?}\n{redacted}");
        assert!(!reachable.contains("secret"), "leaked from {reachable}");
    }
}

#[test]
fn assertion_message_and_diff_are_recomputed_after_redaction() {
    let error = StepError::Assertion(Box::new(AssertionFailure {
        matcher: ValueMatcher::Equal,
        expected: Some(Value::String("secret".into())),
        actual: Value::String("prefix-secret".into()),
        message: "stale secret message".into(),
        diff: ValueDiff::Scalar {
            expected: Some("secret".into()),
            actual: "prefix-secret".into(),
        },
    }));
    let redacted = redact_step_error(error, &[], &["secret".into()], &[]);
    assert!(!format!("{redacted:?}\n{redacted}").contains("secret"));
}

#[test]
fn debugger_snapshots_preserve_utf8_bounds_items_and_secret_overlap() {
    let unicode = "β".repeat(DEBUGGER_SNAPSHOT_BYTES);
    let Value::String(unicode) = debugger_value(&Value::String(unicode), &[], &[]) else {
        panic!("string snapshot")
    };
    assert!(unicode.len() <= DEBUGGER_SNAPSHOT_BYTES);
    assert!(unicode.is_char_boundary(unicode.len()));

    let crossing = format!("{}secret", "a".repeat(DEBUGGER_SNAPSHOT_BYTES - 3));
    let snapshot = debugger_value(&Value::String(crossing), &[], &["secret".into()]);
    let reachable = format!("{snapshot:?}");
    assert!(!reachable.contains("secret"));
    let Value::String(snapshot) = snapshot else {
        panic!("string snapshot")
    };
    assert!(snapshot.len() <= DEBUGGER_SNAPSHOT_BYTES);

    let snapshot = debugger_value(&Value::List((0..400).map(Value::Int).collect()), &[], &[]);
    let Value::List(items) = snapshot else {
        panic!("list snapshot")
    };
    assert!(items.len() <= 255);
}
