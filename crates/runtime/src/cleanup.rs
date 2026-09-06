use crate::{CleanupCause, CleanupFailure, CleanupResource};
use std::{future::Future, time::Duration};
use tokio::time::Instant;

/// All cleanup phases in one scope share a single absolute budget.
#[derive(Clone, Copy)]
pub(crate) struct CleanupDeadline {
    at: Instant,
    budget: Duration,
}
impl CleanupDeadline {
    pub(crate) fn new(budget: Duration) -> Self {
        Self {
            at: Instant::now() + budget,
            budget,
        }
    }
    pub(crate) fn at(at: Instant, budget: Duration) -> Self {
        Self { at, budget }
    }
    pub(crate) fn instant(self) -> Instant {
        self.at
    }
    pub(crate) async fn bound<F: Future>(
        self,
        future: F,
    ) -> Result<F::Output, tokio::time::error::Elapsed> {
        tokio::time::timeout_at(self.at, future).await
    }
    pub(crate) async fn run<T, E>(
        self,
        resource: CleanupResource,
        future: impl Future<Output = Result<T, E>>,
        cause: impl FnOnce(E) -> CleanupCause,
    ) -> Result<T, CleanupFailure> {
        match self.bound(future).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(CleanupFailure {
                resource,
                cause: cause(error),
            }),
            Err(_) => Err(CleanupFailure {
                resource,
                cause: CleanupCause::TimedOut {
                    timeout_ms: self.budget.as_millis().min(u128::from(u64::MAX)) as u64,
                },
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn cleanup_budget_is_separate_from_execution_and_shared_by_cleanup_phases() {
        let deadline = CleanupDeadline::new(Duration::from_secs(3));
        let result = deadline
            .run(
                CleanupResource::BrowserContext,
                async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok::<_, std::io::Error>(())
                },
                |error| CleanupCause::Io(error.into()),
            )
            .await;
        assert!(result.is_ok());
        let result = deadline
            .run(
                CleanupResource::BrowserSession,
                std::future::pending::<Result<(), std::io::Error>>(),
                |error| CleanupCause::Io(error.into()),
            )
            .await;
        assert!(matches!(
            result,
            Err(CleanupFailure {
                cause: CleanupCause::TimedOut { timeout_ms: 3000 },
                ..
            })
        ));
    }
}
