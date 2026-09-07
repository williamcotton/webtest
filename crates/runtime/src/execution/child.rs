//! Owned child execution shared by sibling branches and sequential retry attempts.
use super::*;
use crate::BranchResult;
use webtest_plan::PlanNode;

/// Failed alternatives remain in events and branch results. Only unrecovered
/// failures become current editor diagnostics. Dropping an interrupted subtree
/// still publishes the observations it already collected.
pub(super) struct ObservationProjection<'a> {
    pub pending: ObservationStore,
    parent: &'a ObservationStore,
    plan: &'a TestPlan,
    pub recovered: bool,
}

impl<'a> ObservationProjection<'a> {
    pub fn new(services: &ExecutionServices<'a>) -> Self {
        Self {
            pending: ObservationStore::default(),
            parent: services.observations,
            plan: services.plan,
            recovered: false,
        }
    }
}

impl Drop for ObservationProjection<'_> {
    fn drop(&mut self) {
        if !self.recovered {
            for observation in self
                .pending
                .observations_for(self.plan.file, self.plan.source_revision)
            {
                self.parent.record(observation);
            }
        }
    }
}

pub(super) struct ChildCompletion {
    pub result: BranchResult,
    pub page: Option<Box<dyn webtest_browser::Page>>,
    pub provided: Option<state::ValueTransfer>,
}

pub(super) async fn execute_child(
    services: &ExecutionServices<'_>,
    node: &PlanNode,
    scope: scopes::ExecutionScope,
    mut branch: branch::BranchState,
) -> ChildCompletion {
    let inherited_page = branch.page.is_some();
    let started = StdInstant::now();
    let cleanup_timeout = scope.cleanup_timeout(services.options.cleanup_timeout);
    branch.scopes.start(
        &scope,
        services.execution_id,
        services.events,
        services.event_sink,
    );
    let result = {
        let mut execution = TreeExecution {
            services,
            branch: &mut branch,
        };
        let mut source = wait::TestBodyWait::new(execution.body(node, &scope));
        let mut result = services
            .waits
            .wait(&scope.context, &mut source, cleanup_timeout, |event| {
                emit_event(
                    services.events,
                    services.event_sink,
                    ExecutionEvent::Wait {
                        execution_id: services.execution_id,
                        scope: scope.event.clone(),
                        event,
                    },
                );
            })
            .await;
        if let Some(primary) = source.interrupted_failure.take() {
            result.primary = crate::WaitCompletion::Ready(primary);
        }
        result
    };
    let cleanup = crate::cleanup::CleanupDeadline::at(result.cleanup_deadline, cleanup_timeout);
    let primary = branch
        .primary_failure
        .take()
        .map_or(result.primary, crate::WaitCompletion::Ready);
    let provisional = match primary {
        crate::WaitCompletion::Ready(body) => {
            finalize_body(services, body, scope.event.execution_context.attempt_id).await
        }
        crate::WaitCompletion::Cancelled(cause)
            if cause.reason == webtest_host::CancellationReason::Timeout
                && cause.causing_scope_id == scope.id() =>
        {
            let timeout = match &node.kind {
                webtest_plan::PlanNodeKind::Timeout { duration, .. } => *duration,
                _ => services.options.test_timeout,
            };
            finalize_body(
                services,
                TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
                    timeout,
                    active_step: branch.active_step,
                    origin: Some(node.origin),
                }),
                scope.event.execution_context.attempt_id,
            )
            .await
        }
        crate::WaitCompletion::Cancelled(cause) => ProvisionalTestOutcome::Cancelled {
            reason: cause.reason,
        },
        crate::WaitCompletion::Failed(cause) => {
            branch.cleanup_failures.push(CleanupFailure {
                resource: CleanupResource::ExecutionScope {
                    scope_id: scope.id(),
                },
                cause,
            });
            ProvisionalTestOutcome::Cancelled {
                reason: CancellationReason::UserCancelled,
            }
        }
        crate::WaitCompletion::Rejected(error) => ProvisionalTestOutcome::Aborted {
            failure: RunError::Internal(format!("wait registration invariant: {error:?}")),
        },
    };
    let class = match &provisional {
        ProvisionalTestOutcome::Finalized(outcome) => outcome.failure_class(),
        ProvisionalTestOutcome::Aborted { failure } => Some(failure.failure_class()),
        _ => None,
    };
    if let Some(class) = class {
        for signal in &branch.failure_signals {
            signal.report(class);
        }
    }
    branch
        .cleanup_failures
        .extend(result.secondary.into_iter().map(|failure| CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: scope.id(),
            },
            cause: match failure {
                crate::WaitCleanupFailure::Failed { error, .. } => error,
                crate::WaitCleanupFailure::TimedOut { .. } => CleanupCause::TimedOut {
                    timeout_ms: duration_millis(cleanup_timeout),
                },
            },
        }));
    let page = if inherited_page {
        branch.page.take()
    } else {
        drop(branch.page.take());
        None
    };
    branch.cleanup_failures.extend(
        branch
            .temporary
            .release(
                None,
                &mut branch.bindings,
                cleanup,
                &temporary::ResourceEvents {
                    registry: services.resources,
                    execution_id: services.execution_id,
                    events: services.events,
                    sink: services.event_sink,
                },
            )
            .await,
    );
    if let Some(mut session) = branch.session.take()
        && let Err(error) = cleanup
            .run(
                CleanupResource::BrowserSession,
                session.close(),
                CleanupCause::Browser,
            )
            .await
    {
        branch.cleanup_failures.push(error);
    }
    if let Err(error) = services.resources.validate_owner_finished(scope.id()) {
        branch
            .cleanup_failures
            .push(resource_cleanup_invariant(error));
    }
    if let Err(error) = services.waits.validate_owner_finished(scope.id()) {
        branch.cleanup_failures.push(CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: scope.id(),
            },
            cause: CleanupCause::Internal {
                message: format!("child wait ownership invariant: {error:?}"),
            },
        });
    }
    for failure in &branch.cleanup_failures {
        emit_cleanup_failed(
            services.events,
            services.event_sink,
            services.execution_id,
            Some(services.test.id),
            failure,
        );
    }
    let outcome = combine_test_outcome(provisional, branch.cleanup_failures);
    branch.scopes.finish_descendants(
        &scope,
        services.execution_id,
        services.events,
        services.event_sink,
    );
    branch.scopes.finish(
        &scope,
        scope_outcome(&outcome),
        services.execution_id,
        services.events,
        services.event_sink,
    );
    let mut event = scope.event;
    event.outcome = Some(scope_outcome(&outcome));
    event.cancellation = scope.context.cancellation.cause();
    branch.completed_branches.sort_by(|a, b| {
        a.scope
            .execution_context
            .task_path
            .cmp(&b.scope.execution_context.task_path)
    });
    let provided = if matches!(outcome, TestOutcome::Passed) {
        branch
            .provided
            .take()
            .map(|value| branch.bindings.transfer(value))
    } else {
        None
    };
    ChildCompletion {
        page,
        provided,
        result: BranchResult {
            race_winner: false,
            scope: event,
            outcome,
            duration: started.elapsed(),
            branches: branch.completed_branches,
        },
    }
}

async fn finalize_body(
    services: &ExecutionServices<'_>,
    body: TestBodyOutcome,
    attempt_id: Option<webtest_model::AttemptId>,
) -> ProvisionalTestOutcome {
    match body {
        TestBodyOutcome::PendingFailure(pending) => match process_failure(FailureInput {
            plan: services.plan,
            test_id: services.test.id,
            execution_id: services.execution_id,
            pending: *pending,
            artifact_deadline: services.deadline.at,
            attempt_id,
            options: services.options,
            providers: services.providers,
            observations: services.observations,
            events: services.events,
            event_sink: services.event_sink,
        })
        .await
        {
            Ok(failure) => ProvisionalTestOutcome::Failed(Box::new(failure)),
            Err(failure) => ProvisionalTestOutcome::Aborted { failure },
        },
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
            timeout,
            active_step,
            origin,
        }) => {
            let active = active_step
                .and_then(|id| services.test.steps().into_iter().find(|step| step.id == id));
            emit_test_timeout(
                services.plan,
                services.test,
                active,
                services.execution_id,
                services.events,
                services.event_sink,
                services.providers,
                services.observations,
                timeout,
                origin,
            );
            ProvisionalTestOutcome::TimedOut {
                timeout,
                active_step,
                origin,
            }
        }
        TestBodyOutcome::Provisional(outcome) => outcome,
    }
}
