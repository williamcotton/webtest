use serde::{Deserialize, Serialize};
use webtest_model::{ExecutionScopeId, ResourceGenerationId, RuntimeResourceId};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAccess {
    Shared,
    Exclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionState {
    Acquiring,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeardownState {
    Pending,
    Releasing,
    Released,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceKey {
    pub resource_id: RuntimeResourceId,
    pub generation_id: ResourceGenerationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeResourceEntry {
    pub key: ResourceKey,
    pub owner_scope_id: ExecutionScopeId,
    pub resource_kind: ResourceKind,
    pub access_policy: ResourceAccess,
    pub acquisition_state: AcquisitionState,
    pub teardown_state: TeardownState,
    pub ownership_acquired: bool,
    pub cancellation: Option<webtest_host::Cancellation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceEventKind {
    AcquireStarted,
    Ready,
    AcquireFailed,
    ReleaseStarted,
    Released,
    ReleaseFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceEvent {
    pub kind: ResourceEventKind,
    pub resource: RuntimeResourceEntry,
}

impl ResourceEventKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::AcquireStarted => "resource_acquire_started",
            Self::Ready => "resource_ready",
            Self::AcquireFailed => "resource_acquire_failed",
            Self::ReleaseStarted => "resource_release_started",
            Self::Released => "resource_released",
            Self::ReleaseFailed => "resource_release_failed",
        }
    }
}
