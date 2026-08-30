use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use webtest_browser::{
    BrowserContext, BrowserContextOptions, BrowserError, BrowserHost, BrowserSession,
    EvidenceRequest, InspectionOptions, Locator, Page, PageEvidence, PageInspection,
};
use webtest_hir::{BindingId, StepId, TestId};
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeObservation, RuntimeObservationKind,
};
use webtest_plan::{
    BrowserOperation, EvaluatePureOperation, PlanExpr, PlannedStep, PlannedTest,
    ServerProviderCall, TestOperation, TestPlan,
};
use webtest_provider::{
    CallContext, Capability, OperationName, OperationSchema, ProviderCall, ProviderError,
    ProviderName, ProviderRegistry, ProviderResult, ProviderSchema, ServerProvider, Type, Value,
};
use webtest_runtime::{RunControl, RunError, Runner, RunnerOptions, StepError};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

#[derive(Default)]
struct LifecycleState {
    log: Mutex<Vec<String>>,
    next_session: AtomicUsize,
    next_context: AtomicUsize,
    context_close_failures: Mutex<BTreeSet<usize>>,
    page_errors: Mutex<BTreeMap<String, BrowserError>>,
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
        Ok(())
    }
}

#[async_trait]
impl BrowserContext for LifecycleContext {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        self.state.push(format!("page_create:{}", self.id));
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
        self.state
            .page_errors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(expression)
            .cloned()
            .map_or(Ok(()), Err)
    }

    async fn capture_evidence(&mut self, request: &EvidenceRequest) -> PageEvidence {
        self.state.push(format!(
            "evidence:{}:{}:{}:{}",
            self.context_id, request.include_screenshot, request.include_dom, request.max_dom_bytes
        ));
        PageEvidence::default()
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

fn plan_with_tests(capabilities: Vec<Capability>, operations: Vec<Vec<TestOperation>>) -> TestPlan {
    let file = FileId::new(41);
    let mut next_step = 0;
    let tests = operations
        .into_iter()
        .enumerate()
        .map(|(test_index, operations)| PlannedTest {
            id: TestId(test_index as u32),
            name: format!("test-{test_index}"),
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
        })
        .collect();
    TestPlan {
        file,
        source_revision: SourceRevision::of("lifecycle"),
        required_host_capabilities: capabilities,
        tests,
    }
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
            ExecutionEvent::TestFinished { .. } => "test_finished",
            ExecutionEvent::RunFinished { .. } => "run_finished",
        })
        .collect()
}

#[tokio::test]
async fn provider_only_plan_never_starts_a_browser() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(vec![Capability::Pure], vec![vec![pure(Value::Null)]]);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await
        .expect("provider-free run");

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
        .await
        .expect("browser run");

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
        .await
        .expect("ordinary failure remains a run result");

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
    let error = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await
        .expect_err("infrastructure failure aborts the run");

    assert!(matches!(
        error,
        RunError::Browser(BrowserError::BrowserDisconnected)
    ));
    let log = state.log();
    assert!(log.iter().any(|event| event == "context_close:0"));
    assert!(!log.iter().any(|event| event.contains("unreached")));
    assert!(!log.iter().any(|event| event.starts_with("evidence:")));
    assert!(!log.iter().any(|event| event.starts_with("inspect:")));
}

#[tokio::test]
async fn context_close_failure_restarts_the_session_before_the_next_test() {
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
    Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await
        .expect("restart succeeds");

    assert_eq!(
        state.log(),
        [
            "session_start:0",
            "context_create:0:0",
            "page_create:0",
            "evaluate:0:first",
            "context_close:0",
            "session_close:0",
            "session_start:1",
            "context_create:1:1",
            "page_create:1",
            "evaluate:1:second",
            "context_close:1",
            "session_close:1",
        ]
    );
}

#[tokio::test]
async fn zero_test_browser_plan_starts_and_normally_closes_its_session() {
    let state = Arc::new(LifecycleState::default());
    let plan = plan_with_tests(vec![Capability::Browser], Vec::new());
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &LifecycleHost(Arc::clone(&state)))
        .await
        .expect("empty run");

    assert!(result.tests.is_empty());
    assert_eq!(state.log(), ["session_start:0", "session_close:0"]);
    assert_eq!(event_names(&result.events), ["run_started", "run_finished"]);
}

struct FailingStartHost;

#[async_trait]
impl BrowserHost for FailingStartHost {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
        Err(BrowserError::Launch("cannot launch".into()))
    }
}

#[tokio::test]
async fn observations_are_cleared_before_browser_start_can_fail() {
    let store = Arc::new(ObservationStore::default());
    let plan = plan_with_tests(vec![Capability::Browser], Vec::new());
    store.record(RuntimeObservation {
        execution_id: ExecutionId::next(),
        file: plan.file,
        source_revision: plan.source_revision,
        test_id: TestId(0),
        step_id: StepId(0),
        range: TextRange::default(),
        kind: RuntimeObservationKind::ValueFailure {
            code: "stale".into(),
            message: "stale".into(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        },
    });

    let error = Runner::new(Arc::clone(&store))
        .run(&plan, &FailingStartHost)
        .await
        .expect_err("browser start fails");
    assert!(matches!(error, RunError::Browser(BrowserError::Launch(_))));
    assert!(
        store
            .observations_for(plan.file, plan.source_revision)
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
        .await
        .expect("cancelled run");
    assert!(result.tests.is_empty());
    assert_eq!(event_names(&result.events), ["run_started", "run_finished"]);

    let after_hook = RecordingControl::new(false, true, false);
    let result = Runner::new(Arc::new(ObservationStore::default()))
        .run_with_control(&plan, &LifecycleHost(state), Some(&after_hook))
        .await
        .expect("hook cancellation");
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
        .await
        .expect("custom registry applied last");

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
        .await
        .expect("application error is a test failure");

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
}

#[tokio::test]
async fn provider_timeout_falls_back_to_test_timeout_and_nested_temp_resources_are_cleaned() {
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
        ..RunnerOptions::default()
    };
    Runner::new(Arc::new(ObservationStore::default()))
        .with_options(options)
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await
        .expect("provider run");

    assert_eq!(
        provider.calls.lock().expect("calls")[0].timeout,
        Duration::from_secs(19)
    );
    assert!(!owned.exists());
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
    let error = Runner::new(Arc::clone(&store))
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await
        .expect_err("transport failure aborts");

    assert!(matches!(
        error,
        RunError::Provider(ProviderError::BridgeTransport { .. })
    ));
    assert!(
        store
            .observations_for(plan.file, plan.source_revision)
            .is_empty()
    );
}

#[tokio::test]
async fn with_options_rebuilds_builtins_before_a_later_registry_override() {
    let provider = Arc::new(RecordingProvider::new(Ok(Value::Null)));
    let mut providers = ProviderRegistry::default();
    providers.register(provider);
    let plan = plan_with_tests(
        vec![Capability::Server],
        vec![vec![secret_binding(), provider_operation(None)]],
    );
    let error = Runner::new(Arc::new(ObservationStore::default()))
        .with_provider_registry(providers)
        .with_options(RunnerOptions::default())
        .run(&plan, &LifecycleHost(Arc::new(LifecycleState::default())))
        .await
        .expect_err("with_options replaces the earlier custom registry");
    assert!(matches!(
        error,
        RunError::Provider(ProviderError::NotRegistered { provider }) if provider == "fake"
    ));
}
