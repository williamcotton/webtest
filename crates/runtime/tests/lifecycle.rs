use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use webtest_browser::{
    Action, BrowserContext, BrowserContextOptions, BrowserError, BrowserHost, BrowserSession,
    EvidenceRequest, InspectionOptions, Locator, LocatorState, Page, PageEvidence, PageInspection,
};
use webtest_model::{BindingId, Capability, StepId, TestId, Type, Value};
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeFailure, RuntimeObservation,
    RuntimeObservationKind,
};
use webtest_plan::{
    BrowserOperation, EvaluatePureOperation, PlanExpr, PlannedStep, PlannedTest,
    ServerProviderCall, TestOperation, TestPlan,
};
use webtest_provider::{
    CallContext, FsProviderConfig, HttpProviderConfig, NativeProviderConfig, OperationName,
    OperationSchema, ProcessProviderConfig, ProviderCall, ProviderError, ProviderName,
    ProviderRegistry, ProviderResult, ProviderSchema, ServerProvider,
};
use webtest_runtime::{
    CancellationReason, CleanupCause, CleanupFailure, CleanupResource, FailureClass,
    PriorTestOutcome, RunControl, RunError, RunEventSink, RunOutcome, Runner, RunnerOptions,
    SkipReason, StepError, TestOutcome,
};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

#[derive(Default)]
struct LifecycleState {
    log: Mutex<Vec<String>>,
    next_session: AtomicUsize,
    next_context: AtomicUsize,
    context_close_failures: Mutex<BTreeSet<usize>>,
    session_close_failures: Mutex<BTreeSet<usize>>,
    page_creation_failures: Mutex<BTreeSet<usize>>,
    page_creation_delays: Mutex<BTreeMap<usize, Duration>>,
    page_errors: Mutex<BTreeMap<String, BrowserError>>,
    page_delays: Mutex<BTreeMap<String, Duration>>,
    operation_timeouts: Mutex<Vec<(String, Duration)>>,
    page_evidence: Mutex<PageEvidence>,
}

impl LifecycleState {
    fn push(&self, event: impl Into<String>) {
        self.log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event.into());
    }

    fn log(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct LifecycleHost(Arc<LifecycleState>);

struct LifecycleSession {
    id: usize,
    state: Arc<LifecycleState>,
}

struct LifecycleContext {
    id: usize,
    state: Arc<LifecycleState>,
}

struct LifecyclePage {
    context_id: usize,
    state: Arc<LifecycleState>,
}

#[async_trait]
impl BrowserHost for LifecycleHost {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
        let id = self.0.next_session.fetch_add(1, Ordering::SeqCst);
        self.0.push(format!("session_start:{id}"));
        Ok(Box::new(LifecycleSession {
            id,
            state: Arc::clone(&self.0),
        }))
    }
}

#[async_trait]
impl BrowserSession for LifecycleSession {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        unreachable!("runtime creates explicit contexts")
    }

    async fn new_context(
        &mut self,
        _options: &BrowserContextOptions,
    ) -> Result<Box<dyn BrowserContext>, BrowserError> {
        let id = self.state.next_context.fetch_add(1, Ordering::SeqCst);
        self.state.push(format!("context_create:{}:{id}", self.id));
        Ok(Box::new(LifecycleContext {
            id,
            state: Arc::clone(&self.state),
        }))
    }

    async fn close(&mut self) -> Result<(), BrowserError> {
        self.state.push(format!("session_close:{}", self.id));
        if self
            .state
            .session_close_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&self.id)
        {
            Err(BrowserError::BrowserDisconnected)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl BrowserContext for LifecycleContext {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        self.state.push(format!("page_create:{}", self.id));
        let delay = self
            .state
            .page_creation_delays
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&self.id)
            .copied();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        if self
            .state
            .page_creation_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&self.id)
        {
            return Err(BrowserError::BrowserDisconnected);
        }
        Ok(Box::new(LifecyclePage {
            context_id: self.id,
            state: Arc::clone(&self.state),
        }))
    }

    async fn close(&mut self) -> Result<(), BrowserError> {
        self.state.push(format!("context_close:{}", self.id));
        if self
            .state
            .context_close_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&self.id)
        {
            Err(BrowserError::BrowserDisconnected)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl Page for LifecyclePage {
    async fn open(&mut self, url: &str) -> Result<(), BrowserError> {
        self.state.push(format!("open:{}:{url}", self.context_id));
        Ok(())
    }

    async fn click(&mut self, _locator: &Locator) -> Result<(), BrowserError> {
        Ok(())
    }

    async fn expect_visible(&mut self, _locator: &Locator) -> Result<(), BrowserError> {
        Ok(())
    }

    async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError> {
        self.state
            .push(format!("evaluate:{}:{expression}", self.context_id));
        let delay = self
            .state
            .page_delays
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(expression)
            .copied();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        self.state
            .page_errors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(expression)
            .cloned()
            .map_or(Ok(()), Err)
    }

    async fn open_with_timeout(
        &mut self,
        url: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        self.state
            .operation_timeouts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(("navigation".into(), timeout));
        self.open(url).await
    }

    async fn evaluate_with_timeout(
        &mut self,
        expression: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        self.state
            .operation_timeouts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((format!("evaluate:{expression}"), timeout));
        self.evaluate(expression).await
    }

    async fn perform(&mut self, action: &Action, timeout: Duration) -> Result<(), BrowserError> {
        self.state
            .operation_timeouts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(("action".into(), timeout));
        self.click(action.locator()).await
    }

    async fn wait_for_locator(
        &mut self,
        _locator: &Locator,
        _state: LocatorState,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        self.state
            .operation_timeouts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(("wait".into(), timeout));
        Ok(())
    }

    async fn capture_evidence(&mut self, request: &EvidenceRequest) -> PageEvidence {
        self.state.push(format!(
            "evidence:{}:{}:{}:{}",
            self.context_id, request.include_screenshot, request.include_dom, request.max_dom_bytes
        ));
        self.state
            .page_evidence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn inspect(
        &mut self,
        _options: &InspectionOptions,
    ) -> Result<PageInspection, BrowserError> {
        self.state.push(format!("inspect:{}", self.context_id));
        Ok(empty_inspection())
    }
}

fn empty_inspection() -> PageInspection {
    PageInspection {
        kind: "inspection".into(),
        inspection_schema_version: webtest_browser::INSPECTION_SCHEMA_VERSION,
        snapshot_id: "snapshot".into(),
        browser_version: "fake".into(),
        page: webtest_browser::PageSummary {
            url: "http://example.test/".into(),
            title: "Fake".into(),
        },
        elements: Vec::new(),
        truncation: webtest_browser::InspectionTruncation::default(),
    }
}

fn origin(file: FileId, offset: u32) -> SyntaxOrigin {
    SyntaxOrigin::new(
        file,
        TextRange::new(TextSize::new(offset), TextSize::new(offset + 1)),
    )
}

fn plan_with_tests(
    mut capabilities: Vec<Capability>,
    operations: Vec<Vec<TestOperation>>,
) -> TestPlan {
    let file = FileId::new(41);
    let mut next_step = 0;
    capabilities.sort_unstable();
    capabilities.dedup();
    let tests = operations
        .into_iter()
        .enumerate()
        .map(|(test_index, operations)| {
            let required_host_capabilities = test_capabilities(&capabilities, &operations);
            PlannedTest {
                id: TestId(test_index as u32),
                name: format!("test-{test_index}"),
                required_host_capabilities,
                origin: origin(file, test_index as u32),
                steps: operations
                    .into_iter()
                    .map(|operation| {
                        let step_id = StepId(next_step);
                        next_step += 1;
                        PlannedStep {
                            id: step_id,
                            origin: origin(file, step_id.0 + 10),
                            operation,
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    TestPlan {
        file,
        source_revision: SourceRevision::of("lifecycle"),
        required_host_capabilities: capabilities,
        tests,
    }
}

fn test_capabilities(
    plan_capabilities: &[Capability],
    operations: &[TestOperation],
) -> Vec<Capability> {
    let mut capabilities = BTreeSet::new();
    for operation in operations {
        match operation {
            TestOperation::ServerProviderCall(_) => {
                capabilities.insert(Capability::Server);
            }
            TestOperation::Browser(_)
            | TestOperation::Assertion(
                webtest_plan::AssertionOperation::Locator { .. }
                | webtest_plan::AssertionOperation::Url { .. },
            ) => {
                capabilities.insert(Capability::Browser);
            }
            TestOperation::Assertion(webtest_plan::AssertionOperation::Value { .. }) => {
                capabilities.insert(Capability::Test);
            }
            TestOperation::EvaluatePure(_) => {}
        }
    }
    if capabilities.is_empty() && plan_capabilities.contains(&Capability::Pure) {
        capabilities.insert(Capability::Pure);
    }
    capabilities.into_iter().collect()
}

fn browser_evaluate(expression: &str) -> TestOperation {
    TestOperation::Browser(BrowserOperation::Evaluate {
        expression: expression.into(),
    })
}

fn pure(value: Value) -> TestOperation {
    TestOperation::EvaluatePure(EvaluatePureOperation {
        expression: PlanExpr::Literal(value),
        result_binding: None,
        result_name: None,
        result_type: Type::Null,
    })
}

fn pure_binding(value: Value, binding: BindingId) -> TestOperation {
    TestOperation::EvaluatePure(EvaluatePureOperation {
        expression: PlanExpr::Literal(value),
        result_binding: Some(binding),
        result_name: Some(format!("binding_{}", binding.0)),
        result_type: Type::Json,
    })
}

fn missing_binding() -> TestOperation {
    TestOperation::EvaluatePure(EvaluatePureOperation {
        expression: PlanExpr::Binding(BindingId(999)),
        result_binding: None,
        result_name: None,
        result_type: Type::String,
    })
}

fn event_names(events: &[ExecutionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            ExecutionEvent::RunStarted { .. } => "run_started",
            ExecutionEvent::TestStarted { .. } => "test_started",
            ExecutionEvent::StepStarted { .. } => "step_started",
            ExecutionEvent::StepPassed { .. } => "step_passed",
            ExecutionEvent::ProviderCallStarted { .. } => "provider_started",
            ExecutionEvent::ProviderCallFinished { .. } => "provider_finished",
            ExecutionEvent::ProviderCallFailed { .. } => "provider_failed",
            ExecutionEvent::StepFailed { .. } => "step_failed",
            ExecutionEvent::TestTimedOut { .. } => "test_timed_out",
            ExecutionEvent::CleanupFailed { .. } => "cleanup_failed",
            ExecutionEvent::TestFinished { .. } => "test_finished",
            ExecutionEvent::TestSkipped { .. } => "test_skipped",
            ExecutionEvent::RunFinished { .. } => "run_finished",
        })
        .collect()
}

#[derive(Default)]
struct RecordingEventSink {
    events: Mutex<Vec<ExecutionEvent>>,
}

impl RunEventSink for RecordingEventSink {
    fn publish(&self, event: &ExecutionEvent) {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event.clone());
    }
}

struct ArtifactCheckingEventSink {
    directory: PathBuf,
    artifacts_ready: AtomicBool,
}

impl RunEventSink for ArtifactCheckingEventSink {
    fn publish(&self, event: &ExecutionEvent) {
        let ExecutionEvent::StepFailed {
            execution_id,
            test_id,
            step_id,
            ..
        } = event
        else {
            return;
        };
        let stem = format!(
            "test-{}-step-{}-execution-{}",
            test_id.0, step_id.0, execution_id.0
        );
        self.artifacts_ready.store(
            [
                self.directory.join(format!("{stem}.png")),
                self.directory.join(format!("{stem}.dom.html")),
                self.directory.join(format!("{stem}.evidence.txt")),
            ]
            .iter()
            .all(|path| path.is_file()),
            Ordering::SeqCst,
        );
    }
}

#[tokio::test]
async fn event_sink_receives_the_same_ordered_facts_as_the_final_result() {
    let state = Arc::new(LifecycleState::default());
    let sink = Arc::new(RecordingEventSink::default());
    let plan = plan_with_tests(vec![Capability::Pure], vec![vec![pure(Value::Null)]]);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_event_sink(sink.clone())
        .run(&plan, &LifecycleHost(state))
        .await;

    assert_eq!(
        *sink
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        result.events
    );
}

#[tokio::test]
async fn provider_only_plan_never_starts_a_browser() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(vec![Capability::Pure], vec![vec![pure(Value::Null)]]);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(state.log().is_empty());
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "test_finished",
            "run_finished"
        ]
    );
}

#[tokio::test]
async fn omitted_optional_provider_member_evaluates_to_null_through_the_shared_plan() {
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Record(BTreeMap::new()))));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let result_type = Type::Record(BTreeMap::from([(
        "nickname".into(),
        webtest_model::RecordField {
            ty: Type::String,
            optional: true,
            documentation: String::new(),
            secret: false,
        },
    )]));
    let plan = plan_with_tests(
        vec![Capability::Server, Capability::Test],
        vec![vec![
            TestOperation::ServerProviderCall(ServerProviderCall {
                provider: "fake".into(),
                operation: "call".into(),
                arguments: BTreeMap::new(),
                result_binding: Some(BindingId(1)),
                result_name: Some("user".into()),
                result_type,
                schema_hash: "schema".into(),
                timeout: None,
                redacted_arguments: Vec::new(),
                redacted_result_fields: Vec::new(),
                retry_safe: false,
            }),
            TestOperation::EvaluatePure(EvaluatePureOperation {
                expression: PlanExpr::Member {
                    receiver: Box::new(PlanExpr::Binding(BindingId(1))),
                    member: "nickname".into(),
                    missing_is_null: true,
                },
                result_binding: Some(BindingId(2)),
                result_name: Some("nickname".into()),
                result_type: Type::Option(Box::new(Type::String)),
            }),
            TestOperation::Assertion(webtest_plan::AssertionOperation::Value {
                matcher: webtest_plan::ValueMatcher::Equal,
                actual: PlanExpr::Binding(BindingId(2)),
                expected: Some(PlanExpr::Literal(Value::Null)),
                value_type: Type::Option(Box::new(Type::String)),
            }),
        ]],
    );
    let state = Arc::new(LifecycleState::default());
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert_eq!(result.passed(), 1);
    assert!(state.log().is_empty());
    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
}

#[tokio::test]
async fn browser_session_is_shared_and_each_test_gets_a_context_and_page() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![
            vec![browser_evaluate("first")],
            vec![browser_evaluate("second")],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert_eq!(result.passed(), 2);
    assert_eq!(
        state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "evaluate:0:first",
            "context_close:0",
            "context_create:0:1",
            "page_create:1",
            "evaluate:1:second",
            "context_close:1",
            "session_close:0",
        ]
    );
}

#[tokio::test]
async fn server_browser_server_browser_only_allocates_for_browser_tests() {
    let provider = Arc::new(DelayedProvider::new([Duration::ZERO, Duration::ZERO]));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(
        vec![Capability::Server, Capability::Browser],
        vec![
            vec![secret_binding(), provider_operation(None)],
            vec![browser_evaluate("first")],
            vec![secret_binding(), provider_operation(None)],
            vec![browser_evaluate("second")],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert_eq!(result.passed(), 4);
    assert_eq!(
        state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "evaluate:0:first",
            "context_close:0",
            "context_create:0:1",
            "page_create:1",
            "evaluate:1:second",
            "context_close:1",
            "session_close:0",
        ]
    );
}

#[tokio::test]
async fn ordinary_failure_stops_one_test_but_later_tests_continue() {
    let state = Arc::new(LifecycleState::default());
    state.page_errors.lock().expect("errors").insert(
        "first".into(),
        BrowserError::LocatorNotFound {
            locator: Locator::Id("missing".into()),
        },
    );
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![
            vec![browser_evaluate("first"), browser_evaluate("unreached")],
            vec![browser_evaluate("second")],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert_eq!((result.passed(), result.failed()), (1, 1));
    assert!(!state.log().iter().any(|event| event.contains("unreached")));
    assert!(state.log().iter().any(|event| event == "evaluate:1:second"));
    assert!(
        state
            .log()
            .iter()
            .any(|event| event == "evidence:0:false:false:1048576")
    );
    assert!(state.log().iter().any(|event| event == "inspect:0"));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_failed",
            "test_finished",
            "test_started",
            "step_started",
            "step_passed",
            "test_finished",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn artifact_write_failure_remains_secondary_to_the_ordinary_test_failure() {
    let root = tempfile::tempdir().expect("temporary root");
    let artifact_path = root.path().join("not-a-directory");
    std::fs::write(&artifact_path, b"file").expect("artifact path file");
    let state = Arc::new(LifecycleState::default());
    state.page_errors.lock().expect("errors").insert(
        "missing".into(),
        BrowserError::LocatorNotFound {
            locator: Locator::Id("missing".into()),
        },
    );
    state.page_evidence.lock().expect("evidence").screenshot_png = Some(vec![1, 2, 3]);
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("missing")]],
    );
    let options = RunnerOptions {
        evidence: webtest_runtime::EvidenceOptions {
            screenshot_on_failure: true,
            artifact_directory: artifact_path,
            ..webtest_runtime::EvidenceOptions::default()
        },
        ..RunnerOptions::default()
    };
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(options)
        .run(&plan, &LifecycleHost(state))
        .await;

    let TestOutcome::Failed(failure) = &result.tests[0].outcome else {
        panic!("artifact persistence must not replace the ordinary failure")
    };
    assert!(matches!(
        failure.error,
        StepError::Browser(BrowserError::LocatorNotFound { .. })
    ));
    assert_eq!(failure.evidence.capture_failures.len(), 1);
    assert!(failure.evidence.capture_failures[0].starts_with("artifact directory:"));
    assert!(
        result
            .events
            .iter()
            .all(|event| !matches!(event, ExecutionEvent::CleanupFailed { .. }))
    );
    assert!(matches!(result.outcome, RunOutcome::Completed));
}

#[tokio::test]
async fn failure_event_is_published_only_after_referenced_artifacts_exist() {
    let root = tempfile::tempdir().expect("temporary root");
    let artifact_directory = root.path().join("artifacts");
    let state = Arc::new(LifecycleState::default());
    state.page_errors.lock().expect("errors").insert(
        "missing".into(),
        BrowserError::LocatorNotFound {
            locator: Locator::Id("missing".into()),
        },
    );
    *state.page_evidence.lock().expect("evidence") = PageEvidence {
        screenshot_png: Some(vec![1, 2, 3]),
        dom_snapshot: Some("<main>failure</main>".into()),
        current_url: Some("https://example.test/".into()),
        ..PageEvidence::default()
    };
    let sink = Arc::new(ArtifactCheckingEventSink {
        directory: artifact_directory.clone(),
        artifacts_ready: AtomicBool::new(false),
    });
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("missing")]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            evidence: webtest_runtime::EvidenceOptions {
                screenshot_on_failure: true,
                dom_snapshot_on_failure: true,
                artifact_directory,
                ..webtest_runtime::EvidenceOptions::default()
            },
            ..RunnerOptions::default()
        })
        .with_event_sink(sink.clone())
        .run(&plan, &LifecycleHost(state))
        .await;

    assert!(sink.artifacts_ready.load(Ordering::SeqCst));
    let TestOutcome::Failed(failure) = &result.tests[0].outcome else {
        panic!("browser failure")
    };
    assert_eq!(failure.artifacts.len(), 3);
    assert!(
        failure
            .artifacts
            .iter()
            .all(|artifact| artifact.path.is_file())
    );
}

#[tokio::test]
async fn infrastructure_browser_failure_aborts_before_later_tests() {
    let state = Arc::new(LifecycleState::default());
    state
        .page_errors
        .lock()
        .expect("errors")
        .insert("first".into(), BrowserError::BrowserDisconnected);
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![
            vec![browser_evaluate("first")],
            vec![browser_evaluate("unreached")],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(
        result.outcome,
        RunOutcome::Aborted {
            failure: RunError::Browser(BrowserError::BrowserDisconnected),
            ..
        }
    ));
    assert_eq!((result.aborted(), result.skipped()), (1, 1));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_failed",
            "test_finished",
            "test_skipped",
            "run_finished",
        ]
    );
    assert_terminal_event_invariants(&result.events);
    let log = state.log();
    assert!(log.iter().any(|event| event == "context_close:0"));
    assert!(!log.iter().any(|event| event.contains("unreached")));
    assert!(!log.iter().any(|event| event.starts_with("evidence:")));
    assert!(!log.iter().any(|event| event.starts_with("inspect:")));
}

#[tokio::test]
async fn context_close_failure_aborts_the_test_and_run_before_the_next_test() {
    let state = Arc::new(LifecycleState::default());
    state
        .context_close_failures
        .lock()
        .expect("close failures")
        .insert(0);
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![
            vec![browser_evaluate("first")],
            vec![browser_evaluate("second")],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted {
            failure: RunError::Cleanup(CleanupFailure {
                resource: CleanupResource::BrowserContext,
                cause: CleanupCause::Browser(BrowserError::BrowserDisconnected),
            }),
            prior_outcome: None,
        }
    ));
    assert!(matches!(
        result.tests[1].outcome,
        TestOutcome::Skipped {
            reason: SkipReason::RunAborted,
            failure_class: Some(FailureClass::Infrastructure),
        }
    ));
    assert!(matches!(result.outcome, RunOutcome::Aborted { .. }));
    assert_eq!(
        state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "evaluate:0:first",
            "context_close:0",
            "session_close:0",
        ]
    );
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "cleanup_failed",
            "test_finished",
            "test_skipped",
            "run_finished",
        ]
    );
    assert_terminal_event_invariants(&result.events);
}

#[tokio::test]
async fn context_close_failure_preserves_an_ordinary_step_failure_as_a_typed_prior_outcome() {
    let state = Arc::new(LifecycleState::default());
    state
        .context_close_failures
        .lock()
        .expect("close failures")
        .insert(0);
    state.page_errors.lock().expect("page errors").insert(
        "missing".into(),
        BrowserError::LocatorNotFound {
            locator: Locator::Id("missing".into()),
        },
    );
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("missing")]],
    );
    let sink = Arc::new(RecordingEventSink::default());
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_event_sink(sink.clone())
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    let TestOutcome::Aborted {
        failure:
            RunError::Cleanup(CleanupFailure {
                resource: CleanupResource::BrowserContext,
                cause: CleanupCause::Browser(BrowserError::BrowserDisconnected),
            }),
        prior_outcome: Some(prior),
    } = &result.tests[0].outcome
    else {
        panic!("cleanup should outrank and retain the ordinary failure")
    };
    assert!(matches!(prior.as_ref(), PriorTestOutcome::Failed(_)));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_failed",
            "cleanup_failed",
            "test_finished",
            "run_finished",
        ]
    );
    assert_eq!(
        *sink
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        result.events
    );
}

#[tokio::test]
async fn session_close_failure_aborts_only_after_all_passing_tests_are_final() {
    let state = Arc::new(LifecycleState::default());
    state
        .session_close_failures
        .lock()
        .expect("close failures")
        .insert(0);
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("first")]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(matches!(
        result.outcome,
        RunOutcome::Aborted {
            failure: RunError::Cleanup(CleanupFailure {
                resource: CleanupResource::BrowserSession,
                cause: CleanupCause::Browser(BrowserError::BrowserDisconnected),
            }),
            prior_outcome: None,
        }
    ));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "test_finished",
            "cleanup_failed",
            "run_finished",
        ]
    );
    assert!(matches!(
        result.events[result.events.len() - 2],
        ExecutionEvent::CleanupFailed {
            test_id: None,
            resource: CleanupResource::BrowserSession,
            ..
        }
    ));
    assert!(matches!(
        result.events.last(),
        Some(ExecutionEvent::RunFinished {
            outcome: webtest_observation::RunOutcomeKind::Aborted,
            ..
        })
    ));
}

#[tokio::test]
async fn page_creation_failure_closes_the_acquired_context_once_before_terminal_events() {
    let state = Arc::new(LifecycleState::default());
    state
        .page_creation_failures
        .lock()
        .expect("page failures")
        .insert(0);
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("unreached")]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted {
            failure: RunError::Browser(BrowserError::BrowserDisconnected),
            prior_outcome: None,
        }
    ));
    assert_eq!(
        state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "context_close:0",
            "session_close:0",
        ]
    );
    assert_eq!(
        state
            .log()
            .iter()
            .filter(|entry| entry.as_str() == "context_close:0")
            .count(),
        1
    );
    assert_terminal_event_invariants(&result.events);
}

#[tokio::test]
async fn zero_test_plan_never_starts_or_closes_a_browser_session() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(Vec::new(), Vec::new());
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(result.tests.is_empty());
    assert!(state.log().is_empty());
    assert_eq!(event_names(&result.events), ["run_started", "run_finished"]);
}

#[tokio::test]
async fn already_cancelled_empty_plan_is_cancelled_without_starting_a_browser() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(Vec::new(), Vec::new());
    let control = RecordingControl::new(true, false, true);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(&plan, &LifecycleHost(Arc::clone(&state)), Some(&control))
        .await;

    assert!(result.tests.is_empty());
    assert!(state.log().is_empty());
    assert!(matches!(
        result.outcome,
        RunOutcome::Cancelled {
            reason: CancellationReason::Requested
        }
    ));
    assert_eq!(event_names(&result.events), ["run_started", "run_finished"]);
    assert_terminal_event_invariants(&result.events);
}

struct FailingStartHost;

#[async_trait]
impl BrowserHost for FailingStartHost {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
        Err(BrowserError::Launch("cannot launch".into()))
    }
}

#[tokio::test]
async fn observations_are_cleared_before_lazy_browser_start_can_fail() {
    let store = Arc::new(ObservationStore::default());
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![
            vec![browser_evaluate("unreached")],
            vec![browser_evaluate("also-unreached")],
        ],
    );
    store.record(RuntimeObservation {
        execution_id: ExecutionId::next(),
        file: plan.file,
        source_revision: plan.source_revision,
        test_id: TestId(0),
        step_id: Some(StepId(0)),
        range: TextRange::default(),
        kind: RuntimeObservationKind::ValueFailure {
            code: webtest_observation::RuntimeFailureCode::AssertionFailed,
            message: "stale".into(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        },
    });

    let result = Runner::new(Arc::clone(&store))
        .run(&plan, &FailingStartHost)
        .await;
    assert!(matches!(
        result.outcome,
        RunOutcome::Aborted {
            failure: RunError::Browser(BrowserError::Launch(_)),
            ..
        }
    ));
    assert_eq!((result.aborted(), result.skipped()), (1, 1));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "test_finished",
            "test_skipped",
            "run_finished"
        ]
    );
    assert_terminal_event_invariants(&result.events);
    assert!(
        store
            .observations_for(plan.file, plan.source_revision)
            .is_empty()
    );
}

#[tokio::test]
async fn browser_launch_failure_preserves_earlier_independent_test_result() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(
        vec![Capability::Pure, Capability::Browser],
        vec![
            vec![pure(Value::String("completed first".into()))],
            vec![browser_evaluate("unreached")],
            vec![pure(Value::String("skipped".into()))],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &FailingStartHost)
        .await;

    assert!(state.log().is_empty());
    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(matches!(
        result.tests[1].outcome,
        TestOutcome::Aborted {
            failure: RunError::Browser(BrowserError::Launch(_)),
            ..
        }
    ));
    assert!(matches!(
        result.tests[2].outcome,
        TestOutcome::Skipped {
            reason: SkipReason::RunAborted,
            ..
        }
    ));
}

#[tokio::test]
async fn browser_step_without_test_capability_is_an_internal_plan_error() {
    let state = Arc::new(LifecycleState::default());
    let mut plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("must-not-run")]],
    );
    plan.tests[0].required_host_capabilities.clear();
    plan.required_host_capabilities.clear();
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(state.log().is_empty());
    assert!(matches!(
        result.outcome,
        RunOutcome::Aborted {
            failure: RunError::Internal(_),
            ..
        }
    ));
    assert_eq!((result.aborted(), result.skipped()), (0, 1));
}

#[tokio::test]
async fn inconsistent_plan_union_is_rejected_without_browser_allocation() {
    let state = Arc::new(LifecycleState::default());
    let mut plan = plan_with_tests(vec![Capability::Pure], vec![vec![pure(Value::Null)]]);
    plan.required_host_capabilities = vec![Capability::Browser];
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(state.log().is_empty());
    assert!(matches!(
        result.outcome,
        RunOutcome::Aborted {
            failure: RunError::Internal(_),
            ..
        }
    ));
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Skipped {
            failure_class: Some(FailureClass::Internal),
            ..
        }
    ));
}

#[tokio::test]
async fn internal_step_failure_aborts_with_typed_events_and_no_user_observation() {
    let store = Arc::new(ObservationStore::default());
    let plan = plan_with_tests(
        vec![Capability::Pure],
        vec![vec![missing_binding()], vec![pure(Value::Null)]],
    );
    store.record(RuntimeObservation {
        execution_id: ExecutionId::next(),
        file: plan.file,
        source_revision: plan.source_revision,
        test_id: TestId(0),
        step_id: Some(StepId(0)),
        range: TextRange::default(),
        kind: RuntimeObservationKind::ValueFailure {
            code: webtest_observation::RuntimeFailureCode::AssertionFailed,
            message: "stale".into(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        },
    });
    let control = RecordingControl::new(false, false, true);
    let result = Runner::new(Arc::clone(&store))
        .run_with_control(
            &plan,
            &LifecycleHost(Arc::new(LifecycleState::default())),
            Some(&control),
        )
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted {
            failure: RunError::Internal(_),
            ..
        }
    ));
    assert!(matches!(
        result.tests[1].outcome,
        TestOutcome::Skipped {
            reason: SkipReason::RunAborted,
            failure_class: Some(FailureClass::Internal),
        }
    ));
    assert!(matches!(
        result.outcome,
        RunOutcome::Aborted {
            failure: RunError::Internal(_),
            ..
        }
    ));
    assert!(control.failure.lock().expect("failure hook").is_none());
    assert!(
        store
            .observations_for(plan.file, plan.source_revision)
            .is_empty()
    );
    assert!(result.events.iter().any(|event| matches!(
        event,
        ExecutionEvent::StepFailed {
            failure_class: FailureClass::Internal,
            failure: RuntimeFailure::Internal { .. },
            ..
        }
    )));
    assert!(result.events.iter().any(|event| matches!(
        event,
        ExecutionEvent::TestFinished {
            failure_class: Some(FailureClass::Internal),
            ..
        }
    )));
    assert!(matches!(
        result.events.last(),
        Some(ExecutionEvent::RunFinished {
            failure_class: Some(FailureClass::Internal),
            ..
        })
    ));

    store.record(RuntimeObservation {
        execution_id: result.execution_id,
        file: plan.file,
        source_revision: plan.source_revision,
        test_id: TestId(0),
        step_id: Some(StepId(0)),
        range: TextRange::default(),
        kind: RuntimeObservationKind::ValueFailure {
            code: webtest_observation::RuntimeFailureCode::InternalError,
            message: "old".into(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        },
    });
    let successful = plan_with_tests(vec![Capability::Pure], vec![vec![pure(Value::Null)]]);
    let rerun = Runner::new(Arc::clone(&store))
        .run(
            &successful,
            &LifecycleHost(Arc::new(LifecycleState::default())),
        )
        .await;
    assert!(matches!(rerun.tests[0].outcome, TestOutcome::Passed));
    assert!(
        store
            .observations_for(successful.file, successful.source_revision)
            .is_empty()
    );
}

struct RecordingControl {
    cancel: AtomicBool,
    cancel_after_hook: bool,
    capture: bool,
    log: Mutex<Vec<String>>,
    failure: Mutex<Option<String>>,
}

impl RecordingControl {
    fn new(cancel: bool, cancel_after_hook: bool, capture: bool) -> Self {
        Self {
            cancel: AtomicBool::new(cancel),
            cancel_after_hook,
            capture,
            log: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
        }
    }
}

#[async_trait]
impl RunControl for RecordingControl {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn should_capture_bindings(&self, _test: &PlannedTest, _step: &PlannedStep) -> bool {
        self.capture
    }

    async fn before_step(&self, _test: &PlannedTest, _step: &PlannedStep) {
        self.log.lock().expect("control log").push("before".into());
        if self.cancel_after_hook {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    async fn before_step_with_bindings(
        &self,
        _test: &PlannedTest,
        _step: &PlannedStep,
        bindings: BTreeMap<String, Value>,
    ) {
        self.log
            .lock()
            .expect("control log")
            .push(format!("bindings:{bindings:?}"));
        if self.cancel_after_hook {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    async fn after_step_failure(
        &self,
        _test: &PlannedTest,
        _step: &PlannedStep,
        error: &StepError,
        _bindings: &BTreeMap<String, Value>,
    ) {
        *self.failure.lock().expect("failure") = Some(error.to_string());
    }
}

struct CancelOnCheck {
    checks: AtomicUsize,
    cancel_at: usize,
}

impl CancelOnCheck {
    fn new(cancel_at: usize) -> Self {
        Self {
            checks: AtomicUsize::new(0),
            cancel_at,
        }
    }
}

#[async_trait]
impl RunControl for CancelOnCheck {
    fn is_cancelled(&self) -> bool {
        self.checks.fetch_add(1, Ordering::SeqCst) + 1 >= self.cancel_at
    }

    async fn before_step(&self, _test: &PlannedTest, _step: &PlannedStep) {}
}

fn assert_terminal_event_invariants(events: &[ExecutionEvent]) {
    let started_tests = events
        .iter()
        .filter(|event| matches!(event, ExecutionEvent::TestStarted { .. }))
        .count();
    let finished_tests = events
        .iter()
        .filter(|event| matches!(event, ExecutionEvent::TestFinished { .. }))
        .count();
    assert_eq!(started_tests, finished_tests);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ExecutionEvent::RunStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ExecutionEvent::RunFinished { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn cancellation_and_snapshot_gating_keep_the_existing_hook_order() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("first")]],
    );

    let cancelled = RecordingControl::new(true, false, true);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(&plan, &LifecycleHost(Arc::clone(&state)), Some(&cancelled))
        .await;
    assert_eq!((result.cancelled(), result.skipped()), (0, 1));
    assert!(matches!(
        result.outcome,
        RunOutcome::Cancelled {
            reason: CancellationReason::Requested
        }
    ));
    assert_eq!(
        event_names(&result.events),
        ["run_started", "test_skipped", "run_finished"]
    );
    assert!(
        state.log().is_empty(),
        "cancellation precedes browser startup"
    );
    assert!(
        result
            .tests
            .iter()
            .all(|test| !matches!(test.outcome, TestOutcome::Passed))
    );
    assert!(result.events.iter().all(|event| !matches!(
        event,
        ExecutionEvent::TestFinished {
            outcome: webtest_observation::TestOutcomeKind::Passed,
            ..
        }
    )));
    assert_terminal_event_invariants(&result.events);

    let after_hook = RecordingControl::new(false, true, false);
    let second_state = Arc::new(LifecycleState::default());
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(
            &plan,
            &LifecycleHost(Arc::clone(&second_state)),
            Some(&after_hook),
        )
        .await;
    assert_eq!(
        after_hook.log.lock().expect("control log").as_slice(),
        ["before"]
    );
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "test_finished",
            "run_finished"
        ]
    );
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Cancelled {
            reason: CancellationReason::Requested
        }
    ));
    assert_eq!(
        second_state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "context_close:0",
            "session_close:0",
        ]
    );
    assert_terminal_event_invariants(&result.events);
}

#[tokio::test]
async fn cleanup_failure_outranks_cancellation_and_retains_the_reason() {
    let state = Arc::new(LifecycleState::default());
    state
        .context_close_failures
        .lock()
        .expect("close failures")
        .insert(0);
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("unreached")]],
    );
    let control = RecordingControl::new(false, true, false);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(&plan, &LifecycleHost(Arc::clone(&state)), Some(&control))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted {
            failure: RunError::Cleanup(CleanupFailure {
                resource: CleanupResource::BrowserContext,
                ..
            }),
            prior_outcome: Some(ref prior),
        } if matches!(
            prior.as_ref(),
            PriorTestOutcome::Cancelled {
                reason: CancellationReason::Requested,
            }
        )
    ));
    assert!(matches!(result.outcome, RunOutcome::Aborted { .. }));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "cleanup_failed",
            "test_finished",
            "run_finished",
        ]
    );
    assert_eq!(
        state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "context_close:0",
            "session_close:0",
        ]
    );
}

#[tokio::test]
async fn cancellation_before_first_step_finishes_active_test_and_skips_later_tests_in_plan_order() {
    let plan = plan_with_tests(
        vec![Capability::Pure],
        vec![vec![pure(Value::Int(1))], vec![pure(Value::Int(2))]],
    );
    let control = CancelOnCheck::new(3);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(
            &plan,
            &LifecycleHost(Arc::new(LifecycleState::default())),
            Some(&control),
        )
        .await;

    assert_eq!(
        result
            .tests
            .iter()
            .map(|test| test.name.as_str())
            .collect::<Vec<_>>(),
        ["test-0", "test-1"]
    );
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Cancelled {
            reason: CancellationReason::Requested
        }
    ));
    assert!(matches!(
        result.tests[1].outcome,
        TestOutcome::Skipped {
            reason: SkipReason::RunCancelled,
            ..
        }
    ));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "test_finished",
            "test_skipped",
            "run_finished",
        ]
    );
    assert_terminal_event_invariants(&result.events);
}

#[tokio::test]
async fn cancellation_between_tests_preserves_the_completed_result_and_skips_the_remainder() {
    let plan = plan_with_tests(
        vec![Capability::Pure],
        vec![vec![pure(Value::Int(1))], vec![pure(Value::Int(2))]],
    );
    let control = CancelOnCheck::new(5);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(
            &plan,
            &LifecycleHost(Arc::new(LifecycleState::default())),
            Some(&control),
        )
        .await;

    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(matches!(
        result.tests[1].outcome,
        TestOutcome::Skipped {
            reason: SkipReason::RunCancelled,
            ..
        }
    ));
    assert_eq!((result.passed(), result.skipped()), (1, 1));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "test_finished",
            "test_skipped",
            "run_finished",
        ]
    );
    assert_terminal_event_invariants(&result.events);
}

#[tokio::test]
async fn cancellation_between_steps_keeps_the_passed_step_but_cancels_the_test() {
    let plan = plan_with_tests(
        vec![Capability::Pure],
        vec![vec![pure(Value::Int(1)), pure(Value::Int(2))]],
    );
    let control = CancelOnCheck::new(5);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(
            &plan,
            &LifecycleHost(Arc::new(LifecycleState::default())),
            Some(&control),
        )
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Cancelled {
            reason: CancellationReason::Requested
        }
    ));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "test_finished",
            "run_finished",
        ]
    );
    assert_terminal_event_invariants(&result.events);
}

#[derive(Clone, Debug)]
struct CapturedCall {
    call: ProviderCall,
    project_root: PathBuf,
    timeout: Duration,
    redacted_json_fields: Vec<String>,
}

struct RecordingProvider {
    result: Mutex<Result<Value, ProviderError>>,
    calls: Mutex<Vec<CapturedCall>>,
}

struct DelayedProvider {
    delays: Mutex<VecDeque<Duration>>,
    calls: Mutex<Vec<Duration>>,
}

impl DelayedProvider {
    fn new(delays: impl IntoIterator<Item = Duration>) -> Self {
        Self {
            delays: Mutex::new(delays.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ServerProvider for DelayedProvider {
    fn schema(&self) -> ProviderSchema {
        ProviderSchema {
            name: ProviderName("fake".into()),
            operations: [(
                "call".into(),
                OperationSchema {
                    name: OperationName("call".into()),
                    parameters: Vec::new(),
                    result: Type::Json,
                    capability: Capability::Server,
                    documentation: String::new(),
                    retry_safe: false,
                },
            )]
            .into_iter()
            .collect(),
            schema_identity: Some("schema".into()),
        }
    }

    async fn call(
        &self,
        _call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        self.calls.lock().expect("calls").push(context.timeout);
        let delay = self
            .delays
            .lock()
            .expect("delays")
            .pop_front()
            .unwrap_or(Duration::ZERO);
        tokio::time::sleep(delay).await;
        Ok(ProviderResult { value: Value::Null })
    }
}

impl RecordingProvider {
    fn new(result: Result<Value, ProviderError>) -> Self {
        Self {
            result: Mutex::new(result),
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ServerProvider for RecordingProvider {
    fn schema(&self) -> ProviderSchema {
        ProviderSchema {
            name: ProviderName("fake".into()),
            operations: [(
                "call".into(),
                OperationSchema {
                    name: OperationName("call".into()),
                    parameters: Vec::new(),
                    result: Type::Json,
                    capability: Capability::Server,
                    documentation: String::new(),
                    retry_safe: false,
                },
            )]
            .into_iter()
            .collect(),
            schema_identity: Some("schema".into()),
        }
    }

    fn transport_kind(&self) -> Option<String> {
        Some("fake".into())
    }

    async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        self.calls.lock().expect("calls").push(CapturedCall {
            call,
            project_root: context.project_root,
            timeout: context.timeout,
            redacted_json_fields: context.redacted_json_fields,
        });
        self.result
            .lock()
            .expect("provider result")
            .clone()
            .map(|value| ProviderResult { value })
    }
}

fn provider_operation(timeout: Option<Duration>) -> TestOperation {
    TestOperation::ServerProviderCall(ServerProviderCall {
        provider: "fake".into(),
        operation: "call".into(),
        arguments: [
            (
                "message".into(),
                PlanExpr::Literal(Value::String("hello".into())),
            ),
            ("token".into(), PlanExpr::Binding(BindingId(8))),
        ]
        .into_iter()
        .collect(),
        result_binding: Some(BindingId(9)),
        result_name: Some("result".into()),
        result_type: Type::Json,
        schema_hash: "schema".into(),
        timeout,
        redacted_arguments: vec!["token".into()],
        redacted_result_fields: vec!["token".into()],
        retry_safe: false,
    })
}

fn built_in_provider_operation(
    provider: &str,
    operation: &str,
    arguments: impl IntoIterator<Item = (&'static str, Value)>,
    result_type: Type,
) -> TestOperation {
    TestOperation::ServerProviderCall(ServerProviderCall {
        provider: provider.into(),
        operation: operation.into(),
        arguments: arguments
            .into_iter()
            .map(|(name, value)| (name.into(), PlanExpr::Literal(value)))
            .collect(),
        result_binding: None,
        result_name: None,
        result_type,
        schema_hash: String::new(),
        timeout: None,
        redacted_arguments: Vec::new(),
        redacted_result_fields: Vec::new(),
        retry_safe: false,
    })
}

fn secret_binding() -> TestOperation {
    TestOperation::EvaluatePure(EvaluatePureOperation {
        expression: PlanExpr::Literal(Value::String("private".into())),
        result_binding: Some(BindingId(8)),
        result_name: Some("secret_input".into()),
        result_type: Type::String,
    })
}

#[tokio::test]
async fn provider_context_events_binding_and_builder_precedence_are_preserved() {
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Record(
        [
            ("id".into(), Value::Int(7)),
            ("token".into(), Value::String("private".into())),
        ]
        .into_iter()
        .collect(),
    ))));
    let mut providers = ProviderRegistry::default();
    providers.register(provider.clone());
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![
            secret_binding(),
            provider_operation(Some(Duration::from_secs(2))),
        ]],
    );
    let state = Arc::new(LifecycleState::default());
    let options = RunnerOptions {
        project_root: PathBuf::from("project"),
        test_timeout: Duration::from_secs(19),
        ..RunnerOptions::default()
    };
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(options.clone())
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(state.log().is_empty());
    assert_eq!(
        result.tests[0].bindings["result"].member("token"),
        Some(Value::String("[redacted]".into()))
    );
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "step_started",
            "provider_started",
            "provider_finished",
            "step_passed",
            "test_finished",
            "run_finished",
        ]
    );
    let calls = provider.calls.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].call.provider.0, "fake");
    assert_eq!(calls[0].call.operation.0, "call");
    assert_eq!(calls[0].call.schema_hash, "schema");
    assert_eq!(
        calls[0].call.arguments["message"],
        Value::String("hello".into())
    );
    assert_eq!(calls[0].project_root, PathBuf::from("project"));
    assert_eq!(calls[0].timeout, Duration::from_secs(2));
    assert_eq!(calls[0].redacted_json_fields, options.redacted_json_fields);
    let ExecutionEvent::ProviderCallStarted {
        arguments,
        transport_kind,
        ..
    } = &result.events[5]
    else {
        panic!("provider start event")
    };
    assert_eq!(arguments["token"], "[redacted]");
    assert_eq!(transport_kind.as_deref(), Some("fake"));
}

#[tokio::test]
async fn provider_failure_is_redacted_before_control_events_and_observation() {
    let provider = Arc::new(RecordingProvider::new(Err(ProviderError::Application {
        code: "denied".into(),
        message: "private must never escape".into(),
        retryable: false,
        data: serde_json::json!({"token": "private"}),
    })));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let control = RecordingControl::new(false, false, true);
    let store = Arc::new(ObservationStore::default());
    let result = Runner::new(Arc::clone(&store))
        .with_provider_registry(providers)
        .run_with_control(
            &plan,
            &LifecycleHost(Arc::new(LifecycleState::default())),
            Some(&control),
        )
        .await;

    let hook_log = control.log.lock().expect("control log").join("\n");
    let hook_failure = control
        .failure
        .lock()
        .expect("failure")
        .clone()
        .expect("after failure hook");
    let reachable = format!(
        "{result:?}\n{hook_log}\n{hook_failure}\n{:?}",
        store.observations_for(plan.file, plan.source_revision)
    );
    assert!(!reachable.contains("private"), "secret leaked: {reachable}");
    assert!(hook_log.contains("argument.token"));
    assert!(hook_log.contains("[redacted]"));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "step_started",
            "provider_started",
            "provider_failed",
            "step_failed",
            "test_finished",
            "run_finished",
        ]
    );
    assert!(matches!(result.tests[0].outcome, TestOutcome::Failed(_)));
    assert_eq!(
        store
            .observations_for(plan.file, plan.source_revision)
            .len(),
        1
    );
    assert!(result.events.iter().any(|event| matches!(
        event,
        ExecutionEvent::StepFailed {
            failure_class: FailureClass::Test,
            failure: RuntimeFailure::Provider(ProviderError::Application { .. }),
            ..
        }
    )));
}

#[tokio::test]
async fn provider_timeout_uses_distinct_default_and_nested_temp_resources_are_cleaned() {
    let root = tempfile::tempdir().expect("temporary root");
    let owned = root.path().join("owned");
    std::fs::create_dir(&owned).expect("owned directory");
    std::fs::write(owned.join("value.txt"), b"temporary").expect("temporary value");
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Record(
        [(
            "nested".into(),
            Value::List(vec![Value::TempDirectory(owned.clone())]),
        )]
        .into_iter()
        .collect(),
    ))));
    let mut providers = ProviderRegistry::default();
    providers.register(provider.clone());
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let options = RunnerOptions {
        test_timeout: Duration::from_secs(19),
        provider_call_timeout: Duration::from_secs(7),
        ..RunnerOptions::default()
    };
    Runner::new(Arc::new(ObservationStore::default()))
        .with_options(options)
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert_eq!(
        provider.calls.lock().expect("calls")[0].timeout,
        Duration::from_secs(7)
    );
    assert!(!owned.exists());
}

#[tokio::test]
async fn temporary_directory_cleanup_failure_aborts_a_passing_provider_only_test() {
    let root = tempfile::tempdir().expect("temporary root");
    let claimed_directory = root.path().join("actually-a-file");
    std::fs::write(&claimed_directory, b"not a directory").expect("claimed directory file");
    let provider = Arc::new(RecordingProvider::new(Ok(Value::TempDirectory(
        claimed_directory.clone(),
    ))));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted {
            failure: RunError::Cleanup(CleanupFailure {
                resource: CleanupResource::TemporaryDirectory { ref path },
                cause: CleanupCause::Io(_),
            }),
            prior_outcome: None,
        } if path == &claimed_directory
    ));
    assert_eq!(
        event_names(&result.events),
        [
            "run_started",
            "test_started",
            "step_started",
            "step_passed",
            "step_started",
            "provider_started",
            "provider_finished",
            "step_passed",
            "cleanup_failed",
            "test_finished",
            "run_finished",
        ]
    );
    let cleanup_index = result
        .events
        .iter()
        .position(|event| matches!(event, ExecutionEvent::CleanupFailed { .. }))
        .expect("cleanup event");
    let test_finished_index = result
        .events
        .iter()
        .position(|event| matches!(event, ExecutionEvent::TestFinished { .. }))
        .expect("test terminal event");
    assert!(cleanup_index < test_finished_index);
}

#[tokio::test]
async fn temporary_directory_cleanup_is_deterministic_continues_and_retains_every_failure() {
    let root = tempfile::tempdir().expect("temporary root");
    let first_bad = root.path().join("a-bad");
    let second_bad = root.path().join("b-bad");
    let later_good = root.path().join("c-good");
    std::fs::write(&first_bad, b"file").expect("first bad target");
    std::fs::write(&second_bad, b"file").expect("second bad target");
    std::fs::create_dir(&later_good).expect("later good directory");
    let provider = Arc::new(RecordingProvider::new(Ok(Value::List(vec![
        Value::TempDirectory(second_bad.clone()),
        Value::TempDirectory(later_good.clone()),
        Value::TempDirectory(first_bad.clone()),
    ]))));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert!(!later_good.exists(), "cleanup must continue after failures");
    let TestOutcome::Aborted {
        failure: RunError::Multiple { primary, secondary },
        prior_outcome: None,
    } = &result.tests[0].outcome
    else {
        panic!("both cleanup failures should remain typed")
    };
    assert!(matches!(
        primary.as_ref(),
        RunError::Cleanup(CleanupFailure {
            resource: CleanupResource::TemporaryDirectory { path },
            ..
        }) if path == &first_bad
    ));
    assert!(matches!(
        secondary.as_slice(),
        [RunError::Cleanup(CleanupFailure {
            resource: CleanupResource::TemporaryDirectory { path },
            ..
        })] if path == &second_bad
    ));
    let cleanup_paths = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::CleanupFailed {
                resource: CleanupResource::TemporaryDirectory { path },
                ..
            } => Some(path),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(cleanup_paths, [&first_bad, &second_bad]);
}

#[tokio::test]
async fn temporary_directories_are_deduplicated_and_cleaned_after_an_infrastructure_abort() {
    let root = tempfile::tempdir().expect("temporary root");
    let owned = root.path().join("owned");
    std::fs::create_dir(&owned).expect("owned directory");
    let duplicate = owned.join(".");
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Record(
        [(
            "directories".into(),
            Value::List(vec![
                Value::TempDirectory(owned.clone()),
                Value::TempDirectory(duplicate),
            ]),
        )]
        .into_iter()
        .collect(),
    ))));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let state = Arc::new(LifecycleState::default());
    state
        .page_errors
        .lock()
        .expect("page errors")
        .insert("disconnect".into(), BrowserError::BrowserDisconnected);
    let plan = plan_with_tests(
        vec![Capability::Server, Capability::Browser],
        vec![vec![
            secret_binding(),
            provider_operation(None),
            browser_evaluate("disconnect"),
        ]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(state))
        .await;

    assert!(!owned.exists());
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted {
            failure: RunError::Browser(BrowserError::BrowserDisconnected),
            prior_outcome: None,
        }
    ));
    assert!(
        result
            .events
            .iter()
            .all(|event| !matches!(event, ExecutionEvent::CleanupFailed { .. }))
    );
}

#[tokio::test]
async fn only_provider_result_temp_directories_enter_runtime_cleanup_ownership() {
    let root = tempfile::tempdir().expect("temporary root");
    let not_owned = root.path().join("not-owned");
    std::fs::create_dir(&not_owned).expect("not-owned directory");
    let plan = plan_with_tests(
        vec![Capability::Pure],
        vec![vec![pure_binding(
            Value::TempDirectory(not_owned.clone()),
            BindingId(22),
        )]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(not_owned.exists());
}

#[tokio::test]
async fn provider_infrastructure_failure_aborts_without_recording_an_observation() {
    let provider = Arc::new(RecordingProvider::new(Err(
        ProviderError::BridgeTransport {
            message: "disconnected".into(),
        },
    )));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let store = Arc::new(ObservationStore::default());
    let result = Runner::new(Arc::clone(&store))
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert!(matches!(
        result.outcome,
        RunOutcome::Aborted {
            failure: RunError::Provider(ProviderError::BridgeTransport { .. }),
            ..
        }
    ));
    assert!(
        store
            .observations_for(plan.file, plan.source_revision)
            .is_empty()
    );
    assert!(result.events.iter().any(|event| matches!(
        event,
        ExecutionEvent::StepFailed {
            failure_class: FailureClass::Infrastructure,
            failure: RuntimeFailure::Provider(ProviderError::BridgeTransport { .. }),
            ..
        }
    )));
    assert!(matches!(
        result.events.last(),
        Some(ExecutionEvent::RunFinished {
            failure_class: Some(FailureClass::Infrastructure),
            ..
        })
    ));
}

#[tokio::test]
async fn explicit_registry_survives_options_in_both_builder_orders() {
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Null)));
    let mut providers = ProviderRegistry::default();
    providers.register(provider.clone());
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let options = RunnerOptions {
        project_root: PathBuf::from("configured-project"),
        ..RunnerOptions::default()
    };
    let registry_then_options = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(providers.clone())
        .with_options(options.clone())
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;
    let options_then_registry = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(options)
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert_eq!(registry_then_options.passed(), 1);
    assert_eq!(options_then_registry.passed(), 1);
    let calls = provider.calls.lock().expect("calls");
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|call| call.project_root == Path::new("configured-project"))
    );
}

#[tokio::test]
async fn repeated_options_preserve_explicit_registry_and_last_options_win() {
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Null)));
    let mut providers = ProviderRegistry::default();
    providers.register(provider.clone());
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            project_root: PathBuf::from("first-project"),
            provider_call_timeout: Duration::from_secs(3),
            ..RunnerOptions::default()
        })
        .with_provider_registry(providers)
        .with_options(RunnerOptions {
            project_root: PathBuf::from("final-project"),
            provider_call_timeout: Duration::from_secs(7),
            ..RunnerOptions::default()
        })
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert_eq!(result.passed(), 1);
    let calls = provider.calls.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].project_root, PathBuf::from("final-project"));
    assert_eq!(calls[0].timeout, Duration::from_secs(7));
}

#[tokio::test]
async fn repeated_explicit_registries_use_the_last_registry() {
    let first_provider = Arc::new(RecordingProvider::new(Ok(Value::Null)));
    let second_provider = Arc::new(RecordingProvider::new(Ok(Value::Null)));
    let mut first_registry = ProviderRegistry::default();
    first_registry.register(first_provider.clone());
    let mut second_registry = ProviderRegistry::default();
    second_registry.register(second_provider.clone());
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(first_registry)
        .with_provider_registry(second_registry)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert_eq!(result.passed(), 1);
    assert!(first_provider.calls.lock().expect("first calls").is_empty());
    assert_eq!(second_provider.calls.lock().expect("second calls").len(), 1);
}

#[tokio::test]
async fn event_sink_commutes_with_provider_and_options_setters() {
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Null)));
    let mut providers = ProviderRegistry::default();
    providers.register(provider.clone());
    let sink = Arc::new(RecordingEventSink::default());
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_event_sink(sink.clone())
        .with_provider_registry(providers)
        .with_options(RunnerOptions::default())
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert_eq!(result.passed(), 1);
    assert_eq!(provider.calls.lock().expect("calls").len(), 1);
    assert_eq!(*sink.events.lock().expect("events"), result.events);
}

#[tokio::test]
async fn final_options_configure_all_built_in_providers_without_an_explicit_registry() {
    let root = tempfile::tempdir().expect("temporary project root");
    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        return;
    };
    let address = listener.local_addr().expect("HTTP fixture address");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (mut stream, _) = listener.accept().await.expect("HTTP fixture request");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .expect("HTTP fixture response");
    });

    let final_options = RunnerOptions {
        project_root: root.path().to_path_buf(),
        provider_config: NativeProviderConfig {
            http: HttpProviderConfig {
                base_url: Some(format!("http://{address}")),
                ..HttpProviderConfig::default()
            },
            process: ProcessProviderConfig {
                allowed_working_roots: Vec::new(),
                ..ProcessProviderConfig::default()
            },
            fs: FsProviderConfig {
                write_root: PathBuf::from("generated"),
                ..FsProviderConfig::default()
            },
        },
        ..RunnerOptions::default()
    };
    let initial_options = RunnerOptions {
        project_root: root.path().to_path_buf(),
        ..RunnerOptions::default()
    };
    let host = LifecycleHost(Arc::new(LifecycleState::default()));

    let http_plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![built_in_provider_operation(
            "http",
            "get",
            [("url", Value::String("/health".into()))],
            Type::Response(Box::new(Type::Json)),
        )]],
    );
    let http_result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(initial_options.clone())
        .with_options(final_options.clone())
        .run(&http_plan, &host)
        .await;
    assert_eq!(http_result.passed(), 1);

    let process_plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![built_in_provider_operation(
            "process",
            "run",
            [
                ("executable", Value::String("not-started".into())),
                ("cwd", Value::String(".".into())),
            ],
            Type::ProcessResult,
        )]],
    );
    let process_result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(initial_options.clone())
        .with_options(final_options.clone())
        .run(&process_plan, &host)
        .await;
    assert!(matches!(
        process_result.tests[0].outcome,
        TestOutcome::Failed(ref failure)
            if matches!(failure.error, StepError::Provider(ProviderError::PathEscape { .. }))
    ));

    let fs_plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![built_in_provider_operation(
            "fs",
            "write_text",
            [
                ("path", Value::String("generated/result.txt".into())),
                ("contents", Value::String("final configuration".into())),
            ],
            Type::FilePath,
        )]],
    );
    let fs_result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(initial_options)
        .with_options(final_options)
        .run(&fs_plan, &host)
        .await;
    assert_eq!(fs_result.passed(), 1);
    assert_eq!(
        std::fs::read_to_string(root.path().join("generated/result.txt"))
            .expect("configured filesystem output"),
        "final configuration"
    );
}

#[tokio::test(start_paused = true)]
async fn each_test_gets_its_own_deadline_even_when_aggregate_duration_exceeds_it() {
    let provider = Arc::new(DelayedProvider::new([
        Duration::from_secs(4),
        Duration::from_secs(4),
    ]));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![
            vec![secret_binding(), provider_operation(None)],
            vec![secret_binding(), provider_operation(None)],
        ],
    );
    let options = RunnerOptions {
        test_timeout: Duration::from_secs(5),
        provider_call_timeout: Duration::from_secs(5),
        ..RunnerOptions::default()
    };

    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(options)
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert_eq!(result.passed(), 2);
    assert_eq!(result.timed_out(), 0);
}

#[tokio::test(start_paused = true)]
async fn cumulative_steps_share_one_deadline_and_late_provider_uses_remaining_time() {
    let provider = Arc::new(DelayedProvider::new([
        Duration::from_secs(4),
        Duration::from_secs(4),
    ]));
    let mut providers = ProviderRegistry::default();
    providers.register(provider.clone());
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![
            secret_binding(),
            provider_operation(None),
            provider_operation(Some(Duration::from_secs(10))),
        ]],
    );
    let store = Arc::new(ObservationStore::default());
    let result = Runner::new(Arc::clone(&store))
        .with_options(RunnerOptions {
            test_timeout: Duration::from_secs(6),
            provider_call_timeout: Duration::from_secs(5),
            ..RunnerOptions::default()
        })
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::TimedOut {
            timeout,
            active_step: Some(StepId(2)),
        } if timeout == Duration::from_secs(6)
    ));
    let calls = provider.calls.lock().expect("calls");
    assert_eq!(calls[0], Duration::from_secs(5));
    assert!(calls[1] <= Duration::from_secs(2));
    assert!(matches!(
        store.observations_for(plan.file, plan.source_revision)[0].kind,
        RuntimeObservationKind::TestTimeout {
            timeout_ms: 6_000,
            active_step: Some(StepId(2)),
        }
    ));
    assert!(result.events.iter().any(|event| matches!(
        event,
        ExecutionEvent::TestTimedOut {
            active_step: Some(StepId(2)),
            timeout_ms: 6_000,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn late_browser_action_wait_and_navigation_receive_only_remaining_time() {
    let provider = Arc::new(DelayedProvider::new([Duration::from_secs(4)]));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let locator = webtest_plan::Locator::Id("target".into());
    let plan = plan_with_tests(
        vec![Capability::Server, Capability::Browser],
        vec![vec![
            secret_binding(),
            provider_operation(None),
            TestOperation::Browser(BrowserOperation::Click {
                locator: locator.clone(),
            }),
            TestOperation::Browser(BrowserOperation::WaitForLocator {
                locator,
                state: webtest_plan::LocatorState::Visible,
                timeout: None,
            }),
            TestOperation::Browser(BrowserOperation::Navigate {
                url: PlanExpr::Literal(Value::String("/late".into())),
            }),
        ]],
    );
    let state = Arc::new(LifecycleState::default());
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            base_url: Some("http://example.test".into()),
            test_timeout: Duration::from_secs(6),
            provider_call_timeout: Duration::from_secs(5),
            action_timeout: Duration::from_secs(5),
            assertion_timeout: Duration::from_secs(5),
            navigation_timeout: Duration::from_secs(5),
            ..RunnerOptions::default()
        })
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    let timeouts = state
        .operation_timeouts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(timeouts.len(), 3);
    assert_eq!(
        timeouts
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["action", "wait", "navigation",]
    );
    assert!(
        timeouts.iter().all(|(_, timeout)| {
            *timeout > Duration::ZERO && *timeout <= Duration::from_secs(2)
        })
    );
}

#[tokio::test(start_paused = true)]
async fn timeout_during_page_acquisition_closes_context_and_tainted_session() {
    let state = Arc::new(LifecycleState::default());
    state
        .page_creation_delays
        .lock()
        .expect("page delays")
        .insert(0, Duration::from_secs(5));
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("unreached")]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            test_timeout: Duration::from_secs(2),
            ..RunnerOptions::default()
        })
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::TimedOut {
            active_step: None,
            ..
        }
    ));
    assert_eq!(
        state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "context_close:0",
            "session_close:0",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn timed_out_browser_test_gets_a_fresh_session_for_the_next_browser_test() {
    let state = Arc::new(LifecycleState::default());
    state
        .page_delays
        .lock()
        .expect("page delays")
        .insert("slow".into(), Duration::from_secs(5));
    let plan = plan_with_tests(
        vec![Capability::Pure, Capability::Browser],
        vec![
            vec![browser_evaluate("slow")],
            vec![pure(Value::String("between browser tests".into()))],
            vec![browser_evaluate("fast")],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            test_timeout: Duration::from_secs(2),
            ..RunnerOptions::default()
        })
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::TimedOut { .. }
    ));
    assert!(matches!(result.tests[1].outcome, TestOutcome::Passed));
    assert!(matches!(result.tests[2].outcome, TestOutcome::Passed));
    let log = state.log();
    assert!(log.contains(&"session_start:0".into()));
    assert!(log.contains(&"session_close:0".into()));
    assert!(log.contains(&"session_start:1".into()));
    assert!(log.contains(&"session_close:1".into()));
}

#[tokio::test(start_paused = true)]
async fn provider_only_timeout_does_not_taint_an_existing_browser_session() {
    let provider = Arc::new(DelayedProvider::new([Duration::from_secs(5)]));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(
        vec![Capability::Browser, Capability::Server],
        vec![
            vec![browser_evaluate("first")],
            vec![secret_binding(), provider_operation(None)],
            vec![browser_evaluate("last")],
        ],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            test_timeout: Duration::from_secs(2),
            provider_call_timeout: Duration::from_secs(2),
            ..RunnerOptions::default()
        })
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await;

    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(matches!(
        result.tests[1].outcome,
        TestOutcome::TimedOut { .. }
    ));
    assert!(matches!(result.tests[2].outcome, TestOutcome::Passed));
    assert_eq!(
        state
            .log()
            .into_iter()
            .filter(|entry| entry.starts_with("session_start:"))
            .collect::<Vec<_>>(),
        ["session_start:0"]
    );
}

#[tokio::test(start_paused = true)]
async fn cleanup_failure_after_timeout_outranks_and_retains_the_timeout() {
    let state = Arc::new(LifecycleState::default());
    state
        .page_delays
        .lock()
        .expect("page delays")
        .insert("slow".into(), Duration::from_secs(5));
    state
        .context_close_failures
        .lock()
        .expect("context failures")
        .insert(0);
    let plan = plan_with_tests(
        vec![Capability::Browser],
        vec![vec![browser_evaluate("slow")]],
    );
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            test_timeout: Duration::from_secs(2),
            ..RunnerOptions::default()
        })
        .run(&plan, &LifecycleHost(state))
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted {
            failure: RunError::Cleanup(CleanupFailure {
                resource: CleanupResource::BrowserContext,
                ..
            }),
            prior_outcome: Some(ref prior),
        } if matches!(prior.as_ref(), PriorTestOutcome::TimedOut {
            timeout,
            active_step: Some(StepId(0)),
        } if *timeout == Duration::from_secs(2))
    ));
}

struct DeadlinePauseControl {
    timeout_notified: AtomicBool,
}

#[async_trait]
impl RunControl for DeadlinePauseControl {
    async fn before_step(&self, _test: &PlannedTest, _step: &PlannedStep) {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    fn after_test_timeout(&self, _test: &PlannedTest, active_step: Option<&PlannedStep>) {
        assert_eq!(active_step.map(|step| step.id), Some(StepId(0)));
        self.timeout_notified.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn debugger_pause_time_counts_toward_the_single_test_deadline() {
    let control = DeadlinePauseControl {
        timeout_notified: AtomicBool::new(false),
    };
    let plan = plan_with_tests(vec![Capability::Pure], vec![vec![pure(Value::Null)]]);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .with_options(RunnerOptions {
            test_timeout: Duration::from_secs(2),
            ..RunnerOptions::default()
        })
        .run_with_control(
            &plan,
            &LifecycleHost(Arc::new(LifecycleState::default())),
            Some(&control),
        )
        .await;

    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::TimedOut {
            active_step: Some(StepId(0)),
            ..
        }
    ));
    assert!(control.timeout_notified.load(Ordering::SeqCst));
}
