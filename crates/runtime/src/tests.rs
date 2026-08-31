use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use webtest_browser::{
    BrowserError, BrowserHost, BrowserSession, InspectionOptions, Locator as BrowserLocator, Page,
    PageInspection, RepairHintKind,
};
use webtest_hir::{BindingId, StepId, TestId};
use webtest_observation::ObservationStore;
use webtest_plan::{
    BrowserOperation, Locator, PlanExpr, PlannedStep, PlannedTest, ServerProviderCall,
    TestOperation, TestPlan,
};
use webtest_provider::{Capability, Type, Value};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

use crate::{
    Runner, StepError, TestOutcome,
    execution::repair_hints_for_error,
    redaction::{redact_step_error, visible_step_bindings},
};

struct FakeHost {
    result: Result<(), BrowserError>,
    starts: Arc<AtomicUsize>,
}

struct FakeSession {
    result: Result<(), BrowserError>,
}

struct FakePage {
    result: Mutex<Result<(), BrowserError>>,
}

fn semantic_inspection() -> PageInspection {
    PageInspection {
        kind: "inspection".into(),
        inspection_schema_version: webtest_browser::INSPECTION_SCHEMA_VERSION,
        snapshot_id: "fake-snapshot".into(),
        browser_version: "fake".into(),
        page: webtest_browser::PageSummary {
            url: "http://example.test/login".into(),
            title: "Sign in".into(),
        },
        elements: vec![webtest_browser::InspectableElement {
            role: Some("button".into()),
            accessible_name: Some("Sign in".into()),
            label: None,
            placeholder: None,
            test_id: None,
            dom_id: None,
            states: webtest_browser::ElementStates {
                visible: true,
                enabled: Some(true),
                receives_pointer_input: Some(true),
                ..webtest_browser::ElementStates::default()
            },
            supported_actions: vec![webtest_browser::SupportedAction::Click],
            preferred_locator: webtest_browser::LocatorCandidate {
                source: "role(\"button\", name: \"Sign in\")".into(),
                kind: webtest_browser::LocatorCandidateKind::Role,
                reason: "unique accessible role and name".into(),
            },
            alternate_locators: Vec::new(),
            options: Vec::new(),
        }],
        truncation: webtest_browser::InspectionTruncation::default(),
    }
}

#[async_trait]
impl BrowserHost for FakeHost {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
        self.starts.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(FakeSession {
            result: self.result.clone(),
        }))
    }
}

#[async_trait]
impl BrowserSession for FakeSession {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        Ok(Box::new(FakePage {
            result: Mutex::new(self.result.clone()),
        }))
    }
}

#[async_trait]
impl Page for FakePage {
    async fn open(&mut self, _url: &str) -> Result<(), BrowserError> {
        Ok(())
    }

    async fn click(&mut self, _locator: &BrowserLocator) -> Result<(), BrowserError> {
        self.result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn expect_visible(&mut self, _locator: &BrowserLocator) -> Result<(), BrowserError> {
        self.result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn evaluate(&mut self, _expression: &str) -> Result<(), BrowserError> {
        self.result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn inspect(
        &mut self,
        _options: &InspectionOptions,
    ) -> Result<PageInspection, BrowserError> {
        Ok(semantic_inspection())
    }
}

fn plan(revision: SourceRevision) -> TestPlan {
    let file = FileId::new(0);
    TestPlan {
        file,
        source_revision: revision,
        required_host_capabilities: vec![Capability::Browser],
        tests: vec![PlannedTest {
            id: TestId(0),
            name: "x".into(),
            origin: SyntaxOrigin::new(file, TextRange::empty(TextSize::new(0))),
            steps: vec![PlannedStep {
                id: StepId(0),
                origin: SyntaxOrigin::new(
                    file,
                    TextRange::new(TextSize::new(10), TextSize::new(19)),
                ),
                operation: TestOperation::Browser(BrowserOperation::Click {
                    locator: Locator::Id("missing".into()),
                }),
            }],
        }],
    }
}

#[test]
fn debugger_step_bindings_include_evaluated_redacted_provider_arguments() {
    let file = FileId::new(0);
    let step = PlannedStep {
        id: StepId(0),
        origin: SyntaxOrigin::new(file, TextRange::empty(TextSize::new(0))),
        operation: TestOperation::ServerProviderCall(ServerProviderCall {
            provider: "app".into(),
            operation: "create_user".into(),
            arguments: [
                (
                    "email".into(),
                    PlanExpr::Literal(Value::String("alice@example.test".into())),
                ),
                (
                    "token".into(),
                    PlanExpr::Literal(Value::String("private".into())),
                ),
            ]
            .into_iter()
            .collect(),
            result_binding: None,
            result_name: None,
            result_type: Type::String,
            schema_hash: "schema".into(),
            timeout: None,
            redacted_arguments: vec!["token".into()],
            redacted_result_fields: Vec::new(),
            retry_safe: false,
        }),
    };
    let visible = visible_step_bindings(&step, &HashMap::new(), &HashMap::new(), &[], &[]);
    assert_eq!(
        visible.get("argument.email"),
        Some(&Value::String("alice@example.test".into()))
    );
    assert_eq!(
        visible.get("argument.token"),
        Some(&Value::String("[redacted]".into()))
    );
}

#[test]
fn debugger_step_bindings_keep_server_values_visible_in_later_steps() {
    let file = FileId::new(0);
    let step = PlannedStep {
        id: StepId(2),
        origin: SyntaxOrigin::new(file, TextRange::empty(TextSize::new(0))),
        operation: TestOperation::Browser(BrowserOperation::Navigate {
            url: PlanExpr::Literal(Value::String("/login".into())),
        }),
    };
    let response_id = BindingId(0);
    let user_id = BindingId(1);
    let environment = HashMap::from([
        (
            response_id,
            Value::Response(webtest_provider::ResponseValue {
                status: 201,
                headers: [("authorization".into(), "Bearer private".into())]
                    .into_iter()
                    .collect(),
                body: br#"{"id":7,"token":"private"}"#.to_vec(),
                json: Some(Box::new(Value::Record(
                    [
                        ("id".into(), Value::Int(7)),
                        ("token".into(), Value::String("private".into())),
                    ]
                    .into_iter()
                    .collect(),
                ))),
            }),
        ),
        (
            user_id,
            Value::Record(
                [
                    ("email".into(), Value::String("alice@example.test".into())),
                    ("id".into(), Value::Int(7)),
                ]
                .into_iter()
                .collect(),
            ),
        ),
    ]);
    let names = HashMap::from([(response_id, "response".into()), (user_id, "user".into())]);
    let visible = visible_step_bindings(
        &step,
        &environment,
        &names,
        &["authorization".into(), "token".into()],
        &["private".into()],
    );

    let Value::Response(response) = &visible["response"] else {
        panic!("response should remain visible to the debugger");
    };
    assert_eq!(response.status, 201);
    assert_eq!(response.headers["authorization"], "[redacted]");
    assert!(!String::from_utf8_lossy(&response.body).contains("private"));
    assert_eq!(
        visible["user"].member("email"),
        Some(Value::String("alice@example.test".into()))
    );
}

#[tokio::test]
async fn failure_records_revision_bound_observation_and_success_clears_it() {
    let store = Arc::new(ObservationStore::default());
    let runner = Runner::new(Arc::clone(&store));
    let revision = SourceRevision::of("source");
    let starts = Arc::new(AtomicUsize::new(0));
    let failed = FakeHost {
        result: Err(BrowserError::LocatorNotFound {
            locator: BrowserLocator::Id("missing".into()),
        }),
        starts: Arc::clone(&starts),
    };
    let failed_result = runner.run(&plan(revision), &failed).await;
    assert_eq!(failed_result.failed(), 1);
    let TestOutcome::Failed(failure) = &failed_result.tests[0].outcome else {
        panic!("failed outcome")
    };
    assert_eq!(
        failure.inspection.as_ref().expect("inspection").page.title,
        "Sign in"
    );
    assert!(!failure.repair_hints.is_empty());
    assert_eq!(
        failure.repair_hints[0].source_range,
        Some(webtest_feedback::ByteRange { start: 10, end: 19 })
    );
    assert_eq!(store.observations_for(FileId::new(0), revision).len(), 1);
    let passed = FakeHost {
        result: Ok(()),
        starts,
    };
    runner.run(&plan(revision), &passed).await;
    assert!(store.observations_for(FileId::new(0), revision).is_empty());
}

#[test]
fn repair_hints_are_failure_specific_bounded_and_never_claim_actionability_healing() {
    let inspection = semantic_inspection();
    let requested = BrowserLocator::Role {
        role: "button".into(),
        name: Some("Log in".into()),
    };
    let ambiguous = repair_hints_for_error(
        &StepError::Browser(BrowserError::LocatorAmbiguous {
            locator: requested,
            matches: 2,
        }),
        &inspection,
    );
    assert_eq!(ambiguous[0].kind, RepairHintKind::LocatorCandidate);

    let mut option_inspection = inspection.clone();
    let element = &mut option_inspection.elements[0];
    element.preferred_locator = webtest_browser::LocatorCandidate {
        source: "label(\"Timezone\")".into(),
        kind: webtest_browser::LocatorCandidateKind::Label,
        reason: "unique associated label".into(),
    };
    element.options = vec![
        "Zulu".into(),
        "UTC".into(),
        "UCT".into(),
        "GMT".into(),
        "CST".into(),
        "EST".into(),
    ];
    let options = repair_hints_for_error(
        &StepError::Browser(BrowserError::OptionNotFound {
            locator: BrowserLocator::Label("Timezone".into()),
            option: "UT".into(),
        }),
        &option_inspection,
    );
    assert_eq!(options.len(), webtest_browser::MAX_CANDIDATES);
    assert_eq!(options[0].kind, RepairHintKind::OptionCandidate);
    assert_eq!(
        options[0].replacement,
        webtest_browser::RepairReplacement::text("UCT")
    );

    let url = repair_hints_for_error(
        &StepError::Browser(BrowserError::UrlMismatch {
            expected: "http://example.test/home".into(),
            actual: "http://example.test/dashboard".into(),
        }),
        &inspection,
    );
    assert_eq!(url[0].kind, RepairHintKind::NameCandidate);

    let actionability = repair_hints_for_error(
        &StepError::Browser(BrowserError::ElementDisabled {
            locator: BrowserLocator::Id("submit".into()),
        }),
        &inspection,
    );
    assert!(actionability.is_empty());

    let redacted = redact_step_error(
        StepError::Browser(BrowserError::UrlMismatch {
            expected: "http://example.test/?token=must-not-leak".into(),
            actual: "http://example.test/?token=private&view=secret-value".into(),
        }),
        &[],
        &["secret-value".into()],
        &["token".into()],
    );
    let rendered = redacted.to_string();
    assert!(!rendered.contains("must-not-leak"));
    assert!(!rendered.contains("private"));
    assert!(!rendered.contains("secret-value"));
    let hints = repair_hints_for_error(&redacted, &inspection);
    assert!(!format!("{hints:?}").contains("private"));
}
