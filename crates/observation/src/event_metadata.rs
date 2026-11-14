//! Source and occurrence metadata supplied by execution, never inferred from
//! whichever sibling happened to publish most recently.
use crate::{ExecutionEvent, ScopeEvent};
use serde::{Deserialize, Serialize};
use webtest_model::{
    AttemptId, ExecutionScopeId, OperationExecutionId, PlanNodeId, TestExecutionId, TestId,
};
use webtest_text::{SourceRevision, SyntaxOrigin};

/// Fields are absent for facts without the corresponding runtime occurrence
/// (for example, a skipped test has a TestId but no TestExecutionId).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<webtest_model::ActorId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture: Option<crate::FixtureExecutionContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_declaration_id: Option<webtest_model::TestDeclarationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<webtest_model::TestVariantId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_id: Option<TestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_execution_id: Option<TestExecutionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_path: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<ExecutionScopeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_scope_id: Option<ExecutionScopeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_node_id: Option<PlanNodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_execution_id: Option<OperationExecutionId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMetadata {
    pub execution_context: EventContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<SourceRevision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<SyntaxOrigin>,
}

impl From<&ScopeEvent> for EventMetadata {
    fn from(scope: &ScopeEvent) -> Self {
        let context = &scope.execution_context;
        Self {
            execution_context: EventContext {
                actor: context.actor,
                fixture: context.fixture,
                test_declaration_id: context.test_declaration_id,
                variant_id: context.variant_id,
                test_id: context.test_id,
                test_execution_id: context.test_execution_id,
                task_path: Some(context.task_path.clone()),
                scope_id: Some(context.scope_id),
                parent_scope_id: context.parent_scope_id,
                plan_node_id: Some(context.plan_node_id),
                attempt_id: context.attempt_id,
                operation_execution_id: context.operation_execution_id,
            },
            source_revision: Some(scope.source_revision),
            origin: Some(scope.origin),
        }
    }
}

impl ExecutionEvent {
    pub fn scope(&self) -> Option<&ScopeEvent> {
        match self {
            Self::Scope { event, .. } => Some(event),
            Self::CancellationRequested { scope, .. }
            | Self::ScopeAborted { scope, .. }
            | Self::Semantic { scope, .. }
            | Self::Attempt { scope, .. }
            | Self::Wait { scope, .. } => Some(scope),
            Self::Resource { scope, .. } => scope.as_ref(),
            _ => None,
        }
    }

    pub const fn test_id(&self) -> Option<TestId> {
        match self {
            Self::Scope { event, .. } => event.execution_context.test_id,
            Self::CancellationRequested { scope, .. }
            | Self::ScopeAborted { scope, .. }
            | Self::Semantic { scope, .. }
            | Self::Attempt { scope, .. }
            | Self::Wait { scope, .. } => scope.execution_context.test_id,
            Self::Resource { scope, .. } => match scope {
                Some(scope) => scope.execution_context.test_id,
                None => None,
            },
            Self::FixtureAcquired { test_id, .. }
            | Self::OutputCaptured { test_id, .. }
            | Self::AttachmentCreated { test_id, .. }
            | Self::StepStarted { test_id, .. }
            | Self::StepPassed { test_id, .. }
            | Self::ProviderCallStarted { test_id, .. }
            | Self::ProviderCallFinished { test_id, .. }
            | Self::ProviderCallFailed { test_id, .. }
            | Self::StepFailed { test_id, .. } => *test_id,
            Self::TestStarted { test_id, .. }
            | Self::TestTimedOut { test_id, .. }
            | Self::TestFinished { test_id, .. }
            | Self::TestSkipped { test_id, .. } => Some(*test_id),
            Self::CleanupFailed { test_id, .. } => *test_id,
            Self::RunStarted { .. } | Self::RunFinished { .. } => None,
        }
    }

    pub const fn step_id(&self) -> Option<webtest_model::StepId> {
        match self {
            Self::FixtureAcquired { step_id, .. }
            | Self::OutputCaptured { step_id, .. }
            | Self::AttachmentCreated { step_id, .. }
            | Self::StepStarted { step_id, .. }
            | Self::StepPassed { step_id, .. }
            | Self::ProviderCallStarted { step_id, .. }
            | Self::ProviderCallFinished { step_id, .. }
            | Self::ProviderCallFailed { step_id, .. }
            | Self::StepFailed { step_id, .. } => Some(*step_id),
            Self::TestTimedOut { active_step, .. } => *active_step,
            _ => None,
        }
    }

    pub fn metadata(&self) -> EventMetadata {
        self.scope()
            .map(EventMetadata::from)
            .unwrap_or_else(|| EventMetadata {
                execution_context: EventContext {
                    actor: None,
                    test_id: self.test_id(),
                    ..Default::default()
                },
                ..Default::default()
            })
    }
}

impl EventMetadata {
    pub(crate) fn agrees_with(&self, event: &ExecutionEvent) -> bool {
        let context = &self.execution_context;
        let scoped = context.scope_id.is_some();
        context.test_id == event.test_id()
            && context.test_declaration_id.is_some() == context.variant_id.is_some()
            && (context.test_id.is_some() || context.test_declaration_id.is_none())
            && (context.test_execution_id.is_some() || context.fixture.is_some()) == scoped
            && (context.fixture.is_none()
                || (context.test_execution_id.is_none()
                    && context.test_id.is_none()
                    && context.test_declaration_id.is_none()))
            && context.task_path.is_some() == scoped
            && context.plan_node_id.is_some() == scoped
            && (!scoped
                || ((context.test_id.is_some() || context.fixture.is_some())
                    && self.origin.is_some()
                    && self.source_revision.is_some()))
            && (scoped
                || (context.parent_scope_id.is_none()
                    && context.actor.is_none()
                    && context.operation_execution_id.is_none()
                    && context.attempt_id.is_none()))
            && (self.origin.is_none() || self.source_revision.is_some())
            && event.scope().is_none_or(|scope| self == &Self::from(scope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EventJournal, EventTime, ExecutionContext, ExecutionId, ReplayError, ReplayJournal,
        ReplayOutcome,
    };
    use std::{num::NonZeroUsize, time::Duration};
    use webtest_text::{FileId, TextRange, TextSize};

    fn scope() -> ScopeEvent {
        ScopeEvent {
            execution_context: ExecutionContext {
                actor: None,
                fixture: None,
                test_declaration_id: None,
                variant_id: None,
                test_id: Some(TestId(1)),
                test_execution_id: Some(TestExecutionId(2)),
                task_path: vec![0, 2],
                scope_id: ExecutionScopeId(3),
                parent_scope_id: Some(ExecutionScopeId(2)),
                plan_node_id: PlanNodeId([4; 32]),
                attempt_id: Some(AttemptId(5)),
                operation_execution_id: Some(OperationExecutionId(6)),
            },
            source_revision: SourceRevision::of("é metadata"),
            origin: SyntaxOrigin::new(
                FileId::new(1),
                TextRange::new(TextSize::new(3), TextSize::new(11)),
            ),
            outcome: None,
            cancellation: None,
        }
    }

    #[test]
    fn metadata_preserves_typed_scope_and_source_fields_without_inventing_absent_occurrences() {
        let metadata = EventMetadata::from(&scope());
        let json = serde_json::to_value(&metadata).unwrap();
        assert_eq!(
            serde_json::from_value::<EventMetadata>(json.clone()).unwrap(),
            metadata
        );
        assert_eq!(
            json["execution_context"]["task_path"],
            serde_json::json!([0, 2])
        );
        assert_eq!(json["execution_context"]["operation_execution_id"], 6);
        let run = ExecutionEvent::RunStarted {
            execution_id: ExecutionId::from_u128(1),
        }
        .metadata();
        assert_eq!(
            serde_json::to_value(run).unwrap(),
            serde_json::json!({"execution_context": {}})
        );
        let skipped = ExecutionEvent::TestSkipped {
            execution_id: ExecutionId::from_u128(1),
            test_id: TestId(7),
            name: "skipped".into(),
            reason: crate::SkipReason::RunAborted,
            failure_class: None,
        }
        .metadata();
        assert_eq!(
            serde_json::to_value(skipped).unwrap(),
            serde_json::json!({"execution_context": {"test_id": 7}})
        );
    }

    #[test]
    fn fixture_scope_replay_keeps_real_lifetime_ownership_without_a_test_occurrence() {
        let mut scope = scope();
        let context = &mut scope.execution_context;
        context.test_id = None;
        context.test_execution_id = None;
        context.attempt_id = None;
        context.fixture = Some(crate::FixtureExecutionContext {
            fixture: webtest_model::FixtureId([9; 32]),
            instance: webtest_model::FixtureInstanceId(4),
            owner: crate::FixtureOwnerKey::Suite {
                invocation: ExecutionId::from_u128(1),
            },
            phase: crate::FixturePhase::Setup,
        });
        let event = ExecutionEvent::Scope {
            execution_id: ExecutionId::from_u128(1),
            event: scope,
        };
        let metadata = event.metadata();
        assert!(metadata.agrees_with(&event));
        let json = serde_json::to_value(&metadata).unwrap();
        assert!(json["execution_context"].get("test_id").is_none());
        assert!(json["execution_context"].get("test_execution_id").is_none());
        assert_eq!(
            json["execution_context"]["fixture"]["owner"]["kind"],
            "suite"
        );
        let mut journal = EventJournal::default();
        let record = journal
            .record(
                event,
                EventTime {
                    since_unix_epoch: Duration::ZERO,
                    elapsed: Duration::ZERO,
                },
            )
            .clone();
        let mut replay = ReplayJournal::new(NonZeroUsize::new(4).unwrap());
        assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Inserted));
        let mut wrong = record;
        wrong.metadata.execution_context.test_id = Some(TestId(123));
        assert!(matches!(
            replay.insert(wrong),
            Err(ReplayError::MetadataMismatch { .. })
        ));
    }

    #[test]
    fn replay_rejects_context_disagreement_and_treats_source_changes_as_identity_conflicts() {
        let mut source = EventJournal::default();
        let record = source
            .record(
                ExecutionEvent::Scope {
                    execution_id: ExecutionId::from_u128(1),
                    event: scope(),
                },
                EventTime {
                    since_unix_epoch: Duration::ZERO,
                    elapsed: Duration::ZERO,
                },
            )
            .clone();
        let mut replay = ReplayJournal::new(NonZeroUsize::new(4).unwrap());
        assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Inserted));
        let mut wrong = record.clone();
        wrong.metadata.execution_context.parent_scope_id = None;
        assert!(matches!(
            replay.insert(wrong),
            Err(ReplayError::MetadataMismatch { .. })
        ));
        let mut operation = record.clone();
        operation.event = ExecutionEvent::StepPassed {
            execution_id: ExecutionId::from_u128(1),
            test_id: Some(TestId(1)),
            step_id: webtest_model::StepId(3),
        };
        operation.identity.event_sequence.0 += 1;
        assert_eq!(
            replay.insert(operation.clone()),
            Ok(ReplayOutcome::Inserted)
        );
        let mut changed = operation.clone();
        changed.metadata.source_revision = Some(SourceRevision::of("different"));
        assert!(matches!(
            replay.insert(changed),
            Err(ReplayError::ConflictingIdentity { .. })
        ));
        let mut changed = operation.clone();
        changed.metadata.execution_context.test_execution_id = None;
        assert!(matches!(
            replay.insert(changed),
            Err(ReplayError::MetadataMismatch { .. })
        ));
        assert_eq!(replay.insert(operation), Ok(ReplayOutcome::Duplicate));
        assert_eq!(replay.records_for(ExecutionId::from_u128(1)).count(), 2);
    }
}
