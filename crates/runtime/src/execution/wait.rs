use super::TestBodyOutcome;
use crate::{RunControl, ScopeContext, WaitSource};
use async_trait::async_trait;
use std::{future::Future, pin::Pin};
use webtest_host::Cancellation;
use webtest_observation::CleanupCause;

/// Provider-only bodies retain their future until cancellation-aware hosts finish
/// explicit interruption. The wait driver separately bounds this cleanup phase.
pub(super) struct TestBodyWait<F> {
    pub future: Option<Pin<Box<F>>>,
}
#[async_trait]
impl<F> WaitSource for TestBodyWait<F>
where
    F: Future<Output = TestBodyOutcome> + Send,
{
    type Output = TestBodyOutcome;
    type Error = CleanupCause;
    async fn ready(&mut self) -> Result<Self::Output, Self::Error> {
        let future = self.future.as_mut().ok_or_else(|| CleanupCause::Internal {
            message: "test body wait was already released".into(),
        })?;
        Ok(future.await)
    }
    async fn interrupt(&mut self, _: Cancellation) -> Result<(), Self::Error> {
        if let Some(future) = self.future.as_mut() {
            let result = future.await;
            if let TestBodyOutcome::PendingFailure(pending) = result {
                pending.interruption_cleanup()?;
            }
        }
        Ok(())
    }
    async fn cleanup(&mut self) -> Result<(), Self::Error> {
        self.future.take();
        Ok(())
    }
}

pub(super) async fn with_control<F: Future>(
    control: Option<&dyn RunControl>,
    context: &ScopeContext,
    future: F,
) -> F::Output {
    let Some(control) = control else {
        return future.await;
    };
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => result,
        _ = control.cancelled() => {
            context.cancellation.cancel(Cancellation { reason: control.cancellation_reason(), causing_scope_id: context.scope_id });
            future.await
        }
    }
}
