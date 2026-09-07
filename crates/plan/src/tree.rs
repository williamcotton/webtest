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
    /// First successfully completed child wins; every loser is cancelled and joined.
    Race {
        children: Vec<PlanNode>,
        result: Option<crate::RaceBinding>,
    },
    Parallel {
        children: Vec<PlanNode>,
        failure_policy: ParallelFailurePolicy,
    },
    ResourceScope {
        resource: ResourcePlan,
        body: Box<PlanNode>,
    },
    Retry {
        child: Box<PlanNode>,
        settings: crate::RetrySettings,
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

    pub fn race(
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
            kind: PlanNodeKind::Race {
                children,
                result: None,
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
            PlanNodeKind::Sequence { children }
            | PlanNodeKind::Parallel { children, .. }
            | PlanNodeKind::Race { children, .. } => {
                for (ordinal, child) in children.iter_mut().enumerate() {
                    let mut path = self.path.clone();
                    path.push(ordinal as u32);
                    child.rebase(test, path);
                }
            }
            PlanNodeKind::Retry { child, .. }
            | PlanNodeKind::Timeout { child, .. }
            | PlanNodeKind::ResourceScope { body: child, .. } => {
                let mut path = self.path.clone();
                path.push(0);
                child.rebase(test, path);
            }
            PlanNodeKind::Operation { .. } => {}
        }
        self.assign_identity(test);
    }

    pub fn retry(
        test: PlanDeclarationId,
        origin: SyntaxOrigin,
        revision: SourceRevision,
        path: Vec<u32>,
        child: Self,
        settings: crate::RetrySettings,
    ) -> Self {
        let mut node = Self {
            id: PlanNodeId([0; 32]),
            path,
            origin,
            source_revision: revision,
            required_capabilities: child.required_capabilities.clone(),
            kind: PlanNodeKind::Retry {
                child: Box::new(child),
                settings,
            },
        };
        node.assign_identity(test);
        node
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
            PlanNodeKind::Race { .. } => "race/v2",
            PlanNodeKind::Sequence { .. } => "sequence/v1",
            PlanNodeKind::Timeout { .. } => "timeout/v1",
            PlanNodeKind::Retry { .. } => "retry/v1",
            PlanNodeKind::Operation { step } => match &step.operation {
                crate::TestOperation::EvaluatePure(_) => "eval/v1",
                crate::TestOperation::Provide(_) => "provide/v1",
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
            PlanNodeKind::Retry { child, .. } | PlanNodeKind::Timeout { child, .. } => {
                child.steps()
            }
            PlanNodeKind::Sequence { children }
            | PlanNodeKind::Parallel { children, .. }
            | PlanNodeKind::Race { children, .. } => {
                children.iter().flat_map(Self::steps).collect()
            }
            PlanNodeKind::Operation { step } => vec![step],
        }
    }

    pub fn steps_mut(&mut self) -> Vec<&mut PlannedStep> {
        match &mut self.kind {
            PlanNodeKind::ResourceScope { body, .. } => body.steps_mut(),
            PlanNodeKind::Retry { child, .. } | PlanNodeKind::Timeout { child, .. } => {
                child.steps_mut()
            }
            PlanNodeKind::Sequence { children }
            | PlanNodeKind::Parallel { children, .. }
            | PlanNodeKind::Race { children, .. } => {
                children.iter_mut().flat_map(Self::steps_mut).collect()
            }
            PlanNodeKind::Operation { step } => vec![step],
        }
    }
}

fn operation_capabilities(operation: &crate::TestOperation) -> Vec<Capability> {
    use crate::{AssertionOperation, TestOperation};
    match operation {
        TestOperation::EvaluatePure(_) | TestOperation::Provide(_) => vec![],
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
    InvalidRaceResult,
    UnsafeRetry,
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
        if let PlanNodeKind::Parallel { children, .. } | PlanNodeKind::Race { children, .. } =
            &self.kind
        {
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

            PlanNodeKind::Retry { child, settings } => {
                if !settings.is_valid() {
                    return Err(PlanTreeError::InvalidControlSetting);
                }
                if child.origin.file != self.origin.file {
                    return Err(PlanTreeError::OriginMismatch);
                }
                let mut child_path = path.to_vec();
                child_path.push(0);
                child.validate(test, revision, &child_path, steps)?;
                if !child.retry_safety_violations().is_empty() {
                    return Err(PlanTreeError::UnsafeRetry);
                }
                child.required_capabilities.clone()
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
            PlanNodeKind::Sequence { children }
            | PlanNodeKind::Parallel { children, .. }
            | PlanNodeKind::Race { children, .. } => {
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

impl PlanNode {
    /// Result flow is lexical; nested concurrency cannot provide into its parent branch.
    fn validate_results(
        &self,
        allowed: bool,
        expected: Option<&webtest_model::Type>,
    ) -> Result<bool, PlanTreeError> {
        match &self.kind {
            PlanNodeKind::Operation { step } => {
                if let crate::TestOperation::Provide(value) = &step.operation {
                    if !allowed
                        || !value.result_type.is_transferable()
                        || expected.is_some_and(|ty| !ty.accepts(&value.result_type))
                    {
                        return Err(PlanTreeError::InvalidRaceResult);
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            PlanNodeKind::Sequence { children } => {
                let mut provided = false;
                for child in children {
                    if provided {
                        return Err(PlanTreeError::InvalidRaceResult);
                    }
                    provided = child.validate_results(allowed, expected)?;
                }
                Ok(provided)
            }
            PlanNodeKind::Retry { child, .. }
            | PlanNodeKind::Timeout { child, .. }
            | PlanNodeKind::ResourceScope { body: child, .. } => {
                child.validate_results(allowed, expected)
            }
            PlanNodeKind::Parallel { children, .. } => {
                for child in children {
                    child.validate_results(false, None)?;
                }
                Ok(false)
            }
            PlanNodeKind::Race { children, result } => {
                if result
                    .as_ref()
                    .is_some_and(|binding| !binding.ty.is_transferable() || binding.name.is_empty())
                {
                    return Err(PlanTreeError::InvalidRaceResult);
                }
                for child in children {
                    let provided =
                        child.validate_results(true, result.as_ref().map(|binding| &binding.ty))?;
                    if result.is_some() && !provided {
                        return Err(PlanTreeError::InvalidRaceResult);
                    }
                }
                Ok(false)
            }
        }
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
            test.body.validate_results(false, None)?;
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
            test.body.validate_results(false, None)?;
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

#[cfg(test)]
mod control_tests {
    use super::*;
    use webtest_model::{StepId, Value};
    use webtest_text::{FileId, TextRange, TextSize};

    #[test]
    fn retry_nodes_validate_repeatability_bounds_identity_and_serialization() {
        use crate::{RetryBackoff, RetryPolicy, RetrySettings, TestOperation};
        let test = declaration_identity("retry.webtest", "retry", 0);
        let origin = SyntaxOrigin::new(FileId::new(1), TextRange::default());
        let revision = SourceRevision::of("retry");
        let settings = RetrySettings {
            attempts: 3,
            backoff: RetryBackoff::default(),
            policy: RetryPolicy::SafeFailures,
        };
        let child = branch(test, origin, revision, 0, false);
        let mut node = PlanNode::retry(test, origin, revision, vec![0], child.clone(), settings);
        node.validate(test, revision, &[0], &mut Default::default())
            .unwrap();
        let timeout = PlanNode::timeout(
            test,
            origin,
            revision,
            vec![0],
            child,
            std::time::Duration::from_secs(1),
        );
        assert_ne!(node.id, timeout.id);
        assert_eq!(
            serde_json::from_str::<PlanNode>(&serde_json::to_string(&node).unwrap()).unwrap(),
            node
        );
        assert_eq!(node.steps()[0].origin, origin);
        for attempts in [0, crate::MAX_RETRY_ATTEMPTS + 1] {
            if let PlanNodeKind::Retry { settings, .. } = &mut node.kind {
                settings.attempts = attempts;
            }
            assert_eq!(
                node.validate(test, revision, &[0], &mut Default::default()),
                Err(PlanTreeError::InvalidControlSetting)
            );
        }
        let PlanNodeKind::Retry { child, settings } = &mut node.kind else {
            unreachable!()
        };
        settings.attempts = 3;
        let call = crate::ServerProviderCall {
            provider: "test".into(),
            operation: "call".into(),
            arguments: Default::default(),
            result_binding: None,
            result_name: None,
            result_type: webtest_model::Type::Null,
            schema_hash: "schema".into(),
            timeout: None,
            redacted_arguments: vec![],
            redacted_result_fields: vec![],
            retry_safe: false,
        };
        **child = PlanNode::operation(
            test,
            revision,
            vec![0, 0],
            PlannedStep {
                id: StepId(0),
                origin,
                operation: TestOperation::ServerProviderCall(call),
            },
        );
        node.required_capabilities = vec![Capability::Server];
        assert_eq!(
            node.retry_safety_violations(),
            [crate::RetrySafetyViolation { origin }]
        );
        assert_eq!(
            node.validate(test, revision, &[0], &mut Default::default()),
            Err(PlanTreeError::UnsafeRetry)
        );
        let TestOperation::ServerProviderCall(call) = &mut node.steps_mut()[0].operation else {
            unreachable!()
        };
        call.retry_safe = true;
        node.validate(test, revision, &[0], &mut Default::default())
            .unwrap();
        // A containing acquisition is repeatable; it does not make arbitrary
        // browser mutations repeatable or hide them from the effect summary.
        let mutation = branch(test, origin, revision, 0, true).with_browser_resource(test);
        assert_eq!(mutation.retry_safety_violations().len(), 1);
        let wait = TestOperation::Browser(crate::BrowserOperation::WaitForUrl {
            url: crate::PlanExpr::Literal(Value::String("/ready".into())),
            timeout: None,
        });
        assert!(wait.is_retry_safe());
    }
    fn branch(
        test: PlanDeclarationId,
        origin: SyntaxOrigin,
        revision: SourceRevision,
        ordinal: u32,
        browser: bool,
    ) -> PlanNode {
        PlanNode::operation(
            test,
            revision,
            vec![0, ordinal],
            PlannedStep {
                id: StepId(ordinal),
                origin,
                operation: if browser {
                    crate::TestOperation::Browser(crate::BrowserOperation::Evaluate {
                        expression: "ok".into(),
                    })
                } else {
                    crate::TestOperation::EvaluatePure(crate::EvaluatePureOperation {
                        expression: crate::PlanExpr::Literal(Value::Null),
                        result_binding: None,
                        result_name: None,
                        result_type: webtest_model::Type::Null,
                    })
                },
            },
        )
    }

    #[test]
    fn race_has_distinct_stable_identity_round_trips_and_projects_steps_in_source_order() {
        let test = declaration_identity("race.webtest", "race", 0);
        let origin = SyntaxOrigin::new(
            FileId::new(1),
            TextRange::new(TextSize::from(2), TextSize::from(20)),
        );
        let revision = SourceRevision::of("race source");
        let children: Vec<_> = (0..2)
            .map(|ordinal| branch(test, origin, revision, ordinal, false))
            .collect();
        let race = PlanNode::race(test, origin, revision, vec![0], children.clone());
        let parallel = PlanNode::parallel(test, origin, revision, vec![0], children.clone());
        assert_ne!(race.id, parallel.id);
        assert_eq!(
            race,
            PlanNode::race(test, origin, revision, vec![0], children)
        );
        assert_eq!(
            race.steps().iter().map(|step| step.id).collect::<Vec<_>>(),
            [StepId(0), StepId(1)]
        );
        let json = serde_json::to_value(&race).unwrap();
        assert_eq!(json["kind"]["kind"], "race");
        assert_eq!(serde_json::from_value::<PlanNode>(json).unwrap(), race);
        race.validate(test, revision, &[0], &mut Default::default())
            .unwrap();
        assert_eq!(race.origin, origin);
    }

    #[test]
    fn race_plan_validation_enforces_lexical_provide_and_bound_result_types() {
        let test = declaration_identity("race.webtest", "race", 0);
        let origin = SyntaxOrigin::new(FileId::new(1), TextRange::default());
        let revision = SourceRevision::of("race");
        let mut child = branch(test, origin, revision, 0, false);
        let PlanNodeKind::Operation { step } = &mut child.kind else {
            panic!("operation")
        };
        step.operation = crate::TestOperation::Provide(crate::ProvideOperation {
            expression: crate::PlanExpr::Literal(Value::Int(7)),
            result_type: webtest_model::Type::Int,
        });
        assert_eq!(
            child.validate_results(false, None),
            Err(PlanTreeError::InvalidRaceResult)
        );
        let mut node = PlanNode::race(test, origin, revision, vec![0], vec![child]);
        node.validate_results(false, None).unwrap();
        if let PlanNodeKind::Race { result, .. } = &mut node.kind {
            *result = Some(crate::RaceBinding {
                id: webtest_model::BindingId(0),
                name: "selected".into(),
                ty: webtest_model::Type::String,
            });
        }
        assert_eq!(
            node.validate_results(false, None),
            Err(PlanTreeError::InvalidRaceResult)
        );
        if let PlanNodeKind::Race { result, .. } = &mut node.kind {
            result.as_mut().unwrap().ty = webtest_model::Type::Int;
        }
        node.validate_results(false, None).unwrap();
        if let PlanNodeKind::Race { children, .. } = &mut node.kind {
            children.push(branch(test, origin, revision, 1, false));
        }
        assert_eq!(
            node.validate_results(false, None),
            Err(PlanTreeError::InvalidRaceResult)
        );
    }

    #[test]
    fn race_reuses_concurrency_bounds_and_requires_branch_local_exclusive_resources() {
        let test = declaration_identity("race.webtest", "race", 0);
        let origin = SyntaxOrigin::new(FileId::new(1), TextRange::default());
        let revision = SourceRevision::of("race source");
        for count in [0, MAX_PARALLEL_BRANCHES + 1] {
            let children = (0..count)
                .map(|ordinal| branch(test, origin, revision, ordinal as u32, false))
                .collect();
            let node = PlanNode::race(test, origin, revision, vec![0], children);
            assert_eq!(
                node.validate(test, revision, &[0], &mut Default::default()),
                Err(PlanTreeError::InvalidControlSetting)
            );
        }
        for count in [1, 2] {
            let children: Vec<_> = (0..count)
                .map(|ordinal| branch(test, origin, revision, ordinal, true))
                .collect();
            let node = PlanNode::race(test, origin, revision, vec![0], children.clone());
            assert_eq!(
                node.validate(test, revision, &[0], &mut Default::default()),
                Err(if count == 1 {
                    PlanTreeError::MissingResourceScope
                } else {
                    PlanTreeError::ResourceAccessConflict
                })
            );
            let owned = children
                .into_iter()
                .map(|child| child.with_browser_resource(test))
                .collect();
            let node = PlanNode::race(test, origin, revision, vec![0], owned);
            node.validate(test, revision, &[0], &mut Default::default())
                .unwrap();
            assert!(node.required_resources().is_empty());
        }
    }
}
