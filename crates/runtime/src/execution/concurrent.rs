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
        let children = children
            .iter()
            .map(|node| {
                let scope = services.scopes.branch(parent, node);
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
        let winner = scheduler::schedule(
            &parent.context,
            policy,
            children,
            |completion| match &completion.result.outcome {
                TestOutcome::Passed => scheduler::Completion::Passed,
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
        )
        .await;
        let results = &mut self.branch.completed_branches[first..];
        results.sort_by(|a, b| {
            a.scope
                .execution_context
                .task_path
                .cmp(&b.scope.execution_context.task_path)
        });
        if let Some(winner) = winner {
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
        Some(FailureClass::Test) => 2,
        None if matches!(outcome, TestOutcome::Passed) => 0,
        None => 1,
    }
}
