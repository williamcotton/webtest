use std::time::{Duration, Instant};

use webtest_browser::{BrowserSession, Page};
use webtest_observation::{ExecutionEvent, ExecutionId, ObservationStore};
use webtest_plan::{PlannedTest, TestOperation, TestPlan};
use webtest_provider::{ProviderError, ProviderRegistry};

use crate::{
    CancellationReason, FailureClass, RunControl, RunError, RunEventSink, RunnerOptions,
    TestOutcome, TestResult, events::emit_event, redaction::redact_step_error,
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
    pub(crate) session_tainted: bool,
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
    let mut state = TestExecutionState::new(options.redacted_json_fields.clone());
    let mut outcome = None;
    let mut context = None;
    let mut page: Option<Box<dyn Page>> = None;

    if let Some(session) = session.as_deref_mut() {
        match session.new_context(&options.browser_context).await {
            Ok(created) => context = Some(created),
            Err(error) => {
                outcome = Some(TestOutcome::Aborted {
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
                outcome = Some(TestOutcome::Aborted {
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
            outcome = Some(TestOutcome::Cancelled {
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
                outcome = Some(TestOutcome::Cancelled {
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
                        outcome = Some(TestOutcome::Failed(Box::new(step_failure)));
                        break;
                    }
                    Err(error) => {
                        outcome = Some(TestOutcome::Aborted { failure: error });
                        break;
                    }
                }
            }
        }
    }

    drop(page);
    let cleanup_failed = if let Some(context) = context.as_mut() {
        context.close().await.is_err()
    } else {
        false
    };
    let bindings = state.final_transferable_bindings(&options.redacted_json_fields);
    for directory in state.temporary_directories() {
        if let Err(error) = tokio::fs::remove_dir_all(&directory).await {
            outcome = Some(TestOutcome::Aborted {
                failure: RunError::Provider(ProviderError::Filesystem {
                    path: directory.display().to_string(),
                    message: format!("temporary resource cleanup failed: {error}"),
                }),
            });
            break;
        }
    }
    let outcome = outcome.unwrap_or(TestOutcome::Passed);
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
        session_tainted: cleanup_failed,
    }
}

pub(crate) fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
