use webtest_analysis::{Diagnostic, DiagnosticSeverity};
use webtest_plan::{AssertionOperation, BrowserOperation, PlanExpr, TestOperation, TestPlan};
use webtest_project::Project;
use webtest_provider::Value;

use crate::{
    report::DiagnosticReport,
    source_output::{machine_source, source_span},
};

pub(crate) fn diagnostic_report(
    path: &str,
    source_revision: &str,
    source: &str,
    diagnostic: &Diagnostic,
) -> DiagnosticReport {
    let span = source_span(source, diagnostic.range);
    DiagnosticReport {
        diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        severity: match diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Information => "information",
            DiagnosticSeverity::Hint => "hint",
        }
        .into(),
        code: diagnostic.code.into(),
        message: diagnostic.message.clone(),
        source: machine_source(path, source_revision, &span),
        span,
        semantic_details: diagnostic.semantic_details.clone(),
        repair_hints: diagnostic.repair_hints.clone(),
        reference_queries: diagnostic.reference_queries.clone(),
    }
}

pub(crate) fn config_diagnostics(
    project: &Project,
    path: &str,
    source_revision: &str,
    source: &str,
    plan: &TestPlan,
) -> Vec<DiagnosticReport> {
    let mut diagnostics = Vec::new();
    for test in &plan.tests {
        for step in &test.steps {
            let (url, timeout) = match &step.operation {
                TestOperation::Browser(BrowserOperation::Navigate { url }) => {
                    (literal_string(url), None)
                }
                TestOperation::Browser(BrowserOperation::WaitForUrl { url, timeout })
                | TestOperation::Assertion(AssertionOperation::Url { url, timeout }) => {
                    (literal_string(url), *timeout)
                }
                TestOperation::Browser(BrowserOperation::WaitForLocator { timeout, .. })
                | TestOperation::Assertion(AssertionOperation::Locator { timeout, .. }) => {
                    (None, *timeout)
                }
                _ => (None, None),
            };
            if let Some(url) = url
                && !is_absolute_config_url(url)
                && project.config.browser.base_url.is_none()
            {
                let span = source_span(source, step.origin.range);
                diagnostics.push(DiagnosticReport {
                    diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                    repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                    severity: "error".into(),
                    code: "config.missing_base_url".into(),
                    message: format!("relative URL {url:?} requires browser.base_url"),
                    source: machine_source(path, source_revision, &span),
                    span,
                    semantic_details: Some(serde_json::json!({
                        "url": url,
                        "required_configuration": "browser.base_url",
                    })),
                    repair_hints: Vec::new(),
                    reference_queries: vec!["browser.open".into()],
                });
            }
            if timeout.is_some_and(|timeout| timeout > project.config.timeouts.test) {
                let span = source_span(source, step.origin.range);
                diagnostics.push(DiagnosticReport {
                    diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                    repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                    severity: "error".into(),
                    code: "config.timeout_exceeds_test".into(),
                    message: "step timeout must not exceed timeouts.test".into(),
                    source: machine_source(path, source_revision, &span),
                    span,
                    semantic_details: Some(serde_json::json!({
                        "test_timeout_ms": project.config.timeouts.test.as_millis(),
                        "step_timeout_ms": timeout.map(|timeout| timeout.as_millis()),
                    })),
                    repair_hints: Vec::new(),
                    reference_queries: vec![
                        "browser.wait.locator".into(),
                        "browser.wait.url".into(),
                    ],
                });
            }
        }
    }
    diagnostics
}

fn is_absolute_config_url(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && !rest.is_empty()
            && scheme.chars().next().is_some_and(char::is_alphabetic)
            && scheme.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            })
    })
}

fn literal_string(expression: &PlanExpr) -> Option<&str> {
    match expression {
        PlanExpr::Literal(Value::String(value)) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_url_recognition_matches_configuration_policy() {
        for absolute in [
            "http://example.test",
            "https://example.test",
            "custom+test:value",
            "a-b.c:value",
        ] {
            assert!(is_absolute_config_url(absolute), "{absolute}");
        }
        for relative in ["/login", "login", ":missing", "1http:value", "http:"] {
            assert!(!is_absolute_config_url(relative), "{relative}");
        }
    }
}
