use std::{collections::BTreeMap, time::Duration};

use webtest_browser::{PageEvidence, PageInspection, RepairHint};
use webtest_feedback::FailureClass;
use webtest_model::{StepId, TestId, Value};
use webtest_observation::{
    CancellationReason, ExecutionEvent, ExecutionId, RunOutcomeKind, SkipReason, TestOutcomeKind,
};
use webtest_plan::PlannedStep;

use crate::{Artifact, RunError, StepError};

#[derive(Clone, Debug)]
pub struct StepFailure {
    pub step: PlannedStep,
    pub error: StepError,
    pub evidence: PageEvidence,
    pub artifacts: Vec<Artifact>,
    pub inspection: Option<PageInspection>,
    pub repair_hints: Vec<RepairHint>,
    pub secondary_failures: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub test_declaration_id: webtest_model::TestDeclarationId,
    pub variant_id: webtest_model::TestVariantId,
    pub test_id: TestId,
    pub name: String,
    pub outcome: TestOutcome,
    pub duration: Duration,
    pub bindings: BTreeMap<String, Value>,
    /// Ordered child results, including successes and cancellation/cleanup facts.
    /// `outcome` summarizes this aggregate; it never replaces the child results.
    pub branches: Vec<BranchResult>,
}

#[derive(Clone, Debug)]
pub struct BranchResult {
    pub race_winner: bool,
    pub scope: webtest_observation::ScopeEvent,
    pub outcome: TestOutcome,
    pub duration: Duration,
    pub branches: Vec<BranchResult>,
}

#[derive(Clone, Debug)]
pub enum TestOutcome {
    Verdict {
        kind: webtest_model::VerdictKind,
        message: String,
    },
    Passed,
    Failed(Box<StepFailure>),
    TimedOut {
        timeout: Duration,
        active_step: Option<StepId>,
    },
    Cancelled {
        reason: CancellationReason,
    },
    Skipped {
        reason: SkipReason,
        failure_class: Option<FailureClass>,
    },
    Aborted {
        failure: RunError,
        prior_outcome: Option<Box<PriorTestOutcome>>,
    },
}

#[derive(Clone, Debug)]
pub enum PriorTestOutcome {
    Verdict {
        kind: webtest_model::VerdictKind,
        message: String,
    },
    Failed(Box<StepFailure>),
    TimedOut {
        timeout: Duration,
        active_step: Option<StepId>,
    },
    Cancelled {
        reason: CancellationReason,
    },
}

impl TestOutcome {
    pub(crate) const fn finished_kind(&self) -> TestOutcomeKind {
        match self {
            Self::Verdict { kind, .. } => match kind {
                webtest_model::VerdictKind::Pass => TestOutcomeKind::Passed,
                webtest_model::VerdictKind::Fail => TestOutcomeKind::Failed,
                webtest_model::VerdictKind::Skipped => TestOutcomeKind::Skipped,
                webtest_model::VerdictKind::Inconclusive => TestOutcomeKind::Inconclusive,
            },
            Self::Passed => TestOutcomeKind::Passed,
            Self::Failed(_) => TestOutcomeKind::Failed,
            Self::TimedOut { .. } => TestOutcomeKind::TimedOut,
            Self::Cancelled { .. } => TestOutcomeKind::Cancelled,
            Self::Skipped {
                reason: SkipReason::RunCancelled,
                ..
            } => TestOutcomeKind::Cancelled,
            Self::Skipped {
                reason: SkipReason::RunAborted,
                ..
            }
            | Self::Aborted { .. } => TestOutcomeKind::Aborted,
        }
    }

    pub(crate) fn failure_class(&self) -> Option<FailureClass> {
        match self {
            Self::Verdict { kind, .. } => matches!(
                kind,
                webtest_model::VerdictKind::Fail | webtest_model::VerdictKind::Inconclusive
            )
            .then_some(FailureClass::Test),
            Self::Passed | Self::Cancelled { .. } => None,
            Self::Failed(_) | Self::TimedOut { .. } => Some(FailureClass::Test),
            Self::Skipped { failure_class, .. } => *failure_class,
            Self::Aborted { failure, .. } => Some(failure.failure_class()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum RunOutcome {
    Completed,
    Cancelled {
        reason: CancellationReason,
    },
    Aborted {
        failure: RunError,
        prior_outcome: Option<PriorRunOutcome>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorRunOutcome {
    Cancelled { reason: CancellationReason },
}

impl RunOutcome {
    pub const fn kind(&self) -> RunOutcomeKind {
        match self {
            Self::Completed => RunOutcomeKind::Completed,
            Self::Cancelled { .. } => RunOutcomeKind::Cancelled,
            Self::Aborted { .. } => RunOutcomeKind::Aborted,
        }
    }

    pub const fn failure_class(&self) -> Option<FailureClass> {
        match self {
            Self::Completed | Self::Cancelled { .. } => None,
            Self::Aborted { failure, .. } => Some(failure.failure_class()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunResult {
    /// Absent only when identity allocation failed before execution began.
    pub execution_id: Option<ExecutionId>,
    pub outcome: RunOutcome,
    pub tests: Vec<TestResult>,
    /// Retained authoritative native records; `events` is the compatibility projection.
    /// On journal exhaustion, the run's typed `JournalOverflow` describes the
    /// omitted interval. The reserved final record still reports the abort.
    pub journal: Vec<webtest_observation::RecordedEvent>,
    pub events: Vec<ExecutionEvent>,
    pub duration: Duration,
}

impl RunResult {
    pub fn passed(&self) -> usize {
        self.counts()[0]
    }

    pub fn failed(&self) -> usize {
        self.counts()[1]
    }

    pub fn timed_out(&self) -> usize {
        self.counts()[2]
    }

    pub fn cancelled(&self) -> usize {
        self.counts()[3]
    }

    pub fn skipped(&self) -> usize {
        self.counts()[4]
    }

    pub fn aborted(&self) -> usize {
        self.counts()[5]
    }

    fn counts(&self) -> [usize; 6] {
        let mut counts = [0; 6];
        for result in &self.tests {
            let index = match result.outcome {
                TestOutcome::Verdict { kind, .. } => match kind {
                    webtest_model::VerdictKind::Pass => 0,
                    webtest_model::VerdictKind::Fail | webtest_model::VerdictKind::Inconclusive => {
                        1
                    }
                    webtest_model::VerdictKind::Skipped => 4,
                },
                TestOutcome::Passed => 0,
                TestOutcome::Failed(_) => 1,
                TestOutcome::TimedOut { .. } => 2,
                TestOutcome::Cancelled { .. } => 3,
                TestOutcome::Skipped { .. } => 4,
                TestOutcome::Aborted { .. } => 5,
            };
            counts[index] += 1;
        }
        counts
    }
}
