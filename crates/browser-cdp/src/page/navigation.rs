use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::{Instant, sleep};
use webtest_browser::BrowserError;

use super::{CdpPage, evaluation};

pub(super) async fn open(page: &CdpPage, url: &str) -> Result<(), BrowserError> {
    let navigation = page
        .connection
        .command(
            "Page.navigate",
            Some(json!({ "url": url })),
            Some(&page.session_id),
        )
        .await?;
    if let Some(reason) = navigation.get("errorText").and_then(Value::as_str) {
        return Err(BrowserError::NavigationFailed {
            url: url.to_owned(),
            reason: reason.to_owned(),
        });
    }

    let deadline = Instant::now() + page.navigation_timeout;
    loop {
        let ready = page
            .connection
            .command(
                "Runtime.evaluate",
                Some(json!({
                    "expression": "document.readyState",
                    "returnByValue": true
                })),
                Some(&page.session_id),
            )
            .await?;
        let state = ready.pointer("/result/value").and_then(Value::as_str);
        if matches!(state, Some("interactive" | "complete")) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(BrowserError::NavigationTimeout {
                url: url.to_owned(),
                timeout_ms: duration_millis(page.navigation_timeout),
            });
        }
        sleep(Duration::from_millis(25)).await;
    }
}

pub(super) async fn wait_for_url(
    page: &CdpPage,
    expected: &str,
    timeout: Duration,
) -> Result<(), BrowserError> {
    let deadline = Instant::now() + timeout;
    loop {
        let actual = current_url(page).await?;
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
    let result = evaluation::evaluate_expression(page, "location.href".into()).await?;
    evaluation::evaluation_value(&result)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| evaluation::invalid_evaluation("current URL was not a string"))
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
