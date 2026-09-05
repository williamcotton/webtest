use std::collections::BTreeMap;

use webtest_model::Capability;
use webtest_text::SourceRevision;

use crate::{PlanEnvelope, PlanSourceFile};

/// Explicit host inputs: the plan crate never reads configuration or the filesystem.
pub struct PlanExecutionInputs<'a> {
    pub project_identity: &'a str,
    pub project_input_fingerprint: SourceRevision,
    pub sources: &'a [PlanSourceFile],
    pub provider_schema_hashes: &'a BTreeMap<String, String>,
    pub host_capabilities: &'a [Capability],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanCompatibilityError {
    UnsupportedVersion(crate::UnsupportedPlanVersion),
    CapabilityContract(crate::CapabilityInvariantError),
    InvalidTree(crate::PlanTreeError),
    ProjectIdentityDrift,
    ProjectInputDrift,
    SourceDrift { path: String },
    ProviderSchemaDrift { provider: String },
    MissingCapability { capability: Capability },
}

impl std::fmt::Display for PlanCompatibilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(error) => error.fmt(f),
            Self::CapabilityContract(error) => error.fmt(f),
            Self::InvalidTree(error) => error.fmt(f),
            Self::ProjectIdentityDrift => f.write_str("plan project identity has changed"),
            Self::ProjectInputDrift => f.write_str("plan semantic project inputs have changed"),
            Self::SourceDrift { path } => write!(f, "plan source has changed: {path}"),
            Self::ProviderSchemaDrift { provider } => {
                write!(f, "plan provider schema has changed: {provider}")
            }
            Self::MissingCapability { capability } => {
                write!(f, "host lacks required capability: {capability}")
            }
        }
    }
}
impl std::error::Error for PlanCompatibilityError {}

impl PlanEnvelope {
    pub fn validate_execution_inputs(
        &self,
        inputs: &PlanExecutionInputs<'_>,
    ) -> Result<(), PlanCompatibilityError> {
        self.validate_version()
            .map_err(PlanCompatibilityError::UnsupportedVersion)?;
        self.validate_capabilities()
            .map_err(PlanCompatibilityError::CapabilityContract)?;
        self.validate_tree()
            .map_err(PlanCompatibilityError::InvalidTree)?;
        if self.project_identity != inputs.project_identity {
            return Err(PlanCompatibilityError::ProjectIdentityDrift);
        }
        if self.project_input_fingerprint != inputs.project_input_fingerprint {
            return Err(PlanCompatibilityError::ProjectInputDrift);
        }
        for source in &self.source_files {
            if !inputs
                .sources
                .iter()
                .any(|current| current.path == source.path && current.revision == source.revision)
            {
                return Err(PlanCompatibilityError::SourceDrift {
                    path: source.path.clone(),
                });
            }
        }
        for (provider, hash) in &self.provider_schema_hashes {
            if inputs.provider_schema_hashes.get(provider) != Some(hash) {
                return Err(PlanCompatibilityError::ProviderSchemaDrift {
                    provider: provider.clone(),
                });
            }
        }
        for capability in &self.required_host_capabilities {
            if !inputs.host_capabilities.contains(capability) {
                return Err(PlanCompatibilityError::MissingCapability {
                    capability: *capability,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlannedTest, TestPlan};
    use webtest_model::TestId;
    use webtest_text::{FileId, SyntaxOrigin, TextRange};

    #[test]
    fn compatibility_checks_versions_inputs_schemas_sources_and_host_independently() {
        let file = FileId::new(1);
        let revision = SourceRevision::of("test");
        let plan = TestPlan {
            file,
            source_revision: revision,
            required_host_capabilities: vec![Capability::Server],
            tests: vec![PlannedTest::sequential(
                TestId(0),
                crate::declaration_identity("test.webtest", "test", 0),
                "test".into(),
                vec![Capability::Server],
                SyntaxOrigin::new(file, TextRange::default()),
                revision,
                vec![],
            )],
        };
        let baseline = PlanEnvelope::from_plan(
            &plan,
            "test.webtest",
            "project",
            BTreeMap::from([("app".into(), "schema".into())]),
        );
        let inputs = PlanExecutionInputs {
            project_identity: &baseline.project_identity,
            project_input_fingerprint: baseline.project_input_fingerprint,
            sources: &baseline.source_files,
            provider_schema_hashes: &baseline.provider_schema_hashes,
            host_capabilities: &[Capability::Server],
        };
        assert!(baseline.validate_execution_inputs(&inputs).is_ok());
        let mut plan = baseline.clone();
        plan.runtime_semantics_version += 1;
        assert!(
            matches!(plan.validate_execution_inputs(&inputs), Err(PlanCompatibilityError::UnsupportedVersion(error)) if error.component == "runtime semantics")
        );
        let mut plan = baseline.clone();
        plan.format_version += 1;
        assert!(
            matches!(plan.validate_execution_inputs(&inputs), Err(PlanCompatibilityError::UnsupportedVersion(error)) if error.component == "format")
        );
        let mut plan = baseline.clone();
        plan.project_input_fingerprint = SourceRevision::of("changed");
        assert_eq!(
            plan.validate_execution_inputs(&inputs),
            Err(PlanCompatibilityError::ProjectInputDrift)
        );
        let mut plan = baseline.clone();
        plan.provider_schema_hashes
            .insert("app".into(), "changed".into());
        assert!(matches!(
            plan.validate_execution_inputs(&inputs),
            Err(PlanCompatibilityError::ProviderSchemaDrift { .. })
        ));
        let mut changed_sources = baseline.source_files.clone();
        changed_sources[0].revision = SourceRevision::of("changed");
        assert!(matches!(
            baseline.validate_execution_inputs(&PlanExecutionInputs {
                sources: &changed_sources,
                ..inputs
            }),
            Err(PlanCompatibilityError::SourceDrift { .. })
        ));
        assert!(matches!(
            baseline.validate_execution_inputs(&PlanExecutionInputs {
                host_capabilities: &[],
                ..inputs
            }),
            Err(PlanCompatibilityError::MissingCapability { .. })
        ));
        let mut plan = baseline.clone();
        plan.tests[0].body.path.push(0);
        assert_eq!(
            plan.validate_execution_inputs(&inputs),
            Err(PlanCompatibilityError::InvalidTree(
                crate::PlanTreeError::InvalidPath
            ))
        );
        let mut old = serde_json::to_value(&baseline).expect("JSON");
        old.as_object_mut()
            .expect("object")
            .remove("runtime_semantics_version");
        assert!(serde_json::from_value::<PlanEnvelope>(old).is_err());
    }
}
