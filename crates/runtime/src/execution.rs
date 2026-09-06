use crate::events::EventBuffer;
use std::{
    future::Future,
    time::{Duration, Instant as StdInstant},
};

use tokio::time::Instant;
use webtest_browser::{BrowserHost, BrowserSession, Page};
use webtest_host::OperationContext as _;
use webtest_model::{Capability, StepId, TestId};
use webtest_observation::{
    CleanupCause, CleanupFailure, CleanupResource, ExecutionEvent, ExecutionId, ObservationStore,
    RuntimeFailure, RuntimeObservation, RuntimeObservationKind,
};
use webtest_plan::{PlannedStep, PlannedTest, TestOperation, TestPlan};
use webtest_provider::ProviderRegistry;

use crate::{
    CancellationReason, FailureClass, PriorTestOutcome, RunControl, RunError, RunEventSink,
    RunnerOptions, StepFailure, TestOutcome, TestResult, events::emit_event,
    redaction::redact_step_error,
};

use self::{
    failure::{
        FailureInput, PendingFailure, PrepareFailureInput, prepare_failure, process_failure,
    },
    state::TestExecutionState,
    steps::execute_step,
};

mod browser;
mod failure;
mod provider;
mod resource;
pub(crate) mod scopes;
mod state;
mod steps;
mod temporary;
mod wait;

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
        origin: Option<webtest_text::SyntaxOrigin>,
    },
    Cancelled {
        reason: CancellationReason,
    },
    Aborted {
        failure: RunError,
    },
}

enum TestBodyOutcome {
    Provisional(ProvisionalTestOutcome),
    PendingFailure(Box<PendingFailure>),
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_test(
    plan: &TestPlan,
    test: &PlannedTest,
    execution_id: ExecutionId,
    events: &EventBuffer,
    event_sink: Option<&dyn RunEventSink>,
    browser: &dyn BrowserHost,
    session: &mut Option<Box<dyn BrowserSession>>,
    control: Option<&dyn RunControl>,
    options: &RunnerOptions,
    providers: &ProviderRegistry,
    observations: &ObservationStore,
    ids: scopes::ExecutionIds,
    resources: &crate::ResourceRegistry,
    waits: &crate::WaitRegistry,
) -> ExecutedTest {
    let test_started = StdInstant::now();
    let deadline = TestDeadline::new(options.test_timeout);
    let mut scope_tree = scopes::ScopeTree::new(ids, execution_id, test.id, deadline.at);
    emit_event(
        events,
        event_sink,
        ExecutionEvent::TestStarted {
            execution_id,
            test_id: test.id,
            name: test.name.clone(),
        },
    );
    let (root_scope, root_context) = scope_tree.enter(&test.body, events, event_sink);
    let mut state = TestExecutionState::new(
        options.redacted_json_fields.clone(),
        options.project_root.clone(),
    );
    let mut page: Option<Box<dyn Page>> = None;
    let mut active_step = None;
    let mut temporary = temporary::TemporaryResources::default();
    let uses_browser = test
        .required_host_capabilities
        .contains(&Capability::Browser);

    let mut cleanup_failures = Vec::new();
    let mut resource_cleanup_deadline = None;
    let mut interrupted_body_cleanup = None;
    let body_outcome = wait::with_control(control, &root_context, async {
        if uses_browser {
            let result = crate::ResourceScope {
                registry: resources,
                waits,
                context: &root_context,
                kind: webtest_observation::ResourceKind::BrowserContext,
                access: webtest_observation::ResourceAccess::Exclusive,
                cleanup_timeout: options.cleanup_timeout,
            }
            .run(
                resource::BrowserResource {
                    host: browser,
                    session,
                    options: &options.browser_context,
                    context: None,
                },
                |created| async {
                    page = Some(created);
                    let result = execute_test_body(
                        test,
                        execution_id,
                        events,
                        event_sink,
                        control,
                        options,
                        providers,
                        &deadline,
                        &mut state,
                        &mut page,
                        &mut active_step,
                        &mut scope_tree,
                        waits,
                        &mut cleanup_failures,
                        resources,
                        &mut temporary,
                    )
                    .await;
                    if root_context.cancellation.cause().is_some()
                        && let TestBodyOutcome::PendingFailure(pending) = &result
                        && let Err(cause) = pending.interruption_cleanup()
                    {
                        interrupted_body_cleanup = Some(cause);
                    }
                    drop(page.take());
                    Ok(result)
                },
                |event| match event {
                    crate::ResourceScopeEvent::Resource(event) => {
                        emit_resource_event(events, event_sink, execution_id, &root_scope, event)
                    }
                    crate::ResourceScopeEvent::Wait(event) => emit_event(
                        events,
                        event_sink,
                        ExecutionEvent::Wait {
                            execution_id,
                            scope: root_scope.clone(),
                            event,
                        },
                    ),
                },
            )
            .await;
            resource_cleanup_deadline = Some(result.cleanup_deadline);
            cleanup_failures.extend(
                result
                    .secondary
                    .into_iter()
                    .map(|failure| browser_resource_cleanup(failure, options.cleanup_timeout)),
            );
            match result.primary {
                crate::WaitCompletion::Ready(body) => Some(body),
                crate::WaitCompletion::Cancelled(_) => None,
                crate::WaitCompletion::Failed(crate::ResourceFailure::Host(error)) => Some(
                    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                        failure: RunError::Browser(error),
                    }),
                ),
                crate::WaitCompletion::Failed(crate::ResourceFailure::Invariant(error)) => Some(
                    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                        failure: resource_invariant(error),
                    }),
                ),
                crate::WaitCompletion::Rejected(error) => Some(TestBodyOutcome::Provisional(
                    ProvisionalTestOutcome::Aborted {
                        failure: RunError::Internal(format!(
                            "wait registration invariant: {error:?}"
                        )),
                    },
                )),
            }
        } else {
            let mut source = wait::TestBodyWait {
                future: Some(Box::pin(execute_test_body(
                    test,
                    execution_id,
                    events,
                    event_sink,
                    control,
                    options,
                    providers,
                    &deadline,
                    &mut state,
                    &mut page,
                    &mut active_step,
                    &mut scope_tree,
                    waits,
                    &mut cleanup_failures,
                    resources,
                    &mut temporary,
                ))),
            };
            let result = waits
                .wait(
                    &root_context,
                    &mut source,
                    options.cleanup_timeout,
                    |event| {
                        emit_event(
                            events,
                            event_sink,
                            ExecutionEvent::Wait {
                                execution_id,
                                scope: root_scope.clone(),
                                event,
                            },
                        )
                    },
                )
                .await;
            drop(source);
            resource_cleanup_deadline = Some(result.cleanup_deadline);
            cleanup_failures.extend(result.secondary.into_iter().map(|failure| CleanupFailure {
                resource: CleanupResource::ExecutionScope {
                    scope_id: root_context.scope_id,
                },
                cause: match failure {
                    crate::WaitCleanupFailure::Failed { error, .. } => error,
                    crate::WaitCleanupFailure::TimedOut { .. } => CleanupCause::TimedOut {
                        timeout_ms: duration_millis(options.cleanup_timeout),
                    },
                },
            }));
            match result.primary {
                crate::WaitCompletion::Ready(body) => Some(body),
                crate::WaitCompletion::Cancelled(_) => None,
                crate::WaitCompletion::Failed(error) => {
                    cleanup_failures.push(CleanupFailure {
                        resource: CleanupResource::ExecutionScope {
                            scope_id: root_context.scope_id,
                        },
                        cause: error,
                    });
                    None
                }
                crate::WaitCompletion::Rejected(error) => Some(TestBodyOutcome::Provisional(
                    ProvisionalTestOutcome::Aborted {
                        failure: RunError::Internal(format!(
                            "wait registration invariant: {error:?}"
                        )),
                    },
                )),
            }
        }
    })
    .await;
    if let Some(cause) = interrupted_body_cleanup {
        cleanup_failures.push(CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: root_context.scope_id,
            },
            cause,
        });
    }
    let outcome = match body_outcome {
        Some(TestBodyOutcome::PendingFailure(pending)) => {
            let failure_result = process_failure(FailureInput {
                plan,
                test_id: test.id,
                execution_id,
                pending: *pending,
                artifact_deadline: deadline.at,
                options,
                providers,
                observations,
                events,
                event_sink,
            })
            .await;
            match failure_result {
                Ok(step_failure) => ProvisionalTestOutcome::Failed(Box::new(step_failure)),
                Err(error) => ProvisionalTestOutcome::Aborted { failure: error },
            }
        }
        Some(TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
            timeout,
            active_step,
            origin,
        })) => {
            let active =
                active_step.and_then(|id| test.steps().into_iter().find(|step| step.id == id));
            emit_test_timeout(
                plan,
                test,
                active,
                execution_id,
                events,
                event_sink,
                providers,
                observations,
                timeout,
                origin,
            );
            ProvisionalTestOutcome::TimedOut {
                timeout,
                active_step,
                origin,
            }
        }
        Some(TestBodyOutcome::Provisional(provisional)) => provisional,
        None if root_context
            .cancellation
            .cause()
            .is_some_and(|cause| cause.reason != webtest_host::CancellationReason::Timeout) =>
        {
            if let Some(cause) = root_context.cancellation.cause() {
                scope_tree.interrupt(cause, events, event_sink);
            }
            ProvisionalTestOutcome::Cancelled {
                reason: CancellationReason::Requested,
            }
        }
        None => {
            scope_tree.interrupt(
                webtest_host::Cancellation {
                    reason: webtest_host::CancellationReason::Timeout,
                    causing_scope_id: root_context.scope_id,
                },
                events,
                event_sink,
            );
            let active =
                active_step.and_then(|id| test.steps().into_iter().find(|step| step.id == id));
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
                None,
            );
            ProvisionalTestOutcome::TimedOut {
                timeout: options.test_timeout,
                active_step,
                origin: None,
            }
        }
    };

    if matches!(outcome, ProvisionalTestOutcome::Cancelled { .. }) {
        scope_tree.interrupt(
            webtest_host::Cancellation {
                reason: control.map_or(
                    webtest_host::CancellationReason::UserCancelled,
                    RunControl::cancellation_reason,
                ),
                causing_scope_id: root_context.scope_id,
            },
            events,
            event_sink,
        );
    }
    drop(page.take());
    let cleanup_deadline = resource_cleanup_deadline.map_or_else(
        || crate::cleanup::CleanupDeadline::new(options.cleanup_timeout),
        |at| crate::cleanup::CleanupDeadline::at(at, options.cleanup_timeout),
    );
    if let Err(error) = waits.validate_owner_finished(root_context.scope_id) {
        cleanup_failures.push(CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: root_context.scope_id,
            },
            cause: CleanupCause::Internal {
                message: format!("wait ownership invariant: {error:?}"),
            },
        });
    }
    cleanup_failures.extend(
        temporary
            .release(
                None,
                &mut state,
                cleanup_deadline,
                &temporary::ResourceEvents {
                    registry: resources,
                    execution_id,
                    events,
                    sink: event_sink,
                },
            )
            .await,
    );
    if matches!(
        outcome,
        ProvisionalTestOutcome::TimedOut { .. } | ProvisionalTestOutcome::Cancelled { .. }
    ) && uses_browser
        && let Some(mut tainted) = session.take()
        && let Err(error) = cleanup_deadline
            .run(
                CleanupResource::BrowserSession,
                tainted.close(),
                CleanupCause::Browser,
            )
            .await
    {
        cleanup_failures.push(error);
    }
    if let Err(error) = resources.validate_owner_finished(root_scope.execution_context.scope_id) {
        cleanup_failures.push(resource_cleanup_invariant(error));
    }
    let bindings = state.final_transferable_bindings(&options.redacted_json_fields);
    for failure in &cleanup_failures {
        emit_cleanup_failed(events, event_sink, execution_id, Some(test.id), failure);
    }
    let outcome = combine_test_outcome(outcome, cleanup_failures);
    let scope_outcome = match &outcome {
        TestOutcome::Passed => webtest_observation::ScopeOutcome::Passed,
        TestOutcome::Failed(_) => webtest_observation::ScopeOutcome::Failed,
        TestOutcome::TimedOut { .. } => webtest_observation::ScopeOutcome::TimedOut,
        TestOutcome::Cancelled { .. } | TestOutcome::Skipped { .. } => {
            webtest_observation::ScopeOutcome::Cancelled
        }
        TestOutcome::Aborted { .. } => webtest_observation::ScopeOutcome::Aborted,
    };
    scope_tree.leave(scope_outcome, events, event_sink);
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

#[allow(clippy::too_many_arguments)]
async fn execute_test_body(
    test: &PlannedTest,
    execution_id: ExecutionId,
    events: &EventBuffer,
    event_sink: Option<&dyn RunEventSink>,
    control: Option<&dyn RunControl>,
    options: &RunnerOptions,
    providers: &ProviderRegistry,
    deadline: &TestDeadline,
    state: &mut TestExecutionState,
    page: &mut Option<Box<dyn Page>>,
    active_step: &mut Option<StepId>,
    scope_tree: &mut scopes::ScopeTree,
    waits: &crate::WaitRegistry,
    cleanup_failures: &mut Vec<CleanupFailure>,
    resources: &crate::ResourceRegistry,
    temporary: &mut temporary::TemporaryResources,
) -> TestBodyOutcome {
    TreeExecution {
        test,
        execution_id,
        events,
        event_sink,
        control,
        options,
        providers,
        deadline,
        state,
        page,
        active_step,
        scope_tree,
        waits,
        cleanup_failures,
        resources,
        temporary,
    }
    .node(&test.body)
    .await
}

struct TreeExecution<'a> {
    test: &'a PlannedTest,
    execution_id: ExecutionId,
    events: &'a EventBuffer,
    event_sink: Option<&'a dyn RunEventSink>,
    control: Option<&'a dyn RunControl>,
    options: &'a RunnerOptions,
    providers: &'a ProviderRegistry,
    deadline: &'a TestDeadline,
    state: &'a mut TestExecutionState,
    page: &'a mut Option<Box<dyn Page>>,
    active_step: &'a mut Option<StepId>,
    scope_tree: &'a mut scopes::ScopeTree,
    waits: &'a crate::WaitRegistry,
    cleanup_failures: &'a mut Vec<CleanupFailure>,
    resources: &'a crate::ResourceRegistry,
    temporary: &'a mut temporary::TemporaryResources,
}

impl TreeExecution<'_> {
    fn node<'a>(
        &'a mut self,
        node: &'a webtest_plan::PlanNode,
    ) -> std::pin::Pin<Box<dyn Future<Output = TestBodyOutcome> + Send + 'a>> {
        Box::pin(async move {
            let is_root = node.path.is_empty();
            if !is_root {
                self.scope_tree.enter(node, self.events, self.event_sink);
            }
            let outcome = match &node.kind {
                webtest_plan::PlanNodeKind::Sequence { children } => {
                    let mut outcome = TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed);
                    for child in children {
                        outcome = self.node(child).await;
                        if !matches!(
                            outcome,
                            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
                        ) || !self.cleanup_failures.is_empty()
                        {
                            break;
                        }
                    }
                    outcome
                }
                webtest_plan::PlanNodeKind::Operation { step } => self.leaf(step).await,
                webtest_plan::PlanNodeKind::Timeout {
                    child,
                    duration,
                    cleanup_timeout,
                } => {
                    self.timeout_node(node, child, *duration, *cleanup_timeout)
                        .await
                }
            };
            use webtest_observation::ScopeOutcome;
            let terminal = match &outcome {
                _ if !self.cleanup_failures.is_empty() => ScopeOutcome::Aborted,
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed) => {
                    ScopeOutcome::Passed
                }
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled { .. }) => {
                    ScopeOutcome::Cancelled
                }
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut { .. }) => {
                    if self.scope_tree.current_context().is_some_and(|context| {
                        context
                            .cancellation
                            .cause()
                            .is_some_and(|cause| cause.causing_scope_id == context.scope_id)
                    }) {
                        ScopeOutcome::TimedOut
                    } else {
                        ScopeOutcome::Failed
                    }
                }
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted { .. }) => {
                    ScopeOutcome::Aborted
                }
                TestBodyOutcome::PendingFailure(pending)
                    if pending.failure_class() != FailureClass::Test =>
                {
                    ScopeOutcome::Aborted
                }
                _ => ScopeOutcome::Failed,
            };
            if !is_root {
                self.scope_tree
                    .leave(terminal, self.events, self.event_sink);
            }
            outcome
        })
    }

    async fn timeout_node(
        &mut self,
        node: &webtest_plan::PlanNode,
        child: &webtest_plan::PlanNode,
        duration: Duration,
        cleanup_timeout: Option<Duration>,
    ) -> TestBodyOutcome {
        let (Some(context), Some(scope)) = (
            self.scope_tree.current_context(),
            self.scope_tree.current_event(),
        ) else {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                failure: RunError::Internal("timeout node has no owning scope".into()),
            });
        };
        let cleanup_timeout = cleanup_timeout
            .unwrap_or(self.options.cleanup_timeout)
            .min(self.options.cleanup_timeout);
        let waits = self.waits;
        let events = self.events;
        let sink = self.event_sink;
        let execution_id = self.execution_id;
        let visible = self.state.binding_checkpoint();
        let result = {
            let mut source = wait::TestBodyWait {
                future: Some(Box::pin(self.node(child))),
            };
            waits
                .wait(&context, &mut source, cleanup_timeout, |event| {
                    emit_event(
                        events,
                        sink,
                        ExecutionEvent::Wait {
                            execution_id,
                            scope: scope.clone(),
                            event,
                        },
                    )
                })
                .await
        };
        self.scope_tree.unwind_to(context.scope_id, events, sink);
        self.state.restore_bindings(&visible);
        self.cleanup_failures.extend(
            self.temporary
                .release(
                    Some(context.scope_id),
                    self.state,
                    crate::cleanup::CleanupDeadline::at(result.cleanup_deadline, cleanup_timeout),
                    &temporary::ResourceEvents {
                        registry: self.resources,
                        execution_id,
                        events,
                        sink,
                    },
                )
                .await,
        );
        if let Err(error) = self.resources.validate_owner_finished(context.scope_id) {
            self.cleanup_failures
                .push(resource_cleanup_invariant(error));
        }
        self.cleanup_failures
            .extend(result.secondary.into_iter().map(|failure| CleanupFailure {
                resource: CleanupResource::ExecutionScope {
                    scope_id: context.scope_id,
                },
                cause: match failure {
                    crate::WaitCleanupFailure::Failed { error, .. } => error,
                    crate::WaitCleanupFailure::TimedOut { .. } => CleanupCause::TimedOut {
                        timeout_ms: duration_millis(cleanup_timeout),
                    },
                },
            }));
        match result.primary {
            crate::WaitCompletion::Ready(body) => body,
            crate::WaitCompletion::Cancelled(cause)
                if cause.reason == webtest_host::CancellationReason::Timeout
                    && cause.causing_scope_id == context.scope_id =>
            {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
                    timeout: duration,
                    active_step: *self.active_step,
                    origin: Some(node.origin),
                })
            }
            crate::WaitCompletion::Cancelled(_) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: CancellationReason::Requested,
                })
            }
            crate::WaitCompletion::Failed(cause) => {
                self.cleanup_failures.push(CleanupFailure {
                    resource: CleanupResource::ExecutionScope {
                        scope_id: context.scope_id,
                    },
                    cause,
                });
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: CancellationReason::Requested,
                })
            }
            crate::WaitCompletion::Rejected(error) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                    failure: RunError::Internal(format!("wait registration invariant: {error:?}")),
                })
            }
        }
    }

    async fn leaf(&mut self, step: &PlannedStep) -> TestBodyOutcome {
        let Some(context) = self.scope_tree.current_context() else {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                failure: RunError::Internal("operation has no owning execution scope".into()),
            });
        };
        let host_context = std::sync::Arc::new(context);
        let test = self.test;
        let execution_id = self.execution_id;
        let events = self.events;
        let event_sink = self.event_sink;
        let control = self.control;
        let options = self.options;
        let providers = self.providers;
        let deadline = self.deadline;
        let state = &mut *self.state;
        let page = &mut *self.page;
        let active_step = &mut *self.active_step;

        *active_step = Some(step.id);
        if host_context.cancellation.cause().is_some()
            || control.is_some_and(RunControl::is_cancelled)
        {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                reason: CancellationReason::Requested,
            });
        }
        if let TestOperation::ServerProviderCall(call) = &step.operation {
            state.prepare_provider_arguments(call);
        }
        if let Some(control) = control {
            let before_step = async {
                if control.should_capture_bindings(test, step) {
                    control
                        .before_step_with_bindings(test, step, state.visible_step_bindings(step))
                        .await;
                } else {
                    control.before_step(test, step).await;
                }
            };
            tokio::select! {
                biased;
                _ = host_context.cancellation.cancelled() => {},
                _ = before_step => {},
            }
            if host_context.cancellation.cause().is_some() || control.is_cancelled() {
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: CancellationReason::Requested,
                });
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
        let completion = execute_step(
            providers,
            options,
            page,
            step,
            state,
            host_context
                .remaining()
                .unwrap_or(deadline.remaining())
                .min(deadline.remaining()),
            host_context.clone(),
        )
        .await;
        if let Some((owner, context)) = self.scope_tree.resource_owner()
            && let Err(error) = self.temporary.adopt(
                state,
                &owner,
                &context,
                &temporary::ResourceEvents {
                    registry: self.resources,
                    execution_id,
                    events,
                    sink: event_sink,
                },
            )
        {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                failure: resource_invariant(error),
            });
        }
        match completion {
            Ok(steps::StepCompletion::Cancelled) => {
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: CancellationReason::Requested,
                });
            }
            Ok(steps::StepCompletion::Completed) => {
                if host_context.cancellation.cause().is_some() {
                    if let TestOperation::ServerProviderCall(call) = &step.operation {
                        state.accept_provider_result_metadata(call);
                    }
                    return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                        reason: CancellationReason::Requested,
                    });
                }
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
                if matches!(
                    &error,
                    crate::StepError::Provider(webtest_provider::ProviderError::Cancelled {
                        cleanup_succeeded: true,
                        ..
                    })
                ) {
                    return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                        reason: CancellationReason::Requested,
                    });
                }
                let (redacted_fields, secrets) = state.redaction();
                let error = redact_step_error(
                    error,
                    redacted_fields,
                    secrets,
                    &options.inspection.redacted_query_parameters,
                );
                if host_context.cancellation.cause().is_none()
                    && error.failure_class() != FailureClass::Internal
                    && let Some(control) = control
                {
                    let bindings = state.visible_step_bindings(step);
                    tokio::select! {
                        biased;
                        _ = host_context.cancellation.cancelled() => {},
                        _ = control.after_step_failure(test, step, &error, &bindings) => {},
                    }
                }
                let pending = prepare_failure(PrepareFailureInput {
                    step,
                    error,
                    page,
                    options,
                    elapsed_ms: duration_millis(step_started.elapsed()),
                    secrets,
                })
                .await;
                return TestBodyOutcome::PendingFailure(Box::new(pending));
            }
        }

        *active_step = None;
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_test_timeout(
    plan: &TestPlan,
    test: &PlannedTest,
    active_step: Option<&PlannedStep>,
    execution_id: ExecutionId,
    events: &EventBuffer,
    event_sink: Option<&dyn RunEventSink>,
    providers: &ProviderRegistry,
    observations: &ObservationStore,
    timeout: Duration,
    origin: Option<webtest_text::SyntaxOrigin>,
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
                    code: webtest_observation::RuntimeFailureCode::TestTimeout,
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
        range: origin.map_or(range, |origin| origin.range),
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
                ..
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
            ..
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
    events: &EventBuffer,
    event_sink: Option<&dyn RunEventSink>,
    execution_id: ExecutionId,
    test_id: Option<TestId>,
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
            code: failure.code(),
            message: failure.message(),
        },
    );
}

pub(crate) fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn browser_resource_cleanup(
    failure: crate::ResourceCleanupFailure<webtest_browser::BrowserError>,
    timeout: Duration,
) -> CleanupFailure {
    use crate::{ResourceCleanupFailure as F, ResourceFailure, WaitCleanupFailure};
    let cause = match failure {
        F::Host(error)
        | F::Wait(WaitCleanupFailure::Failed {
            error: ResourceFailure::Host(error),
            ..
        }) => CleanupCause::Browser(error),
        F::Invariant(error)
        | F::Wait(WaitCleanupFailure::Failed {
            error: ResourceFailure::Invariant(error),
            ..
        }) => return resource_cleanup_invariant(error),
        F::TimedOut | F::Wait(WaitCleanupFailure::TimedOut { .. }) => CleanupCause::TimedOut {
            timeout_ms: duration_millis(timeout),
        },
    };
    CleanupFailure {
        resource: CleanupResource::BrowserContext,
        cause,
    }
}

fn resource_invariant(error: crate::ResourceInvariant) -> RunError {
    RunError::Internal(format!("resource ownership invariant: {error:?}"))
}
fn resource_cleanup_invariant(error: crate::ResourceInvariant) -> CleanupFailure {
    CleanupFailure {
        resource: CleanupResource::BrowserContext,
        cause: CleanupCause::Internal {
            message: format!("resource ownership invariant: {error:?}"),
        },
    }
}
fn emit_resource_event(
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    execution_id: ExecutionId,
    scope: &webtest_observation::ScopeEvent,
    event: webtest_observation::ResourceEvent,
) {
    emit_event(
        events,
        sink,
        ExecutionEvent::Resource {
            execution_id,
            scope: scope.clone(),
            event,
        },
    );
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
                origin: None,
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
