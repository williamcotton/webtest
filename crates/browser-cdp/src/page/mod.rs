use std::time::Duration;

use async_trait::async_trait;
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
        navigation::open(self, url).await
    }

    async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        actions::perform(
            self,
            &Action::Click {
                locator: locator.clone(),
            },
            Duration::from_secs(5),
        )
        .await
    }

    async fn expect_visible(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        locator::wait_for_locator(self, locator, LocatorState::Visible, Duration::from_secs(5))
            .await
    }

    async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError> {
        evaluation::evaluate(self, expression).await
    }

    async fn perform(&mut self, action: &Action, timeout: Duration) -> Result<(), BrowserError> {
        actions::perform(self, action, timeout).await
    }

    async fn wait_for_locator(
        &mut self,
        locator: &Locator,
        state: LocatorState,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        locator::wait_for_locator(self, locator, state, timeout).await
    }

    async fn wait_for_url(
        &mut self,
        expected: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        navigation::wait_for_url(self, expected, timeout).await
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
