use crate::{RunEventSink, events::emit_event};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use webtest_model::{ExecutionScopeId, OperationExecutionId, TestExecutionId, TestId};
use webtest_observation::{
    ExecutionContext, ExecutionEvent, ExecutionId, ScopeEvent, ScopeOutcome,
};
use webtest_plan::{PlanNode, PlanNodeKind};

/// One allocator per Runner invocation; counters are never static plan identities.
#[derive(Clone, Default)]
pub(crate) struct ExecutionIds(Arc<AtomicU64>);
impl ExecutionIds {
    pub(crate) fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

pub(super) struct ScopeTree {
    ids: ExecutionIds,
    execution_id: ExecutionId,
    test_execution_id: TestExecutionId,
    test_id: TestId,
    active: Vec<ScopeEvent>,
}

impl ScopeTree {
    pub(super) fn new(ids: ExecutionIds, execution_id: ExecutionId, test_id: TestId) -> Self {
        Self {
            test_execution_id: TestExecutionId(ids.next()),
            ids,
            execution_id,
            test_id,
            active: Vec::new(),
        }
    }

    pub(super) fn enter(
        &mut self,
        node: &PlanNode,
        events: &mut Vec<ExecutionEvent>,
        sink: Option<&dyn RunEventSink>,
    ) {
        let event = ScopeEvent {
            execution_context: ExecutionContext {
                test_execution_id: self.test_execution_id,
                test_id: self.test_id,
                task_path: node.path.clone(),
                scope_id: ExecutionScopeId(self.ids.next()),
                parent_scope_id: self
                    .active
                    .last()
                    .map(|event| event.execution_context.scope_id),
                plan_node_id: node.id,
                attempt_id: None,
                operation_execution_id: matches!(node.kind, PlanNodeKind::Operation { .. })
                    .then(|| OperationExecutionId(self.ids.next())),
            },
            source_revision: node.source_revision,
            origin: node.origin,
            outcome: None,
            cancellation: None,
        };
        self.active.push(event.clone());
        emit_event(
            events,
            sink,
            ExecutionEvent::Scope {
                execution_id: self.execution_id,
                event,
            },
        );
    }

    pub(super) fn leave(
        &mut self,
        outcome: ScopeOutcome,
        events: &mut Vec<ExecutionEvent>,
        sink: Option<&dyn RunEventSink>,
    ) {
        if let Some(mut event) = self.active.pop() {
            event.outcome = Some(outcome);
            emit_event(
                events,
                sink,
                ExecutionEvent::Scope {
                    execution_id: self.execution_id,
                    event,
                },
            );
        }
    }

    pub(super) fn interrupt(
        &mut self,
        events: &mut Vec<ExecutionEvent>,
        sink: Option<&dyn RunEventSink>,
    ) {
        let Some(root) = self.active.first() else {
            return;
        };
        let causing_scope_id = root.execution_context.scope_id;
        while self.active.len() > 1 {
            if let Some(active) = self.active.last_mut() {
                active.cancellation =
                    Some(webtest_observation::ScopeCancellation::Timeout { causing_scope_id });
            }
            self.leave(ScopeOutcome::Cancelled, events, sink);
        }
    }
}
