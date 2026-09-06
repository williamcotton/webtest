use crate::events::EventBuffer;
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
    contexts: Vec<crate::ScopeContext>,
    resource_owners: Vec<bool>,
    deadline: tokio::time::Instant,
}

impl ScopeTree {
    pub(super) fn new(
        ids: ExecutionIds,
        execution_id: ExecutionId,
        test_id: TestId,
        deadline: tokio::time::Instant,
    ) -> Self {
        Self {
            test_execution_id: TestExecutionId(ids.next()),
            ids,
            execution_id,
            test_id,
            active: Vec::new(),
            contexts: Vec::new(),
            resource_owners: Vec::new(),
            deadline,
        }
    }

    pub(super) fn current_context(&self) -> Option<crate::ScopeContext> {
        self.contexts.last().cloned()
    }

    pub(super) fn enter(
        &mut self,
        node: &PlanNode,
        events: &EventBuffer,
        sink: Option<&dyn RunEventSink>,
    ) -> (ScopeEvent, crate::ScopeContext) {
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
        let scope_id = event.execution_context.scope_id;
        let context = self
            .contexts
            .last()
            .map(|parent| {
                parent.child(
                    scope_id,
                    match &node.kind {
                        PlanNodeKind::Timeout { duration, .. } => {
                            Some(tokio::time::Instant::now() + *duration)
                        }
                        _ => None,
                    },
                )
            })
            .unwrap_or_else(|| crate::ScopeContext {
                scope_id,
                cancellation: crate::CancellationToken::default(),
                deadline: Some(self.deadline),
                deadline_scope_id: Some(scope_id),
            });
        self.contexts.push(context.clone());
        self.resource_owners
            .push(node.path.is_empty() || matches!(node.kind, PlanNodeKind::Timeout { .. }));
        self.active.push(event.clone());
        emit_event(
            events,
            sink,
            ExecutionEvent::Scope {
                execution_id: self.execution_id,
                event: event.clone(),
            },
        );
        (event, context)
    }

    pub(super) fn current_event(&self) -> Option<ScopeEvent> {
        self.active.last().cloned()
    }

    pub(super) fn resource_owner(&self) -> Option<(ScopeEvent, crate::ScopeContext)> {
        self.active
            .iter()
            .zip(&self.contexts)
            .zip(&self.resource_owners)
            .rev()
            .find(|(_, owns)| **owns)
            .map(|((event, context), _)| (event.clone(), context.clone()))
    }

    pub(super) fn unwind_to(
        &mut self,
        owner: ExecutionScopeId,
        events: &EventBuffer,
        sink: Option<&dyn RunEventSink>,
    ) {
        while self
            .active
            .last()
            .is_some_and(|event| event.execution_context.scope_id != owner)
        {
            self.leave(ScopeOutcome::Cancelled, events, sink);
        }
    }

    pub(super) fn leave(
        &mut self,
        outcome: ScopeOutcome,
        events: &EventBuffer,
        sink: Option<&dyn RunEventSink>,
    ) {
        if let Some(mut event) = self.active.pop() {
            self.resource_owners.pop();
            let context = self.contexts.pop();
            event.cancellation = context
                .as_ref()
                .and_then(|context| context.cancellation.cause());
            event.outcome = Some(
                if event
                    .cancellation
                    .is_some_and(|cause| cause.causing_scope_id != event.execution_context.scope_id)
                {
                    ScopeOutcome::Cancelled
                } else {
                    outcome
                },
            );
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
        cause: webtest_host::Cancellation,
        events: &EventBuffer,
        sink: Option<&dyn RunEventSink>,
    ) {
        let Some(_root) = self.active.first() else {
            return;
        };
        if let Some(context) = self.contexts.first() {
            context.cancellation.cancel(cause);
        }
        while self.active.len() > 1 {
            if let Some(active) = self.active.last_mut() {
                active.cancellation = Some(cause);
            }
            self.leave(ScopeOutcome::Cancelled, events, sink);
        }
    }
}
