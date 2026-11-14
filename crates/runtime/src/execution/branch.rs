use super::{scopes::BranchScopes, state::TestExecutionState, temporary::TemporaryResources};
use crate::RunnerOptions;
use webtest_browser::Page;
use webtest_model::StepId;
use webtest_observation::CleanupFailure;

/// Mutable execution data has one branch owner. Siblings never borrow or lock this
/// structure together, and native handles/resource ownership are never cloned.
pub(super) struct BranchState {
    pub bindings: TestExecutionState,
    pub check_depth: usize,
    pub ingress: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub page: Option<Box<dyn Page>>,
    pub actors: std::collections::HashMap<webtest_model::ActorId, super::actors::ActorHandle>,
    pub cursors: std::collections::HashMap<webtest_model::BindingId, super::actors::Cursor>,
    pub current_actor: Option<webtest_model::ActorId>,
    pub session: Option<super::resource::OwnedBrowserSession>,
    pub session_owner: Option<webtest_observation::ScopeEvent>,
    pub provided: Option<webtest_model::Value>,
    pub active_step: Option<StepId>,
    pub active_operation: Option<webtest_observation::ScopeEvent>,
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
            check_depth: 0,
            ingress: Default::default(),
            bindings: TestExecutionState::new(
                options.redacted_json_fields.clone(),
                options.project_root.clone(),
            ),
            page: None,
            actors: Default::default(),
            current_actor: None,
            cursors: Default::default(),
            session: None,
            session_owner: None,
            active_step: None,
            active_operation: None,
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
            check_depth: self.check_depth,
            ingress: self.ingress.clone(),
            bindings: self.bindings.transferable_snapshot(),
            page: None,
            actors: self.actors.clone(),
            current_actor: None,
            cursors: self.cursors.clone(),
            session: None,
            session_owner: None,
            active_step: None,
            active_operation: None,
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
