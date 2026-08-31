use std::time::{Duration, Instant};

use webtest_browser::{BrowserSession, Page};
use webtest_observation::{
    CleanupCause, CleanupFailure, CleanupResource, ExecutionEvent, ExecutionId, ObservationStore,
};
use webtest_plan::{PlannedTest, TestOperation, TestPlan};
use webtest_provider::ProviderRegistry;

use crate::{
    CancellationReason, FailureClass, PriorTestOutcome, RunControl, RunError, RunEventSink,
    RunnerOptions, StepFailure, TestOutcome, TestResult, events::emit_event,
    redaction::redact_step_error,
};

use self::{
    failure::{FailureInput, process_failure},
    state::TestExecutionState,
    steps::execute_step,
};

mod browser;
mod failure;
mod provider;
mod state;
mod steps;

pub(crate) use browser::{bounded_timeout, browser_locator, browser_state};

#[cfg(test)]
pub(crate) use failure::repair_hints_for_error;

pub(crate) struct ExecutedTest {
    pub(crate) result: TestResult,
}

#[allow(dead_code)]
enum ProvisionalTestOutcome {
    Passed,
    Failed(Box<StepFailure>),
    TimedOut { timeout: Duration },
    Cancelled { reason: CancellationReason },
    Aborted { failure: RunError },
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_test(
    plan: &TestPlan,
    test: &PlannedTest,
    execution_id: ExecutionId,
    events: &mut Vec<ExecutionEvent>,
    event_sink: Option<&dyn RunEventSink>,
    session: &mut Option<Box<dyn BrowserSession>>,
    control: Option<&dyn RunControl>,
    options: &RunnerOptions,
    providers: &ProviderRegistry,
    observations: &ObservationStore,
) -> ExecutedTest {
    let test_started = Instant::now();
    emit_event(
        events,
        event_sink,
        ExecutionEvent::TestStarted {
            execution_id,
            test_id: test.id,
            name: test.name.clone(),
        },
    );
    let mut state = TestExecutionState::new(
        options.redacted_json_fields.clone(),
        options.project_root.clone(),
    );
    let mut outcome = None;
    let mut context = None;
    let mut page: Option<Box<dyn Page>> = None;

    if let Some(session) = session.as_deref_mut() {
        match session.new_context(&options.browser_context).await {
            Ok(created) => context = Some(created),
            Err(error) => {
                outcome = Some(ProvisionalTestOutcome::Aborted {
                    failure: RunError::Browser(error),
                });
            }
        }
    }
    if outcome.is_none()
        && let Some(created_context) = context.as_mut()
    {
        match created_context.new_page().await {
            Ok(created) => page = Some(created),
            Err(error) => {
                outcome = Some(ProvisionalTestOutcome::Aborted {
                    failure: RunError::Browser(error),
                });
            }
        }
    }

    for step in &test.steps {
        if outcome.is_some() {
            break;
        }
        if control.is_some_and(RunControl::is_cancelled) {
            outcome = Some(ProvisionalTestOutcome::Cancelled {
                reason: CancellationReason::Requested,
            });
            break;
        }
        if let TestOperation::ServerProviderCall(call) = &step.operation {
            state.prepare_provider_arguments(call);
        }
        if let Some(control) = control {
            if control.should_capture_bindings(test, step) {
                control
                    .before_step_with_bindings(test, step, state.visible_step_bindings(step))
                    .await;
            } else {
                control.before_step(test, step).await;
            }
            if control.is_cancelled() {
                outcome = Some(ProvisionalTestOutcome::Cancelled {
                    reason: CancellationReason::Requested,
                });
                break;
            }
        }
        emit_event(
            events,
            event_sink,
            ExecutionEvent::StepStarted {
                execution_id,
                test_id: test.id,
                step_id: step.id,
            },
        );
        if let TestOperation::ServerProviderCall(call) = &step.operation {
            emit_event(
                events,
                event_sink,
                ExecutionEvent::ProviderCallStarted {
                    execution_id,
                    test_id: test.id,
                    step_id: step.id,
                    provider: call.provider.clone(),
                    operation: call.operation.clone(),
                    transport_kind: providers.transport_kind(&call.provider),
                    arguments: state.provider_argument_summaries(call),
                },
            );
        }
        let step_started = Instant::now();
        match execute_step(providers, options, &mut page, step, &mut state).await {
            Ok(()) => {
                if let TestOperation::ServerProviderCall(call) = &step.operation {
                    state.accept_provider_result_metadata(call);
                    emit_event(
                        events,
                        event_sink,
                        ExecutionEvent::ProviderCallFinished {
                            execution_id,
                            test_id: test.id,
                            step_id: step.id,
                            provider: call.provider.clone(),
                            operation: call.operation.clone(),
                            elapsed_ms: duration_millis(step_started.elapsed()),
                            transport_kind: providers.transport_kind(&call.provider),
                            result: state.provider_result_summary(call),
                        },
                    );
                }
                emit_event(
                    events,
                    event_sink,
                    ExecutionEvent::StepPassed {
                        execution_id,
                        test_id: test.id,
                        step_id: step.id,
                    },
                );
            }
            Err(error) => {
                let (redacted_fields, secrets) = state.redaction();
                let error = redact_step_error(
                    error,
                    redacted_fields,
                    secrets,
                    &options.inspection.redacted_query_parameters,
                );
                if error.failure_class() != FailureClass::Internal
                    && let Some(control) = control
                {
                    control
                        .after_step_failure(test, step, &error, &state.visible_step_bindings(step))
                        .await;
                }
                let failure_result = process_failure(FailureInput {
                    plan,
                    test_id: test.id,
                    step,
                    execution_id,
                    error,
                    page: &mut page,
                    options,
                    providers,
                    observations,
                    events,
                    event_sink,
                    elapsed_ms: duration_millis(step_started.elapsed()),
                    secrets,
                })
                .await;
                match failure_result {
                    Ok(step_failure) => {
                        outcome = Some(ProvisionalTestOutcome::Failed(Box::new(step_failure)));
                        break;
                    }
                    Err(error) => {
                        outcome = Some(ProvisionalTestOutcome::Aborted { failure: error });
                        break;
                    }
                }
            }
        }
    }

    drop(page.take());
    let mut cleanup_failures = Vec::new();
    if let Some(mut context) = context.take()
        && let Err(error) = context.close().await
    {
        cleanup_failures.push(CleanupFailure {
            resource: CleanupResource::BrowserContext,
            cause: CleanupCause::Browser(error),
        });
    }
    for directory in state.temporary_directories() {
        if let Err(error) = tokio::fs::remove_dir_all(&directory).await {
            cleanup_failures.push(CleanupFailure {
                resource: CleanupResource::TemporaryDirectory { path: directory },
                cause: CleanupCause::Io(error.into()),
            });
        }
    }
    let bindings = state.final_transferable_bindings(&options.redacted_json_fields);
    for failure in &cleanup_failures {
        emit_cleanup_failed(events, event_sink, execution_id, Some(test.id), failure);
    }
    let outcome = combine_test_outcome(
        outcome.unwrap_or(ProvisionalTestOutcome::Passed),
        cleanup_failures,
    );
    let outcome_kind = outcome.finished_kind();
    let failure_class = outcome.failure_class();
    emit_event(
        events,
        event_sink,
        ExecutionEvent::TestFinished {
            execution_id,
            test_id: test.id,
            outcome: outcome_kind,
            failure_class,
        },
    );
    ExecutedTest {
        result: TestResult {
            name: test.name.clone(),
            outcome,
            duration: test_started.elapsed(),
            bindings,
        },
    }
}

fn combine_test_outcome(
    provisional: ProvisionalTestOutcome,
    cleanup_failures: Vec<CleanupFailure>,
) -> TestOutcome {
    if cleanup_failures.is_empty() {
        return match provisional {
            ProvisionalTestOutcome::Passed => TestOutcome::Passed,
            ProvisionalTestOutcome::Failed(failure) => TestOutcome::Failed(failure),
            ProvisionalTestOutcome::TimedOut { timeout } => TestOutcome::TimedOut { timeout },
            ProvisionalTestOutcome::Cancelled { reason } => TestOutcome::Cancelled { reason },
            ProvisionalTestOutcome::Aborted { failure } => TestOutcome::Aborted {
                failure,
                prior_outcome: None,
            },
        };
    }

    match provisional {
        ProvisionalTestOutcome::Aborted { failure } => TestOutcome::Aborted {
            failure: failure.combine_with_cleanup(cleanup_failures),
            prior_outcome: None,
        },
        ProvisionalTestOutcome::Passed => TestOutcome::Aborted {
            failure: cleanup_run_error(cleanup_failures),
            prior_outcome: None,
        },
        ProvisionalTestOutcome::Failed(failure) => TestOutcome::Aborted {
            failure: cleanup_run_error(cleanup_failures),
            prior_outcome: Some(Box::new(PriorTestOutcome::Failed(failure))),
        },
        ProvisionalTestOutcome::TimedOut { timeout } => TestOutcome::Aborted {
            failure: cleanup_run_error(cleanup_failures),
            prior_outcome: Some(Box::new(PriorTestOutcome::TimedOut { timeout })),
        },
        ProvisionalTestOutcome::Cancelled { reason } => TestOutcome::Aborted {
            failure: cleanup_run_error(cleanup_failures),
            prior_outcome: Some(Box::new(PriorTestOutcome::Cancelled { reason })),
        },
    }
}

fn cleanup_run_error(failures: Vec<CleanupFailure>) -> RunError {
    RunError::from_cleanup_failures(failures).unwrap_or_else(|| {
        RunError::Internal("cleanup outcome was missing its typed failure".into())
    })
}

pub(crate) fn emit_cleanup_failed(
    events: &mut Vec<ExecutionEvent>,
    event_sink: Option<&dyn RunEventSink>,
    execution_id: ExecutionId,
    test_id: Option<webtest_hir::TestId>,
    failure: &CleanupFailure,
) {
    emit_event(
        events,
        event_sink,
        ExecutionEvent::CleanupFailed {
            execution_id,
            test_id,
            resource: failure.resource.clone(),
            failure_class: failure.failure_class(),
            code: failure.code().into(),
            message: failure.message(),
        },
    );
}

pub(crate) fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod finalization_tests {
    use webtest_browser::BrowserError;
    use webtest_observation::{CleanupCause, CleanupFailure, CleanupResource};

    use super::*;

    #[test]
    fn cleanup_failure_outranks_a_provisional_timeout_without_flattening_it() {
        let timeout = Duration::from_secs(3);
        let outcome = combine_test_outcome(
            ProvisionalTestOutcome::TimedOut { timeout },
            vec![CleanupFailure {
                resource: CleanupResource::BrowserContext,
                cause: CleanupCause::Browser(BrowserError::BrowserDisconnected),
            }],
        );

        assert!(matches!(
            outcome,
            TestOutcome::Aborted {
                failure: RunError::Cleanup(_),
                prior_outcome: Some(prior),
            } if matches!(prior.as_ref(), PriorTestOutcome::TimedOut { timeout: actual } if *actual == timeout)
        ));
    }
}
