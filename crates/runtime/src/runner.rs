use std::{collections::BTreeMap, sync::Arc, time::Instant};

use tracing::instrument;
use webtest_browser::BrowserHost;
use webtest_model::{Capability, Value};
use webtest_observation::{
    CleanupCause, CleanupFailure, CleanupResource, ExecutionEvent, ExecutionId, ObservationStore,
    SkipReason,
};
use webtest_plan::{AssertionOperation, PlannedTest, TestOperation, TestPlan};
use webtest_provider::ProviderRegistry;

use crate::{
    CancellationReason, FailureClass, PriorRunOutcome, RunControl, RunError, RunEventSink,
    RunOutcome, RunResult, RunnerOptions, TestOutcome, TestResult,
    events::emit_event,
    execution::{ExecutedTest, emit_cleanup_failed, execute_test},
};

pub struct Runner {
    observations: Arc<ObservationStore>,
    options: RunnerOptions,
    providers: ProviderSelection,
    event_sink: Option<Arc<dyn RunEventSink>>,
}

enum ProviderSelection {
    BuiltInsFromOptions,
    Explicit(ProviderRegistry),
}

impl Runner {
    pub fn new(observations: Arc<ObservationStore>) -> Self {
        Self {
            observations,
            options: RunnerOptions::default(),
            providers: ProviderSelection::BuiltInsFromOptions,
            event_sink: None,
        }
    }

    /// Replaces the runtime options without changing an explicit provider registry.
    ///
    /// When no registry has been supplied, built-in providers are configured from
    /// the final options at the start of each run. This setter therefore commutes
    /// with [`Self::with_provider_registry`].
    pub fn with_options(mut self, options: RunnerOptions) -> Self {
        self.options = options;
        self
    }

    /// Selects an explicit provider registry, which takes precedence over the
    /// built-in provider configuration regardless of builder call order.
    ///
    /// Calling this method repeatedly replaces the previous explicit registry.
    pub fn with_provider_registry(mut self, providers: ProviderRegistry) -> Self {
        self.providers = ProviderSelection::Explicit(providers);
        self
    }

    /// Installs an event sink independently of runtime options and providers.
    pub fn with_event_sink(mut self, sink: Arc<dyn RunEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run(&self, plan: &TestPlan, browser: &dyn BrowserHost) -> RunResult {
        self.run_with_control(plan, browser, None).await
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run_with_control(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
        control: Option<&dyn RunControl>,
    ) -> RunResult {
        // Resolve once so every test in this run observes the same registry. An
        // explicit registry is cloned as a stable map while retaining the same
        // provider instances behind their shared references.
        let providers = match &self.providers {
            ProviderSelection::BuiltInsFromOptions => {
                ProviderRegistry::built_in(self.options.provider_config.clone())
            }
            ProviderSelection::Explicit(providers) => providers.clone(),
        };
        self.observations.clear_for_file(plan.file);
        let run_started = Instant::now();
        let execution_id = ExecutionId::next();
        let mut events = Vec::new();
        emit_event(
            &mut events,
            self.event_sink.as_deref(),
            ExecutionEvent::RunStarted { execution_id },
        );
        let mut tests = Vec::with_capacity(plan.tests.len());
        let mut outcome = RunOutcome::Completed;
        if let Err(error) = validate_plan(plan) {
            outcome = RunOutcome::Aborted {
                failure: RunError::Internal(error),
                prior_outcome: None,
            };
            skip_tests(
                &plan.tests,
                SkipReason::RunAborted,
                Some(FailureClass::Internal),
                execution_id,
                &mut tests,
                &mut events,
                self.event_sink.as_deref(),
            );
            finish_run(
                execution_id,
                outcome,
                tests,
                events,
                run_started,
                self.event_sink.as_deref(),
            )
        } else if control.is_some_and(RunControl::is_cancelled) {
            outcome = RunOutcome::Cancelled {
                reason: CancellationReason::Requested,
            };
            skip_tests(
                &plan.tests,
                SkipReason::RunCancelled,
                None,
                execution_id,
                &mut tests,
                &mut events,
                self.event_sink.as_deref(),
            );
            finish_run(
                execution_id,
                outcome,
                tests,
                events,
                run_started,
                self.event_sink.as_deref(),
            )
        } else {
            let mut session = None;

            for (index, test) in plan.tests.iter().enumerate() {
                if control.is_some_and(RunControl::is_cancelled) {
                    outcome = RunOutcome::Cancelled {
                        reason: CancellationReason::Requested,
                    };
                    skip_tests(
                        &plan.tests[index..],
                        SkipReason::RunCancelled,
                        None,
                        execution_id,
                        &mut tests,
                        &mut events,
                        self.event_sink.as_deref(),
                    );
                    break;
                }
                let ExecutedTest { result } = execute_test(
                    plan,
                    test,
                    execution_id,
                    &mut events,
                    self.event_sink.as_deref(),
                    browser,
                    &mut session,
                    control,
                    &self.options,
                    &providers,
                    &self.observations,
                )
                .await;
                let terminal = match &result.outcome {
                    TestOutcome::Cancelled { reason } => Some((
                        RunOutcome::Cancelled { reason: *reason },
                        SkipReason::RunCancelled,
                        None,
                    )),
                    TestOutcome::Aborted { failure, .. } => Some((
                        RunOutcome::Aborted {
                            failure: failure.clone(),
                            prior_outcome: None,
                        },
                        SkipReason::RunAborted,
                        Some(failure.failure_class()),
                    )),
                    TestOutcome::Passed
                    | TestOutcome::Failed(_)
                    | TestOutcome::TimedOut { .. }
                    | TestOutcome::Skipped { .. } => None,
                };
                tests.push(result);
                if let Some((terminal, reason, failure_class)) = terminal {
                    outcome = terminal;
                    skip_tests(
                        &plan.tests[index + 1..],
                        reason,
                        failure_class,
                        execution_id,
                        &mut tests,
                        &mut events,
                        self.event_sink.as_deref(),
                    );
                    break;
                }
            }

            if let Some(mut session) = session.take()
                && let Err(error) = session.close().await
            {
                let failure = CleanupFailure {
                    resource: CleanupResource::BrowserSession,
                    cause: CleanupCause::Browser(error),
                };
                emit_cleanup_failed(
                    &mut events,
                    self.event_sink.as_deref(),
                    execution_id,
                    None,
                    &failure,
                );
                outcome = combine_run_outcome(outcome, vec![failure]);
            }
            finish_run(
                execution_id,
                outcome,
                tests,
                events,
                run_started,
                self.event_sink.as_deref(),
            )
        }
    }
}

fn validate_plan(plan: &TestPlan) -> Result<(), String> {
    plan.validate_capabilities()
        .map_err(|error| format!("invalid plan capability metadata: {error}"))?;
    for test in &plan.tests {
        if test
            .steps
            .iter()
            .any(|step| operation_requires_browser(&step.operation))
            && !test
                .required_host_capabilities
                .contains(&Capability::Browser)
        {
            return Err(format!(
                "test {} contains a browser operation without the Browser capability",
                test.id.0
            ));
        }
    }
    Ok(())
}

fn operation_requires_browser(operation: &TestOperation) -> bool {
    matches!(operation, TestOperation::Browser(_))
        || matches!(
            operation,
            TestOperation::Assertion(
                AssertionOperation::Locator { .. } | AssertionOperation::Url { .. }
            )
        )
}

fn combine_run_outcome(outcome: RunOutcome, cleanup_failures: Vec<CleanupFailure>) -> RunOutcome {
    if cleanup_failures.is_empty() {
        return outcome;
    }
    match outcome {
        RunOutcome::Completed => RunOutcome::Aborted {
            failure: cleanup_run_error(cleanup_failures),
            prior_outcome: None,
        },
        RunOutcome::Cancelled { reason } => RunOutcome::Aborted {
            failure: cleanup_run_error(cleanup_failures),
            prior_outcome: Some(PriorRunOutcome::Cancelled { reason }),
        },
        RunOutcome::Aborted {
            failure,
            prior_outcome,
        } => RunOutcome::Aborted {
            failure: failure.combine_with_cleanup(cleanup_failures),
            prior_outcome,
        },
    }
}

fn cleanup_run_error(failures: Vec<CleanupFailure>) -> RunError {
    RunError::from_cleanup_failures(failures).unwrap_or_else(|| {
        RunError::Internal("cleanup outcome was missing its typed failure".into())
    })
}

#[allow(clippy::too_many_arguments)]
fn skip_tests(
    planned: &[PlannedTest],
    reason: SkipReason,
    failure_class: Option<FailureClass>,
    execution_id: ExecutionId,
    results: &mut Vec<TestResult>,
    events: &mut Vec<ExecutionEvent>,
    event_sink: Option<&dyn RunEventSink>,
) {
    for test in planned {
        emit_event(
            events,
            event_sink,
            ExecutionEvent::TestSkipped {
                execution_id,
                test_id: test.id,
                name: test.name.clone(),
                reason,
                failure_class,
            },
        );
        results.push(TestResult {
            test_id: test.id,
            name: test.name.clone(),
            outcome: TestOutcome::Skipped {
                reason,
                failure_class,
            },
            duration: std::time::Duration::ZERO,
            bindings: BTreeMap::<String, Value>::new(),
        });
    }
}

fn finish_run(
    execution_id: ExecutionId,
    outcome: RunOutcome,
    tests: Vec<TestResult>,
    mut events: Vec<ExecutionEvent>,
    started: Instant,
    event_sink: Option<&dyn RunEventSink>,
) -> RunResult {
    let failure_class = outcome.failure_class();
    emit_event(
        &mut events,
        event_sink,
        ExecutionEvent::RunFinished {
            execution_id,
            outcome: outcome.kind(),
            failure_class,
        },
    );
    RunResult {
        execution_id,
        outcome,
        tests,
        events,
        duration: started.elapsed(),
    }
}
