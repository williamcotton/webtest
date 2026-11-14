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
use webtest_model::{BindingId, Capability, StepId, TestId, Type, Value};
use webtest_observation::ObservationStore;
use webtest_plan::{
    BrowserOperation, Locator, PlanExpr, PlannedStep, PlannedTest, ServerProviderCall,
    TestOperation, TestPlan,
};
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
    observer: Option<Arc<dyn webtest_browser::reactive::ReactiveObserver>>,
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
            observer: None,
            result: Mutex::new(self.result.clone()),
        }))
    }
}

#[async_trait]
impl Page for FakePage {
    async fn start_reactive(
        &mut self,
        observer: Arc<dyn webtest_browser::reactive::ReactiveObserver>,
        _: &[webtest_model::EventSource],
    ) -> Result<(), BrowserError> {
        self.observer = Some(observer);
        Ok(())
    }
    async fn reactive_barrier(&mut self) -> Result<(), BrowserError> {
        Ok(())
    }
    async fn open(&mut self, _url: &str) -> Result<(), BrowserError> {
        if let Some(observer) = &self.observer {
            observer.observe(webtest_browser::reactive::BrowserEvent{source:webtest_model::EventSource::ConsoleMessage,payload:webtest_model::value_from_json(serde_json::json!({"level":"error","message":if _url.ends_with("/first"){"first"}else if _url.ends_with("/second"){"second"}else{"boom"},"url":{"value":"","path":"","query":"","host":"","scheme":""},"line":0,"column":0,"timestamp":0.0})),captured_bytes:100,truncated:false});
        }
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
        fixtures: Vec::new(),
        source_files: Vec::new(),
        file,
        source_revision: revision,
        required_host_capabilities: vec![Capability::Browser],
        tests: vec![PlannedTest::sequential(
            TestId(0),
            webtest_plan::declaration_identity("runtime.webtest", "x", 0),
            "x".into(),
            vec![Capability::Browser],
            SyntaxOrigin::new(file, TextRange::empty(TextSize::new(0))),
            revision,
            vec![PlannedStep {
                origin_chain: Vec::new(),
                id: StepId(0),
                origin: SyntaxOrigin::new(
                    file,
                    TextRange::new(TextSize::new(10), TextSize::new(19)),
                ),
                operation: TestOperation::Browser(BrowserOperation::Click {
                    locator: Locator::Id("missing".into()),
                }),
            }],
        )],
    }
}

#[test]
fn debugger_step_bindings_include_evaluated_redacted_provider_arguments() {
    let file = FileId::new(0);
    let step = PlannedStep {
        origin_chain: Vec::new(),
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
        origin_chain: Vec::new(),
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
            Value::Response(webtest_model::ResponseValue {
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

#[tokio::test]
async fn imported_pure_helpers_evaluate_parameters_once_and_preserve_short_circuit() {
    let mut database = webtest_analysis::AnalysisDatabase::default();
    database.configure_workspace(webtest_analysis::WorkspaceConfiguration {
        root: "/project".into(),
        ..Default::default()
    });
    database.open_file(
        "/project/support.webtest",
        r#"
        export fn email(name: String) -> String = lower(name) + "@example.com"
        export fn divide(value: Int) -> Float = 10 / value
        export fn twice(value: Float) -> Float = value + value
        export fn record(value: Int) -> { value: Int } = { value: value }
    "#,
    );
    let file = database.open_file(
        "/project/test.webtest",
        r#"
        import { email, divide, twice, record } from "./support"
        test "helpers" {
            let value = 3
            expect email(name: "ALICE") == "alice@example.com"
            expect record(7) matches { value: Int }
            expect twice(divide(value: value + 2)) == 4
            expect (false && divide(0) == 1) == false
        }
    "#,
    );
    let diagnostics = database.diagnostics(file).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let plan = database.test_plan(file).unwrap();
    let store = Arc::new(ObservationStore::default());
    let runner = Runner::new(store);
    let starts = Arc::new(AtomicUsize::new(0));
    let host = FakeHost {
        result: Ok(()),
        starts: Arc::clone(&starts),
    };
    let result = runner.run(&plan, &host).await;
    assert!(
        result
            .tests
            .iter()
            .all(|test| matches!(test.outcome, TestOutcome::Passed)),
        "{result:?}"
    );
    assert_eq!(starts.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn statement_helpers_keep_isolated_bindings_and_cross_file_scope_revisions() {
    let mut database = webtest_analysis::AnalysisDatabase::default();
    database.configure_workspace(webtest_analysis::WorkspaceConfiguration {
        root: "/project".into(),
        ..Default::default()
    });
    let library = r#"
        export fn add(value: Int) -> Int { let other = value + 1 provide other }
        export fn twice(value: Int) -> Int { let first = add(value) let second = add(first) provide second }
    "#;
    let lib = database.open_file("/project/support.webtest", library);
    let file = database.open_file("/project/test.webtest",r#"
        import { twice } from "./support"
        test "helpers" { let value = 9 let result = twice(value: value) expect result == 11 expect value == 9 }
    "#);
    let diagnostics = database.diagnostics(file).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let plan = database.test_plan(file).unwrap();
    assert!(plan.validate_tree().is_ok(), "{:?}", plan.validate_tree());
    let runner = Runner::new(Arc::new(ObservationStore::default()));
    let host = FakeHost {
        result: Ok(()),
        starts: Arc::new(AtomicUsize::new(0)),
    };
    let result = runner.run(&plan, &host).await;
    assert!(
        result
            .tests
            .iter()
            .all(|test| matches!(test.outcome, TestOutcome::Passed)),
        "{result:?}"
    );
    assert!(result.journal.iter().any(|record| {
        record
            .metadata
            .origin
            .is_some_and(|origin| origin.file == lib)
            && record.metadata.source_revision == Some(SourceRevision::of(library))
    }));
    let mut stale = (*plan).clone();
    stale
        .source_files
        .iter_mut()
        .find(|source| source.file == lib)
        .unwrap()
        .revision = SourceRevision::of("changed");
    assert!(stale.validate_tree().is_err());
}

#[tokio::test]
async fn cases_patterns_checks_and_semantic_verdicts_use_the_shared_runtime() {
    let mut db = webtest_analysis::AnalysisDatabase::default();
    let source = r#"pattern User(email: String) = subset {id: range(1,99), email: email, password: absent}
    test "cases" cases [case "one" {id: 1}, case "two" {id: 2}] as input { expect {id: input.id, email: "a"} matches User("a") }
    test "check" { check {expect 1 == 2 fail "second" expect "abc" matches regex("^z$")} expect true }
    test "skip" { assume false else skip "not applicable" fail "unreachable" }
    test "undecided" { inconclusive "external precondition" }
    test "early success" { pass fail "unreachable" }"#;
    let file = db.open_file("h.webtest", source);
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file).unwrap()
    );
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let host = FakeHost {
        result: Ok(()),
        starts: Arc::new(AtomicUsize::new(0)),
    };
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &host)
        .await;
    assert_eq!(result.tests.len(), 6);
    assert_eq!(result.passed(), 3);
    assert_eq!(result.failed(), 2);
    assert_eq!(result.skipped(), 1);
    assert_eq!(host.starts.load(Ordering::Relaxed), 0);
    let TestOutcome::Failed(failure) = &result.tests[2].outcome else {
        panic!("check result: {:?}", result.tests[2].outcome);
    };
    let StepError::Assertion(assertion) = &failure.error else {
        panic!("check aggregate");
    };
    let webtest_observation::ValueDiff::Check {
        failures, total, ..
    } = &assertion.diff
    else {
        panic!("typed aggregate diff");
    };
    assert_eq!(*total, 3);
    assert_eq!(failures.len(), 3);
    assert!(matches!(
        result.tests[3].outcome,
        TestOutcome::Verdict {
            kind: webtest_model::VerdictKind::Skipped,
            ..
        }
    ));
    assert!(matches!(
        result.tests[4].outcome,
        TestOutcome::Verdict {
            kind: webtest_model::VerdictKind::Inconclusive,
            ..
        }
    ));
    assert!(matches!(
        result.tests[5].outcome,
        TestOutcome::Verdict {
            kind: webtest_model::VerdictKind::Pass,
            ..
        }
    ));
}

#[tokio::test]
async fn explicit_actors_have_separate_owned_pages_and_static_exclusive_access() {
    let mut db = webtest_analysis::AnalysisDatabase::default();
    let file = db.open_file(
        "actors.webtest",
        r#"test "actors" {
        actor alice browser
        actor bob browser
        parallel { alice {open "http://alice.test"} bob {open "http://bob.test"} }
        alice {expect true}
    }"#,
    );
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file).unwrap()
    );
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let host = FakeHost {
        result: Ok(()),
        starts: Arc::new(AtomicUsize::new(0)),
    };
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &host)
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests);
    assert_eq!(host.starts.load(Ordering::Relaxed), 2);
    db.set_file_text(file,r#"test "conflict" {actor alice browser parallel {alice {expect true} alice {expect true}}}"#);
    assert!(
        db.diagnostics(file)
            .unwrap()
            .iter()
            .any(|d| d.code == "semantic.concurrent_resource_conflict")
    );
}

#[tokio::test]
async fn checkpoints_replay_synchronous_stimulus_events_without_running_other_alternatives() {
    let mut db = webtest_analysis::AnalysisDatabase::default();
    let file=db.open_file("select.webtest",r#"test "replay" {
        actor alice browser
        let mark = checkpoint(alice)
        alice {open "http://example.test"}
        let result = select alice since mark within 10ms {
            when console.error(e) if e.message == "boom" {provide e.message}
            when console.message(e) {fail "second alternative must not execute"}
            timeout {fail "event was lost"}
        }
        expect result == "boom"
    }
    test "timeout" {actor alice browser select alice within 1ms {when console.error(e) {fail e.message} timeout {skip "no event"}}}
    test "timeout failure" {actor alice browser select alice within 1ms {when console.error(e) {fail e.message}}}
    test "unmatched evidence" {actor alice browser let mark = checkpoint(alice) alice {open "http://example.test"} select alice since mark within 1ms {when console.error(e) if false {fail e.message} timeout {skip "unmatched"}}}
    "#);
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file).unwrap()
    );
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(crate::RunnerOptions {
            reactive: webtest_model::ReactiveLimits {
                max_unmatched_evidence: 0,
                ..Default::default()
            },
            ..Default::default()
        })
        .run(
            &plan,
            &FakeHost {
                result: Ok(()),
                starts: Arc::new(AtomicUsize::new(0)),
            },
        )
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests);
    assert_eq!(result.skipped(), 2);
    assert_eq!(result.failed(), 1);
    assert!(result.events.iter().any(|event| matches!(event, webtest_observation::ExecutionEvent::Semantic {event:webtest_observation::SemanticEvent::SelectionObserved {unmatched:1,unmatched_sequences,truncated:true,..},..} if unmatched_sequences.is_empty())));
    assert!(
        matches!(&result.tests[2].outcome,TestOutcome::Failed(failure) if failure.error.code()==webtest_observation::RuntimeFailureCode::SelectionTimeout)
    );
}

#[tokio::test]
async fn guards_observe_body_final_poll_and_end_with_their_lexical_scope() {
    let mut db = webtest_analysis::AnalysisDatabase::default();
    let file=db.open_file("guards.webtest",r#"
        guard clean(actor: BrowserActor) {when actor.console.error(e) {fail e.message}}
        guard undecided(actor: BrowserActor) {when actor.console.error(e) {inconclusive e.message}}
        test "guard catches final poll" {actor alice browser with guards [clean(alice)] {alice {open "http://example.test"}} fail "unreachable"}
        test "guard ends" {actor alice browser with guards [clean(alice)] {expect true} alice {open "http://example.test"} expect true}
        test "guard verdict" {actor alice browser with guards [undecided(alice)] {alice {open "http://example.test"}}}
    "#);
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file).unwrap()
    );
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(
            &plan,
            &FakeHost {
                result: Ok(()),
                starts: Arc::new(AtomicUsize::new(0)),
            },
        )
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests);
    assert_eq!(result.failed(), 2);
    assert!(
        matches!(&result.tests[0].outcome,TestOutcome::Failed(failure) if failure.error.to_string()=="boom")
    );
    assert!(matches!(
        &result.tests[2].outcome,
        TestOutcome::Verdict {
            kind: webtest_model::VerdictKind::Inconclusive,
            ..
        }
    ));
    db.set_file_text(file,r#"guard bad(actor: BrowserActor){when actor.console.error(e){server {http.get("http://example.test")} fail e.message}}"#);
    assert!(
        db.diagnostics(file)
            .unwrap()
            .iter()
            .any(|d| d.code == "semantic.guard_effect_not_allowed")
    );
}

#[tokio::test]
async fn guard_order_follows_event_ingress_before_clause_poll_order() {
    let mut db = webtest_analysis::AnalysisDatabase::default();
    let file=db.open_file("guard-order.webtest",r#"
        guard ordered(actor: BrowserActor){
            when actor.console.message(e) if e.message == "second" {fail "wrong: later event"}
            when actor.console.message(e) if e.message == "first" {fail "correct: first event"}
        }
        test "ordered" {actor alice browser with guards [ordered(alice)] {alice {open "http://example.test/first" open "http://example.test/second"}}}
    "#);
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file).unwrap()
    );
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(
            &plan,
            &FakeHost {
                result: Ok(()),
                starts: Arc::new(AtomicUsize::new(0)),
            },
        )
        .await;
    assert!(
        matches!(&result.tests[0].outcome,TestOutcome::Failed(failure) if failure.error.to_string()=="correct: first event")
    );
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| matches!(
                e,
                webtest_observation::ExecutionEvent::Semantic {
                    event: webtest_observation::SemanticEvent::GuardTriggered { .. },
                    ..
                }
            ))
            .count(),
        2
    );
}

#[tokio::test]
async fn concurrent_verdicts_terminate_variants_and_bound_races_allow_bottom_branches() {
    use webtest_model::VerdictKind;
    for (operation, expected) in [
        ("pass", Some(VerdictKind::Pass)),
        ("skip \"reason\"", Some(VerdictKind::Skipped)),
        ("inconclusive \"reason\"", Some(VerdictKind::Inconclusive)),
        ("fail \"reason\"", None),
    ] {
        for construct in ["parallel", "race"] {
            let source = format!(
                "test \"terminal\" {{{construct} {{server {{{operation}}} server {{expect true}}}} fail \"unreachable\"}}"
            );
            let mut db = webtest_analysis::AnalysisDatabase::default();
            let file = db.open_file("verdict.webtest", source);
            assert!(
                db.diagnostics(file).unwrap().is_empty(),
                "{:?}",
                db.diagnostics(file)
            );
            let plan = db.test_plan(file).unwrap();
            plan.validate_tree().unwrap();
            let host = FakeHost {
                result: Ok(()),
                starts: Arc::new(AtomicUsize::new(0)),
            };
            let result = Runner::new(Arc::new(ObservationStore::default()))
                .run(&plan, &host)
                .await;
            match expected {
                Some(expected) => assert!(
                    matches!(result.tests[0].outcome, TestOutcome::Verdict {kind,..} if kind == expected),
                    "{construct} {operation}: {:?}",
                    result.tests[0].outcome
                ),
                None => assert!(
                    matches!(&result.tests[0].outcome,TestOutcome::Failed(failure) if matches!(&failure.error, StepError::Assertion(assertion) if assertion.matcher==webtest_plan::ValueMatcher::ExplicitFail))
                ),
            }
        }
    }
    let mut db = webtest_analysis::AnalysisDatabase::default();
    let file = db.open_file(
        "bottom.webtest",
        r#"fn stop() -> Int {pass}
        test "bound" {let value = race {server {pass} server {provide 42}} fail "unreachable"}
        test "helper" {let value = stop() fail "unreachable"}
        test "soft" {check {parallel {server {fail "soft"} server {expect 1 == 2}}}}
    "#,
    );
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file)
    );
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let host = FakeHost {
        result: Ok(()),
        starts: Arc::new(AtomicUsize::new(0)),
    };
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &host)
        .await;
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Verdict {
            kind: VerdictKind::Pass,
            ..
        }
    ));
    assert!(matches!(
        result.tests[1].outcome,
        TestOutcome::Verdict {
            kind: VerdictKind::Pass,
            ..
        }
    ));
    let TestOutcome::Failed(failure) = &result.tests[2].outcome else {
        panic!("soft check")
    };
    let StepError::Assertion(assertion) = &failure.error else {
        panic!("check assertion")
    };
    assert!(matches!(
        assertion.diff,
        webtest_observation::ValueDiff::Check { total: 2, .. }
    ));
}
