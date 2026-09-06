//! Owned wait registrations; readiness, interruption, and cleanup use one driver.
use crate::{ScopeContext, cleanup::CleanupDeadline};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tokio::time::Instant;
use webtest_host::{Cancellation, CancellationReason};
use webtest_model::{ExecutionScopeId, WaitRegistrationId};
use webtest_observation::{WaitEvent, WaitEventKind};

#[async_trait]
pub trait WaitSource: Send {
    type Output: Send;
    type Error: Send;
    async fn ready(&mut self) -> Result<Self::Output, Self::Error>;
    /// Must interrupt underlying host work, not merely discard its Rust future.
    async fn interrupt(&mut self, cause: Cancellation) -> Result<(), Self::Error>;
    /// Releases handlers/registrations on every completion path, including failed readiness.
    async fn cleanup(&mut self) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitInvariant {
    CapacityExceeded,
    LiveRegistration,
}
#[derive(Debug, PartialEq, Eq)]
pub enum WaitCompletion<T, E> {
    Ready(T),
    Failed(E),
    Cancelled(Cancellation),
    Rejected(WaitInvariant),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitCleanupPhase {
    Interrupt,
    Cleanup,
}
#[derive(Debug, PartialEq, Eq)]
pub enum WaitCleanupFailure<E> {
    Failed { phase: WaitCleanupPhase, error: E },
    TimedOut { phase: WaitCleanupPhase },
}
#[derive(Debug, PartialEq, Eq)]
pub struct WaitOutcome<T, E> {
    pub primary: WaitCompletion<T, E>,
    pub secondary: Vec<WaitCleanupFailure<E>>,
    pub cleanup_deadline: Instant,
}

#[derive(Default)]
struct State {
    next: u64,
    active: BTreeMap<WaitRegistrationId, ExecutionScopeId>,
}
pub struct WaitRegistry {
    state: Mutex<State>,
    max_active: usize,
}
impl Default for WaitRegistry {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl WaitRegistry {
    pub fn new(max_active: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            max_active,
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    pub fn validate_owner_finished(&self, owner: ExecutionScopeId) -> Result<(), WaitInvariant> {
        if self.lock().active.values().any(|scope| *scope == owner) {
            Err(WaitInvariant::LiveRegistration)
        } else {
            Ok(())
        }
    }

    pub async fn wait<S: WaitSource>(
        &self,
        context: &ScopeContext,
        source: &mut S,
        cleanup_timeout: Duration,
        mut emit: impl FnMut(WaitEvent) + Send,
    ) -> WaitOutcome<S::Output, S::Error> {
        let registration_id = {
            let mut state = self.lock();
            if state.active.len() >= self.max_active {
                None
            } else {
                let id = WaitRegistrationId(state.next);
                state.next += 1;
                state.active.insert(id, context.scope_id);
                Some(id)
            }
        };
        let Some(registration_id) = registration_id else {
            let cleanup = CleanupDeadline::new(cleanup_timeout);
            let mut secondary = Vec::new();
            record_cleanup(
                cleanup.bound(source.cleanup()).await,
                WaitCleanupPhase::Cleanup,
                &mut secondary,
            );
            return WaitOutcome {
                primary: WaitCompletion::Rejected(WaitInvariant::CapacityExceeded),
                secondary,
                cleanup_deadline: cleanup.instant(),
            };
        };
        emit(WaitEvent {
            registration_id,
            owner_scope_id: context.scope_id,
            kind: WaitEventKind::Registered,
            cancellation: None,
        });
        let deadline = async {
            match context.deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
            Cancellation {
                reason: CancellationReason::Timeout,
                causing_scope_id: context.deadline_scope_id.unwrap_or(context.scope_id),
            }
        };
        let primary = tokio::select! {
            biased;
            cause = context.cancellation.cancelled() => WaitCompletion::Cancelled(cause),
            cause = deadline => { context.cancellation.cancel(cause); WaitCompletion::Cancelled(cause) },
            result = source.ready() => match result { Ok(value) => WaitCompletion::Ready(value), Err(error) => WaitCompletion::Failed(error) },
        };
        let cancellation = match &primary {
            WaitCompletion::Cancelled(cause) => Some(*cause),
            _ => None,
        };
        emit(WaitEvent {
            registration_id,
            owner_scope_id: context.scope_id,
            kind: if cancellation.is_some() {
                WaitEventKind::Cancelled
            } else {
                WaitEventKind::Ready
            },
            cancellation,
        });
        let cleanup = CleanupDeadline::new(cleanup_timeout);
        let mut secondary = Vec::new();
        if let Some(cause) = cancellation {
            record_cleanup(
                cleanup.bound(source.interrupt(cause)).await,
                WaitCleanupPhase::Interrupt,
                &mut secondary,
            );
        }
        record_cleanup(
            cleanup.bound(source.cleanup()).await,
            WaitCleanupPhase::Cleanup,
            &mut secondary,
        );
        self.lock().active.remove(&registration_id);
        WaitOutcome {
            primary,
            secondary,
            cleanup_deadline: cleanup.instant(),
        }
    }
}

fn record_cleanup<E>(
    result: Result<Result<(), E>, tokio::time::error::Elapsed>,
    phase: WaitCleanupPhase,
    failures: &mut Vec<WaitCleanupFailure<E>>,
) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => failures.push(WaitCleanupFailure::Failed { phase, error }),
        Err(_) => failures.push(WaitCleanupFailure::TimedOut { phase }),
    }
}

/// Retry backoff uses the same registration/cleanup protocol as other waits.
pub struct TimerWait {
    pub ready_at: Instant,
}
#[async_trait]
impl WaitSource for TimerWait {
    type Output = ();
    type Error = std::convert::Infallible;
    async fn ready(&mut self) -> Result<(), Self::Error> {
        tokio::time::sleep_until(self.ready_at).await;
        Ok(())
    }
    async fn interrupt(&mut self, _cause: Cancellation) -> Result<(), Self::Error> {
        Ok(())
    }
    async fn cleanup(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FakeSource {
        active: bool,
        ready_after: Duration,
        log: Vec<&'static str>,
        cleanup_fails: bool,
    }
    #[async_trait]
    impl WaitSource for FakeSource {
        type Output = u32;
        type Error = &'static str;
        async fn ready(&mut self) -> Result<u32, Self::Error> {
            self.active = true;
            self.log.push("register");
            tokio::time::sleep(self.ready_after).await;
            self.log.push("ready");
            Ok(42)
        }
        async fn interrupt(&mut self, _cause: Cancellation) -> Result<(), Self::Error> {
            self.log.push("interrupt");
            self.active = false;
            Ok(())
        }
        async fn cleanup(&mut self) -> Result<(), Self::Error> {
            self.log.push("cleanup");
            self.active = false;
            if self.cleanup_fails {
                Err("cleanup")
            } else {
                Ok(())
            }
        }
    }
    fn context() -> ScopeContext {
        ScopeContext {
            scope_id: ExecutionScopeId(1),
            cancellation: crate::CancellationToken::default(),
            deadline: Some(Instant::now() + Duration::from_secs(3)),
            deadline_scope_id: Some(ExecutionScopeId(1)),
        }
    }
    #[tokio::test(start_paused = true)]
    async fn readiness_and_timeout_both_release_the_wait_and_its_handler_once() {
        for (delay, cancelled) in [(1, false), (5, true)] {
            let registry = WaitRegistry::default();
            let context = context();
            let mut source = FakeSource {
                active: false,
                ready_after: Duration::from_secs(delay),
                log: vec![],
                cleanup_fails: false,
            };
            let mut events = Vec::new();
            let result = registry
                .wait(&context, &mut source, Duration::from_secs(1), |event| {
                    events.push(event)
                })
                .await;
            assert_eq!(
                matches!(result.primary, WaitCompletion::Cancelled(_)),
                cancelled
            );
            assert!(!source.active);
            assert!(result.secondary.is_empty());
            assert_eq!(
                source.log,
                if cancelled {
                    vec!["register", "interrupt", "cleanup"]
                } else {
                    vec!["register", "ready", "cleanup"]
                }
            );
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].registration_id, events[1].registration_id);
            registry
                .validate_owner_finished(context.scope_id)
                .expect("no surviving waits");
        }
    }
    #[tokio::test(start_paused = true)]
    async fn cancellation_cause_and_cleanup_failure_do_not_erase_each_other() {
        let context = context();
        let cause = Cancellation {
            reason: CancellationReason::RaceLost,
            causing_scope_id: ExecutionScopeId(9),
        };
        context.cancellation.cancel(cause);
        let mut source = FakeSource {
            active: false,
            ready_after: Duration::from_secs(5),
            log: vec![],
            cleanup_fails: true,
        };
        let outcome = WaitRegistry::default()
            .wait(&context, &mut source, Duration::from_secs(1), |_| {})
            .await;
        assert_eq!(outcome.primary, WaitCompletion::Cancelled(cause));
        assert_eq!(
            outcome.secondary,
            [WaitCleanupFailure::Failed {
                phase: WaitCleanupPhase::Cleanup,
                error: "cleanup"
            }]
        );
        assert_eq!(source.log, ["interrupt", "cleanup"]);
    }
}
