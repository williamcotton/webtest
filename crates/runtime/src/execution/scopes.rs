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
    AttemptEvent, ExecutionContext, ExecutionEvent, ExecutionId, ScopeEvent, ScopeOutcome,
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
    attempt: Option<AttemptEvent>,
    task_id: ExecutionScopeId,
    parent_task_id: Option<ExecutionScopeId>,
    debug_frames: Vec<crate::DebugScope>,
    pub event: ScopeEvent,
    pub context: ScopeContext,
    pub resource_owner: ResourceOwner,
    pub fixture_boundary: Option<ScopeContext>,
    ancestors: Vec<ExecutionScopeId>,
    cleanup_timeout: Option<std::time::Duration>,
}

impl ExecutionScope {
    pub fn debug_context(&self, execution_id: ExecutionId) -> crate::DebugContext {
        crate::DebugContext {
            execution_id,
            task_id: self.task_id,
            parent_task_id: self.parent_task_id,
            frames: self.debug_frames.clone(),
            resources: Vec::new(),
        }
    }
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
    test_execution_id: Option<TestExecutionId>,
    test_id: Option<TestId>,
    test_declaration_id: Option<webtest_model::TestDeclarationId>,
    variant_id: Option<webtest_model::TestVariantId>,
    fixture: Option<webtest_observation::FixtureExecutionContext>,
}

impl ScopeFactory {
    pub fn new(ids: ExecutionIds, test: &webtest_plan::PlannedTest) -> Self {
        Self {
            test_execution_id: Some(TestExecutionId(ids.next())),
            ids,
            test_id: Some(test.id),
            test_declaration_id: Some(test.test_declaration_id),
            variant_id: Some(test.variant_id),
            fixture: None,
        }
    }

    pub fn for_fixture(
        ids: ExecutionIds,
        fixture: webtest_observation::FixtureExecutionContext,
    ) -> Self {
        Self {
            ids,
            test_execution_id: None,
            test_id: None,
            test_declaration_id: None,
            variant_id: None,
            fixture: Some(fixture),
        }
    }
    pub fn fixture_root(
        &self,
        node: &PlanNode,
        owner: &ScopeContext,
        cancellation: crate::CancellationToken,
        name: &str,
    ) -> ExecutionScope {
        let mut root = self.create(node, None, owner.deadline, true);
        root.context.cancellation = cancellation;
        root.context.deadline_scope_id = owner.deadline_scope_id;
        root.resource_owner.context = root.context.clone();
        if let Some(frame) = root.debug_frames.last_mut() {
            frame.display_name = Some(format!("fixture {name}"));
            frame.kind = crate::DebugScopeKind::FixtureLifetime;
        }
        root
    }
    pub fn fixture_phase(
        &self,
        parent: &ExecutionScope,
        node: &PlanNode,
        phase: webtest_observation::FixturePhase,
        deadline: tokio::time::Instant,
    ) -> ExecutionScope {
        let cleanup = phase == webtest_observation::FixturePhase::Teardown;
        let mut child = self.create(node, Some(parent), Some(deadline), cleanup);
        if cleanup {
            child.context.cancellation = crate::CancellationToken::default();
            if let Some(at) = parent.context.cancellation.cleanup_started_at() {
                child.context.cancellation.start_cleanup_at(at);
            }
            child.context.deadline = Some(deadline);
            child.context.deadline_scope_id = Some(child.id());
            child.resource_owner.context = child.context.clone();
        }
        if let Some(fixture) = &mut child.event.execution_context.fixture {
            fixture.phase = phase;
        }
        if cleanup {
            child.resource_owner.event = child.event.clone();
        }
        if let Some(frame) = child.debug_frames.last_mut() {
            frame.scope = child.event.clone();
            frame.kind = match phase {
                webtest_observation::FixturePhase::Lifetime => {
                    crate::DebugScopeKind::FixtureLifetime
                }
                webtest_observation::FixturePhase::Setup => crate::DebugScopeKind::FixtureSetup,
                webtest_observation::FixturePhase::Teardown => {
                    crate::DebugScopeKind::FixtureTeardown
                }
            };
            frame.display_name = Some(
                match phase {
                    webtest_observation::FixturePhase::Lifetime => "fixture lifetime",
                    webtest_observation::FixturePhase::Setup => "fixture setup",
                    webtest_observation::FixturePhase::Teardown => "fixture teardown",
                }
                .into(),
            );
        }
        child
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
                PlanNodeKind::Timeout { .. }
                    | PlanNodeKind::ResourceScope { .. }
                    | PlanNodeKind::Semantic {
                        operation: webtest_plan::semantics::SemanticOperation::Guards { .. },
                        ..
                    }
            ),
        )
    }

    pub fn branch(&self, parent: &ExecutionScope, node: &PlanNode) -> ExecutionScope {
        let deadline = match &node.kind {
            PlanNodeKind::Timeout { duration, .. } => Some(tokio::time::Instant::now() + *duration),
            _ => None,
        };
        let mut scope = self.create(node, Some(parent), deadline, true);
        scope.task_id = scope.id();
        scope.parent_task_id = Some(parent.task_id);
        scope
    }

    pub fn attempt(
        &self,
        parent: &ExecutionScope,
        node: &PlanNode,
        ordinal: u32,
        max_attempts: u32,
    ) -> ExecutionScope {
        let mut scope = self.branch(parent, node);
        scope.attempt = Some(AttemptEvent {
            ordinal,
            max_attempts,
        });
        scope.event.execution_context.attempt_id = Some(AttemptId(self.ids.next()));
        scope.fixture_boundary = Some(scope.context.clone());
        scope.resource_owner.event = scope.event.clone();
        if let Some(frame) = scope.debug_frames.last_mut() {
            frame.scope = scope.event.clone();
            frame.attempt = scope.attempt;
        }
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
                actor: match &node.kind {
                    PlanNodeKind::Semantic {
                        operation:
                            webtest_plan::semantics::SemanticOperation::ActorAccess { actor, .. },
                        ..
                    } => Some(*actor),
                    _ => parent.and_then(|parent| parent.event.execution_context.actor),
                },
                fixture: parent
                    .and_then(|parent| parent.event.execution_context.fixture)
                    .or(self.fixture),
                test_declaration_id: self.test_declaration_id,
                variant_id: self.variant_id,
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
        let mut debug_frames = parent.map_or_else(Vec::new, |parent| parent.debug_frames.clone());
        debug_frames.push(crate::DebugScope {
            display_name: match &node.kind {
                PlanNodeKind::Semantic {
                    operation:
                        webtest_plan::semantics::SemanticOperation::ActorAccess { name, .. }
                        | webtest_plan::semantics::SemanticOperation::ActorScope { name, .. },
                    ..
                } => Some(format!("actor {name}")),
                PlanNodeKind::Semantic { operation, .. } => Some(
                    match operation {
                        webtest_plan::semantics::SemanticOperation::Check { .. } => "check",
                        webtest_plan::semantics::SemanticOperation::Assume { .. } => "assume",
                        webtest_plan::semantics::SemanticOperation::Select { .. } => "select",
                        webtest_plan::semantics::SemanticOperation::Guards { .. } => "with guards",
                        webtest_plan::semantics::SemanticOperation::ActorAccess { .. }
                        | webtest_plan::semantics::SemanticOperation::ActorScope { .. } => "actor",
                    }
                    .into(),
                ),
                PlanNodeKind::Helper { call, .. } => Some(format!("helper {}", call.name)),
                _ => None,
            },
            scope: event.clone(),
            kind: match node.kind {
                PlanNodeKind::Helper { .. } => crate::DebugScopeKind::Helper,
                PlanNodeKind::Semantic { .. }
                | PlanNodeKind::Conditional { .. }
                | PlanNodeKind::Sequence { .. } => crate::DebugScopeKind::Sequence,
                PlanNodeKind::Parallel { .. } => crate::DebugScopeKind::Parallel,
                PlanNodeKind::Race { .. } => crate::DebugScopeKind::Race,
                PlanNodeKind::Retry { .. } => crate::DebugScopeKind::Retry,
                PlanNodeKind::Timeout { .. } => crate::DebugScopeKind::Timeout,
                PlanNodeKind::ResourceScope { .. } => crate::DebugScopeKind::BrowserContext,
                PlanNodeKind::Operation { .. } => crate::DebugScopeKind::Operation,
            },
            attempt: None,
        });
        ExecutionScope {
            attempt: None,
            task_id: parent.map_or(scope_id, |parent| parent.task_id),
            parent_task_id: parent.and_then(|parent| parent.parent_task_id),
            debug_frames,
            cleanup_timeout,
            event,
            fixture_boundary: parent
                .and_then(|parent| parent.fixture_boundary.clone())
                .or_else(|| (self.test_id.is_some() && parent.is_none()).then(|| context.clone())),
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
        if let Some(attempt) = scope.attempt {
            emit_event(
                events,
                sink,
                ExecutionEvent::Attempt {
                    execution_id,
                    scope: scope.event.clone(),
                    event: attempt,
                },
            );
        }
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
                event: event.clone(),
            },
        );
        if let Some(attempt) = scope.attempt {
            emit_event(
                events,
                sink,
                ExecutionEvent::Attempt {
                    execution_id,
                    scope: event,
                    event: attempt,
                },
            );
        }
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
