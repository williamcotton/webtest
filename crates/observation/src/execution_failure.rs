//! Portable abort facts. Native error sources are projected by the runtime.
use crate::{CleanupFailure, EventIdentity, RuntimeFailureCode};
use serde::{Deserialize, Serialize};
use webtest_feedback::FailureClass;
use webtest_model::StepId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalOverflow {
    pub capacity: usize,
    pub first_rejected: EventIdentity,
    pub last_rejected: EventIdentity,
    pub rejected_events: u64,
}
impl std::fmt::Display for JournalOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "event journal capacity {} exceeded; {} events omitted (sequences {} through {})",
            self.capacity,
            self.rejected_events,
            self.first_rejected.event_sequence.0,
            self.last_rejected.event_sequence.0
        )
    }
}
impl std::error::Error for JournalOverflow {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "details",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExecutionFailure {
    IdentityAllocation {
        raw_os_error: Option<i32>,
        message: String,
    },
    JournalOverflow(JournalOverflow),
    FixtureBudget(crate::FixtureBudgetExceeded),
    Browser(webtest_browser::BrowserError),
    Provider(webtest_provider::ProviderError),
    Cleanup(CleanupFailure),
    Multiple {
        primary: Box<Self>,
        secondary: Vec<Self>,
    },
    Internal {
        message: String,
    },
}

impl ExecutionFailure {
    pub fn code(&self) -> RuntimeFailureCode {
        match self {
            Self::IdentityAllocation { .. } => RuntimeFailureCode::ExecutionIdentityUnavailable,
            Self::JournalOverflow(_) => RuntimeFailureCode::JournalCapacityExceeded,
            Self::FixtureBudget(_) => RuntimeFailureCode::FixtureBudgetExceeded,
            Self::Browser(error) => error.into(),
            Self::Provider(error) => error.into(),
            Self::Cleanup(error) => error.code(),
            Self::Multiple { primary, .. } => primary.code(),
            Self::Internal { .. } => RuntimeFailureCode::InternalError,
        }
    }
    pub fn failure_class(&self) -> FailureClass {
        match self {
            Self::IdentityAllocation { .. }
            | Self::FixtureBudget(_)
            | Self::JournalOverflow(_)
            | Self::Browser(_)
            | Self::Provider(_) => FailureClass::Infrastructure,
            Self::Cleanup(error) => error.failure_class(),
            Self::Multiple { primary, .. } => primary.failure_class(),
            Self::Internal { .. } => FailureClass::Internal,
        }
    }
}

/// Outcome superseded by an abort during teardown. Keep the failure and its
/// exact source directly; a static StepId alone cannot identify a retry occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PriorScopeOutcome {
    Failed {
        step_id: StepId,
        origin: webtest_text::SyntaxOrigin,
        failure: Box<crate::RuntimeFailure>,
    },
    TimedOut {
        timeout_ms: u64,
        active_step: Option<StepId>,
    },
    Cancelled {
        reason: crate::CancellationReason,
    },
}
