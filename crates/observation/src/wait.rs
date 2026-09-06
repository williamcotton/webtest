use serde::{Deserialize, Serialize};
use webtest_model::{ExecutionScopeId, WaitRegistrationId};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitEventKind {
    Registered,
    Ready,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitEvent {
    pub registration_id: WaitRegistrationId,
    pub owner_scope_id: ExecutionScopeId,
    pub kind: WaitEventKind,
    pub cancellation: Option<webtest_host::Cancellation>,
}
