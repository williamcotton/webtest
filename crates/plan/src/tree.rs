//! The execution tree is the authoritative operation representation.
use serde::{Deserialize, Serialize};
use webtest_model::{Capability, PlanDeclarationId, PlanNodeId, TestId};
use webtest_text::{SourceRevision, SyntaxOrigin};

use crate::{PlannedStep, PlannedTest};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanNode {
    pub id: PlanNodeId,
    pub path: Vec<u32>,
    pub origin: SyntaxOrigin,
    pub source_revision: SourceRevision,
    pub required_capabilities: Vec<Capability>,
    pub kind: PlanNodeKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanNodeKind {
    Sequence { children: Vec<PlanNode> },
    Operation { step: Box<PlannedStep> },
}

impl PlanNode {
    pub fn sequence(
        test: PlanDeclarationId,
        origin: SyntaxOrigin,
        revision: SourceRevision,
        path: Vec<u32>,
        children: Vec<Self>,
    ) -> Self {
        let required_capabilities = children
            .iter()
            .flat_map(|child| child.required_capabilities.iter().copied())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut node = Self {
            id: PlanNodeId([0; 32]),
            path,
            origin,
            source_revision: revision,
            required_capabilities,
            kind: PlanNodeKind::Sequence { children },
        };
        node.assign_identity(test);
        node
    }

    pub fn operation(
        test: PlanDeclarationId,
        revision: SourceRevision,
        path: Vec<u32>,
        step: PlannedStep,
    ) -> Self {
        let required_capabilities = operation_capabilities(&step.operation);
        let mut node = Self {
            id: PlanNodeId([0; 32]),
            path,
            origin: step.origin,
            source_revision: revision,
            required_capabilities,
            kind: PlanNodeKind::Operation {
                step: Box::new(step),
            },
        };
        node.assign_identity(test);
        node
    }

    /// Source declaration identity and structural path, independent of step allocation.
    fn assign_identity(&mut self, test: PlanDeclarationId) {
        self.id = self.derived_identity(test);
    }

    fn kind_identity(&self) -> &'static str {
        match &self.kind {
            PlanNodeKind::Sequence { .. } => "sequence/v1",
            PlanNodeKind::Operation { step } => match &step.operation {
                crate::TestOperation::EvaluatePure(_) => "eval/v1",
                crate::TestOperation::ServerProviderCall(_) => "provider/v1",
                crate::TestOperation::Browser(_) => "browser/v1",
                crate::TestOperation::Assertion(_) => "assert/v1",
            },
        }
    }

    fn derived_identity(&self, test: PlanDeclarationId) -> PlanNodeId {
        let kind = self.kind_identity();
        PlanNodeId(
            *SourceRevision::of(&format!(
                "webtest-node/v1/{:?}/{:?}/{kind}",
                test.0, self.path
            ))
            .as_bytes(),
        )
    }

    pub fn steps(&self) -> Vec<&PlannedStep> {
        match &self.kind {
            PlanNodeKind::Sequence { children } => children.iter().flat_map(Self::steps).collect(),
            PlanNodeKind::Operation { step } => vec![step],
        }
    }

    pub fn steps_mut(&mut self) -> Vec<&mut PlannedStep> {
        match &mut self.kind {
            PlanNodeKind::Sequence { children } => {
                children.iter_mut().flat_map(Self::steps_mut).collect()
            }
            PlanNodeKind::Operation { step } => vec![step],
        }
    }
}

fn operation_capabilities(operation: &crate::TestOperation) -> Vec<Capability> {
    use crate::{AssertionOperation, TestOperation};
    match operation {
        TestOperation::EvaluatePure(_) => vec![],
        TestOperation::ServerProviderCall(_) => vec![Capability::Server],
        TestOperation::Browser(_) => vec![Capability::Browser],
        TestOperation::Assertion(AssertionOperation::Value { .. }) => vec![Capability::Test],
        TestOperation::Assertion(_) => vec![Capability::Browser],
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanTreeError {
    RootIsNotSequence,
    InvalidPath,
    InvalidIdentity,
    SourceRevisionMismatch,
    OriginMismatch,
    CapabilityMismatch,
    DuplicateStep,
    DuplicateTest,
    MissingSourceFile,
    TooDeep,
}
impl std::fmt::Display for PlanTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid execution tree: {self:?}")
    }
}
impl std::error::Error for PlanTreeError {}

impl PlanNode {
    fn validate(
        &self,
        test: PlanDeclarationId,
        revision: SourceRevision,
        path: &[u32],
        steps: &mut std::collections::BTreeSet<webtest_model::StepId>,
    ) -> Result<(), PlanTreeError> {
        if path.len() > 128 {
            return Err(PlanTreeError::TooDeep);
        }
        if self.path != path {
            return Err(PlanTreeError::InvalidPath);
        }
        if self.source_revision != revision {
            return Err(PlanTreeError::SourceRevisionMismatch);
        }
        if self.id != self.derived_identity(test) {
            return Err(PlanTreeError::InvalidIdentity);
        }
        let capabilities = match &self.kind {
            PlanNodeKind::Sequence { children } => {
                for (ordinal, child) in children.iter().enumerate() {
                    if child.origin.file != self.origin.file {
                        return Err(PlanTreeError::OriginMismatch);
                    }
                    let mut child_path = path.to_vec();
                    child_path.push(ordinal as u32);
                    child.validate(test, revision, &child_path, steps)?;
                }
                children
                    .iter()
                    .flat_map(|child| child.required_capabilities.iter().copied())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect()
            }
            PlanNodeKind::Operation { step } => {
                if step.origin != self.origin {
                    return Err(PlanTreeError::OriginMismatch);
                }
                if !steps.insert(step.id) {
                    return Err(PlanTreeError::DuplicateStep);
                }
                operation_capabilities(&step.operation)
            }
        };
        if self.required_capabilities != capabilities {
            return Err(PlanTreeError::CapabilityMismatch);
        }
        Ok(())
    }
}

impl crate::TestPlan {
    pub fn validate_tree(&self) -> Result<(), PlanTreeError> {
        let mut steps = std::collections::BTreeSet::new();
        let mut tests = std::collections::BTreeSet::new();
        let mut declarations = std::collections::BTreeSet::new();
        for test in &self.tests {
            if !tests.insert(test.id) || !declarations.insert(test.declaration_id) {
                return Err(PlanTreeError::DuplicateTest);
            }
            if !matches!(test.body.kind, PlanNodeKind::Sequence { .. }) {
                return Err(PlanTreeError::RootIsNotSequence);
            }
            if test.origin != test.body.origin || test.origin.file != self.file {
                return Err(PlanTreeError::OriginMismatch);
            }
            test.body
                .validate(test.declaration_id, self.source_revision, &[], &mut steps)?;
            if test
                .body
                .required_capabilities
                .iter()
                .any(|capability| !test.required_host_capabilities.contains(capability))
            {
                return Err(PlanTreeError::CapabilityMismatch);
            }
        }
        Ok(())
    }
}

impl crate::PlanEnvelope {
    pub fn validate_tree(&self) -> Result<(), PlanTreeError> {
        let mut steps = std::collections::BTreeSet::new();
        let mut tests = std::collections::BTreeSet::new();
        let mut declarations = std::collections::BTreeSet::new();
        for test in &self.tests {
            if !tests.insert(test.id) || !declarations.insert(test.declaration_id) {
                return Err(PlanTreeError::DuplicateTest);
            }
            let Some(source) = self
                .source_files
                .iter()
                .find(|source| source.file == test.origin.file)
            else {
                return Err(PlanTreeError::MissingSourceFile);
            };
            if !matches!(test.body.kind, PlanNodeKind::Sequence { .. }) {
                return Err(PlanTreeError::RootIsNotSequence);
            }
            if test.origin != test.body.origin {
                return Err(PlanTreeError::OriginMismatch);
            }
            test.body
                .validate(test.declaration_id, source.revision, &[], &mut steps)?;
            if test
                .body
                .required_capabilities
                .iter()
                .any(|capability| !test.required_host_capabilities.contains(capability))
            {
                return Err(PlanTreeError::CapabilityMismatch);
            }
        }
        Ok(())
    }
}

impl PlannedTest {
    /// A read-only source-order projection for diagnostics and debugger locations.
    /// Execution dispatches the tree itself.
    pub fn steps(&self) -> Vec<&PlannedStep> {
        self.body.steps()
    }

    pub fn sequential(
        id: TestId,
        declaration_id: PlanDeclarationId,
        name: String,
        required_host_capabilities: Vec<Capability>,
        origin: SyntaxOrigin,
        revision: SourceRevision,
        steps: Vec<PlannedStep>,
    ) -> Self {
        let children = steps
            .into_iter()
            .enumerate()
            .map(|(index, step)| {
                PlanNode::operation(declaration_id, revision, vec![index as u32], step)
            })
            .collect();
        Self {
            id,
            declaration_id,
            name,
            required_host_capabilities,
            body: PlanNode::sequence(declaration_id, origin, revision, Vec::new(), children),
            origin,
        }
    }
}

/// Source identity and duplicate-name ordinal distinguish declarations without coupling
/// them to file opening order, global step allocation, or unrelated declarations.
pub fn declaration_identity(
    source_identity: &str,
    name: &str,
    duplicate_ordinal: u32,
) -> PlanDeclarationId {
    PlanDeclarationId(
        *SourceRevision::of(&format!(
            "webtest-declaration/v1/{}:{source_identity}/{}:{name}/{duplicate_ordinal}",
            source_identity.len(),
            name.len()
        ))
        .as_bytes(),
    )
}
