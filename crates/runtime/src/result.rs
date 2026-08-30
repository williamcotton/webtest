use std::{collections::BTreeMap, time::Duration};

use webtest_browser::{PageEvidence, PageInspection, RepairHint};
use webtest_observation::{ExecutionEvent, ExecutionId};
use webtest_plan::PlannedStep;
use webtest_provider::Value;

use crate::{Artifact, StepError};

#[derive(Clone, Debug)]
pub struct StepFailure {
    pub step: PlannedStep,
    pub error: StepError,
    pub evidence: PageEvidence,
    pub artifacts: Vec<Artifact>,
    pub inspection: Option<PageInspection>,
    pub repair_hints: Vec<RepairHint>,
    pub secondary_failures: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub failure: Option<StepFailure>,
    pub duration: Duration,
    pub bindings: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct RunResult {
    pub execution_id: ExecutionId,
    pub tests: Vec<TestResult>,
    pub events: Vec<ExecutionEvent>,
    pub duration: Duration,
}

impl RunResult {
    pub fn passed(&self) -> usize {
        self.tests.iter().filter(|result| result.passed).count()
    }

    pub fn failed(&self) -> usize {
        self.tests.len() - self.passed()
    }
}
