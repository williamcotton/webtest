//! Sequential execution of protocol-neutral test plans.

mod artifacts;
mod assertions;
mod cancellation;
mod cleanup;
mod control;
mod error;
mod evaluation;
mod events;
mod execution;
mod options;
mod redaction;
mod resource_scope;
mod resources;
mod result;
mod runner;
mod subscription;
mod url;
mod waits;

pub use artifacts::{Artifact, ArtifactKind};
pub use cancellation::{CancellationToken, ScopeContext};
pub use control::RunControl;
pub use error::{
    AssertionFailure, DecodeFailure, EvaluationFailure, EvaluationFailureKind, RunError, StepError,
};
pub use events::{JournalOverflow, RunEventSink};
pub use options::{EvidenceOptions, RunnerOptions};
pub use resource_scope::{
    AcquisitionOwnership, ResourceAdapter, ResourceCleanupFailure, ResourceFailure, ResourceScope,
    ResourceScopeEvent, ResourceScopeOutcome,
};
pub use resources::{ResourceInvariant, ResourceLease, ResourceRegistry};
pub use result::{
    BranchResult, PriorRunOutcome, PriorTestOutcome, RunOutcome, RunResult, StepFailure,
    TestOutcome, TestResult,
};
pub use runner::{
    InvalidJobLimit, JobLimit, Runner, TestRun, TestWorker, run_jobs, run_jobs_on_workers,
};
pub use subscription::{
    EventSubscription, SubscriptionGapReason, SubscriptionItem, SubscriptionOverflow,
};
pub use url::resolve_browser_url;
pub use waits::{
    TimerWait, WaitCleanupFailure, WaitCleanupPhase, WaitCompletion, WaitInvariant, WaitOutcome,
    WaitRegistry, WaitSource,
};
pub use webtest_feedback::FailureClass;
pub use webtest_observation::{
    CancellationReason, CleanupCause, CleanupFailure, CleanupIoErrorKind, CleanupIoFailure,
    CleanupResource, SkipReason,
};

#[cfg(test)]
mod tests;
