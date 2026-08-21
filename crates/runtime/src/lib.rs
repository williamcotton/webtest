//! Sequential execution of protocol-neutral test plans.

use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use tracing::instrument;
use webtest_browser::{
    Action, BrowserContextOptions, BrowserError, BrowserHost, EvidenceRequest,
    Locator as BrowserLocator, LocatorState as BrowserLocatorState, Page, PageEvidence,
};
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeFailure, RuntimeObservation,
    RuntimeObservationKind,
};
use webtest_plan::{
    AssertionOperation, BrowserOperation, Locator, LocatorState, PlannedStep, TestOperation,
    TestPlan,
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
    pub error: BrowserError,
    pub evidence: PageEvidence,
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub failure: Option<StepFailure>,
    pub duration: Duration,
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
        }
    }
}

pub struct Runner {
    observations: Arc<ObservationStore>,
    options: RunnerOptions,
}

#[async_trait]
pub trait RunControl: Send + Sync {
    async fn before_step(&self, test: &webtest_plan::PlannedTest, step: &PlannedStep);
}

impl Runner {
    pub fn new(observations: Arc<ObservationStore>) -> Self {
        Self {
            observations,
            options: RunnerOptions::default(),
        }
    }

    pub fn with_options(mut self, options: RunnerOptions) -> Self {
        self.options = options;
        self
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
    ) -> Result<RunResult, BrowserError> {
        self.run_with_control(plan, browser, None).await
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run_with_control(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
        control: Option<&dyn RunControl>,
    ) -> Result<RunResult, BrowserError> {
        self.observations.clear_for_file(plan.file);
        let run_started = std::time::Instant::now();
        let execution_id = ExecutionId::next();
        let mut events = vec![ExecutionEvent::RunStarted { execution_id }];
        let mut session = browser.start().await?;
        let mut tests = Vec::with_capacity(plan.tests.len());

        for (index, test) in plan.tests.iter().enumerate() {
            let (result, tainted) = self
                .run_test(
                    plan,
                    test,
                    execution_id,
                    &mut events,
                    session.as_mut(),
                    control,
                )
                .await?;
            tests.push(result);
            if tainted && index + 1 < plan.tests.len() {
                let _ = session.close().await;
                session = browser.start().await?;
            }
        }

        events.push(ExecutionEvent::RunFinished { execution_id });
        session.close().await?;
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
        session: &mut dyn webtest_browser::BrowserSession,
        control: Option<&dyn RunControl>,
    ) -> Result<(TestResult, bool), BrowserError> {
        let test_started = std::time::Instant::now();
        events.push(ExecutionEvent::TestStarted {
            execution_id,
            test_id: test.id,
            name: test.name.clone(),
        });
        let mut context = session.new_context(&self.options.browser_context).await?;
        let mut page = context.new_page().await?;
        let mut failure = None;
        let mut secrets = Vec::new();

        for step in &test.steps {
            if let Some(control) = control {
                control.before_step(test, step).await
            }
            events.push(ExecutionEvent::StepStarted {
                execution_id,
                test_id: test.id,
                step_id: step.id,
            });
            let step_started = std::time::Instant::now();
            if let TestOperation::Browser(BrowserOperation::Fill { value, .. }) = &step.operation {
                secrets.push(value.clone());
            }
            match execute_step(page.as_mut(), step, &self.options).await {
                Ok(()) => events.push(ExecutionEvent::StepPassed {
                    execution_id,
                    test_id: test.id,
                    step_id: step.id,
                }),
                Err(error) => {
                    let mut evidence = if !error.is_infrastructure()
                        && (self.options.evidence.screenshot_on_failure
                            || self.options.evidence.dom_snapshot_on_failure)
                    {
                        page.capture_evidence(&EvidenceRequest {
                            locator: step_browser_locator(step),
                            include_screenshot: self.options.evidence.screenshot_on_failure,
                            include_dom: self.options.evidence.dom_snapshot_on_failure,
                            max_dom_bytes: self.options.evidence.max_dom_bytes,
                            redactions: secrets.clone(),
                        })
                        .await
                    } else {
                        PageEvidence::default()
                    };
                    if !self.options.evidence.screenshot_on_failure {
                        evidence.screenshot_png = None
                    }
                    let artifacts = write_artifacts(
                        &self.options.evidence.artifact_directory,
                        execution_id,
                        test.id,
                        step.id,
                        &mut evidence,
                    );
                    let elapsed_ms = duration_millis(step_started.elapsed());
                    if !error.is_infrastructure() {
                        self.observations.record(RuntimeObservation {
                            execution_id,
                            file: plan.file,
                            source_revision: plan.source_revision,
                            test_id: test.id,
                            step_id: step.id,
                            range: step.origin.range,
                            kind: RuntimeObservationKind::BrowserFailure {
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
                            },
                        });
                    }
                    events.push(ExecutionEvent::StepFailed {
                        execution_id,
                        test_id: test.id,
                        step_id: step.id,
                        failure: RuntimeFailure::Browser(error.clone()),
                    });
                    failure = Some(StepFailure {
                        step: step.clone(),
                        error,
                        evidence,
                        artifacts,
                    });
                    break;
                }
            }
        }

        drop(page);
        let cleanup_failed = context.close().await.is_err();
        let tainted = cleanup_failed
            || failure
                .as_ref()
                .is_some_and(|failure| failure.error.is_infrastructure());
        let passed = failure.is_none();
        events.push(ExecutionEvent::TestFinished {
            execution_id,
            test_id: test.id,
            passed,
        });
        Ok((
            TestResult {
                name: test.name.clone(),
                passed,
                failure,
                duration: test_started.elapsed(),
            },
            tainted,
        ))
    }
}

async fn execute_step(
    page: &mut dyn Page,
    step: &PlannedStep,
    options: &RunnerOptions,
) -> Result<(), BrowserError> {
    match &step.operation {
        TestOperation::Browser(BrowserOperation::Navigate { url }) => {
            page.open(&resolve_url(options.base_url.as_deref(), url)?)
                .await
        }
        TestOperation::Browser(BrowserOperation::Click { locator }) => {
            page.perform(
                &Action::Click {
                    locator: browser_locator(locator),
                },
                options.action_timeout,
            )
            .await
        }
        TestOperation::Browser(BrowserOperation::Fill { locator, value }) => {
            page.perform(
                &Action::Fill {
                    locator: browser_locator(locator),
                    value: value.clone(),
                },
                options.action_timeout,
            )
            .await
        }
        TestOperation::Browser(BrowserOperation::Type { locator, value }) => {
            page.perform(
                &Action::Type {
                    locator: browser_locator(locator),
                    value: value.clone(),
                },
                options.action_timeout,
            )
            .await
        }
        TestOperation::Browser(BrowserOperation::Press { locator, key }) => {
            page.perform(
                &Action::Press {
                    locator: browser_locator(locator),
                    key: key.clone(),
                },
                options.action_timeout,
            )
            .await
        }
        TestOperation::Browser(BrowserOperation::Check { locator, checked }) => {
            page.perform(
                &Action::Check {
                    locator: browser_locator(locator),
                    checked: *checked,
                },
                options.action_timeout,
            )
            .await
        }
        TestOperation::Browser(BrowserOperation::Select { locator, option }) => {
            page.perform(
                &Action::Select {
                    locator: browser_locator(locator),
                    option: option.clone(),
                },
                options.action_timeout,
            )
            .await
        }
        TestOperation::Browser(BrowserOperation::Hover { locator }) => {
            page.perform(
                &Action::Hover {
                    locator: browser_locator(locator),
                },
                options.action_timeout,
            )
            .await
        }
        TestOperation::Browser(BrowserOperation::WaitForLocator {
            locator,
            state,
            timeout,
        })
        | TestOperation::Assertion(AssertionOperation::Locator {
            locator,
            state,
            timeout,
        }) => {
            let timeout = bounded_timeout(
                timeout.unwrap_or(options.assertion_timeout),
                options.test_timeout,
            );
            page.wait_for_locator(&browser_locator(locator), browser_state(*state), timeout)
                .await
        }
        TestOperation::Browser(BrowserOperation::WaitForUrl { url, timeout })
        | TestOperation::Assertion(AssertionOperation::Url { url, timeout }) => {
            let expected = resolve_url(options.base_url.as_deref(), url)?;
            let timeout = bounded_timeout(
                timeout.unwrap_or(options.assertion_timeout),
                options.test_timeout,
            );
            page.wait_for_url(&expected, timeout).await
        }
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
        TestOperation::Browser(BrowserOperation::Navigate { .. })
        | TestOperation::Browser(BrowserOperation::WaitForUrl { .. })
        | TestOperation::Assertion(AssertionOperation::Url { .. }) => None,
    }
}

fn resolve_url(base_url: Option<&str>, value: &str) -> Result<String, BrowserError> {
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

    use webtest_browser::{BrowserContext, BrowserSession, Page};
    use webtest_hir::{StepId, TestId};
    use webtest_plan::PlannedTest;
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
    struct ContextHost {
        starts: Arc<AtomicUsize>,
        contexts: Arc<AtomicUsize>,
        closes: Arc<AtomicUsize>,
        fail_first_cleanup: bool,
    }
    struct ContextSession {
        contexts: Arc<AtomicUsize>,
        closes: Arc<AtomicUsize>,
        fail_first_cleanup: bool,
    }
    struct CountingContext {
        closes: Arc<AtomicUsize>,
        fail_cleanup: bool,
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
    impl BrowserHost for ContextHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            self.starts.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(ContextSession {
                contexts: Arc::clone(&self.contexts),
                closes: Arc::clone(&self.closes),
                fail_first_cleanup: self.fail_first_cleanup,
            }))
        }
    }
    #[async_trait]
    impl BrowserSession for ContextSession {
        async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
            unreachable!("runtime uses contexts")
        }
        async fn new_context(
            &mut self,
            _options: &BrowserContextOptions,
        ) -> Result<Box<dyn BrowserContext>, BrowserError> {
            let index = self.contexts.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(CountingContext {
                closes: Arc::clone(&self.closes),
                fail_cleanup: self.fail_first_cleanup && index == 0,
            }))
        }
    }
    #[async_trait]
    impl BrowserContext for CountingContext {
        async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
            Ok(Box::new(FakePage {
                result: Mutex::new(Ok(())),
            }))
        }
        async fn close(&mut self) -> Result<(), BrowserError> {
            self.closes.fetch_add(1, Ordering::Relaxed);
            if self.fail_cleanup {
                Err(BrowserError::Protocol {
                    method: "context.close".into(),
                    message: "failed".into(),
                })
            } else {
                Ok(())
            }
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
        async fn evaluate_expression(&mut self, _expression: &str) -> Result<(), BrowserError> {
            self.result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    fn plan(revision: SourceRevision) -> TestPlan {
        let file = FileId::new(0);
        TestPlan {
            file,
            source_revision: revision,
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

    fn two_test_plan() -> TestPlan {
        let mut plan = plan(SourceRevision::of("two"));
        let mut second = plan.tests[0].clone();
        second.id = TestId(1);
        second.name = "y".into();
        second.steps[0].id = StepId(1);
        plan.tests.push(second);
        plan
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
        assert_eq!(
            runner
                .run(&plan(revision), &failed)
                .await
                .expect("run")
                .failed(),
            1
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

    #[test]
    fn artifact_names_are_stable_and_do_not_use_test_names() {
        let directory = tempfile::tempdir().expect("directory");
        let mut evidence = PageEvidence {
            screenshot_png: Some(vec![137, 80, 78, 71]),
            current_url: Some("about:blank".into()),
            ..PageEvidence::default()
        };
        let artifacts = write_artifacts(
            directory.path(),
            ExecutionId(9),
            TestId(2),
            StepId(3),
            &mut evidence,
        );
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.path.ends_with("test-2-step-3-execution-9.png"))
        );
    }

    #[tokio::test]
    async fn tests_use_fresh_contexts_and_cleanup_failure_restarts_the_process() {
        let starts = Arc::new(AtomicUsize::new(0));
        let contexts = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let host = ContextHost {
            starts: Arc::clone(&starts),
            contexts: Arc::clone(&contexts),
            closes: Arc::clone(&closes),
            fail_first_cleanup: true,
        };
        let result = Runner::new(Arc::new(ObservationStore::default()))
            .run(&two_test_plan(), &host)
            .await
            .expect("run");
        assert_eq!(result.passed(), 2);
        assert_eq!(contexts.load(Ordering::Relaxed), 2);
        assert_eq!(closes.load(Ordering::Relaxed), 2);
        assert_eq!(
            starts.load(Ordering::Relaxed),
            2,
            "tainted browser process was replaced"
        );
    }
}
