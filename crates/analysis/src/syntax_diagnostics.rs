//! Ordered adaptation of parser and lossless-CST facts into analysis diagnostics.

use crate::diagnostic::{Diagnostic, DiagnosticSeverity, DiagnosticSource};
use webtest_syntax::Parse;

pub(crate) fn collect(parsed: &Parse) -> Vec<Diagnostic> {
    let mut diagnostics = parsed
        .errors()
        .iter()
        .map(|error| Diagnostic {
            range: error.range,
            severity: DiagnosticSeverity::Error,
            code: error.code,
            message: error.message.clone(),
            source: DiagnosticSource::Syntax,
            semantic_details: None,
            repair_hints: Vec::new(),
            reference_queries: syntax_reference_queries(parsed, error),
        })
        .collect::<Vec<_>>();
    diagnostics.extend(invalid_duration_diagnostics(parsed));
    diagnostics
}

fn syntax_reference_queries(parsed: &Parse, error: &webtest_syntax::SyntaxError) -> Vec<String> {
    let mut queries = vec!["grammar".into()];
    match error.code {
        "syntax.expected_server_statement" => queries.push("scope.server".into()),
        "syntax.expected_browser_statement" => queries.push("scope.browser".into()),
        _ => return queries,
    }
    let offending = parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.text_range() == error.range)
        .map(|token| token.text().to_owned());
    let reference = match offending.as_deref() {
        Some("open") => Some("browser.open"),
        Some("evaluate") => Some("browser.evaluate"),
        Some("click") => Some("browser.click"),
        Some("fill") => Some("browser.fill"),
        Some("type") => Some("browser.type"),
        Some("press") => Some("browser.press"),
        Some("check") => Some("browser.check"),
        Some("uncheck") => Some("browser.uncheck"),
        Some("select") => Some("browser.select"),
        Some("hover") => Some("browser.hover"),
        Some("wait") => Some("browser.wait.locator"),
        _ => None,
    };
    if let Some(reference) = reference {
        queries.push(reference.into());
    }
    queries
}

fn invalid_duration_diagnostics(parsed: &Parse) -> Vec<Diagnostic> {
    parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| {
            let token = element.into_token()?;
            if token.kind() != webtest_syntax::SyntaxKind::Duration {
                return None;
            }
            let valid = token
                .text()
                .strip_suffix("ms")
                .or_else(|| token.text().strip_suffix('s'))
                .or_else(|| token.text().strip_suffix('m'))
                .and_then(|number| number.parse::<u64>().ok())
                .is_some_and(|number| number > 0);
            (!valid).then(|| Diagnostic {
                range: token.text_range(),
                severity: DiagnosticSeverity::Error,
                code: "semantic.invalid_duration",
                message: format!("invalid positive duration `{}`", token.text()),
                source: DiagnosticSource::Semantic,
                semantic_details: Some(serde_json::json!({ "literal": token.text() })),
                repair_hints: Vec::new(),
                reference_queries: vec!["grammar".into(), "type.Duration".into()],
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_diagnostics_precede_duration_diagnostics_and_source_stays_lossless() {
        let source = r#"test "x" { browser { wait id("x") 0ms open } }"#;
        let parsed = webtest_syntax::parse(source);
        assert_eq!(parsed.syntax().text().to_string(), source);
        let diagnostics = collect(&parsed);
        let duration = diagnostics
            .iter()
            .position(|diagnostic| diagnostic.code == "semantic.invalid_duration")
            .expect("duration diagnostic");
        assert!(
            diagnostics[..duration]
                .iter()
                .all(|diagnostic| diagnostic.source == DiagnosticSource::Syntax)
        );
    }

    #[test]
    fn illegal_browser_action_in_server_scope_links_to_the_authoritative_reference() {
        let parsed = webtest_syntax::parse(
            r#"test "illegal" { server { click role("button", name: "Sign in") } }"#,
        );
        let diagnostics = collect(&parsed);
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "syntax.expected_server_statement")
            .expect("server-scope syntax diagnostic");
        assert!(
            diagnostic
                .reference_queries
                .contains(&"scope.server".into())
        );
        assert!(
            diagnostic
                .reference_queries
                .contains(&"browser.click".into())
        );
    }

    #[test]
    fn zero_and_overflow_durations_are_rejected_in_cst_order() {
        let parsed = webtest_syntax::parse(
            r#"test "durations" { browser { wait id("x") 0ms wait id("y") 18446744073709551616ms } }"#,
        );
        let diagnostics = collect(&parsed)
            .into_iter()
            .filter(|diagnostic| diagnostic.code == "semantic.invalid_duration")
            .collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].range.start() < diagnostics[1].range.start());
        assert_eq!(
            diagnostics[0].reference_queries,
            vec!["grammar", "type.Duration"]
        );
    }
}
