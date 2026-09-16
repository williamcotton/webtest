use std::collections::BTreeMap;

use async_trait::async_trait;
use webtest_model::Value;
use webtest_plan::{PlannedStep, PlannedTest};

use crate::StepError;

/// Immutable runtime identity and ancestry, never a view into mutable branch state.
#[derive(Clone, Debug)]
pub struct DebugContext {
    pub execution_id: webtest_observation::ExecutionId,
    pub task_id: webtest_model::ExecutionScopeId,
    pub parent_task_id: Option<webtest_model::ExecutionScopeId>,
    pub frames: Vec<DebugScope>,
}

#[derive(Clone, Debug)]
pub struct DebugScope {
    pub scope: webtest_observation::ScopeEvent,
    pub kind: DebugScopeKind,
    pub attempt: Option<webtest_observation::AttemptEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugScopeKind {
    Sequence,
    Parallel,
    Race,
    Retry,
    Timeout,
    BrowserContext,
    Operation,
}

/// Finishes debugger task bookkeeping even if a bounded wait drops its body.
/// The guard encloses owned teardown; it never owns runtime resources.
pub(crate) struct DebugTaskGuard<'a> {
    control: Option<&'a dyn RunControl>,
    context: DebugContext,
}
impl<'a> DebugTaskGuard<'a> {
    pub fn new(
        control: Option<&'a dyn RunControl>,
        test: &PlannedTest,
        context: DebugContext,
    ) -> Self {
        if let Some(control) = control {
            control.task_started(test, &context);
        }
        Self { control, context }
    }
}
impl Drop for DebugTaskGuard<'_> {
    fn drop(&mut self) {
        if let Some(control) = self.control {
            control.task_finished(&self.context);
        }
    }
}

#[async_trait]
pub trait RunControl: Send + Sync {
    fn task_started(&self, _test: &PlannedTest, _context: &DebugContext) {}
    fn task_finished(&self, _context: &DebugContext) {}

    async fn before_step_in_context(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        _context: &DebugContext,
        bindings: Option<BTreeMap<String, Value>>,
    ) {
        if let Some(bindings) = bindings {
            self.before_step_with_bindings(test, step, bindings).await;
        } else {
            self.before_step(test, step).await;
        }
    }

    async fn after_step_failure_in_context(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        _context: &DebugContext,
        error: &StepError,
        bindings: &BTreeMap<String, Value>,
    ) {
        self.after_step_failure(test, step, error, bindings).await;
    }

    fn is_cancelled(&self) -> bool {
        false
    }

    fn cancellation_reason(&self) -> webtest_host::CancellationReason {
        webtest_host::CancellationReason::UserCancelled
    }

    /// Hosts may override this with their own wakeup primitive. No task is spawned.
    async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn before_step(&self, test: &PlannedTest, step: &PlannedStep);

    /// Called after the per-test deadline drops an active debugger hook or step.
    /// Debug pause time counts toward the deadline in the current runtime.
    fn after_test_timeout(&self, _test: &PlannedTest, _active_step: Option<&PlannedStep>) {}

    fn should_capture_bindings(&self, _test: &PlannedTest, _step: &PlannedStep) -> bool {
        true
    }

    async fn after_step_failure(
        &self,
        _test: &PlannedTest,
        _step: &PlannedStep,
        _error: &StepError,
        _bindings: &BTreeMap<String, Value>,
    ) {
    }

    async fn before_step_with_bindings(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        _bindings: BTreeMap<String, Value>,
    ) {
        self.before_step(test, step).await;
    }
}
