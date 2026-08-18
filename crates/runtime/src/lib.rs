//! Sequential execution of protocol-neutral test plans.

use std::sync::Arc;

use tracing::instrument;
use webtest_browser::{BrowserError, BrowserHost, Locator as BrowserLocator, Page};
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeFailure, RuntimeObservation,
    RuntimeObservationKind,
};
use webtest_plan::{BrowserOperation, Locator, PlannedStep, TestOperation, TestPlan};

#[derive(Clone, Debug)]
pub struct StepFailure {
    pub step: PlannedStep,
    pub error: BrowserError,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub failure: Option<StepFailure>,
}

#[derive(Clone, Debug)]
pub struct RunResult {
    pub execution_id: ExecutionId,
    pub tests: Vec<TestResult>,
    pub events: Vec<ExecutionEvent>,
}

impl RunResult {
    pub fn passed(&self) -> usize {
        self.tests.iter().filter(|result| result.passed).count()
    }

    pub fn failed(&self) -> usize {
        self.tests.len() - self.passed()
    }
}

pub struct Runner {
    observations: Arc<ObservationStore>,
}

impl Runner {
    pub fn new(observations: Arc<ObservationStore>) -> Self {
        Self { observations }
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
    ) -> Result<RunResult, BrowserError> {
        self.observations.clear_for_file(plan.file);
        let execution_id = ExecutionId::next();
        let mut events = vec![ExecutionEvent::RunStarted { execution_id }];
        let mut session = browser.start().await?;
        let mut tests = Vec::with_capacity(plan.tests.len());

        for test in &plan.tests {
            events.push(ExecutionEvent::TestStarted {
                execution_id,
                test_id: test.id,
                name: test.name.clone(),
            });
            let mut page = session.new_page().await?;
            let mut failure = None;
            for step in &test.steps {
                events.push(ExecutionEvent::StepStarted {
                    execution_id,
                    test_id: test.id,
                    step_id: step.id,
                });
                match execute_step(page.as_mut(), step).await {
                    Ok(()) => events.push(ExecutionEvent::StepPassed {
                        execution_id,
                        test_id: test.id,
                        step_id: step.id,
                    }),
                    Err(error) => {
                        if let BrowserError::LocatorNotFound { locator } = &error {
                            self.observations.record(RuntimeObservation {
                                execution_id,
                                file: plan.file,
                                source_revision: plan.source_revision,
                                test_id: test.id,
                                step_id: step.id,
                                range: step.origin.range,
                                kind: RuntimeObservationKind::LocatorNotFound {
                                    locator: locator.clone(),
                                    page_url: None,
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
                        });
                        break;
                    }
                }
            }
            let passed = failure.is_none();
            events.push(ExecutionEvent::TestFinished {
                execution_id,
                test_id: test.id,
                passed,
            });
            tests.push(TestResult {
                name: test.name.clone(),
                passed,
                failure,
            });
        }
        events.push(ExecutionEvent::RunFinished { execution_id });
        Ok(RunResult {
            execution_id,
            tests,
            events,
        })
    }
}

async fn execute_step(page: &mut dyn Page, step: &PlannedStep) -> Result<(), BrowserError> {
    match &step.operation {
        TestOperation::Browser(BrowserOperation::Open { url }) => page.open(url).await,
        TestOperation::Browser(BrowserOperation::Click { locator }) => {
            let locator = match locator {
                Locator::Id(value) => BrowserLocator::Id(value.clone()),
            };
            page.click(&locator).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use webtest_browser::{BrowserSession, Page};
    use webtest_hir::{StepId, TestId};
    use webtest_plan::{PlannedStep, PlannedTest};
    use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

    use super::*;

    struct FakeHost {
        click: Result<(), BrowserError>,
    }

    struct FakeSession {
        click: Result<(), BrowserError>,
    }

    struct FakePage {
        click: Mutex<Result<(), BrowserError>>,
    }

    #[async_trait]
    impl BrowserHost for FakeHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Ok(Box::new(FakeSession {
                click: self.click.clone(),
            }))
        }
    }

    #[async_trait]
    impl BrowserSession for FakeSession {
        async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
            Ok(Box::new(FakePage {
                click: Mutex::new(self.click.clone()),
            }))
        }
    }

    #[async_trait]
    impl Page for FakePage {
        async fn open(&mut self, _url: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn click(&mut self, _locator: &BrowserLocator) -> Result<(), BrowserError> {
            self.click.lock().unwrap_or_else(|p| p.into_inner()).clone()
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

    #[tokio::test]
    async fn failure_records_revision_bound_observation_and_success_clears_it() {
        let store = Arc::new(ObservationStore::default());
        let runner = Runner::new(Arc::clone(&store));
        let revision = SourceRevision::of("source");
        let missing = BrowserLocator::Id("missing".into());
        let failed = FakeHost {
            click: Err(BrowserError::LocatorNotFound { locator: missing }),
        };
        let result = runner.run(&plan(revision), &failed).await.expect("run");
        assert_eq!(result.failed(), 1);
        assert_eq!(store.observations_for(FileId::new(0), revision).len(), 1);

        let passed = FakeHost { click: Ok(()) };
        runner.run(&plan(revision), &passed).await.expect("run");
        assert!(store.observations_for(FileId::new(0), revision).is_empty());
    }
}
