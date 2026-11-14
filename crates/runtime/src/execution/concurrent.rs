use super::child::{ObservationProjection, execute_child};
use super::*;
use webtest_plan::PlanNode;

impl TreeExecution<'_, '_> {
    pub(super) async fn concurrent_node(
        &mut self,
        children: &[PlanNode],
        parent: &scopes::ExecutionScope,
        policy: scheduler::SiblingPolicy,
        binding: Option<&webtest_plan::RaceBinding>,
    ) -> TestBodyOutcome {
        let mut observations = ObservationProjection::new(self.services);
        let services = ExecutionServices {
            observations: &observations.pending,
            ..*self.services
        };
        self.branch.active_step = None;
        self.branch.active_operation = None;
        let mut child_scopes = Vec::with_capacity(children.len());
        let children = children
            .iter()
            .map(|node| {
                let scope = services.scopes.branch(parent, node);
                child_scopes.push(scope.event.clone());
                let mut state = self.branch.fork();
                let (signal, receiver) = scheduler::FailureSignal::channel();
                state.failure_signals.push(signal);
                (
                    scope.context.clone(),
                    receiver,
                    execute_child(&services, node, scope, state),
                )
            })
            .collect();
        let first = self.branch.completed_branches.len();
        let mut transfers = std::collections::BTreeMap::new();
        let check_depth = self.branch.check_depth;
        let decision = scheduler::schedule(
            &parent.context,
            policy,
            children,
            |completion| match &completion.result.outcome {
                TestOutcome::Passed => scheduler::Completion::Passed,
                TestOutcome::Verdict { .. } => scheduler::Completion::Verdict,
                TestOutcome::Failed(failure) if check_depth==0&&matches!(&failure.error,crate::StepError::Assertion(assertion) if assertion.matcher==webtest_plan::ValueMatcher::ExplicitFail)=>scheduler::Completion::Verdict,
                outcome => outcome.failure_class().map_or(
                    scheduler::Completion::Cancelled,
                    scheduler::Completion::Failed,
                ),
            },
            |ordinal, completion| {
                if let Some(value) = completion.provided {
                    transfers.insert(ordinal, value);
                }
                let result = completion.result;
                // Persist completed children in the parent's own state as they
                // finish. An enclosing interrupted wait cannot discard them.
                self.branch.completed_branches.push(result);
            },
            |ordinal, cause| {
                crate::events::emit_cancellation_request(
                    &child_scopes[ordinal],
                    services.events,
                    services.event_sink,
                    services.execution_id,
                    cause,
                )
            },
        )
        .await;
        let results = &mut self.branch.completed_branches[first..];
        results.sort_by(|a, b| {
            a.scope
                .execution_context
                .task_path
                .cmp(&b.scope.execution_context.task_path)
        });
        if let Some(winner) = decision.winner {
            if let Some(binding) = binding {
                let Some(transfer) = transfers.remove(&winner) else {
                    return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                        failure: RunError::Internal(
                            "race winner did not provide its required result".into(),
                        ),
                    });
                };
                if let Err(failure) = self.branch.bindings.bind_transfer(binding, transfer) {
                    return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                        failure,
                    });
                }
            }
            results[winner].race_winner = true;
            observations.recovered = true;
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed);
        }
        if let Some(ordinal) = decision.verdict
            && !results.iter().any(|child| {
                child
                    .outcome
                    .failure_class()
                    .is_some_and(|class| class != FailureClass::Test)
            })
        {
            let selected = &results[ordinal].outcome;
            if policy == scheduler::SiblingPolicy::FirstSuccess
                || !results.iter().any(|child| {
                    matches!(
                        child.outcome,
                        TestOutcome::Failed(_) | TestOutcome::TimedOut { .. }
                    )
                })
            {
                observations.recovered = policy == scheduler::SiblingPolicy::FirstSuccess;
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(Box::new(
                    selected.clone(),
                )));
            }
        }
        let mut summary = &TestOutcome::Passed;
        for child in results {
            if severity(&child.outcome) > severity(summary) {
                summary = &child.outcome;
            }
        }
        if matches!(summary, TestOutcome::Passed) {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
        } else {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(Box::new(
                summary.clone(),
            )))
        }
    }
}

fn severity(outcome: &TestOutcome) -> u8 {
    match outcome.failure_class() {
        Some(FailureClass::Internal) => 4,
        Some(FailureClass::Infrastructure) => 3,
        _ if matches!(outcome, TestOutcome::Verdict { .. }) => 2,
        Some(FailureClass::Test) => 2,
        None if matches!(outcome, TestOutcome::Passed) => 0,
        None => 1,
    }
}
