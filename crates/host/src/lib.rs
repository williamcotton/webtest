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
    GuardFailed,
    Verdict,
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

/// Resource categories shared by native host ownership and journal facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    ApplicationProcess,
    BridgeEndpoint,
    BrowserSession,
    BrowserContext,
    TemporaryDirectory,
    Conformance,
}

/// Observes physical ownership without introducing a scheduler into a host.
/// The returned observation belongs to the same owner as the native handle.
pub trait ResourceObserver: Send + Sync {
    fn acquired(&self, kind: ResourceKind) -> Box<dyn ResourceObservation>;
}

pub trait ResourceObservation: Send + Sync {
    fn ready(&mut self);
    fn releasing(&mut self);
    fn released(&mut self, succeeded: bool);
}
