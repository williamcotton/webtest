//! Structured execution events and revision-safe source observations.

use std::{
    collections::BTreeMap,
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use webtest_browser::{CandidateEvidence, Locator, PageSummary};
use webtest_feedback::{FailureClass, RepairHint};
use webtest_hir::{StepId, TestId};
use webtest_text::{FileId, SourceRevision, TextRange};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueDiff {
    Scalar {
        expected: Option<String>,
        actual: String,
    },
    String {
        common_prefix_chars: usize,
        expected_segment: String,
        actual_segment: String,
    },
    List {
        expected_len: usize,
        actual_len: usize,
        differing_indices: Vec<usize>,
    },
    Record {
        missing_fields: Vec<String>,
        unexpected_fields: Vec<String>,
        mismatched_fields: Vec<String>,
    },
    Contains {
        expected_item: String,
        actual: String,
    },
}

static NEXT_EXECUTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExecutionId(pub u64);

impl ExecutionId {
    pub fn next() -> Self {
        Self(NEXT_EXECUTION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    Requested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    RunCancelled,
    RunAborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcomeKind {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcomeKind {
    Completed,
    Cancelled,
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeFailure {
    Browser(webtest_browser::BrowserError),
    Provider(webtest_provider::ProviderError),
    Assertion { message: String, diff: ValueDiff },
    Decode { message: String },
    Evaluation { code: String, message: String },
    Internal { message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionEvent {
    RunStarted {
        execution_id: ExecutionId,
    },
    TestStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        name: String,
    },
    StepStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
    },
    StepPassed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
    },
    ProviderCallStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        provider: String,
        operation: String,
        transport_kind: Option<String>,
        arguments: BTreeMap<String, String>,
    },
    ProviderCallFinished {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        provider: String,
        operation: String,
        elapsed_ms: u64,
        transport_kind: Option<String>,
        result: Option<String>,
    },
    ProviderCallFailed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        provider: String,
        operation: String,
        code: String,
        message: String,
        failure_class: FailureClass,
        elapsed_ms: u64,
        transport_kind: Option<String>,
    },
    StepFailed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        failure_class: FailureClass,
        failure: RuntimeFailure,
        repair_hints: Vec<RepairHint>,
        page: Option<PageSummary>,
    },
    TestFinished {
        execution_id: ExecutionId,
        test_id: TestId,
        outcome: TestOutcomeKind,
        failure_class: Option<FailureClass>,
    },
    TestSkipped {
        execution_id: ExecutionId,
        test_id: TestId,
        name: String,
        reason: SkipReason,
        failure_class: Option<FailureClass>,
    },
    RunFinished {
        execution_id: ExecutionId,
        outcome: RunOutcomeKind,
        failure_class: Option<FailureClass>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeObservation {
    pub execution_id: ExecutionId,
    pub file: FileId,
    pub source_revision: SourceRevision,
    pub test_id: TestId,
    pub step_id: StepId,
    pub range: TextRange,
    pub kind: RuntimeObservationKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeObservationKind {
    BrowserFailure {
        code: String,
        message: String,
        locator: Option<Locator>,
        page_url: Option<String>,
        candidates: Vec<CandidateEvidence>,
        actionability: Vec<String>,
        artifacts: Vec<String>,
        elapsed_ms: u64,
        repair_hints: Vec<RepairHint>,
    },
    ValueFailure {
        code: String,
        message: String,
        path: Option<String>,
        expected: Option<String>,
        actual: Option<String>,
        diff: Option<ValueDiff>,
    },
    LocatorNotFound {
        locator: Locator,
        page_url: Option<String>,
    },
    LocatorAmbiguous {
        locator: Locator,
        matches: usize,
        page_url: Option<String>,
    },
    LocatorNotVisible {
        locator: Locator,
        page_url: Option<String>,
    },
}

#[derive(Default)]
pub struct ObservationStore {
    observations: Mutex<HashMap<(FileId, SourceRevision), Vec<RuntimeObservation>>>,
}

impl ObservationStore {
    pub fn clear(&self) {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    pub fn clear_for_file(&self, file: FileId) {
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        observations.retain(|(stored_file, _), _| *stored_file != file);
    }

    pub fn clear_for_execution(&self, execution_id: ExecutionId) {
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for values in observations.values_mut() {
            values.retain(|observation| observation.execution_id != execution_id);
        }
    }

    pub fn replace_for_file_revision(
        &self,
        file: FileId,
        revision: SourceRevision,
        values: Vec<RuntimeObservation>,
    ) {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert((file, revision), values);
    }

    pub fn record(&self, observation: RuntimeObservation) {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry((observation.file, observation.source_revision))
            .or_default()
            .push(observation);
    }

    pub fn observations_for(
        &self,
        file: FileId,
        revision: SourceRevision,
    ) -> Vec<RuntimeObservation> {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(file, revision))
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use webtest_text::TextSize;

    use super::*;

    #[test]
    fn observations_are_partitioned_by_source_revision() {
        let store = ObservationStore::default();
        let file = FileId::new(1);
        let revision = SourceRevision::of("a");
        store.record(RuntimeObservation {
            execution_id: ExecutionId::next(),
            file,
            source_revision: revision,
            test_id: TestId(0),
            step_id: StepId(0),
            range: TextRange::empty(TextSize::new(0)),
            kind: RuntimeObservationKind::LocatorNotFound {
                locator: Locator::Id("missing".into()),
                page_url: None,
            },
        });
        assert_eq!(store.observations_for(file, revision).len(), 1);
        assert!(
            store
                .observations_for(file, SourceRevision::of("b"))
                .is_empty()
        );
        store.clear_for_file(file);
        assert!(store.observations_for(file, revision).is_empty());
    }
}
