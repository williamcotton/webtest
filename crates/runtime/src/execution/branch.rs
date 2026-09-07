use super::{scopes::BranchScopes, state::TestExecutionState, temporary::TemporaryResources};
use crate::RunnerOptions;
use webtest_browser::{BrowserSession, Page};
use webtest_model::StepId;
use webtest_observation::CleanupFailure;

/// Mutable execution data has one branch owner. Siblings never borrow or lock this
/// structure together, and native handles/resource ownership are never cloned.
pub(super) struct BranchState {
    pub bindings: TestExecutionState,
    pub page: Option<Box<dyn Page>>,
    pub session: Option<Box<dyn BrowserSession>>,
    pub provided: Option<webtest_model::Value>,
    pub active_step: Option<StepId>,
    pub scopes: BranchScopes,
    pub temporary: TemporaryResources,
    pub cleanup_failures: Vec<CleanupFailure>,
    pub primary_failure: Option<super::TestBodyOutcome>,
    pub failure_signals: Vec<super::scheduler::FailureSignal>,
    pub completed_branches: Vec<crate::BranchResult>,
}

impl BranchState {
    pub fn new(options: &RunnerOptions) -> Self {
        Self {
            bindings: TestExecutionState::new(
                options.redacted_json_fields.clone(),
                options.project_root.clone(),
            ),
            page: None,
            session: None,
            active_step: None,
            provided: None,
            scopes: BranchScopes::default(),
            temporary: TemporaryResources::default(),
            cleanup_failures: Vec::new(),
            primary_failure: None,
            failure_signals: Vec::new(),
            completed_branches: Vec::new(),
        }
    }

    pub fn fork(&self) -> Self {
        Self {
            bindings: self.bindings.transferable_snapshot(),
            page: None,
            session: None,
            active_step: None,
            provided: None,
            scopes: BranchScopes::default(),
            temporary: TemporaryResources::default(),
            cleanup_failures: Vec::new(),
            primary_failure: None,
            failure_signals: self.failure_signals.clone(),
            completed_branches: Vec::new(),
        }
    }
}
