use crate::{AcquisitionOwnership, ResourceAdapter, ResourceFailure, ScopeContext};
use async_trait::async_trait;
use webtest_browser::{
    BrowserContext, BrowserContextOptions, BrowserError, BrowserHost, BrowserSession, Page,
};
use webtest_host::Cancellation;

pub(super) struct BrowserResource<'a> {
    pub host: &'a dyn BrowserHost,
    pub session: &'a mut Option<Box<dyn BrowserSession>>,
    pub options: &'a BrowserContextOptions,
    pub context: Option<Box<dyn BrowserContext>>,
}

#[async_trait]
impl ResourceAdapter for BrowserResource<'_> {
    type Handle = Box<dyn Page>;
    type Error = BrowserError;

    fn cooperative_body_interruption(&self) -> bool {
        true
    }

    async fn acquire(
        &mut self,
        _: &ScopeContext,
        ownership: &AcquisitionOwnership<'_>,
    ) -> Result<Self::Handle, ResourceFailure<Self::Error>> {
        if self.session.is_none() {
            *self.session = Some(self.host.start().await.map_err(ResourceFailure::Host)?);
        }
        let session = self.session.as_mut().ok_or(ResourceFailure::Invariant(
            crate::ResourceInvariant::InvalidTransition,
        ))?;
        self.context = Some(
            session
                .new_context(self.options)
                .await
                .map_err(ResourceFailure::Host)?,
        );
        ownership.acquired()?;
        let context = self.context.as_mut().ok_or(ResourceFailure::Invariant(
            crate::ResourceInvariant::InvalidTransition,
        ))?;
        context.new_page().await.map_err(ResourceFailure::Host)
    }

    async fn interrupt(&mut self, cause: Cancellation) -> Result<(), Self::Error> {
        if let Some(context) = self.context.as_mut() {
            context.interrupt(cause).await?;
        }
        Ok(())
    }

    async fn teardown(&mut self) -> Result<(), Self::Error> {
        if let Some(mut context) = self.context.take() {
            context.close().await?;
        }
        Ok(())
    }
}
