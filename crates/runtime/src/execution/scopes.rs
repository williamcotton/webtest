use crate::events::EventBuffer;
use crate::{RunEventSink, ScopeContext, events::emit_event};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use webtest_model::{AttemptId, ExecutionScopeId, OperationExecutionId, TestExecutionId, TestId};
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

/// Immutable ownership metadata. Clones share cancellation, never execution state.
#[derive(Clone)]
pub(super) struct ResourceOwner {
    pub event: ScopeEvent,
    pub context: ScopeContext,
}

/// Explicit runtime occurrence and ancestry. Parentage does not depend on which
/// sibling happens to be running, suspended, or finishing.
#[derive(Clone)]
pub(super) struct ExecutionScope {
    pub event: ScopeEvent,
    pub context: ScopeContext,
    pub resource_owner: ResourceOwner,
    ancestors: Vec<ExecutionScopeId>,
    cleanup_timeout: Option<std::time::Duration>,
}

impl ExecutionScope {
    pub fn cleanup_timeout(&self, default: std::time::Duration) -> std::time::Duration {
        self.cleanup_timeout
            .map_or(default, |timeout| timeout.min(default))
    }
    pub fn id(&self) -> ExecutionScopeId {
        self.context.scope_id
    }
    fn is_descendant_of(&self, parent: ExecutionScopeId) -> bool {
        self.ancestors.contains(&parent)
    }
}

/// Shared identity service; creating a child always requires its explicit parent.
/// There is no shared "current scope" or mutable execution stack.
pub(super) struct ScopeFactory {
    ids: ExecutionIds,
    test_execution_id: TestExecutionId,
    test_id: TestId,
}

impl ScopeFactory {
    pub fn new(ids: ExecutionIds, test_id: TestId) -> Self {
        Self {
            test_execution_id: TestExecutionId(ids.next()),
            ids,
            test_id,
        }
    }

    pub fn root(&self, node: &PlanNode, deadline: tokio::time::Instant) -> ExecutionScope {
        self.create(node, None, Some(deadline), true)
    }

    pub fn child(&self, parent: &ExecutionScope, node: &PlanNode) -> ExecutionScope {
        let deadline = match &node.kind {
            PlanNodeKind::Timeout { duration, .. } => Some(tokio::time::Instant::now() + *duration),
            _ => None,
        };
        self.create(
            node,
            Some(parent),
            deadline,
            matches!(
                node.kind,
                PlanNodeKind::Timeout { .. } | PlanNodeKind::ResourceScope { .. }
            ),
        )
    }

    pub fn branch(&self, parent: &ExecutionScope, node: &PlanNode) -> ExecutionScope {
        let deadline = match &node.kind {
            PlanNodeKind::Timeout { duration, .. } => Some(tokio::time::Instant::now() + *duration),
            _ => None,
        };
        self.create(node, Some(parent), deadline, true)
    }

    pub fn attempt(&self, parent: &ExecutionScope, node: &PlanNode) -> ExecutionScope {
        let mut scope = self.branch(parent, node);
        scope.event.execution_context.attempt_id = Some(AttemptId(self.ids.next()));
        scope.resource_owner.event = scope.event.clone();
        scope
    }

    fn create(
        &self,
        node: &PlanNode,
        parent: Option<&ExecutionScope>,
        deadline: Option<tokio::time::Instant>,
        owns_resources: bool,
    ) -> ExecutionScope {
        let scope_id = ExecutionScopeId(self.ids.next());
        let event = ScopeEvent {
            execution_context: ExecutionContext {
                test_execution_id: self.test_execution_id,
                test_id: self.test_id,
                task_path: node.path.clone(),
                scope_id,
                parent_scope_id: parent.map(ExecutionScope::id),
                plan_node_id: node.id,
                attempt_id: parent.and_then(|parent| parent.event.execution_context.attempt_id),
                operation_execution_id: matches!(node.kind, PlanNodeKind::Operation { .. })
                    .then(|| OperationExecutionId(self.ids.next())),
            },
            source_revision: node.source_revision,
            origin: node.origin,
            outcome: None,
            cancellation: None,
        };
        let context = parent
            .map(|parent| parent.context.child(scope_id, deadline))
            .unwrap_or_else(|| ScopeContext {
                scope_id,
                cancellation: crate::CancellationToken::default(),
                deadline,
                deadline_scope_id: deadline.map(|_| scope_id),
            });
        let resource_owner = match parent {
            Some(parent) if !owns_resources => parent.resource_owner.clone(),
            _ => ResourceOwner {
                event: event.clone(),
                context: context.clone(),
            },
        };
        let mut ancestors = parent.map_or_else(Vec::new, |parent| parent.ancestors.clone());
        if let Some(parent) = parent {
            ancestors.push(parent.id());
        }
        let local_cleanup = match &node.kind {
            PlanNodeKind::Timeout {
                cleanup_timeout, ..
            } => *cleanup_timeout,
            _ => None,
        };
        let cleanup_timeout = match (
            parent.and_then(|parent| parent.cleanup_timeout),
            local_cleanup,
        ) {
            (Some(inherited), Some(local)) => Some(inherited.min(local)),
            (inherited, local) => inherited.or(local),
        };
        ExecutionScope {
            cleanup_timeout,
            event,
            context,
            resource_owner,
            ancestors,
        }
    }
}

/// Branch-local bookkeeping for terminal facts if bounded interruption has to
/// abandon a suspended subtree. It does not determine parentage or share bindings.
#[derive(Default)]
pub(super) struct BranchScopes {
    active: BTreeMap<ExecutionScopeId, ExecutionScope>,
}

impl BranchScopes {
    pub fn start(
        &mut self,
        scope: &ExecutionScope,
        execution_id: ExecutionId,
        events: &EventBuffer,
        sink: Option<&dyn RunEventSink>,
    ) {
        self.active.insert(scope.id(), scope.clone());
        emit_event(
            events,
            sink,
            ExecutionEvent::Scope {
                execution_id,
                event: scope.event.clone(),
            },
        );
    }

    pub fn finish(
        &mut self,
        scope: &ExecutionScope,
        outcome: ScopeOutcome,
        execution_id: ExecutionId,
        events: &EventBuffer,
        sink: Option<&dyn RunEventSink>,
    ) {
        if self.active.remove(&scope.id()).is_none() {
            return;
        }
        let mut event = scope.event.clone();
        event.cancellation = scope.context.cancellation.cause();
        event.outcome = Some(
            if event
                .cancellation
                .is_some_and(|cause| cause.causing_scope_id != scope.id())
                && !matches!(outcome, ScopeOutcome::Failed | ScopeOutcome::Aborted)
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
                execution_id,
                event,
            },
        );
    }

    pub fn subtree_ids(&self, scope: &ExecutionScope) -> BTreeSet<ExecutionScopeId> {
        self.active
            .values()
            .filter(|active| active.id() == scope.id() || active.is_descendant_of(scope.id()))
            .map(ExecutionScope::id)
            .collect()
    }

    /// Called after subtree resources are finalized. Stable postorder preserves
    /// child-before-parent terminal ordering even when siblings finish out of order.
    pub fn finish_descendants(
        &mut self,
        scope: &ExecutionScope,
        execution_id: ExecutionId,
        events: &EventBuffer,
        sink: Option<&dyn RunEventSink>,
    ) {
        let mut descendants: Vec<_> = self
            .active
            .values()
            .filter(|active| active.is_descendant_of(scope.id()))
            .cloned()
            .collect();
        descendants.sort_by(|a, b| {
            b.ancestors.len().cmp(&a.ancestors.len()).then_with(|| {
                a.event
                    .execution_context
                    .task_path
                    .cmp(&b.event.execution_context.task_path)
            })
        });
        for descendant in descendants {
            self.finish(
                &descendant,
                ScopeOutcome::Cancelled,
                execution_id,
                events,
                sink,
            );
        }
    }
}
