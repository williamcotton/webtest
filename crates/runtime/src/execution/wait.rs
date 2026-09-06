use super::{ProvisionalTestOutcome, TestBodyOutcome};
use crate::{RunControl, ScopeContext, WaitSource};
use async_trait::async_trait;
use std::{future::Future, pin::Pin};
use webtest_host::Cancellation;
use webtest_observation::CleanupCause;

/// Provider-only bodies retain their future until cancellation-aware hosts finish
/// explicit interruption. The wait driver separately bounds this cleanup phase.
pub(super) struct TestBodyWait<F> {
    pub future: Option<Pin<Box<F>>>,
    /// A resource may already have failed before an enclosing cancellation is
    /// delivered during its teardown. Its eventual primary failure must survive
    /// the wait's cancellation result.
    pub interrupted_failure: Option<TestBodyOutcome>,
}

impl<F> TestBodyWait<F> {
    pub fn new(future: F) -> Self {
        Self {
            future: Some(Box::pin(future)),
            interrupted_failure: None,
        }
    }
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
            match result {
                TestBodyOutcome::PendingFailure(pending) => pending.interruption_cleanup()?,
                result @ TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                    ..
                }) => self.interrupted_failure = Some(result),
                result @ TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(_))
                    if result
                        .failure_class()
                        .is_some_and(|class| class != crate::FailureClass::Test) =>
                {
                    self.interrupted_failure = Some(result)
                }
                _ => {}
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
