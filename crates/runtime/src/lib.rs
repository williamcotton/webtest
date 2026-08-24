//! Sequential execution of protocol-neutral test plans.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use thiserror::Error;
use tracing::instrument;
use webtest_browser::{
    Action, BrowserContextOptions, BrowserError, BrowserHost, EvidenceRequest, InspectionOptions,
    Locator as BrowserLocator, LocatorState as BrowserLocatorState, Page, PageEvidence,
    PageInspection, RepairHint, RepairHintKind, locator_repair_hints,
};
use webtest_hir::{BinaryOperator, BindingId, UnaryOperator};
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeFailure, RuntimeObservation,
    RuntimeObservationKind, ValueDiff,
};
use webtest_plan::{
    AssertionOperation, BrowserOperation, Locator, LocatorState, PlanExpr, PlannedStep,
    ServerProviderCall, TestOperation, TestPlan, ValueMatcher,
};
use webtest_provider::{
    CallContext, Capability, NativeProviderConfig, OperationName, ProviderCall, ProviderError,
    ProviderName, ProviderRegistry, Type, Value,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Screenshot,
    DomSnapshot,
    Evidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StepFailure {
    pub step: PlannedStep,
    pub error: StepError,
    pub evidence: PageEvidence,
    pub artifacts: Vec<Artifact>,
    pub inspection: Option<PageInspection>,
    pub repair_hints: Vec<RepairHint>,
    pub secondary_failures: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum StepError {
    Browser(BrowserError),
    Provider(ProviderError),
    Assertion(Box<AssertionFailure>),
    Decode(DecodeFailure),
    Evaluation(EvaluationFailure),
    Internal(String),
}

impl StepError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Browser(error) => error.code(),
            Self::Provider(error) => error.code(),
            Self::Assertion(_) => "assertion_failed",
            Self::Decode(_) => "json_decode_failed",
            Self::Evaluation(error) => error.code,
            Self::Internal(_) => "internal_error",
        }
    }

    pub fn is_infrastructure(&self) -> bool {
        match self {
            Self::Browser(error) => error.is_infrastructure(),
            Self::Provider(error) => error.is_infrastructure(),
            Self::Assertion(_) | Self::Decode(_) | Self::Evaluation(_) | Self::Internal(_) => false,
        }
    }
}

impl std::fmt::Display for StepError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Browser(error) => error.fmt(formatter),
            Self::Provider(error) => error.fmt(formatter),
            Self::Assertion(error) => error.fmt(formatter),
            Self::Decode(error) => error.fmt(formatter),
            Self::Evaluation(error) => error.fmt(formatter),
            Self::Internal(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for StepError {}

#[derive(Clone, Debug)]
pub struct AssertionFailure {
    pub matcher: ValueMatcher,
    pub expected: Option<Value>,
    pub actual: Value,
    pub message: String,
    pub diff: ValueDiff,
}

impl std::fmt::Display for AssertionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Debug)]
pub struct DecodeFailure {
    pub path: String,
    pub expected: Type,
    pub actual: String,
    pub response_operation: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EvaluationFailure {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for EvaluationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::fmt::Display for DecodeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "JSON decode failed at {}: expected {}, got {}",
            self.path, self.expected, self.actual
        )
    }
}

#[derive(Debug, Error)]
pub enum RunError {
    #[error(transparent)]
    Browser(#[from] BrowserError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("internal runtime error: {0}")]
    Internal(String),
}

impl RunError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Browser(error) => error.code(),
            Self::Provider(error) => error.code(),
            Self::Internal(_) => "internal_error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub failure: Option<StepFailure>,
    pub duration: Duration,
    pub bindings: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct RunResult {
    pub execution_id: ExecutionId,
    pub tests: Vec<TestResult>,
    pub events: Vec<ExecutionEvent>,
    pub duration: Duration,
}

impl RunResult {
    pub fn passed(&self) -> usize {
        self.tests.iter().filter(|result| result.passed).count()
    }

    pub fn failed(&self) -> usize {
        self.tests.len() - self.passed()
    }
}

#[derive(Clone, Debug)]
pub struct EvidenceOptions {
    pub screenshot_on_failure: bool,
    pub dom_snapshot_on_failure: bool,
    pub max_dom_bytes: usize,
    pub artifact_directory: PathBuf,
}

impl Default for EvidenceOptions {
    fn default() -> Self {
        Self {
            screenshot_on_failure: false,
            dom_snapshot_on_failure: false,
            max_dom_bytes: 1_048_576,
            artifact_directory: PathBuf::from(".webtest/artifacts"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunnerOptions {
    pub base_url: Option<String>,
    pub action_timeout: Duration,
    pub assertion_timeout: Duration,
    pub test_timeout: Duration,
    pub browser_context: BrowserContextOptions,
    pub evidence: EvidenceOptions,
    pub project_root: PathBuf,
    pub redacted_json_fields: Vec<String>,
    pub provider_config: NativeProviderConfig,
    pub inspection: InspectionOptions,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            base_url: None,
            action_timeout: Duration::from_secs(5),
            assertion_timeout: Duration::from_secs(5),
            test_timeout: Duration::from_secs(60),
            browser_context: BrowserContextOptions::default(),
            evidence: EvidenceOptions::default(),
            project_root: PathBuf::from("."),
            redacted_json_fields: vec![
                "password".into(),
                "token".into(),
                "secret".into(),
                "authorization".into(),
                "cookie".into(),
                "set-cookie".into(),
            ],
            provider_config: NativeProviderConfig::default(),
            inspection: InspectionOptions::default(),
        }
    }
}

pub struct Runner {
    observations: Arc<ObservationStore>,
    options: RunnerOptions,
    providers: ProviderRegistry,
}

#[async_trait]
pub trait RunControl: Send + Sync {
    async fn before_step(&self, test: &webtest_plan::PlannedTest, step: &PlannedStep);

    async fn before_step_with_bindings(
        &self,
        test: &webtest_plan::PlannedTest,
        step: &PlannedStep,
        _bindings: &BTreeMap<String, Value>,
    ) {
        self.before_step(test, step).await;
    }
}

impl Runner {
    pub fn new(observations: Arc<ObservationStore>) -> Self {
        Self {
            observations,
            options: RunnerOptions::default(),
            providers: ProviderRegistry::built_in(NativeProviderConfig::default()),
        }
    }

    pub fn with_options(mut self, options: RunnerOptions) -> Self {
        self.providers = ProviderRegistry::built_in(options.provider_config.clone());
        self.options = options;
        self
    }

    pub fn with_provider_registry(mut self, providers: ProviderRegistry) -> Self {
        self.providers = providers;
        self
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
    ) -> Result<RunResult, RunError> {
        self.run_with_control(plan, browser, None).await
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run_with_control(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
        control: Option<&dyn RunControl>,
    ) -> Result<RunResult, RunError> {
        self.observations.clear_for_file(plan.file);
        let run_started = std::time::Instant::now();
        let execution_id = ExecutionId::next();
        let mut events = vec![ExecutionEvent::RunStarted { execution_id }];
        let needs_browser = plan
            .required_host_capabilities
            .contains(&Capability::Browser);
        let mut session = if needs_browser {
            Some(browser.start().await?)
        } else {
            None
        };
        let mut tests = Vec::with_capacity(plan.tests.len());

        for (index, test) in plan.tests.iter().enumerate() {
            let (result, tainted) = self
                .run_test(plan, test, execution_id, &mut events, &mut session, control)
                .await?;
            tests.push(result);
            if tainted && index + 1 < plan.tests.len() {
                if let Some(mut current) = session.take() {
                    let _ = current.close().await;
                }
                session = Some(browser.start().await?);
            }
        }

        events.push(ExecutionEvent::RunFinished { execution_id });
        if let Some(mut session) = session {
            session.close().await?;
        }
        Ok(RunResult {
            execution_id,
            tests,
            events,
            duration: run_started.elapsed(),
        })
    }

    async fn run_test(
        &self,
        plan: &TestPlan,
        test: &webtest_plan::PlannedTest,
        execution_id: ExecutionId,
        events: &mut Vec<ExecutionEvent>,
        session: &mut Option<Box<dyn webtest_browser::BrowserSession>>,
        control: Option<&dyn RunControl>,
    ) -> Result<(TestResult, bool), RunError> {
        let test_started = std::time::Instant::now();
        events.push(ExecutionEvent::TestStarted {
            execution_id,
            test_id: test.id,
            name: test.name.clone(),
        });
        let mut context = if let Some(session) = session.as_deref_mut() {
            Some(session.new_context(&self.options.browser_context).await?)
        } else {
            None
        };
        let mut page = if let Some(context) = context.as_mut() {
            Some(context.new_page().await?)
        } else {
            None
        };
        let mut failure = None;
        let mut environment = HashMap::new();
        let mut binding_names = HashMap::new();
        let mut secrets = Vec::new();

        for step in &test.steps {
            if let TestOperation::ServerProviderCall(call) = &step.operation {
                collect_provider_secrets(
                    call,
                    &environment,
                    &self.options.redacted_json_fields,
                    &mut secrets,
                );
            }
            if let Some(control) = control {
                let bindings = visible_bindings(
                    &environment,
                    &binding_names,
                    &self.options.redacted_json_fields,
                    &secrets,
                );
                control
                    .before_step_with_bindings(test, step, &bindings)
                    .await;
            }
            events.push(ExecutionEvent::StepStarted {
                execution_id,
                test_id: test.id,
                step_id: step.id,
            });
            if let TestOperation::ServerProviderCall(call) = &step.operation {
                events.push(ExecutionEvent::ProviderCallStarted {
                    execution_id,
                    test_id: test.id,
                    step_id: step.id,
                    provider: call.provider.clone(),
                    operation: call.operation.clone(),
                });
            }
            let step_started = std::time::Instant::now();
            let result = self
                .execute_step(&mut page, step, &mut environment, &mut binding_names)
                .await;
            match result {
                Ok(()) => {
                    if let TestOperation::ServerProviderCall(call) = &step.operation {
                        events.push(ExecutionEvent::ProviderCallFinished {
                            execution_id,
                            test_id: test.id,
                            step_id: step.id,
                            provider: call.provider.clone(),
                            operation: call.operation.clone(),
                            elapsed_ms: duration_millis(step_started.elapsed()),
                        });
                    }
                    events.push(ExecutionEvent::StepPassed {
                        execution_id,
                        test_id: test.id,
                        step_id: step.id,
                    });
                }
                Err(error) => {
                    let error = redact_step_error(
                        error,
                        &self.options.redacted_json_fields,
                        &secrets,
                        &self.options.inspection.redacted_query_parameters,
                    );
                    let mut evidence =
                        if matches!(error, StepError::Browser(_)) && !error.is_infrastructure() {
                            if let Some(page) = page.as_deref_mut() {
                                page.capture_evidence(&EvidenceRequest {
                                    locator: step_browser_locator(step),
                                    include_screenshot: self.options.evidence.screenshot_on_failure,
                                    include_dom: self.options.evidence.dom_snapshot_on_failure,
                                    max_dom_bytes: self.options.evidence.max_dom_bytes,
                                    redactions: secrets.clone(),
                                    redacted_query_parameters: self
                                        .options
                                        .inspection
                                        .redacted_query_parameters
                                        .clone(),
                                })
                                .await
                            } else {
                                PageEvidence::default()
                            }
                        } else {
                            PageEvidence::default()
                        };
                    let (inspection, secondary_failures) = if matches!(error, StepError::Browser(_))
                        && !error.is_infrastructure()
                    {
                        if let Some(page) = page.as_deref_mut() {
                            let mut options = self.options.inspection.clone();
                            options.redacted_values.extend(secrets.clone());
                            match page.inspect(&options).await {
                                Ok(inspection) => (Some(inspection), Vec::new()),
                                Err(secondary) => (
                                    None,
                                    vec![format!("semantic inspection unavailable: {secondary}")],
                                ),
                            }
                        } else {
                            (
                                None,
                                vec!["semantic inspection unavailable: page is closed".into()],
                            )
                        }
                    } else {
                        (None, Vec::new())
                    };
                    let mut repair_hints = inspection
                        .as_ref()
                        .map(|inspection| repair_hints_for_error(&error, inspection))
                        .unwrap_or_default();
                    for hint in &mut repair_hints {
                        hint.source_range = Some(webtest_feedback::ByteRange {
                            start: step.origin.range.start().into(),
                            end: step.origin.range.end().into(),
                        });
                    }
                    if !self.options.evidence.screenshot_on_failure {
                        evidence.screenshot_png = None;
                    }
                    let artifacts = if self.options.evidence.screenshot_on_failure
                        || self.options.evidence.dom_snapshot_on_failure
                    {
                        write_artifacts(
                            &self.options.evidence.artifact_directory,
                            execution_id,
                            test.id,
                            step.id,
                            &mut evidence,
                        )
                    } else {
                        Vec::new()
                    };
                    let elapsed_ms = duration_millis(step_started.elapsed());
                    if let TestOperation::ServerProviderCall(call) = &step.operation {
                        events.push(ExecutionEvent::ProviderCallFailed {
                            execution_id,
                            test_id: test.id,
                            step_id: step.id,
                            provider: call.provider.clone(),
                            operation: call.operation.clone(),
                            code: error.code().into(),
                            message: error.to_string(),
                            elapsed_ms,
                        });
                    }
                    if !error.is_infrastructure() {
                        self.record_observation(
                            plan,
                            test.id,
                            step,
                            execution_id,
                            &error,
                            &evidence,
                            &repair_hints,
                            &artifacts,
                            elapsed_ms,
                        );
                    }
                    events.push(ExecutionEvent::StepFailed {
                        execution_id,
                        test_id: test.id,
                        step_id: step.id,
                        failure: runtime_failure(&error),
                        repair_hints: repair_hints.clone(),
                        page: inspection
                            .as_ref()
                            .map(|inspection| inspection.page.clone()),
                    });
                    if error.is_infrastructure() {
                        drop(page);
                        if let Some(context) = context.as_mut() {
                            let _ = context.close().await;
                        }
                        return Err(match error {
                            StepError::Browser(error) => RunError::Browser(error),
                            StepError::Provider(error) => RunError::Provider(error),
                            other => RunError::Internal(other.to_string()),
                        });
                    }
                    failure = Some(StepFailure {
                        step: step.clone(),
                        error,
                        evidence,
                        artifacts,
                        inspection,
                        repair_hints,
                        secondary_failures,
                    });
                    break;
                }
            }
        }

        drop(page);
        let cleanup_failed = if let Some(context) = context.as_mut() {
            context.close().await.is_err()
        } else {
            false
        };
        let passed = failure.is_none();
        events.push(ExecutionEvent::TestFinished {
            execution_id,
            test_id: test.id,
            passed,
        });
        let bindings = binding_names
            .into_iter()
            .filter_map(|(id, name)| {
                environment
                    .get(&id)
                    .filter(|value| runtime_transferable(value))
                    .map(|value| {
                        value.redacted_with_secrets(&self.options.redacted_json_fields, &secrets)
                    })
                    .map(|value| (name, value))
            })
            .collect();
        for directory in environment.values().flat_map(temporary_directories) {
            tokio::fs::remove_dir_all(&directory)
                .await
                .map_err(|error| {
                    RunError::Provider(ProviderError::Filesystem {
                        path: directory.display().to_string(),
                        message: format!("temporary resource cleanup failed: {error}"),
                    })
                })?;
        }
        Ok((
            TestResult {
                name: test.name.clone(),
                passed,
                failure,
                duration: test_started.elapsed(),
                bindings,
            },
            cleanup_failed,
        ))
    }

    async fn execute_step(
        &self,
        page: &mut Option<Box<dyn Page>>,
        step: &PlannedStep,
        environment: &mut HashMap<BindingId, Value>,
        binding_names: &mut HashMap<BindingId, String>,
    ) -> Result<(), StepError> {
        match &step.operation {
            TestOperation::EvaluatePure(operation) => {
                let value = evaluate(&operation.expression, environment)?;
                if let Some(binding) = operation.result_binding {
                    environment.insert(binding, value);
                    binding_names.insert(
                        binding,
                        operation
                            .result_name
                            .clone()
                            .unwrap_or_else(|| format!("binding_{}", binding.0)),
                    );
                }
                Ok(())
            }
            TestOperation::ServerProviderCall(call) => {
                let value = self.execute_provider(call, environment).await?;
                if let Some(binding) = call.result_binding {
                    environment.insert(binding, value);
                    binding_names.insert(
                        binding,
                        call.result_name
                            .clone()
                            .unwrap_or_else(|| format!("binding_{}", binding.0)),
                    );
                }
                Ok(())
            }
            TestOperation::Browser(operation) => {
                let page = page.as_deref_mut().ok_or_else(|| {
                    StepError::Internal("browser operation has no browser page".into())
                })?;
                execute_browser(page, operation, environment, &self.options).await
            }
            TestOperation::Assertion(assertion) => {
                execute_assertion(page.as_deref_mut(), assertion, environment, &self.options).await
            }
        }
    }

    async fn execute_provider(
        &self,
        call: &ServerProviderCall,
        environment: &HashMap<BindingId, Value>,
    ) -> Result<Value, StepError> {
        let mut arguments = BTreeMap::new();
        for (name, expression) in &call.arguments {
            arguments.insert(name.clone(), evaluate(expression, environment)?);
        }
        let result = self
            .providers
            .call(
                ProviderCall {
                    provider: ProviderName(call.provider.clone()),
                    operation: OperationName(call.operation.clone()),
                    arguments,
                },
                CallContext {
                    project_root: self.options.project_root.clone(),
                    timeout: call.timeout.unwrap_or(self.options.test_timeout),
                    redacted_json_fields: self.options.redacted_json_fields.clone(),
                },
            )
            .await
            .map_err(StepError::Provider)?;
        Ok(result.value)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_observation(
        &self,
        plan: &TestPlan,
        test_id: webtest_hir::TestId,
        step: &PlannedStep,
        execution_id: ExecutionId,
        error: &StepError,
        evidence: &PageEvidence,
        repair_hints: &[RepairHint],
        artifacts: &[Artifact],
        elapsed_ms: u64,
    ) {
        let kind = match error {
            StepError::Browser(error) => RuntimeObservationKind::BrowserFailure {
                code: error.code().into(),
                message: error.to_string(),
                locator: step_browser_locator(step),
                page_url: evidence.current_url.clone(),
                candidates: evidence.candidates.clone(),
                actionability: evidence.actionability.clone(),
                artifacts: artifacts
                    .iter()
                    .map(|artifact| artifact.path.display().to_string())
                    .collect(),
                elapsed_ms,
                repair_hints: repair_hints.to_vec(),
            },
            StepError::Decode(error) => RuntimeObservationKind::ValueFailure {
                code: "json_decode_failed".into(),
                message: error.to_string(),
                path: Some(error.path.clone()),
                expected: Some(error.expected.to_string()),
                actual: Some(error.actual.clone()),
                diff: None,
            },
            StepError::Assertion(error) => RuntimeObservationKind::ValueFailure {
                code: "assertion_failed".into(),
                message: error.message.clone(),
                path: None,
                expected: error.expected.as_ref().map(display_value),
                actual: Some(display_value(&error.actual)),
                diff: Some(error.diff.clone()),
            },
            StepError::Provider(error) => RuntimeObservationKind::ValueFailure {
                code: error.code().into(),
                message: error.to_string(),
                path: None,
                expected: None,
                actual: None,
                diff: None,
            },
            StepError::Evaluation(error) => RuntimeObservationKind::ValueFailure {
                code: error.code.into(),
                message: error.message.clone(),
                path: None,
                expected: None,
                actual: None,
                diff: None,
            },
            StepError::Internal(message) => RuntimeObservationKind::ValueFailure {
                code: "internal_error".into(),
                message: message.clone(),
                path: None,
                expected: None,
                actual: None,
                diff: None,
            },
        };
        self.observations.record(RuntimeObservation {
            execution_id,
            file: plan.file,
            source_revision: plan.source_revision,
            test_id,
            step_id: step.id,
            range: step.origin.range,
            kind,
        });
    }
}

fn runtime_failure(error: &StepError) -> RuntimeFailure {
    match error {
        StepError::Browser(error) => RuntimeFailure::Browser(error.clone()),
        StepError::Provider(error) => RuntimeFailure::Provider(error.clone()),
        StepError::Assertion(error) => RuntimeFailure::Assertion {
            message: error.to_string(),
            diff: error.diff.clone(),
        },
        StepError::Decode(error) => RuntimeFailure::Decode {
            message: error.to_string(),
        },
        StepError::Evaluation(error) => RuntimeFailure::Evaluation {
            code: error.code.into(),
            message: error.message.clone(),
        },
        StepError::Internal(message) => RuntimeFailure::Internal {
            message: message.clone(),
        },
    }
}

fn repair_hints_for_error(error: &StepError, inspection: &PageInspection) -> Vec<RepairHint> {
    match error {
        StepError::Browser(BrowserError::LocatorNotFound { locator })
        | StepError::Browser(BrowserError::LocatorAmbiguous { locator, .. }) => {
            locator_repair_hints(locator, inspection, webtest_browser::MAX_CANDIDATES)
        }
        StepError::Browser(BrowserError::OptionNotFound { locator, option }) => {
            let requested_source = locator.to_string();
            let mut candidates = inspection
                .elements
                .iter()
                .filter(|element| {
                    element.preferred_locator.source == requested_source
                        || element
                            .alternate_locators
                            .iter()
                            .any(|candidate| candidate.source == requested_source)
                })
                .flat_map(|element| element.options.iter())
                .map(|candidate| (runtime_edit_distance(candidate, option), candidate.clone()))
                .collect::<Vec<_>>();
            candidates.sort();
            candidates.dedup_by(|left, right| left.1 == right.1);
            candidates
                .into_iter()
                .take(webtest_browser::MAX_CANDIDATES)
                .map(|(_, candidate)| {
                    let mut hint = RepairHint::text(RepairHintKind::OptionCandidate, candidate);
                    hint.reason = Some("available option with a nearby label or value".into());
                    hint
                })
                .collect()
        }
        StepError::Browser(BrowserError::UrlMismatch { actual, .. }) => {
            let mut hint = RepairHint::text(RepairHintKind::NameCandidate, actual.clone());
            hint.reason = Some("current URL observed when the assertion failed".into());
            vec![hint]
        }
        _ => Vec::new(),
    }
}

fn runtime_edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right.iter().enumerate() {
            current.push(
                (previous[right_index + 1] + 1)
                    .min(current[right_index] + 1)
                    .min(previous[right_index] + usize::from(left_char != *right_char)),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

fn redact_step_error(
    error: StepError,
    fields: &[String],
    secrets: &[String],
    query_parameters: &[String],
) -> StepError {
    match error {
        StepError::Assertion(error) => {
            let expected = error
                .expected
                .map(|value| value.redacted_with_secrets(fields, secrets));
            let actual = error.actual.redacted_with_secrets(fields, secrets);
            StepError::Assertion(Box::new(AssertionFailure {
                matcher: error.matcher,
                message: assertion_message(error.matcher, &actual, expected.as_ref()),
                diff: value_diff(error.matcher, &actual, expected.as_ref()),
                expected,
                actual,
            }))
        }
        StepError::Provider(error) => StepError::Provider(error.redacted(secrets)),
        StepError::Browser(error) => {
            StepError::Browser(redact_browser_error(error, secrets, query_parameters))
        }
        error => error,
    }
}

fn redact_browser_error(
    error: BrowserError,
    secrets: &[String],
    query_parameters: &[String],
) -> BrowserError {
    let locator = |locator| redact_locator(locator, secrets);
    let text = |value: String| redact_text(value, secrets);
    let url = |value: String| redact_url(value, secrets, query_parameters);
    match error {
        BrowserError::LocatorNotFound { locator: value } => BrowserError::LocatorNotFound {
            locator: locator(value),
        },
        BrowserError::LocatorAmbiguous {
            locator: value,
            matches,
        } => BrowserError::LocatorAmbiguous {
            locator: locator(value),
            matches,
        },
        BrowserError::LocatorInvalid {
            locator: value,
            message,
        } => BrowserError::LocatorInvalid {
            locator: locator(value),
            message: text(message),
        },
        BrowserError::ElementDetached { locator: value } => BrowserError::ElementDetached {
            locator: locator(value),
        },
        BrowserError::LocatorNotVisible { locator: value } => BrowserError::LocatorNotVisible {
            locator: locator(value),
        },
        BrowserError::ElementUnstable { locator: value } => BrowserError::ElementUnstable {
            locator: locator(value),
        },
        BrowserError::ElementDisabled { locator: value } => BrowserError::ElementDisabled {
            locator: locator(value),
        },
        BrowserError::ElementObscured { locator: value } => BrowserError::ElementObscured {
            locator: locator(value),
        },
        BrowserError::ElementNotEditable { locator: value } => BrowserError::ElementNotEditable {
            locator: locator(value),
        },
        BrowserError::OptionNotFound {
            locator: value,
            option,
        } => BrowserError::OptionNotFound {
            locator: locator(value),
            option: text(option),
        },
        BrowserError::OptionAmbiguous {
            locator: value,
            option,
            matches,
        } => BrowserError::OptionAmbiguous {
            locator: locator(value),
            option: text(option),
            matches,
        },
        BrowserError::InvalidKey { key } => BrowserError::InvalidKey { key: text(key) },
        BrowserError::ActionTimeout {
            locator: value,
            timeout_ms,
        } => BrowserError::ActionTimeout {
            locator: locator(value),
            timeout_ms,
        },
        BrowserError::AssertionFailed {
            locator: value,
            expected,
            actual,
        } => BrowserError::AssertionFailed {
            locator: locator(value),
            expected,
            actual: text(actual),
        },
        BrowserError::UrlMismatch { expected, actual } => BrowserError::UrlMismatch {
            expected: url(expected),
            actual: url(actual),
        },
        BrowserError::NavigationFailed { url: value, reason } => BrowserError::NavigationFailed {
            url: url(value),
            reason: text(reason),
        },
        BrowserError::NavigationTimeout {
            url: value,
            timeout_ms,
        } => BrowserError::NavigationTimeout {
            url: url(value),
            timeout_ms,
        },
        BrowserError::CommandTimeout { method, timeout_ms } => BrowserError::CommandTimeout {
            method: text(method),
            timeout_ms,
        },
        BrowserError::BrowserDisconnected => BrowserError::BrowserDisconnected,
        BrowserError::BrowserCrashed { status } => BrowserError::BrowserCrashed {
            status: text(status),
        },
        BrowserError::MalformedProtocol { message } => BrowserError::MalformedProtocol {
            message: text(message),
        },
        BrowserError::Protocol { method, message } => BrowserError::Protocol {
            method: text(method),
            message: text(message),
        },
        BrowserError::Launch(message) => BrowserError::Launch(text(message)),
        BrowserError::EvaluationFailed {
            expression,
            message,
        } => BrowserError::EvaluationFailed {
            expression: text(expression),
            message: text(message),
        },
        BrowserError::UnsupportedCapability { capability } => BrowserError::UnsupportedCapability {
            capability: text(capability),
        },
    }
}

fn redact_locator(locator: BrowserLocator, secrets: &[String]) -> BrowserLocator {
    match locator {
        BrowserLocator::Id(value) => BrowserLocator::Id(redact_text(value, secrets)),
        BrowserLocator::Role { role, name } => BrowserLocator::Role {
            role: redact_text(role, secrets),
            name: name.map(|value| redact_text(value, secrets)),
        },
        BrowserLocator::Label(value) => BrowserLocator::Label(redact_text(value, secrets)),
        BrowserLocator::Text(value) => BrowserLocator::Text(redact_text(value, secrets)),
        BrowserLocator::Placeholder(value) => {
            BrowserLocator::Placeholder(redact_text(value, secrets))
        }
        BrowserLocator::TestId(value) => BrowserLocator::TestId(redact_text(value, secrets)),
        BrowserLocator::Css(value) => BrowserLocator::Css(redact_text(value, secrets)),
        BrowserLocator::XPath(value) => BrowserLocator::XPath(redact_text(value, secrets)),
    }
}

fn redact_text(mut value: String, secrets: &[String]) -> String {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        value = value.replace(secret, "[redacted]");
    }
    value
}

fn redact_url(value: String, secrets: &[String], query_parameters: &[String]) -> String {
    let mut value = if let Ok(mut parsed) = url::Url::parse(&value) {
        let pairs = parsed
            .query_pairs()
            .map(|(name, value)| {
                let value = if query_parameters
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&name))
                {
                    "[redacted]".into()
                } else {
                    value.into_owned()
                };
                (name.into_owned(), value)
            })
            .collect::<Vec<_>>();
        if !pairs.is_empty() {
            parsed.query_pairs_mut().clear().extend_pairs(pairs);
        }
        parsed.to_string()
    } else {
        value
    };
    value = redact_text(value, secrets);
    value
}

async fn execute_browser(
    page: &mut dyn Page,
    operation: &BrowserOperation,
    environment: &HashMap<BindingId, Value>,
    options: &RunnerOptions,
) -> Result<(), StepError> {
    match operation {
        BrowserOperation::Navigate { url } => {
            let url = string_value(evaluate(url, environment)?)?;
            page.open(&resolve_url(options.base_url.as_deref(), &url)?)
                .await
                .map_err(StepError::Browser)
        }
        BrowserOperation::Evaluate { expression } => {
            page.evaluate(expression).await.map_err(StepError::Browser)
        }
        BrowserOperation::Click { locator } => page
            .perform(
                &Action::Click {
                    locator: browser_locator(locator),
                },
                options.action_timeout,
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::Fill { locator, value } => {
            let value = string_value(evaluate(value, environment)?)?;
            page.perform(
                &Action::Fill {
                    locator: browser_locator(locator),
                    value,
                },
                options.action_timeout,
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Type { locator, value } => {
            let value = string_value(evaluate(value, environment)?)?;
            page.perform(
                &Action::Type {
                    locator: browser_locator(locator),
                    value,
                },
                options.action_timeout,
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Press { locator, key } => {
            let key = string_value(evaluate(key, environment)?)?;
            page.perform(
                &Action::Press {
                    locator: browser_locator(locator),
                    key,
                },
                options.action_timeout,
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Check { locator, checked } => page
            .perform(
                &Action::Check {
                    locator: browser_locator(locator),
                    checked: *checked,
                },
                options.action_timeout,
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::Select { locator, option } => {
            let option = string_value(evaluate(option, environment)?)?;
            page.perform(
                &Action::Select {
                    locator: browser_locator(locator),
                    option,
                },
                options.action_timeout,
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Hover { locator } => page
            .perform(
                &Action::Hover {
                    locator: browser_locator(locator),
                },
                options.action_timeout,
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::WaitForLocator {
            locator,
            state,
            timeout,
        } => page
            .wait_for_locator(
                &browser_locator(locator),
                browser_state(*state),
                bounded_timeout(
                    timeout.unwrap_or(options.assertion_timeout),
                    options.test_timeout,
                ),
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::WaitForUrl { url, timeout } => {
            let url = string_value(evaluate(url, environment)?)?;
            let expected = resolve_url(options.base_url.as_deref(), &url)?;
            page.wait_for_url(
                &expected,
                bounded_timeout(
                    timeout.unwrap_or(options.assertion_timeout),
                    options.test_timeout,
                ),
            )
            .await
            .map_err(StepError::Browser)
        }
    }
}

async fn execute_assertion(
    page: Option<&mut (dyn Page + '_)>,
    assertion: &AssertionOperation,
    environment: &HashMap<BindingId, Value>,
    options: &RunnerOptions,
) -> Result<(), StepError> {
    match assertion {
        AssertionOperation::Locator {
            locator,
            state,
            timeout,
        } => page
            .ok_or_else(|| StepError::Internal("locator assertion has no browser page".into()))?
            .wait_for_locator(
                &browser_locator(locator),
                browser_state(*state),
                bounded_timeout(
                    timeout.unwrap_or(options.assertion_timeout),
                    options.test_timeout,
                ),
            )
            .await
            .map_err(StepError::Browser),
        AssertionOperation::Url { url, timeout } => {
            let url = string_value(evaluate(url, environment)?)?;
            let expected = resolve_url(options.base_url.as_deref(), &url)?;
            page.ok_or_else(|| StepError::Internal("URL assertion has no browser page".into()))?
                .wait_for_url(
                    &expected,
                    bounded_timeout(
                        timeout.unwrap_or(options.assertion_timeout),
                        options.test_timeout,
                    ),
                )
                .await
                .map_err(StepError::Browser)
        }
        AssertionOperation::Value {
            matcher,
            actual,
            expected,
            ..
        } => {
            let actual = evaluate(actual, environment)?;
            if *matcher == ValueMatcher::Matches {
                let expected_type = expected
                    .as_ref()
                    .and_then(|expression| match expression {
                        PlanExpr::Type(ty) => Some(ty),
                        _ => None,
                    })
                    .ok_or_else(|| StepError::Internal("matches assertion has no type".into()))?;
                decode_value(&actual, expected_type, "$", None)
                    .map(|_| ())
                    .map_err(StepError::Decode)
            } else {
                let expected = expected
                    .as_ref()
                    .map(|expected| evaluate(expected, environment))
                    .transpose()?;
                if assertion_matches(*matcher, &actual, expected.as_ref()) {
                    Ok(())
                } else {
                    Err(StepError::Assertion(Box::new(AssertionFailure {
                        matcher: *matcher,
                        message: assertion_message(*matcher, &actual, expected.as_ref()),
                        diff: value_diff(*matcher, &actual, expected.as_ref()),
                        expected,
                        actual,
                    })))
                }
            }
        }
    }
}

fn evaluate(
    expression: &PlanExpr,
    environment: &HashMap<BindingId, Value>,
) -> Result<Value, StepError> {
    match expression {
        PlanExpr::Literal(value) => Ok(value.clone()),
        PlanExpr::Binding(binding) => environment.get(binding).cloned().ok_or_else(|| {
            StepError::Internal(format!("binding {} has no runtime value", binding.0))
        }),
        PlanExpr::List(items) => items
            .iter()
            .map(|item| evaluate(item, environment))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        PlanExpr::Record(fields) => fields
            .iter()
            .map(|(name, value)| Ok((name.clone(), evaluate(value, environment)?)))
            .collect::<Result<BTreeMap<_, _>, StepError>>()
            .map(Value::Record),
        PlanExpr::Type(_) => Err(StepError::Internal(
            "type pattern cannot be evaluated as a value".into(),
        )),
        PlanExpr::Member { receiver, member } => {
            let receiver = evaluate(receiver, environment)?;
            receiver.member(member).ok_or_else(|| {
                if matches!(receiver, Value::Response(_))
                    && matches!(member.as_str(), "json" | "text")
                {
                    StepError::Evaluation(EvaluationFailure {
                        code: "response_decode_failed",
                        message: format!(
                            "response body is not available as `{member}` for this operation"
                        ),
                    })
                } else {
                    StepError::Internal(format!("runtime value has no member `{member}`"))
                }
            })
        }
        PlanExpr::Unary { operator, operand } => {
            let operand = evaluate(operand, environment)?;
            match (operator, operand) {
                (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                (UnaryOperator::Negate, Value::Int(value)) => Ok(Value::Int(-value)),
                (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                _ => Err(StepError::Internal("invalid typed unary operation".into())),
            }
        }
        PlanExpr::Binary {
            operator,
            left,
            right,
        } => {
            let left = evaluate(left, environment)?;
            match (operator, &left) {
                (BinaryOperator::And, Value::Bool(false)) => Ok(Value::Bool(false)),
                (BinaryOperator::Or, Value::Bool(true)) => Ok(Value::Bool(true)),
                _ => evaluate_binary(*operator, left, evaluate(right, environment)?),
            }
        }
        PlanExpr::Decode {
            value,
            target,
            response_operation,
        } => {
            let value = evaluate(value, environment)?;
            decode_value(&value, target, "$", response_operation.clone()).map_err(StepError::Decode)
        }
    }
}

fn evaluate_binary(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, StepError> {
    let value = match operator {
        BinaryOperator::Equal => Value::Bool(values_equal(&left, &right)),
        BinaryOperator::NotEqual => Value::Bool(!values_equal(&left, &right)),
        BinaryOperator::Less => Value::Bool(compare_values(&left, &right).is_some_and(|it| it < 0)),
        BinaryOperator::LessEqual => {
            Value::Bool(compare_values(&left, &right).is_some_and(|it| it <= 0))
        }
        BinaryOperator::Greater => {
            Value::Bool(compare_values(&left, &right).is_some_and(|it| it > 0))
        }
        BinaryOperator::GreaterEqual => {
            Value::Bool(compare_values(&left, &right).is_some_and(|it| it >= 0))
        }
        BinaryOperator::Add => match (left, right) {
            (Value::Int(left), Value::Int(right)) => Value::Int(left + right),
            (Value::Float(left), Value::Float(right)) => Value::Float(left + right),
            (Value::Int(left), Value::Float(right)) => Value::Float(left as f64 + right),
            (Value::Float(left), Value::Int(right)) => Value::Float(left + right as f64),
            (Value::String(left), Value::String(right)) => Value::String(left + &right),
            _ => return Err(StepError::Internal("invalid typed addition".into())),
        },
        BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide => {
            numeric_binary(operator, left, right)?
        }
        BinaryOperator::And => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Value::Bool(left && right),
            _ => {
                return Err(StepError::Internal(
                    "invalid typed boolean operation".into(),
                ));
            }
        },
        BinaryOperator::Or => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Value::Bool(left || right),
            _ => {
                return Err(StepError::Internal(
                    "invalid typed boolean operation".into(),
                ));
            }
        },
        BinaryOperator::Contains => Value::Bool(value_contains(&left, &right)),
        BinaryOperator::Matches => {
            return Err(StepError::Internal(
                "matches is evaluated by assertion execution".into(),
            ));
        }
    };
    Ok(value)
}

fn numeric_binary(operator: BinaryOperator, left: Value, right: Value) -> Result<Value, StepError> {
    if let (Value::Int(left), Value::Int(right)) = (&left, &right)
        && operator != BinaryOperator::Divide
    {
        return Ok(Value::Int(match operator {
            BinaryOperator::Subtract => left - right,
            BinaryOperator::Multiply => left * right,
            _ => unreachable!(),
        }));
    }
    let left = number(&left).ok_or_else(|| StepError::Internal("expected number".into()))?;
    let right = number(&right).ok_or_else(|| StepError::Internal("expected number".into()))?;
    if operator == BinaryOperator::Divide && right == 0.0 {
        return Err(StepError::Evaluation(EvaluationFailure {
            code: "division_by_zero",
            message: "division by zero".into(),
        }));
    }
    Ok(Value::Float(match operator {
        BinaryOperator::Subtract => left - right,
        BinaryOperator::Multiply => left * right,
        BinaryOperator::Divide => left / right,
        _ => unreachable!(),
    }))
}

fn decode_value(
    value: &Value,
    expected: &Type,
    path: &str,
    response_operation: Option<String>,
) -> Result<Value, DecodeFailure> {
    let failure = || DecodeFailure {
        path: path.into(),
        expected: expected.clone(),
        actual: value.type_name().into(),
        response_operation: response_operation.clone(),
    };
    match expected {
        Type::Json | Type::Unknown => Ok(value.clone()),
        Type::Null if matches!(value, Value::Null) => Ok(Value::Null),
        Type::Bool if matches!(value, Value::Bool(_)) => Ok(value.clone()),
        Type::Int if matches!(value, Value::Int(_)) => Ok(value.clone()),
        Type::Float => match value {
            Value::Float(_) => Ok(value.clone()),
            Value::Int(value) => Ok(Value::Float(*value as f64)),
            _ => Err(failure()),
        },
        Type::String | Type::Url if matches!(value, Value::String(_)) => Ok(value.clone()),
        Type::Duration if matches!(value, Value::DurationMillis(_)) => Ok(value.clone()),
        Type::Bytes if matches!(value, Value::Bytes(_)) => Ok(value.clone()),
        Type::StatusCode if matches!(value, Value::Int(_)) => Ok(value.clone()),
        Type::Option(_) if matches!(value, Value::Null) => Ok(Value::Null),
        Type::Option(inner) => decode_value(value, inner, path, response_operation),
        Type::List(inner) => {
            let Value::List(values) = value else {
                return Err(failure());
            };
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    decode_value(
                        value,
                        inner,
                        &format!("{path}[{index}]"),
                        response_operation.clone(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List)
        }
        Type::Record(expected_fields) => {
            let Value::Record(values) = value else {
                return Err(failure());
            };
            let mut decoded = BTreeMap::new();
            for (name, field) in expected_fields {
                match values.get(name) {
                    Some(value) => {
                        decoded.insert(
                            name.clone(),
                            decode_value(
                                value,
                                &field.ty,
                                &format!("{path}.{name}"),
                                response_operation.clone(),
                            )?,
                        );
                    }
                    None if field.optional => {
                        decoded.insert(name.clone(), Value::Null);
                    }
                    None => {
                        return Err(DecodeFailure {
                            path: format!("{path}.{name}"),
                            expected: field.ty.clone(),
                            actual: "missing field".into(),
                            response_operation,
                        });
                    }
                }
            }
            Ok(Value::Record(decoded))
        }
        Type::FilePath if matches!(value, Value::FilePath(_)) => Ok(value.clone()),
        Type::TempDirectory if matches!(value, Value::TempDirectory(_)) => Ok(value.clone()),
        Type::ProcessResult if matches!(value, Value::ProcessResult(_)) => Ok(value.clone()),
        Type::Response(_) if matches!(value, Value::Response(_)) => Ok(value.clone()),
        Type::Headers if matches!(value, Value::Headers(_)) => Ok(value.clone()),
        _ => Err(failure()),
    }
}

fn assertion_matches(matcher: ValueMatcher, actual: &Value, expected: Option<&Value>) -> bool {
    match matcher {
        ValueMatcher::Truthy => matches!(actual, Value::Bool(true)),
        ValueMatcher::Equal => expected.is_some_and(|expected| values_equal(actual, expected)),
        ValueMatcher::NotEqual => expected.is_some_and(|expected| !values_equal(actual, expected)),
        ValueMatcher::Less => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering < 0),
        ValueMatcher::LessEqual => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering <= 0),
        ValueMatcher::Greater => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering > 0),
        ValueMatcher::GreaterEqual => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering >= 0),
        ValueMatcher::Contains => expected.is_some_and(|expected| value_contains(actual, expected)),
        ValueMatcher::Matches => false,
    }
}

fn assertion_message(matcher: ValueMatcher, actual: &Value, expected: Option<&Value>) -> String {
    match expected {
        Some(expected) => format!(
            "assertion {matcher:?} failed: expected {}, got {}",
            bounded_display_value(expected),
            bounded_display_value(actual)
        ),
        None => format!(
            "assertion {matcher:?} failed for {}",
            bounded_display_value(actual)
        ),
    }
}

fn value_diff(matcher: ValueMatcher, actual: &Value, expected: Option<&Value>) -> ValueDiff {
    if matcher == ValueMatcher::Contains {
        return ValueDiff::Contains {
            expected_item: expected.map(bounded_display_value).unwrap_or_default(),
            actual: bounded_display_value(actual),
        };
    }
    if matcher == ValueMatcher::Equal {
        match (actual, expected) {
            (Value::String(actual), Some(Value::String(expected))) => {
                let actual_chars: Vec<_> = actual.chars().collect();
                let expected_chars: Vec<_> = expected.chars().collect();
                let common_prefix_chars = actual_chars
                    .iter()
                    .zip(&expected_chars)
                    .take_while(|(actual, expected)| actual == expected)
                    .count();
                return ValueDiff::String {
                    common_prefix_chars,
                    expected_segment: bounded_char_segment(&expected_chars, common_prefix_chars),
                    actual_segment: bounded_char_segment(&actual_chars, common_prefix_chars),
                };
            }
            (Value::List(actual), Some(Value::List(expected))) => {
                let common = actual.len().min(expected.len());
                let mut differing_indices: Vec<_> = (0..common)
                    .filter(|index| !values_equal(&actual[*index], &expected[*index]))
                    .take(20)
                    .collect();
                differing_indices.extend(
                    (common..actual.len().max(expected.len()))
                        .take(20usize.saturating_sub(differing_indices.len())),
                );
                return ValueDiff::List {
                    expected_len: expected.len(),
                    actual_len: actual.len(),
                    differing_indices,
                };
            }
            (Value::Record(actual), Some(Value::Record(expected))) => {
                let missing_fields = expected
                    .keys()
                    .filter(|name| !actual.contains_key(*name))
                    .take(20)
                    .cloned()
                    .collect();
                let unexpected_fields = actual
                    .keys()
                    .filter(|name| !expected.contains_key(*name))
                    .take(20)
                    .cloned()
                    .collect();
                let mismatched_fields = expected
                    .iter()
                    .filter(|(name, expected)| {
                        actual
                            .get(*name)
                            .is_some_and(|actual| !values_equal(actual, expected))
                    })
                    .map(|(name, _)| name.clone())
                    .take(20)
                    .collect();
                return ValueDiff::Record {
                    missing_fields,
                    unexpected_fields,
                    mismatched_fields,
                };
            }
            _ => {}
        }
    }
    ValueDiff::Scalar {
        expected: expected.map(bounded_display_value),
        actual: bounded_display_value(actual),
    }
}

fn bounded_char_segment(characters: &[char], difference: usize) -> String {
    const CONTEXT: usize = 24;
    const LIMIT: usize = 80;
    let start = difference.saturating_sub(CONTEXT);
    let mut segment: String = characters.iter().skip(start).take(LIMIT).collect();
    if start > 0 {
        segment.insert_str(0, "...");
    }
    if start + LIMIT < characters.len() {
        segment.push_str("...");
    }
    segment
}

fn bounded_display_value(value: &Value) -> String {
    const LIMIT: usize = 240;
    let value = display_value(value);
    let mut characters = value.chars();
    let mut bounded: String = characters.by_ref().take(LIMIT).collect();
    if characters.next().is_some() {
        bounded.push_str("...");
    }
    bounded
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Int(left), Value::Float(right)) => *left as f64 == *right,
        (Value::Float(left), Value::Int(right)) => *left == *right as f64,
        _ => left == right,
    }
}

fn compare_values(left: &Value, right: &Value) -> Option<i8> {
    match (left, right) {
        (Value::String(left), Value::String(right)) => Some(match left.cmp(right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        _ => {
            let left = number(left)?;
            let right = number(right)?;
            Some(if left < right {
                -1
            } else if left > right {
                1
            } else {
                0
            })
        }
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(value) => Some(*value as f64),
        Value::Float(value) => Some(*value),
        _ => None,
    }
}

fn value_contains(container: &Value, value: &Value) -> bool {
    match (container, value) {
        (Value::String(container), Value::String(value)) => container.contains(value),
        (Value::List(values), value) => values.iter().any(|item| values_equal(item, value)),
        _ => false,
    }
}

fn display_value(value: &Value) -> String {
    webtest_provider::value_to_json(value)
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| format!("<{:?}>", value.type_name()))
}

fn runtime_transferable(value: &Value) -> bool {
    match value {
        Value::Null
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DurationMillis(_) => true,
        Value::List(values) => values.iter().all(runtime_transferable),
        Value::Record(values) => values.values().all(runtime_transferable),
        Value::Headers(_)
        | Value::Bytes(_)
        | Value::Response(_)
        | Value::ProcessResult(_)
        | Value::FilePath(_)
        | Value::TempDirectory(_) => false,
    }
}

fn visible_bindings(
    environment: &HashMap<BindingId, Value>,
    names: &HashMap<BindingId, String>,
    redacted_fields: &[String],
    secrets: &[String],
) -> BTreeMap<String, Value> {
    names
        .iter()
        .filter_map(|(id, name)| {
            environment
                .get(id)
                .filter(|value| runtime_transferable(value))
                .map(|value| {
                    (
                        name.clone(),
                        value.redacted_with_secrets(redacted_fields, secrets),
                    )
                })
        })
        .collect()
}

fn collect_provider_secrets(
    call: &ServerProviderCall,
    environment: &HashMap<BindingId, Value>,
    redacted_fields: &[String],
    secrets: &mut Vec<String>,
) {
    for (name, expression) in &call.arguments {
        let Ok(value) = evaluate(expression, environment) else {
            continue;
        };
        collect_sensitive_values(
            &value,
            redacted_fields,
            call.redacted_arguments
                .iter()
                .any(|argument| argument == name),
            secrets,
        );
    }
    secrets.sort();
    secrets.dedup();
}

fn collect_sensitive_values(
    value: &Value,
    redacted_fields: &[String],
    sensitive: bool,
    secrets: &mut Vec<String>,
) {
    match value {
        Value::String(value) if sensitive && !value.is_empty() => secrets.push(value.clone()),
        Value::Record(values) => {
            for (name, value) in values {
                let sensitive = sensitive
                    || redacted_fields
                        .iter()
                        .any(|field| field.eq_ignore_ascii_case(name));
                collect_sensitive_values(value, redacted_fields, sensitive, secrets);
            }
        }
        Value::List(values) => {
            for value in values {
                collect_sensitive_values(value, redacted_fields, sensitive, secrets);
            }
        }
        _ => {}
    }
}

fn temporary_directories(value: &Value) -> Vec<PathBuf> {
    match value {
        Value::TempDirectory(path) => vec![path.clone()],
        Value::List(values) => values.iter().flat_map(temporary_directories).collect(),
        Value::Record(values) => values.values().flat_map(temporary_directories).collect(),
        _ => Vec::new(),
    }
}

fn string_value(value: Value) -> Result<String, StepError> {
    if let Value::String(value) = value {
        Ok(value)
    } else {
        Err(StepError::Internal(format!(
            "typed expression produced {}, expected string",
            value.type_name()
        )))
    }
}

fn bounded_timeout(timeout: Duration, test_timeout: Duration) -> Duration {
    timeout.min(test_timeout)
}

fn browser_locator(locator: &Locator) -> BrowserLocator {
    match locator {
        Locator::Id(value) => BrowserLocator::Id(value.clone()),
        Locator::Role { role, name } => BrowserLocator::Role {
            role: role.clone(),
            name: name.clone(),
        },
        Locator::Label(value) => BrowserLocator::Label(value.clone()),
        Locator::Text(value) => BrowserLocator::Text(value.clone()),
        Locator::Placeholder(value) => BrowserLocator::Placeholder(value.clone()),
        Locator::TestId(value) => BrowserLocator::TestId(value.clone()),
        Locator::Css(value) => BrowserLocator::Css(value.clone()),
        Locator::XPath(value) => BrowserLocator::XPath(value.clone()),
    }
}

fn browser_state(state: LocatorState) -> BrowserLocatorState {
    match state {
        LocatorState::Visible => BrowserLocatorState::Visible,
        LocatorState::Hidden => BrowserLocatorState::Hidden,
        LocatorState::Attached => BrowserLocatorState::Attached,
        LocatorState::Detached => BrowserLocatorState::Detached,
        LocatorState::Enabled => BrowserLocatorState::Enabled,
        LocatorState::Disabled => BrowserLocatorState::Disabled,
        LocatorState::Checked => BrowserLocatorState::Checked,
        LocatorState::Unchecked => BrowserLocatorState::Unchecked,
    }
}

fn step_browser_locator(step: &PlannedStep) -> Option<BrowserLocator> {
    match &step.operation {
        TestOperation::Browser(BrowserOperation::Click { locator })
        | TestOperation::Browser(BrowserOperation::Fill { locator, .. })
        | TestOperation::Browser(BrowserOperation::Type { locator, .. })
        | TestOperation::Browser(BrowserOperation::Press { locator, .. })
        | TestOperation::Browser(BrowserOperation::Check { locator, .. })
        | TestOperation::Browser(BrowserOperation::Select { locator, .. })
        | TestOperation::Browser(BrowserOperation::Hover { locator })
        | TestOperation::Browser(BrowserOperation::WaitForLocator { locator, .. })
        | TestOperation::Assertion(AssertionOperation::Locator { locator, .. }) => {
            Some(browser_locator(locator))
        }
        _ => None,
    }
}

pub fn resolve_browser_url(base_url: Option<&str>, value: &str) -> Result<String, BrowserError> {
    if is_absolute_url(value) {
        return Ok(normalize_url(value));
    }
    let base = base_url.ok_or_else(|| BrowserError::NavigationFailed {
        url: value.into(),
        reason: "relative URL requires browser.base_url".into(),
    })?;
    let resolved = if value.starts_with('/') {
        let scheme_end = base.find("://").map(|index| index + 3).unwrap_or(0);
        let authority_end = base[scheme_end..]
            .find('/')
            .map(|index| scheme_end + index)
            .unwrap_or(base.len());
        format!("{}{}", &base[..authority_end], value)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), value)
    };
    Ok(normalize_url(&resolved))
}

fn resolve_url(base_url: Option<&str>, value: &str) -> Result<String, StepError> {
    resolve_browser_url(base_url, value).map_err(StepError::Browser)
}

fn is_absolute_url(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && !rest.is_empty()
            && scheme.chars().next().is_some_and(char::is_alphabetic)
            && scheme.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            })
    })
}

fn normalize_url(value: &str) -> String {
    let Some(scheme) = value.find("://") else {
        return value.into();
    };
    let after_scheme = scheme + 3;
    if !value[after_scheme..].contains(['/', '?', '#']) {
        format!("{value}/")
    } else {
        value.into()
    }
}

fn write_artifacts(
    directory: &std::path::Path,
    execution_id: ExecutionId,
    test_id: webtest_hir::TestId,
    step_id: webtest_hir::StepId,
    evidence: &mut PageEvidence,
) -> Vec<Artifact> {
    if evidence.screenshot_png.is_none()
        && evidence.dom_snapshot.is_none()
        && evidence.current_url.is_none()
        && evidence.capture_failures.is_empty()
    {
        return Vec::new();
    }
    if let Err(error) = std::fs::create_dir_all(directory) {
        evidence
            .capture_failures
            .push(format!("artifact directory: {error}"));
        return Vec::new();
    }
    let stem = format!(
        "test-{}-step-{}-execution-{}",
        test_id.0, step_id.0, execution_id.0
    );
    let mut artifacts = Vec::new();
    if let Some(png) = evidence.screenshot_png.clone() {
        write_artifact(
            directory.join(format!("{stem}.png")),
            &png,
            ArtifactKind::Screenshot,
            evidence,
            &mut artifacts,
        );
    }
    if let Some(dom) = evidence.dom_snapshot.clone() {
        write_artifact(
            directory.join(format!("{stem}.dom.html")),
            dom.as_bytes(),
            ArtifactKind::DomSnapshot,
            evidence,
            &mut artifacts,
        );
    }
    let summary = format!(
        "url: {}\ntitle: {}\nelapsed evidence candidates: {}\nactionability: {:?}\nconsole errors: {:?}\ncapture failures: {:?}\n",
        evidence.current_url.as_deref().unwrap_or("<unavailable>"),
        evidence.title.as_deref().unwrap_or("<unavailable>"),
        evidence.candidates.len(),
        evidence.actionability,
        evidence.console_errors,
        evidence.capture_failures,
    );
    write_artifact(
        directory.join(format!("{stem}.evidence.txt")),
        summary.as_bytes(),
        ArtifactKind::Evidence,
        evidence,
        &mut artifacts,
    );
    artifacts
}

fn write_artifact(
    path: PathBuf,
    contents: &[u8],
    kind: ArtifactKind,
    evidence: &mut PageEvidence,
    artifacts: &mut Vec<Artifact>,
) {
    match std::fs::write(&path, contents) {
        Ok(()) => artifacts.push(Artifact { kind, path }),
        Err(error) => evidence
            .capture_failures
            .push(format!("write {}: {error}", path.display())),
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use webtest_browser::{BrowserSession, Page};
    use webtest_hir::{StepId, TestId};
    use webtest_plan::{PlannedTest, TestPlan};
    use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

    use super::*;

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
        let failed_result = runner.run(&plan(revision), &failed).await.expect("run");
        assert_eq!(failed_result.failed(), 1);
        let failure = failed_result.tests[0].failure.as_ref().expect("failure");
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
        runner.run(&plan(revision), &passed).await.expect("run");
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

    #[test]
    fn typed_decode_reports_the_exact_json_path() {
        let value = Value::Record(
            [
                ("id".into(), Value::String("wrong".into())),
                ("email".into(), Value::String("a@example.test".into())),
            ]
            .into_iter()
            .collect(),
        );
        let expected = Type::Record(
            [(
                "id".into(),
                webtest_provider::RecordField {
                    ty: Type::Int,
                    optional: false,
                },
            )]
            .into_iter()
            .collect(),
        );
        let error = decode_value(&value, &expected, "$", Some("http.post".into()))
            .expect_err("decode should fail");
        assert_eq!(error.path, "$.id");
        assert_eq!(error.expected, Type::Int);
        assert_eq!(error.actual, "string");
    }

    #[test]
    fn assertion_diffs_are_bounded_structural_and_unicode_safe() {
        let string = value_diff(
            ValueMatcher::Equal,
            &Value::String("prefix-β-actual".into()),
            Some(&Value::String("prefix-β-expected".into())),
        );
        assert!(matches!(
            string,
            ValueDiff::String {
                common_prefix_chars: 9,
                ..
            }
        ));

        let record = value_diff(
            ValueMatcher::Equal,
            &Value::Record(
                [
                    ("id".into(), Value::String("wrong".into())),
                    ("extra".into(), Value::Bool(true)),
                ]
                .into_iter()
                .collect(),
            ),
            Some(&Value::Record(
                [
                    ("id".into(), Value::Int(7)),
                    ("email".into(), Value::String("a@example.test".into())),
                ]
                .into_iter()
                .collect(),
            )),
        );
        assert_eq!(
            record,
            ValueDiff::Record {
                missing_fields: vec!["email".into()],
                unexpected_fields: vec!["extra".into()],
                mismatched_fields: vec!["id".into()],
            }
        );
    }

    #[test]
    fn dynamic_expression_errors_are_test_failures_not_internal_invariants() {
        let expression = PlanExpr::Binary {
            operator: BinaryOperator::Divide,
            left: Box::new(PlanExpr::Literal(Value::Int(1))),
            right: Box::new(PlanExpr::Literal(Value::Int(0))),
        };
        let error = evaluate(&expression, &HashMap::new()).expect_err("division should fail");
        assert_eq!(error.code(), "division_by_zero");
        assert!(!error.is_infrastructure());
        assert!(matches!(error, StepError::Evaluation(_)));
    }

    #[test]
    fn resolves_relative_and_normalizes_absolute_urls() {
        assert_eq!(
            resolve_url(Some("http://example.test/base"), "/login").unwrap(),
            "http://example.test/login"
        );
        assert_eq!(
            resolve_url(None, "http://example.test").unwrap(),
            "http://example.test/"
        );
        assert!(resolve_url(None, "/login").is_err());
    }
}
