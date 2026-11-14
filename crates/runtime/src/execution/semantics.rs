use super::child::finalize_body;
use super::*;
use crate::StepError;
use webtest_plan::{PlanNode, semantics::SemanticOperation};
impl TreeExecution<'_, '_> {
    pub(super) async fn semantic_node(
        &mut self,
        operation: &SemanticOperation,
        children: &[PlanNode],
        scope: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        match operation {
            SemanticOperation::Guards { clauses, failures } => {
                self.guard_node(clauses, *failures, children, scope).await
            }
            SemanticOperation::Select {
                actor,
                cursor,
                duration,
                alternatives,
                timeout,
                failures,
                result,
            } => {
                self.select_node(
                    *actor,
                    *cursor,
                    *duration,
                    alternatives,
                    *timeout,
                    *failures,
                    result.as_ref(),
                    children,
                    scope,
                )
                .await
            }
            SemanticOperation::ActorScope {
                actor,
                sources,
                name,
            } => {
                if let Some(body) = children.first() {
                    self.browser_resource_body(
                        fixtures::BrowserBody::Actor {
                            actor: *actor,
                            sources,
                            name,
                            body,
                        },
                        scope,
                    )
                    .await
                } else {
                    internal("actor has no owned body")
                }
            }
            SemanticOperation::ActorAccess { actor, .. } => {
                if let Some(body) = children.first() {
                    self.actor_access(*actor, body, scope).await
                } else {
                    internal("actor access has no body")
                }
            }
            SemanticOperation::Assume { condition } => {
                match crate::evaluation::evaluate(condition, self.branch.bindings.environment()) {
                    Ok(webtest_model::Value::Bool(true)) => {
                        TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
                    }
                    Ok(webtest_model::Value::Bool(false)) => {
                        if let Some(child) = children.first() {
                            self.node(child, scope).await
                        } else {
                            internal("assume has no false verdict")
                        }
                    }
                    _ => internal("assume predicate is not a typed Bool"),
                }
            }
            SemanticOperation::Check { .. } => {
                let visible = self.branch.bindings.binding_checkpoint();
                let depth = self.branch.check_depth;
                self.branch.check_depth += 1;
                let outcome = self.check_node(children, scope).await;
                self.branch.bindings.restore_bindings(&visible);
                self.branch.check_depth = depth;
                outcome
            }
        }
    }
    async fn check_node(
        &mut self,
        children: &[PlanNode],
        scope: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        emit_semantic(
            self.services,
            scope,
            webtest_observation::SemanticEvent::CheckStarted,
        );
        let mut first_failure = None;
        let mut details = Vec::new();
        let mut total = 0usize;
        let statements = children;
        for child in statements {
            let primary = self.branch.primary_failure.clone();
            let first = self.branch.completed_branches.len();
            let result = self.node(child, scope).await;
            let result = finalize_body(
                self.services,
                result,
                &scope.event,
                self.branch.active_operation.as_ref(),
            )
            .await;
            let outcome = combine_test_outcome(result, Vec::new());
            match outcome {
                TestOutcome::Passed => (),
                TestOutcome::Failed(failure)
                    if recoverable(&failure)
                        && self.branch.completed_branches[first..]
                            .iter()
                            .all(|branch| recoverable_outcome(&branch.outcome)) =>
                {
                    self.branch.primary_failure = primary;
                    let mut failures = Vec::new();
                    if matches!(child.kind, webtest_plan::PlanNodeKind::Parallel { .. }) {
                        for branch in &self.branch.completed_branches[first..] {
                            collect_branch_assertions(branch, &mut failures);
                        }
                    }
                    if failures.is_empty() {
                        failures.push(&failure);
                    }
                    for failure in failures {
                        total += 1;
                        if details.len() < 32 {
                            details.push(webtest_observation::CheckAssertion {
                                origin: failure.step.origin,
                                source_revision: self
                                    .services
                                    .plan
                                    .source_revision_for(failure.step.origin.file)
                                    .unwrap_or(self.services.plan.source_revision),
                                failure: Box::new(failure::runtime_failure(&failure.error)),
                            });
                        }
                    }
                    if first_failure.is_none() {
                        first_failure = Some(failure);
                    }
                }
                TestOutcome::Verdict { .. } if first_failure.is_some() => break,
                outcome => {
                    return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(
                        Box::new(outcome),
                    ));
                }
            }
            if !self.branch.cleanup_failures.is_empty() {
                return internal("check child cleanup failed");
            }
        }
        emit_semantic(
            self.services,
            scope,
            webtest_observation::SemanticEvent::CheckFinished {
                failures: total,
                truncated: total > 32,
            },
        );
        let Some(mut primary) = first_failure else {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed);
        };
        primary.error = StepError::Assertion(Box::new(crate::AssertionFailure {
            matcher: webtest_plan::ValueMatcher::Check,
            expected: None,
            actual: webtest_model::Value::Null,
            message: format!("check failed with {total} assertion failure(s)"),
            diff: webtest_observation::ValueDiff::Check {
                failures: details,
                total,
                truncated: total > 32,
            },
        }));
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(Box::new(
            TestOutcome::Failed(primary),
        )))
    }
}
fn collect_branch_assertions<'a>(
    branch: &'a crate::BranchResult,
    failures: &mut Vec<&'a StepFailure>,
) {
    if let TestOutcome::Failed(failure) = &branch.outcome {
        let before = failures.len();
        for child in &branch.branches {
            collect_branch_assertions(child, failures);
        }
        if failures.len() == before {
            failures.push(failure);
        }
    }
}

fn recoverable(failure: &StepFailure) -> bool {
    match (&failure.step.operation, &failure.error) {
        (_, StepError::Assertion(_)) => true,
        (TestOperation::Assertion(_), StepError::Decode(_)) => true,
        (TestOperation::Assertion(_), StepError::Browser(error)) => !error.is_infrastructure(),
        _ => false,
    }
}
fn recoverable_outcome(outcome: &TestOutcome) -> bool {
    match outcome {
        TestOutcome::Passed => true,
        TestOutcome::Failed(failure) => recoverable(failure),
        _ => false,
    }
}
fn internal(message: &str) -> TestBodyOutcome {
    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
        failure: RunError::Internal(message.into()),
    })
}

pub(super) fn emit_semantic(
    services: &ExecutionServices<'_>,
    scope: &scopes::ExecutionScope,
    event: webtest_observation::SemanticEvent,
) {
    emit_event(
        services.events,
        services.event_sink,
        ExecutionEvent::Semantic {
            execution_id: services.execution_id,
            scope: scope.event.clone(),
            event,
        },
    );
}
