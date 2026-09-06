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
    Parallel {
        children: Vec<PlanNode>,
        failure_policy: ParallelFailurePolicy,
    },
    ResourceScope {
        resource: ResourcePlan,
        body: Box<PlanNode>,
    },
    Timeout {
        child: Box<PlanNode>,
        duration: std::time::Duration,
        cleanup_timeout: Option<std::time::Duration>,
    },
    Sequence {
        children: Vec<PlanNode>,
    },
    Operation {
        step: Box<PlannedStep>,
    },
}

/// A portable acquisition/ready/teardown recipe. Backend handles and runtime
/// resource generations are injected by the host and never serialized here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePlan {
    /// Acquire one exclusive browser context, acknowledge a ready page, execute
    /// the body, and explicitly close the context exactly once on every outcome.
    BrowserContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelFailurePolicy {
    CollectTestFailures,
}
pub const MAX_PARALLEL_BRANCHES: usize = 64;

pub const MAX_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

impl PlanNode {
    pub fn parallel(
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
            kind: PlanNodeKind::Parallel {
                children,
                failure_policy: ParallelFailurePolicy::CollectTestFailures,
            },
        };
        node.assign_identity(test);
        node
    }

    pub fn resource_scope(
        test: PlanDeclarationId,
        origin: SyntaxOrigin,
        revision: SourceRevision,
        path: Vec<u32>,
        resource: ResourcePlan,
        body: Self,
    ) -> Self {
        let mut node = Self {
            id: PlanNodeId([0; 32]),
            path,
            origin,
            source_revision: revision,
            required_capabilities: body.required_capabilities.clone(),
            kind: PlanNodeKind::ResourceScope {
                resource,
                body: Box::new(body),
            },
        };
        if !node.required_capabilities.contains(&Capability::Browser) {
            node.required_capabilities.push(Capability::Browser);
            node.required_capabilities.sort();
        }
        node.assign_identity(test);
        node
    }

    /// Lower a lexical browser lifetime while preserving the mandatory root
    /// sequence. The caller decides ownership; this does not inspect source.
    pub fn with_browser_resource(self, test: PlanDeclarationId) -> Self {
        let mut body_path = self.path.clone();
        body_path.extend([0, 0]);
        let origin = self.origin;
        let revision = self.source_revision;
        let path = self.path.clone();
        let mut body = self;
        body.rebase(test, body_path);
        let mut resource_path = path.clone();
        resource_path.push(0);
        let resource = Self::resource_scope(
            test,
            origin,
            revision,
            resource_path,
            ResourcePlan::BrowserContext,
            body,
        );
        Self::sequence(test, origin, revision, path, vec![resource])
    }

    fn rebase(&mut self, test: PlanDeclarationId, path: Vec<u32>) {
        self.path = path;
        match &mut self.kind {
            PlanNodeKind::Sequence { children } | PlanNodeKind::Parallel { children, .. } => {
                for (ordinal, child) in children.iter_mut().enumerate() {
                    let mut path = self.path.clone();
                    path.push(ordinal as u32);
                    child.rebase(test, path);
                }
            }
            PlanNodeKind::Timeout { child, .. }
            | PlanNodeKind::ResourceScope { body: child, .. } => {
                let mut path = self.path.clone();
                path.push(0);
                child.rebase(test, path);
            }
            PlanNodeKind::Operation { .. } => {}
        }
        self.assign_identity(test);
    }

    pub fn timeout(
        test: PlanDeclarationId,
        origin: SyntaxOrigin,
        revision: SourceRevision,
        path: Vec<u32>,
        child: Self,
        duration: std::time::Duration,
    ) -> Self {
        let mut node = Self {
            id: PlanNodeId([0; 32]),
            path,
            origin,
            source_revision: revision,
            required_capabilities: child.required_capabilities.clone(),
            kind: PlanNodeKind::Timeout {
                child: Box::new(child),
                duration,
                cleanup_timeout: None,
            },
        };
        node.assign_identity(test);
        node
    }
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
            PlanNodeKind::ResourceScope { .. } => "resource/browser-context/v1",
            PlanNodeKind::Parallel { .. } => "parallel/v1",
            PlanNodeKind::Sequence { .. } => "sequence/v1",
            PlanNodeKind::Timeout { .. } => "timeout/v1",
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
            PlanNodeKind::ResourceScope { body, .. } => body.steps(),
            PlanNodeKind::Timeout { child, .. } => child.steps(),
            PlanNodeKind::Sequence { children } | PlanNodeKind::Parallel { children, .. } => {
                children.iter().flat_map(Self::steps).collect()
            }
            PlanNodeKind::Operation { step } => vec![step],
        }
    }

    pub fn steps_mut(&mut self) -> Vec<&mut PlannedStep> {
        match &mut self.kind {
            PlanNodeKind::ResourceScope { body, .. } => body.steps_mut(),
            PlanNodeKind::Timeout { child, .. } => child.steps_mut(),
            PlanNodeKind::Sequence { children } | PlanNodeKind::Parallel { children, .. } => {
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
    InvalidControlSetting,
    InvalidPath,
    InvalidIdentity,
    SourceRevisionMismatch,
    OriginMismatch,
    CapabilityMismatch,
    DuplicateStep,
    DuplicateTest,
    MissingSourceFile,
    MissingResourceScope,
    ResourceAccessConflict,
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
        if let PlanNodeKind::Parallel { children, .. } = &self.kind {
            if children.is_empty() || children.len() > MAX_PARALLEL_BRANCHES {
                return Err(PlanTreeError::InvalidControlSetting);
            }
            let accesses: Vec<_> = children.iter().map(Self::required_resources).collect();
            if !crate::conflicting_resource_accesses(&accesses).is_empty() {
                return Err(PlanTreeError::ResourceAccessConflict);
            }
            if children
                .iter()
                .any(|child| !child.required_resources().is_empty())
            {
                return Err(PlanTreeError::MissingResourceScope);
            }
        }
        let capabilities = match &self.kind {
            PlanNodeKind::ResourceScope {
                resource: ResourcePlan::BrowserContext,
                body,
            } => {
                if body.origin.file != self.origin.file {
                    return Err(PlanTreeError::OriginMismatch);
                }
                let mut child_path = path.to_vec();
                child_path.push(0);
                body.validate(test, revision, &child_path, steps)?;
                let mut capabilities = body.required_capabilities.clone();
                if !capabilities.contains(&Capability::Browser) {
                    capabilities.push(Capability::Browser);
                    capabilities.sort();
                }
                capabilities
            }

            PlanNodeKind::Timeout {
                child,
                duration,
                cleanup_timeout,
            } => {
                if duration.is_zero()
                    || *duration > MAX_CONTROL_TIMEOUT
                    || cleanup_timeout
                        .is_some_and(|value| value.is_zero() || value > MAX_CONTROL_TIMEOUT)
                {
                    return Err(PlanTreeError::InvalidControlSetting);
                }
                if child.origin.file != self.origin.file {
                    return Err(PlanTreeError::OriginMismatch);
                }
                let mut child_path = path.to_vec();
                child_path.push(0);
                child.validate(test, revision, &child_path, steps)?;
                child.required_capabilities.clone()
            }
            PlanNodeKind::Sequence { children } | PlanNodeKind::Parallel { children, .. } => {
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
            if !test.body.required_resources().is_empty() {
                return Err(PlanTreeError::MissingResourceScope);
            }
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
            if !test.body.required_resources().is_empty() {
                return Err(PlanTreeError::MissingResourceScope);
            }
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
        let body = PlanNode::sequence(declaration_id, origin, revision, Vec::new(), children);
        let body = if required_host_capabilities.contains(&Capability::Browser) {
            body.with_browser_resource(declaration_id)
        } else {
            body
        };
        Self {
            id,
            declaration_id,
            name,
            required_host_capabilities,
            body,
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
