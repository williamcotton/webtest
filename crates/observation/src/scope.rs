use serde::{Deserialize, Serialize};
use webtest_model::{
    AttemptId, ExecutionScopeId, OperationExecutionId, PlanNodeId, TestExecutionId, TestId,
};
use webtest_text::{SourceRevision, SyntaxOrigin};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub test_execution_id: TestExecutionId,
    pub test_id: TestId,
    pub task_path: Vec<u32>,
    pub scope_id: ExecutionScopeId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_scope_id: Option<ExecutionScopeId>,
    pub plan_node_id: PlanNodeId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_execution_id: Option<OperationExecutionId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeOutcome {
    Passed,
    Failed,
    Aborted,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeCancellation {
    Timeout { causing_scope_id: ExecutionScopeId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeEvent {
    pub execution_context: ExecutionContext,
    pub source_revision: SourceRevision,
    pub origin: SyntaxOrigin,
    /// Absent on entry; a terminal event carries the outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ScopeOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellation: Option<ScopeCancellation>,
}

impl ScopeEvent {
    pub fn kind(&self) -> &'static str {
        match (
            self.execution_context.operation_execution_id.is_some(),
            self.outcome.is_some(),
        ) {
            (false, false) => "scope_started",
            (false, true) => "scope_finished",
            (true, false) => "operation_started",
            (true, true) => "operation_finished",
        }
    }
}
