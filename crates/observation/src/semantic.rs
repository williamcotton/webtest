//! Source-mapped semantic decisions. Browser payloads in public facts must be
//! bounded and redacted by runtime before publication.
use serde::{Deserialize, Serialize};
use webtest_model::{ActorId, VerdictKind};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticEvent {
    BrowserEvent {
        actor: ActorId,
        generation: webtest_model::ExecutionScopeId,
        sequence: u64,
        ingress: u64,
        source: webtest_model::EventSource,
        payload: serde_json::Value,
        captured_bytes: usize,
        truncated: bool,
    },
    EventLoss {
        actor: ActorId,
        through: u64,
    },
    Checkpoint {
        actor: ActorId,
        generation: webtest_model::ExecutionScopeId,
        sequence: u64,
        binding: webtest_model::BindingId,
    },
    SelectionStarted {
        actor: ActorId,
        since: u64,
        duration_ms: u64,
        alternatives: Vec<webtest_model::AlternativeId>,
    },
    SelectionObserved {
        actor: ActorId,
        observed: u64,
        unmatched: u64,
        unmatched_sequences: Vec<u64>,
        truncated: bool,
    },
    SelectionMatched {
        actor: ActorId,
        sequence: u64,
        ingress: u64,
        alternative: webtest_model::AlternativeId,
        source: webtest_model::EventSource,
    },
    GuardActivated {
        guard: webtest_model::GuardId,
        actor: ActorId,
        alternative: webtest_model::AlternativeId,
        since: u64,
    },
    GuardTriggered {
        guard: webtest_model::GuardId,
        actor: ActorId,
        alternative: webtest_model::AlternativeId,
        sequence: u64,
        ingress: u64,
        primary: bool,
    },
    GuardSecondaryTruncated {
        omitted: usize,
    },
    GuardDeactivated {
        guard: webtest_model::GuardId,
        actor: ActorId,
        alternative: webtest_model::AlternativeId,
    },
    Verdict {
        verdict: VerdictKind,
        message: String,
    },
    CheckStarted,
    CheckFinished {
        failures: usize,
        truncated: bool,
    },
    ActorOpened {
        actor: ActorId,
        name: String,
    },
    ActorClosed {
        actor: ActorId,
    },
}
impl SemanticEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BrowserEvent { .. } => "browser_event",
            Self::EventLoss { .. } => "event_loss",
            Self::Checkpoint { .. } => "checkpoint",
            Self::SelectionStarted { .. } => "selection_started",
            Self::SelectionObserved { .. } => "selection_observed",
            Self::SelectionMatched { .. } => "selection_matched",
            Self::GuardActivated { .. } => "guard_activated",
            Self::GuardTriggered { .. } => "guard_triggered",
            Self::GuardSecondaryTruncated { .. } => "guard_secondary_truncated",
            Self::GuardDeactivated { .. } => "guard_deactivated",
            Self::Verdict { .. } => "verdict",
            Self::CheckStarted => "check_started",
            Self::CheckFinished { .. } => "check_finished",
            Self::ActorOpened { .. } => "actor_opened",
            Self::ActorClosed { .. } => "actor_closed",
        }
    }
}

impl SemanticEvent {
    pub fn actor(&self) -> Option<ActorId> {
        match self {
            Self::BrowserEvent { actor, .. }
            | Self::EventLoss { actor, .. }
            | Self::Checkpoint { actor, .. }
            | Self::SelectionStarted { actor, .. }
            | Self::SelectionObserved { actor, .. }
            | Self::SelectionMatched { actor, .. }
            | Self::GuardActivated { actor, .. }
            | Self::GuardTriggered { actor, .. }
            | Self::GuardDeactivated { actor, .. }
            | Self::ActorOpened { actor, .. }
            | Self::ActorClosed { actor, .. } => Some(*actor),
            _ => None,
        }
    }
}
