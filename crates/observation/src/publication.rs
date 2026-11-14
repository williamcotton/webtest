//! Portable completed observation batches. Native transport owns paths/authentication.
pub const OBSERVATION_IPC_VERSION: u32 = 1;
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_locations: Vec<webtest_text::SourceLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_declaration_id: Option<webtest_model::TestDeclarationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<webtest_model::TestVariantId>,
    pub test_id: TestId,
    pub step_id: Option<StepId>,
    pub start: u32,
    pub end: u32,
    pub kind: RuntimeObservationKind,
}

impl From<RuntimeObservation> for PublishedObservation {
    fn from(value: RuntimeObservation) -> Self {
        Self {
            related_locations: value.related_locations,
            test_declaration_id: value.test_declaration_id,
            variant_id: value.variant_id,
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
                    related_locations: value.related_locations,
                    test_declaration_id: value.test_declaration_id,
                    variant_id: value.variant_id,
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
