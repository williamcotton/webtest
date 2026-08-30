use base64::Engine;
use serde_json::{Value, json};
use webtest_browser::{EvidenceRequest, PageEvidence};

use super::{
    CdpPage,
    evaluation::{self, evaluation_value, invalid_evaluation},
    locator, redaction,
};

pub(super) async fn capture(page: &CdpPage, request: &EvidenceRequest) -> PageEvidence {
    let mut evidence = PageEvidence::default();
    if request.include_screenshot {
        match page
            .connection
            .command(
                "Page.captureScreenshot",
                Some(json!({ "format": "png", "fromSurface": true })),
                Some(&page.session_id),
            )
            .await
        {
            Ok(value) => {
                match value
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
                {
                    Some(png) => evidence.screenshot_png = Some(png),
                    None => evidence
                        .capture_failures
                        .push("screenshot response was invalid".into()),
                }
            }
            Err(error) => evidence
                .capture_failures
                .push(format!("screenshot: {error}")),
        }
    }
    let page_state = evaluation::evaluate_expression(
        page,
        "({url: location.href, title: document.title})".into(),
    )
    .await;
    match page_state.and_then(|result| {
        evaluation_value(&result)
            .cloned()
            .ok_or_else(|| invalid_evaluation("page state missing"))
    }) {
        Ok(value) => {
            evidence.current_url = value.get("url").and_then(Value::as_str).map(str::to_owned);
            evidence.title = value
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        Err(error) => evidence
            .capture_failures
            .push(format!("page state: {error}")),
    }
    if let Some(locator) = &request.locator {
        match locator::resolve(page, locator).await {
            Ok(snapshot) => {
                evidence.actionability = snapshot.actionability_facts();
                evidence.candidates = snapshot.candidates;
            }
            Err(error) => evidence
                .capture_failures
                .push(format!("locator evidence: {error}")),
        }
    }
    if request.include_dom {
        let expression = "(() => { const root = document.documentElement.cloneNode(true); root.querySelectorAll('input,textarea').forEach(e => { e.removeAttribute('value'); if (e.tagName === 'TEXTAREA') e.textContent = ''; }); return '<!doctype html>' + root.outerHTML; })()";
        match evaluation::evaluate_expression(page, expression.into()).await {
            Ok(result) => match evaluation_value(&result).and_then(Value::as_str) {
                Some(dom) => {
                    evidence.dom_snapshot =
                        Some(redaction::truncate_utf8(dom, request.max_dom_bytes))
                }
                None => evidence
                    .capture_failures
                    .push("DOM snapshot was not a string".into()),
            },
            Err(error) => evidence
                .capture_failures
                .push(format!("DOM snapshot: {error}")),
        }
    }
    evidence.console_errors = page.connection.console_errors().await;
    redaction::redact_evidence(
        &mut evidence,
        &request.redactions,
        &request.redacted_query_parameters,
    );
    evidence
}
