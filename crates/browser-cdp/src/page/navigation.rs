use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::{Instant, sleep};
use webtest_browser::BrowserError;

use super::{CdpPage, evaluation};

pub(super) async fn open(page: &CdpPage, url: &str, timeout: Duration) -> Result<(), BrowserError> {
    let deadline = Instant::now() + timeout;
    let navigation = page
        .connection
        .command_with_timeout(
            "Page.navigate",
            Some(json!({ "url": url })),
            Some(&page.session_id),
            deadline.saturating_duration_since(Instant::now()),
        )
        .await?;
    if let Some(reason) = navigation.get("errorText").and_then(Value::as_str) {
        return Err(BrowserError::NavigationFailed {
            url: url.to_owned(),
            reason: reason.to_owned(),
        });
    }

    loop {
        let ready = page
            .connection
            .command_with_timeout(
                "Runtime.evaluate",
                Some(json!({
                    "expression": "document.readyState",
                    "returnByValue": true
                })),
                Some(&page.session_id),
                deadline.saturating_duration_since(Instant::now()),
            )
            .await?;
        let state = ready.pointer("/result/value").and_then(Value::as_str);
        if matches!(state, Some("interactive" | "complete")) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(BrowserError::NavigationTimeout {
                url: url.to_owned(),
                timeout_ms: duration_millis(timeout),
            });
        }
        sleep(Duration::from_millis(25)).await;
    }
}

pub(super) async fn wait_for_url(
    page: &CdpPage,
    expected: &str,
    deadline: Instant,
) -> Result<(), BrowserError> {
    loop {
        let actual =
            current_url_with_timeout(page, deadline.saturating_duration_since(Instant::now()))
                .await?;
        if actual == expected {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(BrowserError::UrlMismatch {
                expected: expected.into(),
                actual,
            });
        }
        sleep(Duration::from_millis(25)).await;
    }
}

pub(super) async fn current_url(page: &CdpPage) -> Result<String, BrowserError> {
    current_url_with_timeout(page, page.connection.command_timeout()).await
}

async fn current_url_with_timeout(
    page: &CdpPage,
    timeout: Duration,
) -> Result<String, BrowserError> {
    let result =
        evaluation::evaluate_expression_with_timeout(page, "location.href".into(), timeout).await?;
    evaluation::evaluation_value(&result)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| evaluation::invalid_evaluation("current URL was not a string"))
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
