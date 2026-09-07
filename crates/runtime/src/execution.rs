use crate::events::EventBuffer;
use std::{
    future::Future,
    time::{Duration, Instant as StdInstant},
};

use tokio::time::Instant;
use webtest_browser::{BrowserHost, BrowserSession};
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
    steps::execute_step,
};

mod branch;
mod browser;
mod child;
mod concurrent;
mod failure;
mod provider;
mod resource;
mod retry;
mod scheduler;
pub(crate) mod scopes;
mod state;
mod steps;
mod temporary;
mod wait;

#[cfg(test)]
mod tests;

pub(crate) use browser::{bounded_timeout, browser_locator, browser_state};
pub(crate) use scheduler::FailureSignal;

#[derive(Default)]
pub(crate) struct RootExecutionPolicy {
    pub close_session: bool,
    pub primary_failure: Option<FailureSignal>,
}

#[cfg(test)]
pub(crate) use failure::repair_hints_for_error;

pub(crate) struct ExecutedTest {
    pub(crate) result: TestResult,
}

#[derive(Clone)]
enum ProvisionalTestOutcome {
    Finalized(Box<TestOutcome>),
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

#[derive(Clone)]
enum TestBodyOutcome {
    Provisional(ProvisionalTestOutcome),
    PendingFailure(Box<PendingFailure>),
}

impl TestBodyOutcome {
    fn failure_class(&self) -> Option<FailureClass> {
        match self {
            Self::PendingFailure(pending) => Some(pending.failure_class()),
            Self::Provisional(ProvisionalTestOutcome::Aborted { failure }) => {
                Some(failure.failure_class())
            }
            Self::Provisional(ProvisionalTestOutcome::Finalized(outcome)) => {
                outcome.failure_class()
            }
            Self::Provisional(
                ProvisionalTestOutcome::Failed(_) | ProvisionalTestOutcome::TimedOut { .. },
            ) => Some(FailureClass::Test),
            _ => None,
        }
    }
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
    policy: RootExecutionPolicy,
) -> ExecutedTest {
    let test_started = StdInstant::now();
    let deadline = TestDeadline::new(options.test_timeout);
    let scope_factory = scopes::ScopeFactory::new(ids, test.id);
    emit_event(
        events,
        event_sink,
        ExecutionEvent::TestStarted {
            execution_id,
            test_id: test.id,
            name: test.name.clone(),
        },
    );
    let root = scope_factory.root(&test.body, deadline.at);
    let root_scope = root.event.clone();
    let root_context = root.context.clone();
    let mut branch = branch::BranchState::new(options);
    branch.failure_signals.extend(policy.primary_failure);
    branch.session = session.take();
    branch.scopes.start(&root, execution_id, events, event_sink);
    let services = ExecutionServices {
        plan,
        observations,
        test,
        execution_id,
        events,
        event_sink,
        control,
        options,
        providers,
        deadline: &deadline,
        waits,
        resources,
        scopes: &scope_factory,
        browser,
    };
    let uses_browser = test
        .required_host_capabilities
        .contains(&Capability::Browser);

    let mut resource_cleanup_deadline = None;
    let body_outcome = wait::with_control(control, &root_context, async {
        let mut source = wait::TestBodyWait::new(execute_test_body(&services, &mut branch, &root));
        let mut result = waits
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
        if let Some(primary) = source.interrupted_failure.take() {
            result.primary = crate::WaitCompletion::Ready(primary);
        }
        drop(source);
        resource_cleanup_deadline = Some(result.cleanup_deadline);
        branch
            .cleanup_failures
            .extend(result.secondary.into_iter().map(|failure| CleanupFailure {
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
                branch.cleanup_failures.push(CleanupFailure {
                    resource: CleanupResource::ExecutionScope {
                        scope_id: root_context.scope_id,
                    },
                    cause: error,
                });
                None
            }
            crate::WaitCompletion::Rejected(error) => Some(TestBodyOutcome::Provisional(
                ProvisionalTestOutcome::Aborted {
                    failure: RunError::Internal(format!("wait registration invariant: {error:?}")),
                },
            )),
        }
    })
    .await;
    let body_outcome = branch.primary_failure.take().or(body_outcome);
    let outcome = match body_outcome {
        Some(TestBodyOutcome::PendingFailure(pending)) => {
            let failure_result = process_failure(FailureInput {
                plan,
                test_id: test.id,
                execution_id,
                pending: *pending,
                artifact_deadline: deadline.at,
                attempt_id: None,
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
            ProvisionalTestOutcome::Cancelled {
                reason: root_context
                    .cancellation
                    .cause()
                    .map_or(CancellationReason::UserCancelled, |cause| cause.reason),
            }
        }
        None => {
            root_context
                .cancellation
                .cancel(webtest_host::Cancellation {
                    reason: webtest_host::CancellationReason::Timeout,
                    causing_scope_id: root_context.scope_id,
                });
            let active = branch
                .active_step
                .and_then(|id| test.steps().into_iter().find(|step| step.id == id));
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
                active_step: branch.active_step,
                origin: None,
            }
        }
    };

    if matches!(outcome, ProvisionalTestOutcome::Cancelled { .. }) {
        root_context
            .cancellation
            .cancel(webtest_host::Cancellation {
                reason: control.map_or(
                    webtest_host::CancellationReason::UserCancelled,
                    RunControl::cancellation_reason,
                ),
                causing_scope_id: root_context.scope_id,
            });
    }
    drop(branch.page.take());
    let cleanup_deadline = resource_cleanup_deadline.map_or_else(
        || crate::cleanup::CleanupDeadline::new(options.cleanup_timeout),
        |at| crate::cleanup::CleanupDeadline::at(at, options.cleanup_timeout),
    );
    if let Err(error) = waits.validate_owner_finished(root_context.scope_id) {
        branch.cleanup_failures.push(CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: root_context.scope_id,
            },
            cause: CleanupCause::Internal {
                message: format!("wait ownership invariant: {error:?}"),
            },
        });
    }
    branch.cleanup_failures.extend(
        branch
            .temporary
            .release(
                None,
                &mut branch.bindings,
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
    if (policy.close_session
        || matches!(
            outcome,
            ProvisionalTestOutcome::TimedOut { .. } | ProvisionalTestOutcome::Cancelled { .. }
        ))
        && uses_browser
        && let Some(mut owned_session) = branch.session.take()
        && let Err(error) = cleanup_deadline
            .run(
                CleanupResource::BrowserSession,
                owned_session.close(),
                CleanupCause::Browser,
            )
            .await
    {
        branch.cleanup_failures.push(error);
    }
    if let Err(error) = resources.validate_owner_finished(root_scope.execution_context.scope_id) {
        branch
            .cleanup_failures
            .push(resource_cleanup_invariant(error));
    }
    *session = branch.session.take();
    let bindings = branch
        .bindings
        .final_transferable_bindings(&options.redacted_json_fields);
    for failure in &branch.cleanup_failures {
        emit_cleanup_failed(events, event_sink, execution_id, Some(test.id), failure);
    }
    let outcome = combine_test_outcome(outcome, branch.cleanup_failures);
    let scope_outcome = scope_outcome(&outcome);
    branch
        .scopes
        .finish_descendants(&root, execution_id, events, event_sink);
    branch
        .scopes
        .finish(&root, scope_outcome, execution_id, events, event_sink);
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
            branches: {
                branch.completed_branches.sort_by(|a, b| {
                    a.scope
                        .execution_context
                        .task_path
                        .cmp(&b.scope.execution_context.task_path)
                });
                branch.completed_branches
            },
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

async fn execute_test_body(
    services: &ExecutionServices<'_>,
    branch: &mut branch::BranchState,
    root: &scopes::ExecutionScope,
) -> TestBodyOutcome {
    TreeExecution { services, branch }
        .body(&services.test.body, root)
        .await
}

/// Shared immutable inputs and run services. No bindings, page handles, active
/// operation, or mutable scope cursor is stored here.
#[derive(Clone, Copy)]
struct ExecutionServices<'a> {
    plan: &'a TestPlan,
    observations: &'a ObservationStore,
    test: &'a PlannedTest,
    execution_id: ExecutionId,
    events: &'a EventBuffer,
    event_sink: Option<&'a dyn RunEventSink>,
    control: Option<&'a dyn RunControl>,
    options: &'a RunnerOptions,
    providers: &'a ProviderRegistry,
    deadline: &'a TestDeadline,
    waits: &'a crate::WaitRegistry,
    resources: &'a crate::ResourceRegistry,
    scopes: &'a scopes::ScopeFactory,
    browser: &'a dyn BrowserHost,
}

struct TreeExecution<'a, 'b> {
    services: &'a ExecutionServices<'a>,
    branch: &'b mut branch::BranchState,
}

impl TreeExecution<'_, '_> {
    fn node<'a>(
        &'a mut self,
        node: &'a webtest_plan::PlanNode,
        parent: &'a scopes::ExecutionScope,
    ) -> std::pin::Pin<Box<dyn Future<Output = TestBodyOutcome> + Send + 'a>> {
        Box::pin(async move {
            let scope = self.services.scopes.child(parent, node);
            self.branch.scopes.start(
                &scope,
                self.services.execution_id,
                self.services.events,
                self.services.event_sink,
            );
            let outcome = self.body(node, &scope).await;
            if let Some(class) = outcome.failure_class() {
                if scope.context.cancellation.cause().is_none() {
                    self.branch
                        .primary_failure
                        .get_or_insert_with(|| outcome.clone());
                }
                for signal in &self.branch.failure_signals {
                    signal.report(class);
                }
            }
            use webtest_observation::ScopeOutcome;
            let terminal = match &outcome {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(outcome))
                    if self.branch.cleanup_failures.is_empty() =>
                {
                    scope_outcome(outcome)
                }
                _ if !self.branch.cleanup_failures.is_empty() => ScopeOutcome::Aborted,
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed) => {
                    ScopeOutcome::Passed
                }
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled { .. }) => {
                    ScopeOutcome::Cancelled
                }
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut { .. }) => {
                    if scope
                        .context
                        .cancellation
                        .cause()
                        .is_some_and(|cause| cause.causing_scope_id == scope.id())
                    {
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
            self.branch.scopes.finish(
                &scope,
                terminal,
                self.services.execution_id,
                self.services.events,
                self.services.event_sink,
            );
            outcome
        })
    }

    fn body<'a>(
        &'a mut self,
        node: &'a webtest_plan::PlanNode,
        scope: &'a scopes::ExecutionScope,
    ) -> std::pin::Pin<Box<dyn Future<Output = TestBodyOutcome> + Send + 'a>> {
        Box::pin(async move {
            match &node.kind {
                webtest_plan::PlanNodeKind::Retry { child, settings } => {
                    self.retry_node(child, *settings, scope).await
                }
                webtest_plan::PlanNodeKind::Parallel { children, .. } => {
                    self.concurrent_node(children, scope, scheduler::SiblingPolicy::All, None)
                        .await
                }
                webtest_plan::PlanNodeKind::Race { children, result } => {
                    self.concurrent_node(
                        children,
                        scope,
                        scheduler::SiblingPolicy::FirstSuccess,
                        result.as_ref(),
                    )
                    .await
                }
                webtest_plan::PlanNodeKind::ResourceScope { resource, body } => {
                    self.resource_node(*resource, body, scope).await
                }
                webtest_plan::PlanNodeKind::Sequence { children } => {
                    let mut outcome = TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed);
                    for child in children {
                        outcome = self.node(child, scope).await;
                        if !matches!(
                            outcome,
                            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
                        ) || !self.branch.cleanup_failures.is_empty()
                            || self.branch.provided.is_some()
                        {
                            break;
                        }
                    }
                    outcome
                }
                webtest_plan::PlanNodeKind::Operation { step } => self.leaf(step, scope).await,
                webtest_plan::PlanNodeKind::Timeout {
                    child,
                    duration,
                    cleanup_timeout,
                } => {
                    self.timeout_node(node, child, *duration, *cleanup_timeout, scope)
                        .await
                }
            }
        })
    }

    async fn resource_node(
        &mut self,
        resource: webtest_plan::ResourcePlan,
        body: &webtest_plan::PlanNode,
        owner: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        match resource {
            webtest_plan::ResourcePlan::BrowserContext => {
                self.browser_resource_node(body, owner).await
            }
        }
    }

    async fn browser_resource_node(
        &mut self,
        body: &webtest_plan::PlanNode,
        owner: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        if self.branch.page.is_some() {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                failure: resource_invariant(crate::ResourceInvariant::AccessConflict),
            });
        }
        let services = self.services;
        let cleanup_timeout = owner.cleanup_timeout(services.options.cleanup_timeout);
        let context = owner.context.clone();
        let scope = owner.event.clone();
        let mut session = self.branch.session.take();
        let mut interrupted_body_cleanup = None;
        let signals = self.branch.failure_signals.clone();
        let result = crate::ResourceScope {
            registry: services.resources,
            waits: services.waits,
            context: &context,
            kind: webtest_observation::ResourceKind::BrowserContext,
            access: webtest_observation::ResourceAccess::Exclusive,
            cleanup_timeout,
        }
        .run_observed(
            resource::BrowserResource {
                host: services.browser,
                session: &mut session,
                options: &services.options.browser_context,
                context: None,
            },
            |created| async {
                self.branch.page = Some(created);
                let result = self.node(body, owner).await;
                if context.cancellation.cause().is_some()
                    && let TestBodyOutcome::PendingFailure(pending) = &result
                    && let Err(cause) = pending.interruption_cleanup()
                {
                    interrupted_body_cleanup = Some(cause);
                }
                drop(self.branch.page.take());
                Ok(result)
            },
            |event| match event {
                crate::ResourceScopeEvent::Resource(event) => emit_resource_event(
                    services.events,
                    services.event_sink,
                    services.execution_id,
                    &scope,
                    event,
                ),
                crate::ResourceScopeEvent::Wait(event) => emit_event(
                    services.events,
                    services.event_sink,
                    ExecutionEvent::Wait {
                        execution_id: services.execution_id,
                        scope: scope.clone(),
                        event,
                    },
                ),
            },
            |primary| {
                let class = match primary {
                    crate::WaitCompletion::Ready(body) => body.failure_class(),
                    crate::WaitCompletion::Failed(crate::ResourceFailure::Host(_)) => {
                        Some(FailureClass::Infrastructure)
                    }
                    crate::WaitCompletion::Failed(crate::ResourceFailure::Invariant(_))
                    | crate::WaitCompletion::Rejected(_) => Some(FailureClass::Internal),
                    _ => None,
                };
                if let Some(class) = class {
                    for signal in &signals {
                        signal.report(class);
                    }
                }
            },
        )
        .await;
        self.branch.session = session;
        self.branch.cleanup_failures.extend(
            result
                .secondary
                .into_iter()
                .map(|failure| browser_resource_cleanup(failure, cleanup_timeout)),
        );
        if let Some(cause) = interrupted_body_cleanup {
            self.branch.cleanup_failures.push(CleanupFailure {
                resource: CleanupResource::ExecutionScope {
                    scope_id: context.scope_id,
                },
                cause,
            });
        }
        // Temporary handles transferred during the body belong to this explicit
        // resource scope and finish before its terminal scope event.
        let owned_scopes = self.branch.scopes.subtree_ids(owner);
        self.branch.cleanup_failures.extend(
            self.branch
                .temporary
                .release(
                    Some(&owned_scopes),
                    &mut self.branch.bindings,
                    crate::cleanup::CleanupDeadline::at(result.cleanup_deadline, cleanup_timeout),
                    &temporary::ResourceEvents {
                        registry: services.resources,
                        execution_id: services.execution_id,
                        events: services.events,
                        sink: services.event_sink,
                    },
                )
                .await,
        );
        if let Err(error) = services.resources.validate_owner_finished(owner.id()) {
            self.branch
                .cleanup_failures
                .push(resource_cleanup_invariant(error));
        }
        self.branch.scopes.finish_descendants(
            owner,
            services.execution_id,
            services.events,
            services.event_sink,
        );
        match result.primary {
            crate::WaitCompletion::Ready(body) => body,
            crate::WaitCompletion::Cancelled(cause) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: cause.reason,
                })
            }
            crate::WaitCompletion::Failed(crate::ResourceFailure::Host(error)) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                    failure: RunError::Browser(error),
                })
            }
            crate::WaitCompletion::Failed(crate::ResourceFailure::Invariant(error)) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                    failure: resource_invariant(error),
                })
            }
            crate::WaitCompletion::Rejected(error) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                    failure: RunError::Internal(format!("wait registration invariant: {error:?}")),
                })
            }
        }
    }

    async fn timeout_node(
        &mut self,
        node: &webtest_plan::PlanNode,
        child: &webtest_plan::PlanNode,
        duration: Duration,
        cleanup_timeout: Option<Duration>,
        owner: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let context = owner.context.clone();
        let scope = owner.event.clone();
        let cleanup_timeout = cleanup_timeout
            .unwrap_or(self.services.options.cleanup_timeout)
            .min(owner.cleanup_timeout(self.services.options.cleanup_timeout));
        let waits = self.services.waits;
        let events = self.services.events;
        let sink = self.services.event_sink;
        let execution_id = self.services.execution_id;
        let visible = self.branch.bindings.binding_checkpoint();
        let result = {
            let mut source = wait::TestBodyWait::new(self.node(child, owner));
            let mut result = waits
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
                .await;
            if let Some(primary) = source.interrupted_failure.take() {
                result.primary = crate::WaitCompletion::Ready(primary);
            }
            result
        };
        let owned_scopes = self.branch.scopes.subtree_ids(owner);
        self.branch.bindings.restore_bindings(&visible);
        self.branch.cleanup_failures.extend(
            self.branch
                .temporary
                .release(
                    Some(&owned_scopes),
                    &mut self.branch.bindings,
                    crate::cleanup::CleanupDeadline::at(result.cleanup_deadline, cleanup_timeout),
                    &temporary::ResourceEvents {
                        registry: self.services.resources,
                        execution_id,
                        events,
                        sink,
                    },
                )
                .await,
        );
        self.branch
            .scopes
            .finish_descendants(owner, execution_id, events, sink);
        if let Err(error) = self
            .services
            .resources
            .validate_owner_finished(context.scope_id)
        {
            self.branch
                .cleanup_failures
                .push(resource_cleanup_invariant(error));
        }
        self.branch
            .cleanup_failures
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
                    active_step: self.branch.active_step,
                    origin: Some(node.origin),
                })
            }
            crate::WaitCompletion::Cancelled(cause) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: cause.reason,
                })
            }
            crate::WaitCompletion::Failed(cause) => {
                self.branch.cleanup_failures.push(CleanupFailure {
                    resource: CleanupResource::ExecutionScope {
                        scope_id: context.scope_id,
                    },
                    cause,
                });
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: CancellationReason::UserCancelled,
                })
            }
            crate::WaitCompletion::Rejected(error) => {
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                    failure: RunError::Internal(format!("wait registration invariant: {error:?}")),
                })
            }
        }
    }

    async fn leaf(
        &mut self,
        step: &PlannedStep,
        scope: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let host_context = std::sync::Arc::new(scope.context.clone());
        let test = self.services.test;
        let execution_id = self.services.execution_id;
        let events = self.services.events;
        let event_sink = self.services.event_sink;
        let control = self.services.control;
        let options = self.services.options;
        let providers = self.services.providers;
        let deadline = self.services.deadline;
        let state = &mut self.branch.bindings;
        let page = &mut self.branch.page;
        let active_step = &mut self.branch.active_step;

        *active_step = Some(step.id);
        if host_context.cancellation.cause().is_some()
            || control.is_some_and(RunControl::is_cancelled)
        {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                reason: host_context.cancellation.cause().map_or_else(
                    || {
                        control.map_or(
                            CancellationReason::UserCancelled,
                            RunControl::cancellation_reason,
                        )
                    },
                    |cause| cause.reason,
                ),
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
                    reason: host_context
                        .cancellation
                        .cause()
                        .map_or_else(|| control.cancellation_reason(), |cause| cause.reason),
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
        let completion = match completion {
            Ok(steps::StepCompletion::Provided(value)) => {
                self.branch.provided = Some(value);
                Ok(steps::StepCompletion::Completed)
            }
            other => other,
        };
        if let Err(error) = self.branch.temporary.adopt(
            state,
            &scope.resource_owner.event,
            &scope.resource_owner.context,
            &temporary::ResourceEvents {
                registry: self.services.resources,
                execution_id,
                events,
                sink: event_sink,
            },
        ) {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                failure: resource_invariant(error),
            });
        }
        match completion {
            Ok(steps::StepCompletion::Cancelled) => {
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: host_context.cancellation.cause().map_or_else(
                        || {
                            control.map_or(
                                CancellationReason::UserCancelled,
                                RunControl::cancellation_reason,
                            )
                        },
                        |cause| cause.reason,
                    ),
                });
            }
            Ok(steps::StepCompletion::Completed | steps::StepCompletion::Provided(_)) => {
                if host_context.cancellation.cause().is_some() {
                    if let TestOperation::ServerProviderCall(call) = &step.operation {
                        state.accept_provider_result_metadata(call);
                    }
                    return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                        reason: host_context.cancellation.cause().map_or_else(
                            || {
                                control.map_or(
                                    CancellationReason::UserCancelled,
                                    RunControl::cancellation_reason,
                                )
                            },
                            |cause| cause.reason,
                        ),
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
                if let crate::StepError::Provider(webtest_provider::ProviderError::Cancelled {
                    cleanup_succeeded: true,
                    cause,
                }) = &error
                {
                    return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                        reason: cause.reason,
                    });
                }
                for signal in &self.branch.failure_signals {
                    signal.report(error.failure_class());
                }
                let (redacted_fields, secrets) = state.redaction();
                let error = redact_step_error(
                    error,
                    redacted_fields,
                    secrets,
                    &options.inspection.redacted_query_parameters,
                );
                let primary_precedes_cancellation = host_context.cancellation.cause().is_none();
                if primary_precedes_cancellation {
                    self.branch.primary_failure = Some(TestBodyOutcome::PendingFailure(Box::new(
                        PendingFailure::primary(
                            step,
                            error.clone(),
                            duration_millis(step_started.elapsed()),
                        ),
                    )));
                }
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
                let outcome = TestBodyOutcome::PendingFailure(Box::new(pending));
                if primary_precedes_cancellation {
                    self.branch.primary_failure = Some(outcome.clone());
                }
                return outcome;
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
    let outcome = match provisional {
        ProvisionalTestOutcome::Finalized(outcome) => *outcome,
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
    if cleanup_failures.is_empty() {
        return outcome;
    }
    let prior_outcome = match outcome {
        TestOutcome::Aborted {
            failure,
            prior_outcome,
        } => {
            return TestOutcome::Aborted {
                failure: failure.combine_with_cleanup(cleanup_failures),
                prior_outcome,
            };
        }
        TestOutcome::Passed | TestOutcome::Skipped { .. } => None,
        TestOutcome::Failed(failure) => Some(Box::new(PriorTestOutcome::Failed(failure))),
        TestOutcome::TimedOut {
            timeout,
            active_step,
        } => Some(Box::new(PriorTestOutcome::TimedOut {
            timeout,
            active_step,
        })),
        TestOutcome::Cancelled { reason } => Some(Box::new(PriorTestOutcome::Cancelled { reason })),
    };
    TestOutcome::Aborted {
        failure: cleanup_run_error(cleanup_failures),
        prior_outcome,
    }
}

fn scope_outcome(outcome: &TestOutcome) -> webtest_observation::ScopeOutcome {
    use webtest_observation::ScopeOutcome;
    match outcome {
        TestOutcome::Passed => ScopeOutcome::Passed,
        TestOutcome::Failed(_) => ScopeOutcome::Failed,
        TestOutcome::TimedOut { .. } => ScopeOutcome::TimedOut,
        TestOutcome::Cancelled { .. } | TestOutcome::Skipped { .. } => ScopeOutcome::Cancelled,
        TestOutcome::Aborted { .. } => ScopeOutcome::Aborted,
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
