use std::collections::BTreeMap;

use async_trait::async_trait;
use webtest_plan::{PlannedStep, PlannedTest};
use webtest_provider::Value;

use crate::StepError;

#[async_trait]
pub trait RunControl: Send + Sync {
    fn is_cancelled(&self) -> bool {
        false
    }

    async fn before_step(&self, test: &PlannedTest, step: &PlannedStep);

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
