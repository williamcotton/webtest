//! Test-root admission is separate from scheduling descendants within a test.
//! This driver owns every admitted future through teardown, and only it mutates
//! result slots. Roots share immutable providers and append-only event services.
use std::{fmt, str::FromStr, time::Instant};

use futures::{StreamExt, stream::FuturesUnordered};
use webtest_browser::BrowserHost;
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeObservation, SkipReason,
};
use webtest_plan::TestPlan;
use webtest_provider::ProviderRegistry;

use super::{ProviderSelection, Runner, finish_run, skip_tests, validate_plan};
use crate::events::{EventBuffer, emit_event};
use crate::execution::{ExecutedTest, execute_test};
use crate::{
    CancellationReason, PriorRunOutcome, PriorTestOutcome, RunControl, RunError, RunOutcome,
    RunResult, TestOutcome, TestResult,
};

/// A bounded number of concurrently active test roots, including their teardown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobLimit(usize);

impl JobLimit {
    pub const MAX: usize = 64;

    pub fn new(value: usize) -> Result<Self, InvalidJobLimit> {
        if (1..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidJobLimit)
        }
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for JobLimit {
    fn default() -> Self {
        Self(1)
    }
}

impl fmt::Display for JobLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
#[error("jobs must be an integer between 1 and 64")]
pub struct InvalidJobLimit;

impl FromStr for JobLimit {
    type Err = InvalidJobLimit;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value.parse().map_err(|_| InvalidJobLimit)?)
    }
}

/// A prepared file run. Callers supply files in deterministic project order;
/// tests retain their order in the plan. File IDs must be unique within each
/// runner's observation store, as for ordinary `Runner::run` invocations.
pub struct TestRun<'a> {
    pub runner: &'a Runner,
    pub plan: &'a TestPlan,
    pub browser: &'a dyn BrowserHost,
    pub control: Option<&'a dyn RunControl>,
}

/// Runtime services bound to one reusable application worker. A worker is lent
/// to one test root at a time, including all descendant work and teardown.
pub struct TestWorker {
    pub options: crate::RunnerOptions,
    pub providers: ProviderRegistry,
}

pub async fn run_jobs_on_workers(
    inputs: &[TestRun<'_>],
    workers: &[TestWorker],
) -> Result<Vec<RunResult>, InvalidJobLimit> {
    let jobs = JobLimit::new(workers.len())?;
    Ok(run_scheduled(inputs, jobs, Some(workers)).await)
}

struct FileServices {
    execution_id: ExecutionId,
    events: EventBuffer,
    ids: crate::execution::scopes::ExecutionIds,
    providers: ProviderRegistry,
    started: Instant,
    primary_failure: crate::execution::FailureSignal,
    failure_notice: tokio::sync::watch::Receiver<Option<crate::execution::PrimarySignal>>,
}

struct FileResults {
    fixture_failures: Vec<RunError>,
    outcome: RunOutcome,
    tests: Vec<Option<TestResult>>,
    observations: Vec<Vec<RuntimeObservation>>,
}

/// Run at most `jobs` test roots across all prepared files. No worker or browser
/// session is shared between simultaneous tests. A slot is released only after
/// the root's browser session and resources finish teardown. `jobs = 1` uses the
/// existing sequential runner, including its per-file browser session reuse.
///
/// Assertion failures do not stop admission. An aborted/cancelled file stops
/// admitting its remaining tests; already admitted siblings are awaited. Only
/// an explicit RunControl cancellation cancels running roots.
pub async fn run_jobs(inputs: &[TestRun<'_>], jobs: JobLimit) -> Vec<RunResult> {
    if jobs.get() == 1 && inputs.iter().all(|input| input.plan.fixtures.is_empty()) {
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            results.push(
                input
                    .runner
                    .run_with_control(input.plan, input.browser, input.control)
                    .await,
            );
        }
        return results;
    }

    run_scheduled(inputs, jobs, None).await
}

pub(super) async fn run_scheduled(
    inputs: &[TestRun<'_>],
    jobs: JobLimit,
    workers: Option<&[TestWorker]>,
) -> Vec<RunResult> {
    let mut ready = Vec::new();
    let mut identities = Vec::new();
    let mut indices = Vec::new();
    let mut results = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        match input.runner.identity_source.allocate() {
            Ok(id) => {
                ready.push(TestRun {
                    runner: input.runner,
                    plan: input.plan,
                    browser: input.browser,
                    control: input.control,
                });
                identities.push(id);
                indices.push(index);
            }
            Err(error) => results.push((index, input.runner.failed_identity(input.plan, error))),
        }
    }
    let invocation = if ready.iter().any(|input| !input.plan.fixtures.is_empty()) {
        match ready[0].runner.identity_source.allocate() {
            Ok(identity) => Some(identity),
            Err(error) => {
                // Admission has not started; no fixture or test can own resources.
                for (index, input) in indices.into_iter().zip(&ready) {
                    results.push((
                        index,
                        input.runner.failed_identity(input.plan, error.clone()),
                    ));
                }
                results.sort_by_key(|(index, _)| *index);
                return results.into_iter().map(|(_, result)| result).collect();
            }
        }
    } else {
        None
    };
    let completed = run_allocated(&ready, jobs, workers, &identities, invocation).await;
    results.extend(indices.into_iter().zip(completed));
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

async fn run_allocated(
    inputs: &[TestRun<'_>],
    jobs: JobLimit,
    workers: Option<&[TestWorker]>,
    identities: &[ExecutionId],
    invocation: Option<ExecutionId>,
) -> Vec<RunResult> {
    if inputs.is_empty() {
        return Vec::new();
    }
    let ids = crate::execution::scopes::ExecutionIds::default();
    let services: Vec<_> = inputs
        .iter()
        .zip(identities.iter().copied())
        .map(|(input, execution_id)| {
            let (primary_failure, failure_notice) = crate::execution::FailureSignal::channel();
            input.runner.observations.begin_workspace_execution(
                input.plan.file,
                &input
                    .plan
                    .sources()
                    .iter()
                    .map(|(file, _)| *file)
                    .collect::<Vec<_>>(),
                execution_id,
            );
            let events = EventBuffer::new(
                input.runner.options.journal_max_events,
                input.runner.subscribers.clone(),
            )
            .for_plan(input.plan);
            emit_event(
                &events,
                input.runner.event_sink.as_deref(),
                ExecutionEvent::RunStarted { execution_id },
            );
            FileServices {
                primary_failure,
                failure_notice,
                execution_id,
                events,
                ids: ids.clone(),
                providers: match &input.runner.providers {
                    ProviderSelection::BuiltInsFromOptions => {
                        ProviderRegistry::built_in(input.runner.options.provider_config.clone())
                    }
                    ProviderSelection::Explicit(providers) => providers.clone(),
                },
                started: Instant::now(),
            }
        })
        .collect();
    let mut files: Vec<_> = inputs
        .iter()
        .map(|input| FileResults {
            fixture_failures: Vec::new(),
            outcome: validate_plan(input.plan).map_or_else(
                |error| RunOutcome::Aborted {
                    failure: RunError::Internal(error),
                    prior_outcome: None,
                },
                |()| {
                    if input.control.is_some_and(RunControl::is_cancelled) {
                        RunOutcome::Cancelled {
                            reason: input.control.map_or(
                                CancellationReason::UserCancelled,
                                RunControl::cancellation_reason,
                            ),
                        }
                    } else {
                        RunOutcome::Completed
                    }
                },
            ),
            tests: vec![None; input.plan.tests.len()],
            observations: vec![Vec::new(); input.plan.tests.len()],
        })
        .collect();
    let environments = inputs
        .iter()
        .zip(&services)
        .flat_map(|(input, service)| {
            (0..jobs.get()).map(
                move |worker| crate::execution::fixtures::FixtureEnvironment {
                    environment: crate::fixtures::FixtureRequestEnvironment {
                        execution: service.execution_id,
                        worker: worker as u32,
                    },
                    plan: input.plan,
                    events: &service.events,
                    sink: input.runner.event_sink.as_deref(),
                    browser: input.browser,
                    control: input.control,
                    options: workers
                        .map_or(&input.runner.options, |workers| &workers[worker].options),
                    providers: workers
                        .map_or(&service.providers, |workers| &workers[worker].providers),
                },
            )
        })
        .collect::<Vec<_>>();
    // No fixture requests exist in the compatibility path; the first real file
    // identity is sufficient for its unused owner table.
    let invocation = invocation.unwrap_or(identities[0]);
    let contexts = crate::execution::fixtures::FixtureAccess::shared_contexts(
        invocation,
        &environments
            .iter()
            .map(|environment| environment.environment)
            .collect::<Vec<_>>(),
        &ids,
    );
    let driver = crate::execution::fixtures::NativeFixtureDriver {
        environments,
        invocation,
        contexts: contexts.clone(),
        ids,
        client: Default::default(),
        resources: Default::default(),
        waits: Default::default(),
    };
    let (client, coordinator) = crate::fixtures::FixtureCoordinator::new(&driver);
    let _ = driver.client.set(client.clone());
    let root_work =
        async {
            let mut roots = inputs.iter().enumerate().flat_map(|(file, input)| {
                (0..input.plan.tests.len()).map(move |test| (file, test))
            });
            let mut available: std::collections::VecDeque<_> = (0..jobs.get()).collect();
            let mut pending = FuturesUnordered::new();
            loop {
                while let Some(&worker) = available.front() {
                    let Some((file, test)) = roots.next() else {
                        break;
                    };
                    let input = &inputs[file];
                    let state = &mut files[file];
                    if matches!(state.outcome, RunOutcome::Completed)
                        && input.control.is_some_and(RunControl::is_cancelled)
                    {
                        state.outcome = RunOutcome::Cancelled {
                            reason: input.control.map_or(
                                CancellationReason::UserCancelled,
                                RunControl::cancellation_reason,
                            ),
                        };
                    }
                    let skip = if services[file].events.overflow().is_some() {
                        Some((
                            SkipReason::RunAborted,
                            Some(crate::FailureClass::Infrastructure),
                        ))
                    } else {
                        match &state.outcome {
                            RunOutcome::Completed => services[file]
                                .failure_notice
                                .borrow()
                                .and_then(|signal| match signal {
                                    crate::execution::PrimarySignal::Failure(class) => {
                                        Some((SkipReason::RunAborted, Some(class)))
                                    }
                                    crate::execution::PrimarySignal::Verdict { .. } => None,
                                }),
                            RunOutcome::Cancelled { .. } => Some((SkipReason::RunCancelled, None)),
                            RunOutcome::Aborted { failure, .. } => {
                                Some((SkipReason::RunAborted, Some(failure.failure_class())))
                            }
                        }
                    };
                    if let Some((reason, class)) = skip {
                        let mut skipped = Vec::new();
                        skip_tests(
                            &input.plan.tests[test..test + 1],
                            reason,
                            class,
                            services[file].execution_id,
                            &mut skipped,
                            &services[file].events,
                            input.runner.event_sink.as_deref(),
                        );
                        state.tests[test] = skipped.pop();
                    } else {
                        available.pop_front();
                        pending.push(run_root(
                            file,
                            test,
                            worker,
                            input,
                            &services[file],
                            workers.map(|workers| &workers[worker]),
                            crate::execution::fixtures::FixtureAccess {
                                client: client.clone(),
                                invocation,
                                contexts: contexts.clone(),
                                test_owner: None,
                                environment: crate::fixtures::FixtureRequestEnvironment {
                                    execution: services[file].execution_id,
                                    worker: worker as u32,
                                },
                            },
                        ));
                    }
                }
                let Some((file, test, worker, result, observations)) = pending.next().await else {
                    break;
                };
                available.push_back(worker);
                let state = &mut files[file];
                // Every failure remains in its source-ordered test result. The run
                // outcome is only a summary and never replaces the test aggregate.
                match &result.outcome {
                    TestOutcome::Aborted { failure, .. }
                        if !matches!(state.outcome, RunOutcome::Aborted { .. }) =>
                    {
                        let prior_outcome = match state.outcome {
                            RunOutcome::Cancelled { reason } => {
                                Some(PriorRunOutcome::Cancelled { reason })
                            }
                            _ => None,
                        };
                        state.outcome = RunOutcome::Aborted {
                            failure: failure.clone(),
                            prior_outcome,
                        };
                    }
                    TestOutcome::Cancelled { reason }
                        if matches!(state.outcome, RunOutcome::Completed) =>
                    {
                        state.outcome = RunOutcome::Cancelled { reason: *reason };
                    }
                    _ => {}
                }
                state.tests[test] = Some(result);
                state.observations[test] = observations;
            }
            drop(pending);
            for (owner, context) in contexts.iter() {
                context
                    .cancellation
                    .start_cleanup_at(tokio::time::Instant::now());
                let errors = client
                    .close(*owner)
                    .await
                    .unwrap_or_else(|error| vec![error]);
                let failure = fixture_run_failure(errors, context.scope_id);
                if let Some(failure) = failure {
                    for (service, state) in services.iter().zip(&mut files) {
                        let relevant = match owner {
                            crate::fixtures::FixtureOwnerKey::File { execution } => {
                                service.execution_id == *execution
                            }
                            _ => true,
                        };
                        if relevant {
                            state.fixture_failures.push(failure.clone());
                        }
                    }
                }
            }
        };
    let (_, drain_errors) = coordinator.drive(root_work).await;
    if let Some(failure) = fixture_run_failure(
        drain_errors,
        contexts.values().next().map_or_else(
            || webtest_model::ExecutionScopeId(driver.ids.next()),
            |context| context.scope_id,
        ),
    ) {
        for state in &mut files {
            state.fixture_failures.push(failure.clone());
        }
    }
    drop(driver);
    inputs
        .iter()
        .zip(services)
        .zip(files)
        .map(|((input, service), mut state)| {
            let cancellation = state
                .tests
                .iter()
                .flatten()
                .find_map(|test| match &test.outcome {
                    TestOutcome::Cancelled { reason } => Some(*reason),
                    TestOutcome::Aborted {
                        prior_outcome: Some(prior),
                        ..
                    } => match prior.as_ref() {
                        PriorTestOutcome::Cancelled { reason } => Some(*reason),
                        _ => None,
                    },
                    _ => None,
                });
            // Select a stable run summary even when failures completed in a
            // different order. Cancellation causality is likewise derived from
            // the complete aggregate, including cancellation after an abort.
            if let Some(failure) =
                state
                    .tests
                    .iter()
                    .flatten()
                    .find_map(|test| match &test.outcome {
                        TestOutcome::Aborted { failure, .. } => Some(failure),
                        _ => None,
                    })
            {
                let prior_outcome =
                    cancellation.map(|reason| PriorRunOutcome::Cancelled { reason });
                state.outcome = RunOutcome::Aborted {
                    failure: failure.clone(),
                    prior_outcome,
                };
            } else if let Some(reason) = cancellation {
                state.outcome = RunOutcome::Cancelled { reason };
            }
            for failure in std::mem::take(&mut state.fixture_failures) {
                append_fixture_failure(&mut state, failure);
            }
            input.runner.observations.complete_workspace_execution(
                input.plan.file,
                &input.plan.sources(),
                service.execution_id,
                state.observations.into_iter().flatten().collect(),
            );
            finish_run(
                service.execution_id,
                state.outcome,
                state.tests.into_iter().flatten().collect(),
                &service.events,
                service.started,
                input.runner.event_sink.as_deref(),
                &input.runner.options.inspection.redacted_query_parameters,
            )
        })
        .collect()
}

async fn run_root(
    file: usize,
    test: usize,
    worker: usize,
    input: &TestRun<'_>,
    service: &FileServices,
    runtime: Option<&TestWorker>,
    fixtures: crate::execution::fixtures::FixtureAccess,
) -> (usize, usize, usize, TestResult, Vec<RuntimeObservation>) {
    // All mutable runtime and native resource state belongs to this future.
    let mut session = None;
    let observations = ObservationStore::default();
    let resources = crate::ResourceRegistry::default();
    let waits = crate::WaitRegistry::default();
    let ExecutedTest { result } = execute_test(
        input.plan,
        &input.plan.tests[test],
        service.execution_id,
        &service.events,
        input.runner.event_sink.as_deref(),
        input.browser,
        &mut session,
        input.control,
        runtime.map_or(&input.runner.options, |worker| &worker.options),
        runtime.map_or(&service.providers, |worker| &worker.providers),
        &observations,
        service.ids.clone(),
        &resources,
        &waits,
        crate::execution::RootExecutionPolicy {
            fixtures: Some(&fixtures),
            close_session: true,
            primary_failure: Some(service.primary_failure.clone()),
        },
    )
    .await;
    (
        file,
        test,
        worker,
        result,
        input
            .plan
            .sources()
            .into_iter()
            .flat_map(|(file, revision)| observations.observations_for(file, revision))
            .collect(),
    )
}

fn fixture_run_failure(
    errors: Vec<crate::fixtures::FixtureError>,
    scope_id: webtest_model::ExecutionScopeId,
) -> Option<RunError> {
    let mut errors = errors.into_iter().map(|error| match error {
        crate::fixtures::FixtureError::Cleanup(error) => RunError::Cleanup(error),
        crate::fixtures::FixtureError::Aborted(error) => error,
        error => RunError::Cleanup(crate::CleanupFailure {
            resource: crate::CleanupResource::ExecutionScope { scope_id },
            cause: crate::CleanupCause::Internal {
                message: error.to_string(),
            },
        }),
    });
    let primary = errors.next()?;
    let secondary = errors.collect::<Vec<_>>();
    Some(if secondary.is_empty() {
        primary
    } else {
        RunError::Multiple {
            primary: Box::new(primary),
            secondary,
        }
    })
}
fn append_fixture_failure(state: &mut FileResults, failure: RunError) {
    state.outcome = match &state.outcome {
        RunOutcome::Aborted {
            failure: primary,
            prior_outcome,
        } => RunOutcome::Aborted {
            failure: RunError::Multiple {
                primary: Box::new(primary.clone()),
                secondary: vec![failure],
            },
            prior_outcome: *prior_outcome,
        },
        RunOutcome::Cancelled { reason } => RunOutcome::Aborted {
            failure,
            prior_outcome: Some(PriorRunOutcome::Cancelled { reason: *reason }),
        },
        RunOutcome::Completed => RunOutcome::Aborted {
            failure,
            prior_outcome: None,
        },
    };
}
