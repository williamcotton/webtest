use super::*;
use crate::BranchResult;
use webtest_plan::PlanNode;

impl TreeExecution<'_, '_> {
    pub(super) async fn parallel_node(
        &mut self,
        children: &[PlanNode],
        parent: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let services = self.services;
        let children = children.iter().map(|node| {
            let scope = services.scopes.branch(parent, node);
            let state = self.branch.fork();
            (scope.context.clone(), execute_branch(services, node, scope, state))
        }).collect();
        let first = self.branch.completed_branches.len();
        scheduler::parallel(
            &parent.context,
            children,
            |result| result.outcome.failure_class(),
            |_, result| {
                // Persist completed children in the parent's own state as they
                // finish. An enclosing interrupted wait cannot discard them.
                self.branch.completed_branches.push(result);
            },
        ).await;
        let results = &mut self.branch.completed_branches[first..];
        results.sort_by(|a, b| a.scope.execution_context.task_path.cmp(&b.scope.execution_context.task_path));
        let mut summary = &TestOutcome::Passed;
        for child in results {
            if severity(&child.outcome) > severity(summary) {
                summary = &child.outcome;
            }
        }
        if matches!(summary, TestOutcome::Passed) {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
        } else {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(Box::new(summary.clone())))
        }
    }
}

fn severity(outcome: &TestOutcome) -> u8 {
    match outcome.failure_class() {
        Some(FailureClass::Internal) => 4,
        Some(FailureClass::Infrastructure) => 3,
        Some(FailureClass::Test) => 2,
        None if matches!(outcome, TestOutcome::Passed) => 0,
        None => 1,
    }
}

async fn execute_branch(
    services: &ExecutionServices<'_>,
    node: &PlanNode,
    scope: scopes::ExecutionScope,
    mut branch: branch::BranchState,
) -> BranchResult {
    let started = StdInstant::now();
    branch.scopes.start(&scope, services.execution_id, services.events, services.event_sink);
    let result = {
        let mut execution = TreeExecution { services, branch: &mut branch };
        let mut source = wait::TestBodyWait { future: Some(Box::pin(execution.body(node, &scope))) };
        services.waits.wait(&scope.context, &mut source, services.options.cleanup_timeout, |event| {
            emit_event(services.events, services.event_sink, ExecutionEvent::Wait {
                execution_id: services.execution_id, scope: scope.event.clone(), event,
            });
        }).await
    };
    let cleanup = crate::cleanup::CleanupDeadline::at(result.cleanup_deadline, services.options.cleanup_timeout);
    let provisional = match result.primary {
        crate::WaitCompletion::Ready(body) => finalize_body(services, body).await,
        crate::WaitCompletion::Cancelled(cause) if cause.reason == webtest_host::CancellationReason::Timeout && cause.causing_scope_id == scope.id() => {
            let timeout = match &node.kind {
                webtest_plan::PlanNodeKind::Timeout { duration, .. } => *duration,
                _ => services.options.test_timeout,
            };
            finalize_body(services, TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
                timeout, active_step: branch.active_step, origin: Some(node.origin),
            })).await
        }
        crate::WaitCompletion::Cancelled(_) => ProvisionalTestOutcome::Cancelled { reason: CancellationReason::Requested },
        crate::WaitCompletion::Failed(cause) => {
            branch.cleanup_failures.push(CleanupFailure {
                resource: CleanupResource::ExecutionScope { scope_id: scope.id() }, cause,
            });
            ProvisionalTestOutcome::Cancelled { reason: CancellationReason::Requested }
        }
        crate::WaitCompletion::Rejected(error) => ProvisionalTestOutcome::Aborted {
            failure: RunError::Internal(format!("wait registration invariant: {error:?}")),
        },
    };
    branch.cleanup_failures.extend(result.secondary.into_iter().map(|failure| CleanupFailure {
        resource: CleanupResource::ExecutionScope { scope_id: scope.id() },
        cause: match failure {
            crate::WaitCleanupFailure::Failed { error, .. } => error,
            crate::WaitCleanupFailure::TimedOut { .. } => CleanupCause::TimedOut { timeout_ms: duration_millis(services.options.cleanup_timeout) },
        },
    }));
    drop(branch.page.take());
    branch.cleanup_failures.extend(branch.temporary.release(
        None, &mut branch.bindings, cleanup,
        &temporary::ResourceEvents {
            registry: services.resources, execution_id: services.execution_id,
            events: services.events, sink: services.event_sink,
        },
    ).await);
    if let Some(mut session) = branch.session.take()
        && let Err(error) = cleanup.run(CleanupResource::BrowserSession, session.close(), CleanupCause::Browser).await {
        branch.cleanup_failures.push(error);
    }
    if let Err(error) = services.resources.validate_owner_finished(scope.id()) {
        branch.cleanup_failures.push(resource_cleanup_invariant(error));
    }
    for failure in &branch.cleanup_failures {
        emit_cleanup_failed(services.events, services.event_sink, services.execution_id, Some(services.test.id), failure);
    }
    let outcome = combine_test_outcome(provisional, branch.cleanup_failures);
    branch.scopes.finish_descendants(&scope, services.execution_id, services.events, services.event_sink);
    branch.scopes.finish(&scope, scope_outcome(&outcome), services.execution_id, services.events, services.event_sink);
    let mut event = scope.event;
    event.outcome = Some(scope_outcome(&outcome));
    event.cancellation = scope.context.cancellation.cause();
    branch.completed_branches.sort_by(|a, b| a.scope.execution_context.task_path.cmp(&b.scope.execution_context.task_path));
    BranchResult { scope: event, outcome, duration: started.elapsed(), branches: branch.completed_branches }
}

async fn finalize_body(services: &ExecutionServices<'_>, body: TestBodyOutcome) -> ProvisionalTestOutcome {
    match body {
        TestBodyOutcome::PendingFailure(pending) => match process_failure(FailureInput {
            plan: services.plan, test_id: services.test.id, execution_id: services.execution_id,
            pending: *pending, artifact_deadline: services.deadline.at, options: services.options,
            providers: services.providers, observations: services.observations, events: services.events, event_sink: services.event_sink,
        }).await {
            Ok(failure) => ProvisionalTestOutcome::Failed(Box::new(failure)),
            Err(failure) => ProvisionalTestOutcome::Aborted { failure },
        },
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut { timeout, active_step, origin }) => {
            let active = active_step.and_then(|id| services.test.steps().into_iter().find(|step| step.id == id));
            emit_test_timeout(services.plan, services.test, active, services.execution_id, services.events, services.event_sink,
                services.providers, services.observations, timeout, origin);
            ProvisionalTestOutcome::TimedOut { timeout, active_step, origin }
        }
        TestBodyOutcome::Provisional(outcome) => outcome,
    }
}
