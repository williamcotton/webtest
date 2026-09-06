//! Protocol-neutral host interruption contracts. Scheduling belongs to the runtime.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, time::Duration};
pub use webtest_model::ExecutionScopeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    UserCancelled,
    ParentFailed,
    RaceLost,
    Timeout,
    DebugDisconnect,
    FailFast,
    RunnerShutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancellation {
    pub reason: CancellationReason,
    pub causing_scope_id: ExecutionScopeId,
}

/// A host observes remaining time and cancellation without depending on the scheduler.
/// Implementations must not return a duration exceeding any inherited deadline.
#[async_trait]
pub trait OperationContext: Debug + Send + Sync {
    fn scope_id(&self) -> ExecutionScopeId;
    fn remaining(&self) -> Option<Duration>;
    fn cancellation(&self) -> Option<Cancellation>;
    async fn cancelled(&self) -> Cancellation;
}
