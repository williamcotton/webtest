//! Runtime-facing, syntax-independent, serializable test plans.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use webtest_hir::{BinaryOperator, BindingId, StepId, TestId, UnaryOperator};
use webtest_provider::{Capability, Type, Value};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin};

pub const PLAN_FORMAT_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanEnvelope {
    pub format_version: u32,
    pub compiler_version: String,
    pub project_identity: String,
    pub source_files: Vec<PlanSourceFile>,
    pub required_host_capabilities: Vec<Capability>,
    pub provider_schema_hashes: BTreeMap<String, String>,
    pub tests: Vec<PlannedTest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSourceFile {
    pub file: FileId,
    pub path: String,
    pub revision: SourceRevision,
}

impl PlanEnvelope {
    pub fn from_plan(
        plan: &TestPlan,
        path: impl Into<String>,
        project_identity: impl Into<String>,
        provider_schema_hashes: BTreeMap<String, String>,
    ) -> Self {
        Self {
            format_version: PLAN_FORMAT_VERSION,
            compiler_version: env!("CARGO_PKG_VERSION").into(),
            project_identity: project_identity.into(),
            source_files: vec![PlanSourceFile {
                file: plan.file,
                path: path.into(),
                revision: plan.source_revision,
            }],
            required_host_capabilities: plan.required_host_capabilities.clone(),
            provider_schema_hashes,
            tests: plan.tests.clone(),
        }
    }

    pub fn validate_version(&self) -> Result<(), UnsupportedPlanVersion> {
        if self.format_version == PLAN_FORMAT_VERSION {
            Ok(())
        } else {
            Err(UnsupportedPlanVersion {
                found: self.format_version,
                supported: PLAN_FORMAT_VERSION,
            })
        }
    }

    pub fn validate_capabilities(&self) -> Result<(), CapabilityInvariantError> {
        validate_capability_contract(&self.required_host_capabilities, &self.tests)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedPlanVersion {
    pub found: u32,
    pub supported: u32,
}

impl std::fmt::Display for UnsupportedPlanVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "unsupported plan format version {}; this executable supports version {}",
            self.found, self.supported
        )
    }
}

impl std::error::Error for UnsupportedPlanVersion {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestPlan {
    pub file: FileId,
    pub source_revision: SourceRevision,
    pub required_host_capabilities: Vec<Capability>,
    pub tests: Vec<PlannedTest>,
}

impl TestPlan {
    pub fn required_capability_union(tests: &[PlannedTest]) -> Vec<Capability> {
        tests
            .iter()
            .flat_map(|test| test.required_host_capabilities.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn validate_capabilities(&self) -> Result<(), CapabilityInvariantError> {
        validate_capability_contract(&self.required_host_capabilities, &self.tests)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedTest {
    pub id: TestId,
    pub name: String,
    pub required_host_capabilities: Vec<Capability>,
    pub steps: Vec<PlannedStep>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityInvariantError {
    TestCapabilitiesNotSorted {
        test: TestId,
    },
    PlanCapabilitiesNotSorted,
    PlanUnionMismatch {
        declared: Vec<Capability>,
        expected: Vec<Capability>,
    },
}

impl std::fmt::Display for CapabilityInvariantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TestCapabilitiesNotSorted { test } => write!(
                formatter,
                "test {} host capabilities are not sorted and unique",
                test.0
            ),
            Self::PlanCapabilitiesNotSorted => {
                formatter.write_str("plan host capabilities are not sorted and unique")
            }
            Self::PlanUnionMismatch { declared, expected } => write!(
                formatter,
                "plan host capabilities {declared:?} do not equal the per-test union {expected:?}"
            ),
        }
    }
}

impl std::error::Error for CapabilityInvariantError {}

fn validate_capability_contract(
    declared: &[Capability],
    tests: &[PlannedTest],
) -> Result<(), CapabilityInvariantError> {
    if !is_sorted_unique(declared) {
        return Err(CapabilityInvariantError::PlanCapabilitiesNotSorted);
    }
    for test in tests {
        if !is_sorted_unique(&test.required_host_capabilities) {
            return Err(CapabilityInvariantError::TestCapabilitiesNotSorted { test: test.id });
        }
    }
    let expected = TestPlan::required_capability_union(tests);
    if declared != expected {
        return Err(CapabilityInvariantError::PlanUnionMismatch {
            declared: declared.to_vec(),
            expected,
        });
    }
    Ok(())
}

fn is_sorted_unique(capabilities: &[Capability]) -> bool {
    capabilities.windows(2).all(|pair| pair[0] < pair[1])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedStep {
    pub id: StepId,
    pub operation: TestOperation,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "operation", rename_all = "snake_case")]
pub enum TestOperation {
    EvaluatePure(EvaluatePureOperation),
    ServerProviderCall(ServerProviderCall),
    Browser(BrowserOperation),
    Assertion(AssertionOperation),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluatePureOperation {
    pub expression: PlanExpr,
    pub result_binding: Option<BindingId>,
    pub result_name: Option<String>,
    pub result_type: Type,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerProviderCall {
    pub provider: String,
    pub operation: String,
    pub arguments: BTreeMap<String, PlanExpr>,
    pub result_binding: Option<BindingId>,
    pub result_name: Option<String>,
    pub result_type: Type,
    pub schema_hash: String,
    pub timeout: Option<Duration>,
    pub redacted_arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_result_fields: Vec<String>,
    #[serde(default)]
    pub retry_safe: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PlanExpr {
    Literal(Value),
    Binding(BindingId),
    List(Vec<PlanExpr>),
    Record(BTreeMap<String, PlanExpr>),
    Type(Type),
    Member {
        receiver: Box<PlanExpr>,
        member: String,
        missing_is_null: bool,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<PlanExpr>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<PlanExpr>,
        right: Box<PlanExpr>,
    },
    Decode {
        value: Box<PlanExpr>,
        target: Type,
        response_operation: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserOperation {
    Navigate {
        url: PlanExpr,
    },
    Evaluate {
        expression: String,
    },
    Click {
        locator: Locator,
    },
    Fill {
        locator: Locator,
        value: PlanExpr,
    },
    Type {
        locator: Locator,
        value: PlanExpr,
    },
    Press {
        locator: Locator,
        key: PlanExpr,
    },
    Check {
        locator: Locator,
        checked: bool,
    },
    Select {
        locator: Locator,
        option: PlanExpr,
    },
    Hover {
        locator: Locator,
    },
    WaitForLocator {
        locator: Locator,
        state: LocatorState,
        timeout: Option<Duration>,
    },
    WaitForUrl {
        url: PlanExpr,
        timeout: Option<Duration>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssertionOperation {
    Locator {
        locator: Locator,
        state: LocatorState,
        timeout: Option<Duration>,
    },
    Url {
        url: PlanExpr,
        timeout: Option<Duration>,
    },
    Value {
        matcher: ValueMatcher,
        actual: PlanExpr,
        expected: Option<PlanExpr>,
        value_type: Type,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueMatcher {
    Truthy,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Contains,
    Matches,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Locator {
    Id(String),
    Role { role: String, name: Option<String> },
    Label(String),
    Text(String),
    Placeholder(String),
    TestId(String),
    Css(String),
    XPath(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocatorState {
    Visible,
    Hidden,
    Attached,
    Detached,
    Enabled,
    Disabled,
    Checked,
    Unchecked,
}

impl std::fmt::Display for LocatorState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
            Self::Attached => "attached",
            Self::Detached => "detached",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Checked => "checked",
            Self::Unchecked => "unchecked",
        })
    }
}

pub fn locator_from_hir(locator: &webtest_hir::HirLocatorKind) -> Locator {
    match locator {
        webtest_hir::HirLocatorKind::Id(value) => Locator::Id(value.clone()),
        webtest_hir::HirLocatorKind::Role { role, name } => Locator::Role {
            role: role.clone(),
            name: name.clone(),
        },
        webtest_hir::HirLocatorKind::Label(value) => Locator::Label(value.clone()),
        webtest_hir::HirLocatorKind::Text(value) => Locator::Text(value.clone()),
        webtest_hir::HirLocatorKind::Placeholder(value) => Locator::Placeholder(value.clone()),
        webtest_hir::HirLocatorKind::TestId(value) => Locator::TestId(value.clone()),
        webtest_hir::HirLocatorKind::Css(value) => Locator::Css(value.clone()),
        webtest_hir::HirLocatorKind::XPath(value) => Locator::XPath(value.clone()),
    }
}

pub fn locator_state_from_hir(state: webtest_hir::LocatorState) -> LocatorState {
    match state {
        webtest_hir::LocatorState::Visible => LocatorState::Visible,
        webtest_hir::LocatorState::Hidden => LocatorState::Hidden,
        webtest_hir::LocatorState::Attached => LocatorState::Attached,
        webtest_hir::LocatorState::Detached => LocatorState::Detached,
        webtest_hir::LocatorState::Enabled => LocatorState::Enabled,
        webtest_hir::LocatorState::Disabled => LocatorState::Disabled,
        webtest_hir::LocatorState::Checked => LocatorState::Checked,
        webtest_hir::LocatorState::Unchecked => LocatorState::Unchecked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_text::TextRange;

    #[test]
    fn envelope_is_versioned_serializable_and_rejects_unknown_versions() {
        let file = FileId::new(4);
        let revision = SourceRevision::of("test");
        let plan = TestPlan {
            file,
            source_revision: revision,
            required_host_capabilities: vec![Capability::Server],
            tests: vec![PlannedTest {
                id: TestId(0),
                name: "x".into(),
                required_host_capabilities: vec![Capability::Server],
                steps: vec![PlannedStep {
                    id: StepId(0),
                    operation: TestOperation::EvaluatePure(EvaluatePureOperation {
                        expression: PlanExpr::Literal(Value::Int(1)),
                        result_binding: Some(BindingId(0)),
                        result_name: Some("value".into()),
                        result_type: Type::Int,
                    }),
                    origin: SyntaxOrigin::new(file, TextRange::default()),
                }],
                origin: SyntaxOrigin::new(file, TextRange::default()),
            }],
        };
        let envelope = PlanEnvelope::from_plan(&plan, "x.webtest", "project", BTreeMap::new());
        assert_eq!(envelope.format_version, 3);
        assert_eq!(
            envelope.tests[0].required_host_capabilities,
            [Capability::Server]
        );
        assert!(envelope.validate_capabilities().is_ok());
        let encoded = serde_json::to_string(&envelope).expect("serialize plan");
        let decoded: PlanEnvelope = serde_json::from_str(&encoded).expect("deserialize plan");
        assert_eq!(decoded, envelope);
        for version in [1, PLAN_FORMAT_VERSION + 1] {
            let mut unsupported = decoded.clone();
            unsupported.format_version = version;
            assert!(unsupported.validate_version().is_err());
        }

        let mut version_one_shape = serde_json::to_value(&decoded).expect("plan value");
        version_one_shape["format_version"] = serde_json::json!(1);
        version_one_shape["tests"][0]
            .as_object_mut()
            .expect("test object")
            .remove("required_host_capabilities");
        assert!(serde_json::from_value::<PlanEnvelope>(version_one_shape).is_err());
    }

    #[test]
    fn optional_member_fact_is_serialized_deterministically() {
        let expression = PlanExpr::Member {
            receiver: Box::new(PlanExpr::Record(BTreeMap::new())),
            member: "optional".into(),
            missing_is_null: true,
        };
        let first = serde_json::to_string(&expression).expect("serialize member");
        let second = serde_json::to_string(&expression).expect("serialize member again");
        assert_eq!(first, second);
        assert!(first.contains("\"missing_is_null\":true"));
        assert_eq!(
            serde_json::from_str::<PlanExpr>(&first).expect("deserialize member"),
            expression
        );

        let mut old_shape = serde_json::to_value(&expression).expect("member JSON");
        old_shape["value"]
            .as_object_mut()
            .expect("member value")
            .remove("missing_is_null");
        assert!(serde_json::from_value::<PlanExpr>(old_shape).is_err());
    }

    #[test]
    fn capability_contract_requires_sorted_exact_test_union() {
        let file = FileId::new(7);
        let planned_test = |id, capabilities| PlannedTest {
            id: TestId(id),
            name: format!("test-{id}"),
            required_host_capabilities: capabilities,
            steps: Vec::new(),
            origin: SyntaxOrigin::new(file, TextRange::default()),
        };
        let mut plan = TestPlan {
            file,
            source_revision: SourceRevision::of("tests"),
            required_host_capabilities: vec![Capability::Server, Capability::Browser],
            tests: vec![
                planned_test(0, vec![Capability::Server]),
                planned_test(1, vec![Capability::Browser]),
            ],
        };
        assert!(plan.validate_capabilities().is_ok());

        plan.required_host_capabilities = vec![Capability::Browser];
        assert!(matches!(
            plan.validate_capabilities(),
            Err(CapabilityInvariantError::PlanUnionMismatch { .. })
        ));

        plan.required_host_capabilities = vec![Capability::Server, Capability::Browser];
        plan.tests[1].required_host_capabilities = vec![Capability::Browser, Capability::Server];
        assert!(matches!(
            plan.validate_capabilities(),
            Err(CapabilityInvariantError::TestCapabilitiesNotSorted { test: TestId(1) })
        ));
    }
}
