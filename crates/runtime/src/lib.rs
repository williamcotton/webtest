//! Sequential execution of protocol-neutral test plans.

mod artifacts;
mod assertions;
mod control;
mod error;
mod evaluation;
mod events;
mod execution;
mod options;
mod redaction;
mod result;
mod runner;
mod url;

pub use artifacts::{Artifact, ArtifactKind};
pub use control::RunControl;
pub use error::{
    AssertionFailure, DecodeFailure, EvaluationFailure, EvaluationFailureKind, RunError, StepError,
};
pub use events::RunEventSink;
pub use options::{EvidenceOptions, RunnerOptions};
pub use result::{
    PriorRunOutcome, PriorTestOutcome, RunOutcome, RunResult, StepFailure, TestOutcome, TestResult,
};
pub use runner::Runner;
pub use url::resolve_browser_url;
pub use webtest_feedback::FailureClass;
pub use webtest_observation::{
    CancellationReason, CleanupCause, CleanupFailure, CleanupIoErrorKind, CleanupIoFailure,
    CleanupResource, SkipReason,
};

#[cfg(test)]
mod tests;
