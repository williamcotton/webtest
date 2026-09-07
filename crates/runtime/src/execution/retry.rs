use super::{child::ObservationProjection, *};
use crate::{BranchResult, StepError, TimerWait, WaitCompletion};
use webtest_plan::{PlanNode, RetryPolicy, RetrySettings};

impl TreeExecution<'_, '_> {
    pub(super) async fn retry_node(
        &mut self,
        child: &PlanNode,
        settings: RetrySettings,
        parent: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let mut observations = ObservationProjection::new(self.services);
        let services = ExecutionServices {
            observations: &observations.pending,
            ..*self.services
        };
        self.branch.active_step = None;
        for ordinal in 0..settings.attempts {
            if let Some(cause) = parent.context.cancellation.cause() {
                return cancelled(cause.reason);
            }
            let scope = services.scopes.attempt(parent, child);
            let mut state = self.branch.fork();
            // A sequential attempt may exclusively borrow a proven observation-only
            // browser context. Its lexical owner still controls interruption and
            // teardown. No sibling future receives this handle.
            state.page = self.branch.page.take();
            let completion = child::execute_child(&services, child, scope, state).await;
            self.branch.page = completion.page;
            let passed = matches!(completion.result.outcome, TestOutcome::Passed);
            let retryable = match settings.policy {
                RetryPolicy::SafeFailures => retryable_result(&completion.result),
            };
            let outcome = completion.result.outcome.clone();
            self.branch.completed_branches.push(completion.result);
            if passed {
                if let Some(cause) = parent.context.cancellation.cause() {
                    return cancelled(cause.reason);
                }
                if let Some(transfer) = completion.provided {
                    self.branch.provided = Some(self.branch.bindings.accept_transfer(transfer));
                }
                observations.recovered = true;
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed);
            }
            if !retryable || ordinal + 1 == settings.attempts {
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(Box::new(
                    outcome,
                )));
            }
            // Completion includes terminal resource/scope events. Register backoff
            // only after all attempt teardown; inherited deadlines never restart.
            let mut timer = TimerWait {
                ready_at: Instant::now() + settings.backoff.delay(ordinal),
            };
            let cleanup_timeout = parent.cleanup_timeout(services.options.cleanup_timeout);
            let result = services
                .waits
                .wait(&parent.context, &mut timer, cleanup_timeout, |event| {
                    emit_event(
                        services.events,
                        services.event_sink,
                        ExecutionEvent::Wait {
                            execution_id: services.execution_id,
                            scope: parent.event.clone(),
                            event,
                        },
                    )
                })
                .await;
            for failure in result.secondary {
                let cause = match failure {
                    crate::WaitCleanupFailure::Failed { error, .. } => match error {},
                    crate::WaitCleanupFailure::TimedOut { .. } => CleanupCause::TimedOut {
                        timeout_ms: duration_millis(cleanup_timeout),
                    },
                };
                self.branch.cleanup_failures.push(CleanupFailure {
                    resource: CleanupResource::ExecutionScope {
                        scope_id: parent.id(),
                    },
                    cause,
                });
            }
            match result.primary {
                WaitCompletion::Ready(()) if self.branch.cleanup_failures.is_empty() => {}
                WaitCompletion::Cancelled(cause) => return cancelled(cause.reason),
                WaitCompletion::Failed(error) => match error {},
                WaitCompletion::Rejected(error) => {
                    return internal(format!("retry backoff registration invariant: {error:?}"));
                }
                WaitCompletion::Ready(()) => {
                    return internal("retry backoff cleanup failed".into());
                }
            }
        }
        internal("retry settings were not validated".into())
    }
}

fn cancelled(reason: CancellationReason) -> TestBodyOutcome {
    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled { reason })
}

fn internal(message: String) -> TestBodyOutcome {
    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
        failure: RunError::Internal(message),
    })
}

/// Inspect every unrecovered failure, not just the aggregate's severity summary.
/// Successful child computations may themselves retain recovered alternatives.
fn retryable_result(result: &BranchResult) -> bool {
    match &result.outcome {
        TestOutcome::Failed(failure) if retryable_error(&failure.error) => result
            .branches
            .iter()
            .all(|child| matches!(child.outcome, TestOutcome::Passed) || retryable_result(child)),
        _ => false,
    }
}

fn retryable_error(error: &StepError) -> bool {
    use webtest_browser::BrowserError;
    use webtest_provider::ProviderError;
    matches!(
        error,
        StepError::Assertion(_)
            | StepError::Browser(
                BrowserError::AssertionFailed { .. }
                    | BrowserError::UrlMismatch { .. }
                    | BrowserError::ActionTimeout { .. }
            )
            | StepError::Provider(ProviderError::Application {
                retryable: true,
                ..
            })
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_browser::{BrowserError, Locator};

    #[test]
    fn retry_policy_does_not_treat_all_browser_test_failures_as_timeouts() {
        for error in [
            BrowserError::LocatorAmbiguous {
                locator: Locator::Text("x".into()),
                matches: 2,
            },
            BrowserError::LocatorInvalid {
                locator: Locator::Text("x".into()),
                message: "invalid".into(),
            },
            BrowserError::ElementDetached {
                locator: Locator::Text("x".into()),
            },
            BrowserError::BrowserDisconnected,
            BrowserError::CommandTimeout {
                method: "call".into(),
                timeout_ms: 10,
            },
        ] {
            assert!(!retryable_error(&StepError::Browser(error)));
        }
        assert!(retryable_error(&StepError::Browser(
            BrowserError::ActionTimeout {
                locator: Locator::Text("x".into()),
                timeout_ms: 10,
            }
        )));
        assert!(!retryable_error(&StepError::Internal("invariant".into())));
    }
}
