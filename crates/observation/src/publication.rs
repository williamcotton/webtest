//! Portable completed observation batches. Native transport owns paths/authentication.
use crate::{ExecutionId, RuntimeObservation, RuntimeObservationKind};
use webtest_model::{StepId, TestId};
use webtest_text::{FileId, SourceRevision, TextRange, TextSize};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationPublication {
    pub execution_id: ExecutionId,
    pub revision: SourceRevision,
    pub observations: Vec<PublishedObservation>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedObservation {
    pub test_id: TestId,
    pub step_id: Option<StepId>,
    pub start: u32,
    pub end: u32,
    pub kind: RuntimeObservationKind,
}

impl From<RuntimeObservation> for PublishedObservation {
    fn from(value: RuntimeObservation) -> Self {
        Self {
            test_id: value.test_id,
            step_id: value.step_id,
            start: value.range.start().into(),
            end: value.range.end().into(),
            kind: value.kind,
        }
    }
}

impl ObservationPublication {
    pub fn into_observations(self, file: FileId, source: &str) -> Option<Vec<RuntimeObservation>> {
        if SourceRevision::of(source) != self.revision {
            return None;
        }
        self.observations
            .into_iter()
            .map(|value| {
                if value.start > value.end
                    || !source.is_char_boundary(value.start as usize)
                    || !source.is_char_boundary(value.end as usize)
                {
                    return None;
                }
                Some(RuntimeObservation {
                    execution_id: self.execution_id,
                    file,
                    source_revision: self.revision,
                    test_id: value.test_id,
                    step_id: value.step_id,
                    range: TextRange::new(TextSize::from(value.start), TextSize::from(value.end)),
                    kind: value.kind,
                })
            })
            .collect()
    }
}
