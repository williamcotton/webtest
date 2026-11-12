use crate::{AcquisitionOwnership, ResourceAdapter, ResourceFailure, ScopeContext};
use async_trait::async_trait;
use webtest_browser::{
    BrowserContext, BrowserContextOptions, BrowserError, BrowserHost, BrowserSession, Page,
};
use webtest_host::Cancellation;

pub(super) struct BrowserResource<'a> {
    pub host: &'a dyn BrowserHost,
    pub session: &'a mut Option<OwnedBrowserSession>,
    pub owner: Option<&'a webtest_observation::ScopeEvent>,
    pub acquisition: &'a mut Option<webtest_observation::ResourceKey>,
    pub events: super::temporary::ResourceEvents<'a>,
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
            let event = self.events.registry.begin_acquisition(
                self.owner.map(|scope| scope.execution_context.scope_id),
                webtest_observation::ResourceKind::BrowserSession,
                webtest_observation::ResourceAccess::Exclusive,
            );
            let key = event.resource.key;
            *self.acquisition = Some(key);
            emit_session_event(&self.events, event, self.owner);
            let handle = self.host.start().await.map_err(ResourceFailure::Host)?;
            // Retain physical ownership before any fallible metadata transition.
            *self.session = Some(OwnedBrowserSession {
                handle,
                key,
                scope: self.owner.cloned(),
            });
            self.events
                .registry
                .acknowledge_ownership(key)
                .map_err(ResourceFailure::Invariant)?;
            let event = self
                .events
                .registry
                .ready(key)
                .map_err(ResourceFailure::Invariant)?;
            emit_session_event(&self.events, event, self.owner);
            *self.acquisition = None;
        }
        let session = self.session.as_mut().ok_or(ResourceFailure::Invariant(
            crate::ResourceInvariant::InvalidTransition,
        ))?;
        self.context = Some(
            session
                .handle
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

/// Sequential reuse belongs to the file run. Isolated test roots and sibling
/// branches retain their explicit owner and close their handles before joining.
pub(crate) struct OwnedBrowserSession {
    handle: Box<dyn BrowserSession>,
    key: webtest_observation::ResourceKey,
    scope: Option<webtest_observation::ScopeEvent>,
}

fn emit_session_event(
    output: &super::temporary::ResourceEvents<'_>,
    event: webtest_observation::ResourceEvent,
    scope: Option<&webtest_observation::ScopeEvent>,
) {
    crate::events::emit_event(
        output.events,
        output.sink,
        webtest_observation::ExecutionEvent::Resource {
            execution_id: output.execution_id,
            scope: scope.cloned(),
            event,
        },
    );
}

pub(super) fn finish_session_acquisition(
    key: webtest_observation::ResourceKey,
    scope: Option<&webtest_observation::ScopeEvent>,
    output: &super::temporary::ResourceEvents<'_>,
) -> Result<(), crate::ResourceInvariant> {
    emit_session_event(output, output.registry.acquire_failed(key)?, scope);
    Ok(())
}

impl OwnedBrowserSession {
    pub async fn close(
        &mut self,
        deadline: crate::cleanup::CleanupDeadline,
        output: &super::temporary::ResourceEvents<'_>,
        cancellation: Option<webtest_host::Cancellation>,
    ) -> Result<(), crate::CleanupFailure> {
        if let Some(cause) = cancellation {
            output
                .registry
                .cancel(self.key, cause)
                .map_err(super::resource_cleanup_invariant)?;
            if let Some(scope) = &mut self.scope {
                scope.cancellation = Some(cause);
            }
        }
        let event = output
            .registry
            .begin_release(self.key)
            .map_err(super::resource_cleanup_invariant)?;
        emit_session_event(output, event, self.scope.as_ref());
        let result = deadline
            .run(
                crate::CleanupResource::BrowserSession,
                self.handle.close(),
                crate::CleanupCause::Browser,
            )
            .await;
        let event = output
            .registry
            .finish_release(self.key, result.is_ok())
            .map_err(super::resource_cleanup_invariant)?;
        emit_session_event(output, event, self.scope.as_ref());
        result
    }
}
