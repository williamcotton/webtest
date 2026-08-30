use std::time::{Duration, Instant};

use webtest_browser::{BrowserSession, Page};
use webtest_observation::{ExecutionEvent, ExecutionId, ObservationStore};
use webtest_plan::{PlannedTest, TestOperation, TestPlan};
use webtest_provider::{ProviderError, ProviderRegistry};

use crate::{
    RunControl, RunError, RunEventSink, RunnerOptions, TestResult, events::emit_event,
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
) -> Result<ExecutedTest, RunError> {
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
    let mut context = if let Some(session) = session.as_deref_mut() {
        Some(session.new_context(&options.browser_context).await?)
    } else {
        None
    };
    let mut page: Option<Box<dyn Page>> = if let Some(context) = context.as_mut() {
        Some(context.new_page().await?)
    } else {
        None
    };
    let mut failure = None;
    let mut state = TestExecutionState::new(options.redacted_json_fields.clone());

    for step in &test.steps {
        if control.is_some_and(RunControl::is_cancelled) {
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
                if let Some(control) = control {
                    control
                        .after_step_failure(test, step, &error, &state.visible_step_bindings(step))
                        .await;
                }
                let outcome = process_failure(FailureInput {
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
                match outcome {
                    Ok(step_failure) => {
                        failure = Some(step_failure);
                        break;
                    }
                    Err(error) => {
                        drop(page);
                        if let Some(context) = context.as_mut() {
                            let _ = context.close().await;
                        }
                        return Err(error);
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
    let passed = failure.is_none();
    emit_event(
        events,
        event_sink,
        ExecutionEvent::TestFinished {
            execution_id,
            test_id: test.id,
            passed,
        },
    );
    let bindings = state.final_transferable_bindings(&options.redacted_json_fields);
    for directory in state.temporary_directories() {
        tokio::fs::remove_dir_all(&directory)
            .await
            .map_err(|error| {
                RunError::Provider(ProviderError::Filesystem {
                    path: directory.display().to_string(),
                    message: format!("temporary resource cleanup failed: {error}"),
                })
            })?;
    }
    Ok(ExecutedTest {
        result: TestResult {
            name: test.name.clone(),
            passed,
            failure,
            duration: test_started.elapsed(),
            bindings,
        },
        session_tainted: cleanup_failed,
    })
}

pub(crate) fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
