use std::time::Duration;

use async_trait::async_trait;
use std::future::Future;
use tokio::time::Instant;
use webtest_browser::{
    Action, BrowserError, EvidenceRequest, InspectionOptions, Locator, LocatorState, Page,
    PageEvidence, PageInspection,
};

use crate::connection::CdpConnection;

mod actions;
mod evaluation;
mod evidence;
mod inspection;
mod locator;
mod navigation;
mod redaction;

pub(crate) struct CdpPage {
    operation_context: Option<std::sync::Arc<dyn webtest_host::OperationContext>>,
    connection: CdpConnection,
    session_id: String,
    navigation_timeout: Duration,
    test_id_attribute: String,
}

impl CdpPage {
    pub(crate) fn new(
        connection: CdpConnection,
        session_id: String,
        navigation_timeout: Duration,
        test_id_attribute: String,
    ) -> Self {
        Self {
            operation_context: None,
            connection,
            session_id,
            navigation_timeout,
            test_id_attribute,
        }
    }

    fn operation_timeout(&self, timeout: Duration) -> Duration {
        self.operation_context
            .as_ref()
            .and_then(|context| context.remaining())
            .map_or(timeout, |remaining| remaining.min(timeout))
    }
}

#[async_trait]
impl Page for CdpPage {
    async fn start_reactive(
        &mut self,
        observer: std::sync::Arc<dyn webtest_browser::reactive::ReactiveObserver>,
        sources: &[webtest_model::EventSource],
    ) -> Result<(), BrowserError> {
        self.connection
            .reactive_start(self.session_id.clone(), observer, sources);
        if sources.iter().any(|s| {
            matches!(
                s,
                webtest_model::EventSource::NetworkRequest
                    | webtest_model::EventSource::NetworkResponse
                    | webtest_model::EventSource::WebsocketReceived
                    | webtest_model::EventSource::WebsocketSent
                    | webtest_model::EventSource::WebsocketReceivedJson
                    | webtest_model::EventSource::WebsocketSentJson
            )
        }) {
            self.connection
                .command(
                    "Network.enable",
                    Some(serde_json::json!({"maxTotalBufferSize":0,"maxResourceBufferSize":0})),
                    Some(&self.session_id),
                )
                .await?;
        }
        Ok(())
    }
    async fn reactive_barrier(&mut self) -> Result<(), BrowserError> {
        self.connection
            .command(
                "Runtime.evaluate",
                Some(serde_json::json!({"expression":"void 0","returnByValue":true})),
                Some(&self.session_id),
            )
            .await
            .map(|_| ())
    }

    fn set_operation_context(
        &mut self,
        context: Option<std::sync::Arc<dyn webtest_host::OperationContext>>,
    ) {
        self.operation_context = context;
    }
    async fn open(&mut self, url: &str) -> Result<(), BrowserError> {
        self.open_with_timeout(url, self.navigation_timeout).await
    }

    async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        let timeout = self.operation_timeout(Duration::from_secs(5));
        actions::perform(
            self,
            &Action::Click {
                locator: locator.clone(),
            },
            timeout,
            Instant::now() + timeout,
        )
        .await
    }

    async fn expect_visible(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        let timeout = self.operation_timeout(Duration::from_secs(5));
        locator::wait_for_locator(
            self,
            locator,
            LocatorState::Visible,
            Instant::now() + timeout,
        )
        .await
    }

    async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError> {
        evaluation::evaluate(self, expression).await
    }

    async fn open_with_timeout(
        &mut self,
        url: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        navigation::open(
            self,
            url,
            self.operation_timeout(timeout.min(self.navigation_timeout)),
        )
        .await
    }

    async fn evaluate_with_timeout(
        &mut self,
        expression: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        evaluation::evaluate_with_timeout(self, expression, self.operation_timeout(timeout)).await
    }

    async fn perform(&mut self, action: &Action, timeout: Duration) -> Result<(), BrowserError> {
        let timeout = self.operation_timeout(timeout);
        let deadline = Instant::now() + timeout;
        match complete_before_deadline(deadline, actions::perform(self, action, timeout, deadline))
            .await
        {
            Ok(result) => result,
            Err(_) => Err(BrowserError::ActionTimeout {
                locator: action.locator().clone(),
                timeout_ms: duration_millis(timeout),
            }),
        }
    }

    async fn wait_for_locator(
        &mut self,
        locator: &Locator,
        state: LocatorState,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        let timeout = self.operation_timeout(timeout);
        let deadline = Instant::now() + timeout;
        match complete_before_deadline(
            deadline,
            locator::wait_for_locator(self, locator, state, deadline),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(BrowserError::AssertionFailed {
                locator: locator.clone(),
                expected: state,
                actual: format!("timed out after {}ms", duration_millis(timeout)),
            }),
        }
    }

    async fn wait_for_url(
        &mut self,
        expected: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        let timeout = self.operation_timeout(timeout);
        let deadline = Instant::now() + timeout;
        match complete_before_deadline(deadline, navigation::wait_for_url(self, expected, deadline))
            .await
        {
            Ok(result) => result,
            Err(_) => Err(BrowserError::UrlMismatch {
                expected: expected.into(),
                actual: format!("<timed out after {}ms>", duration_millis(timeout)),
            }),
        }
    }

    async fn current_url(&mut self) -> Result<String, BrowserError> {
        navigation::current_url(self).await
    }

    async fn capture_evidence(&mut self, request: &EvidenceRequest) -> PageEvidence {
        evidence::capture(self, request).await
    }

    async fn inspect(
        &mut self,
        options: &InspectionOptions,
    ) -> Result<PageInspection, BrowserError> {
        inspection::inspect(self, options).await
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

async fn complete_before_deadline<F>(deadline: Instant, future: F) -> Result<F::Output, ()>
where
    F: Future,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => Ok(result),
        _ = tokio::time::sleep_until(deadline) => Err(()),
    }
}

impl Drop for CdpPage {
    fn drop(&mut self) {
        self.connection.reactive_stop(&self.session_id);
    }
}
