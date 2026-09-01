use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    sync::broadcast,
    time::{Instant, sleep},
};
use webtest_browser::BrowserError;

use crate::{connection::CdpEvent, wire::string_field};

use super::{CdpPage, evaluation};

const ACTION_NAVIGATION_GRACE: Duration = Duration::from_millis(100);

pub(super) struct NavigationWatcher {
    events: broadcast::Receiver<CdpEvent>,
}

pub(super) fn watch(page: &CdpPage) -> NavigationWatcher {
    NavigationWatcher {
        events: page.connection.subscribe(),
    }
}

pub(super) async fn open(page: &CdpPage, url: &str, timeout: Duration) -> Result<(), BrowserError> {
    let deadline = Instant::now() + timeout;
    let mut watcher = watch(page);
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
    let frame_id = string_field(&navigation, "frameId", "Page.navigate")?;
    let Some(loader_id) = navigation.get("loaderId").and_then(Value::as_str) else {
        return Ok(());
    };
    if wait_for_lifecycle(
        page,
        &mut watcher.events,
        &frame_id,
        Some(loader_id),
        deadline,
    )
    .await?
    {
        Ok(())
    } else {
        Err(BrowserError::NavigationTimeout {
            url: url.to_owned(),
            timeout_ms: duration_millis(timeout),
        })
    }
}

pub(super) async fn wait_for_navigation_after_action(
    page: &CdpPage,
    watcher: &mut NavigationWatcher,
    deadline: Instant,
) -> Result<bool, BrowserError> {
    let grace_deadline = deadline.min(Instant::now() + ACTION_NAVIGATION_GRACE);
    let Some(frame_id) =
        wait_for_requested_navigation(page, &mut watcher.events, grace_deadline).await?
    else {
        return Ok(true);
    };
    wait_for_lifecycle(page, &mut watcher.events, &frame_id, None, deadline).await
}

async fn wait_for_requested_navigation(
    page: &CdpPage,
    events: &mut broadcast::Receiver<CdpEvent>,
    deadline: Instant,
) -> Result<Option<String>, BrowserError> {
    loop {
        let event = match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(event)) => event,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Err(BrowserError::BrowserDisconnected);
            }
            Err(_) => return Ok(None),
        };
        if let Some(error) = event.terminal {
            return Err(error);
        }
        if event.session_id.as_deref() != Some(page.session_id.as_str())
            || event.method != "Page.frameRequestedNavigation"
        {
            continue;
        }
        if let Some(frame_id) = event.params.get("frameId").and_then(Value::as_str) {
            return Ok(Some(frame_id.to_owned()));
        }
    }
}

async fn wait_for_lifecycle(
    page: &CdpPage,
    events: &mut broadcast::Receiver<CdpEvent>,
    frame_id: &str,
    loader_id: Option<&str>,
    deadline: Instant,
) -> Result<bool, BrowserError> {
    loop {
        let event = match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(event)) => event,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Err(BrowserError::BrowserDisconnected);
            }
            Err(_) => return Ok(false),
        };
        if let Some(error) = event.terminal {
            return Err(error);
        }
        if event.session_id.as_deref() != Some(page.session_id.as_str()) {
            continue;
        }
        if event.method == "Page.navigatedWithinDocument"
            && loader_id.is_none()
            && event.params.get("frameId").and_then(Value::as_str) == Some(frame_id)
        {
            return Ok(true);
        }
        if event.method != "Page.lifecycleEvent"
            || event.params.get("frameId").and_then(Value::as_str) != Some(frame_id)
            || event.params.get("name").and_then(Value::as_str) != Some("load")
            || loader_id.is_some_and(|expected| {
                event.params.get("loaderId").and_then(Value::as_str) != Some(expected)
            })
        {
            continue;
        }
        return Ok(true);
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
