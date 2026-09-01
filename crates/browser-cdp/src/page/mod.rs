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
            connection,
            session_id,
            navigation_timeout,
            test_id_attribute,
        }
    }
}

#[async_trait]
impl Page for CdpPage {
    async fn open(&mut self, url: &str) -> Result<(), BrowserError> {
        navigation::open(self, url, self.navigation_timeout).await
    }

    async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        let timeout = Duration::from_secs(5);
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
        let timeout = Duration::from_secs(5);
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
        navigation::open(self, url, timeout.min(self.navigation_timeout)).await
    }

    async fn evaluate_with_timeout(
        &mut self,
        expression: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        evaluation::evaluate_with_timeout(self, expression, timeout).await
    }

    async fn perform(&mut self, action: &Action, timeout: Duration) -> Result<(), BrowserError> {
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
