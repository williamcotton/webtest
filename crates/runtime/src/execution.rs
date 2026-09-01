use std::{
    future::Future,
    time::{Duration, Instant as StdInstant},
};

use tokio::time::Instant;
use webtest_browser::{BrowserContext, BrowserHost, BrowserSession, Page};
use webtest_hir::StepId;
use webtest_observation::{
    CleanupCause, CleanupFailure, CleanupResource, ExecutionEvent, ExecutionId, ObservationStore,
    RuntimeFailure, RuntimeObservation, RuntimeObservationKind,
};
use webtest_plan::{PlannedStep, PlannedTest, TestOperation, TestPlan};
use webtest_provider::{Capability, ProviderRegistry};

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
    TimedOut {
        timeout: Duration,
        active_step: Option<StepId>,
    },
    Cancelled {
        reason: CancellationReason,
    },
    Aborted {
        failure: RunError,
    },
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_test(
    plan: &TestPlan,
    test: &PlannedTest,
    execution_id: ExecutionId,
    events: &mut Vec<ExecutionEvent>,
    event_sink: Option<&dyn RunEventSink>,
    browser: &dyn BrowserHost,
    session: &mut Option<Box<dyn BrowserSession>>,
    control: Option<&dyn RunControl>,
    options: &RunnerOptions,
    providers: &ProviderRegistry,
    observations: &ObservationStore,
) -> ExecutedTest {
    let test_started = StdInstant::now();
    let deadline = TestDeadline::new(options.test_timeout);
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
    let mut context: Option<Box<dyn BrowserContext>> = None;
    let mut page: Option<Box<dyn Page>> = None;
    let mut active_step = None;
    let uses_browser = test
        .required_host_capabilities
        .contains(&Capability::Browser);

    let outcome = match run_until_deadline(
        deadline.at,
        execute_test_body(
            plan,
            test,
            execution_id,
            events,
            event_sink,
            browser,
            session,
            control,
            options,
            providers,
            observations,
            &deadline,
            uses_browser,
            &mut state,
            &mut context,
            &mut page,
            &mut active_step,
        ),
    )
    .await
    {
        Some(provisional) => provisional,
        None => {
            let active = active_step.and_then(|id| test.steps.iter().find(|step| step.id == id));
            if let Some(control) = control {
                control.after_test_timeout(test, active);
            }
            emit_test_timeout(
                plan,
                test,
                active,
                execution_id,
                events,
                event_sink,
                providers,
                observations,
                options.test_timeout,
            );
            ProvisionalTestOutcome::TimedOut {
                timeout: options.test_timeout,
                active_step,
            }
        }
    };

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
    if matches!(outcome, ProvisionalTestOutcome::TimedOut { .. })
        && uses_browser
        && let Some(mut tainted) = session.take()
        && let Err(error) = tainted.close().await
    {
        cleanup_failures.push(CleanupFailure {
            resource: CleanupResource::BrowserSession,
            cause: CleanupCause::Browser(error),
        });
    }
    let bindings = state.final_transferable_bindings(&options.redacted_json_fields);
    for failure in &cleanup_failures {
        emit_cleanup_failed(events, event_sink, execution_id, Some(test.id), failure);
    }
    let outcome = combine_test_outcome(outcome, cleanup_failures);
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
            test_id: test.id,
            name: test.name.clone(),
            outcome,
            duration: test_started.elapsed(),
            bindings,
        },
    }
}

struct TestDeadline {
    at: Instant,
}

impl TestDeadline {
    fn new(timeout: Duration) -> Self {
        Self {
            at: Instant::now() + timeout,
        }
    }

    fn remaining(&self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }
}

async fn run_until_deadline<F>(deadline: Instant, future: F) -> Option<F::Output>
where
    F: Future,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = tokio::time::sleep_until(deadline) => None,
        output = &mut future => Some(output),
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_test_body(
    plan: &TestPlan,
    test: &PlannedTest,
    execution_id: ExecutionId,
    events: &mut Vec<ExecutionEvent>,
    event_sink: Option<&dyn RunEventSink>,
    browser: &dyn BrowserHost,
    session: &mut Option<Box<dyn BrowserSession>>,
    control: Option<&dyn RunControl>,
    options: &RunnerOptions,
    providers: &ProviderRegistry,
    observations: &ObservationStore,
    deadline: &TestDeadline,
    uses_browser: bool,
    state: &mut TestExecutionState,
    context: &mut Option<Box<dyn BrowserContext>>,
    page: &mut Option<Box<dyn Page>>,
    active_step: &mut Option<StepId>,
) -> ProvisionalTestOutcome {
    if uses_browser {
        if session.is_none() {
            match browser.start().await {
                Ok(started) => *session = Some(started),
                Err(error) => {
                    return ProvisionalTestOutcome::Aborted {
                        failure: RunError::Browser(error),
                    };
                }
            }
        }
        let Some(browser_session) = session.as_deref_mut() else {
            return ProvisionalTestOutcome::Aborted {
                failure: RunError::Internal("browser test has no active browser session".into()),
            };
        };
        match browser_session.new_context(&options.browser_context).await {
            Ok(created) => *context = Some(created),
            Err(error) => {
                return ProvisionalTestOutcome::Aborted {
                    failure: RunError::Browser(error),
                };
            }
        }
        match context.as_deref_mut() {
            Some(created_context) => match created_context.new_page().await {
                Ok(created) => *page = Some(created),
                Err(error) => {
                    return ProvisionalTestOutcome::Aborted {
                        failure: RunError::Browser(error),
                    };
                }
            },
            None => {
                return ProvisionalTestOutcome::Aborted {
                    failure: RunError::Internal(
                        "browser context disappeared during page acquisition".into(),
                    ),
                };
            }
        }
    }

    for step in &test.steps {
        *active_step = Some(step.id);
        if control.is_some_and(RunControl::is_cancelled) {
            return ProvisionalTestOutcome::Cancelled {
                reason: CancellationReason::Requested,
            };
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
                return ProvisionalTestOutcome::Cancelled {
                    reason: CancellationReason::Requested,
                };
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
        let step_started = StdInstant::now();
        match execute_step(providers, options, page, step, state, deadline.remaining()).await {
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
                    page,
                    options,
                    providers,
                    observations,
                    events,
                    event_sink,
                    elapsed_ms: duration_millis(step_started.elapsed()),
                    secrets,
                })
                .await;
                return match failure_result {
                    Ok(step_failure) => ProvisionalTestOutcome::Failed(Box::new(step_failure)),
                    Err(error) => ProvisionalTestOutcome::Aborted { failure: error },
                };
            }
        }
    }
    *active_step = None;
    ProvisionalTestOutcome::Passed
}

#[allow(clippy::too_many_arguments)]
fn emit_test_timeout(
    plan: &TestPlan,
    test: &PlannedTest,
    active_step: Option<&PlannedStep>,
    execution_id: ExecutionId,
    events: &mut Vec<ExecutionEvent>,
    event_sink: Option<&dyn RunEventSink>,
    providers: &ProviderRegistry,
    observations: &ObservationStore,
    timeout: Duration,
) {
    let timeout_ms = duration_millis(timeout);
    emit_event(
        events,
        event_sink,
        ExecutionEvent::TestTimedOut {
            execution_id,
            test_id: test.id,
            active_step: active_step.map(|step| step.id),
            timeout_ms,
        },
    );
    if let Some(step) = active_step {
        if let TestOperation::ServerProviderCall(call) = &step.operation {
            emit_event(
                events,
                event_sink,
                ExecutionEvent::ProviderCallFailed {
                    execution_id,
                    test_id: test.id,
                    step_id: step.id,
                    provider: call.provider.clone(),
                    operation: call.operation.clone(),
                    code: "test_timeout".into(),
                    message: format!("test timed out after {timeout_ms}ms"),
                    failure_class: FailureClass::Test,
                    elapsed_ms: timeout_ms,
                    transport_kind: providers.transport_kind(&call.provider),
                },
            );
        }
        emit_event(
            events,
            event_sink,
            ExecutionEvent::StepFailed {
                execution_id,
                test_id: test.id,
                step_id: step.id,
                failure_class: FailureClass::Test,
                failure: RuntimeFailure::TestTimeout {
                    timeout_ms,
                    active_step: Some(step.id),
                },
                repair_hints: Vec::new(),
                page: None,
            },
        );
    }
    let (step_id, range) = active_step.map_or((None, test.origin.range), |step| {
        (Some(step.id), step.origin.range)
    });
    observations.record(RuntimeObservation {
        execution_id,
        file: plan.file,
        source_revision: plan.source_revision,
        test_id: test.id,
        step_id,
        range,
        kind: RuntimeObservationKind::TestTimeout {
            timeout_ms,
            active_step: step_id,
        },
    });
}

fn combine_test_outcome(
    provisional: ProvisionalTestOutcome,
    cleanup_failures: Vec<CleanupFailure>,
) -> TestOutcome {
    if cleanup_failures.is_empty() {
        return match provisional {
            ProvisionalTestOutcome::Passed => TestOutcome::Passed,
            ProvisionalTestOutcome::Failed(failure) => TestOutcome::Failed(failure),
            ProvisionalTestOutcome::TimedOut {
                timeout,
                active_step,
            } => TestOutcome::TimedOut {
                timeout,
                active_step,
            },
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
        ProvisionalTestOutcome::TimedOut {
            timeout,
            active_step,
        } => TestOutcome::Aborted {
            failure: cleanup_run_error(cleanup_failures),
            prior_outcome: Some(Box::new(PriorTestOutcome::TimedOut {
                timeout,
                active_step,
            })),
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
            ProvisionalTestOutcome::TimedOut {
                timeout,
                active_step: Some(StepId(7)),
            },
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
            } if matches!(prior.as_ref(), PriorTestOutcome::TimedOut { timeout: actual, active_step: Some(StepId(7)) } if *actual == timeout)
        ));
    }
}
