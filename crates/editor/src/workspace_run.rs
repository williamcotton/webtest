//! Coherent selected-test execution, independent of editor transports.
use std::sync::Arc;
use webtest_analysis::SourceLocation;
use webtest_browser::BrowserHost;
use webtest_model::{TestDeclarationId, TestVariantId};
use webtest_observation::{Artifact, TestOutcomeKind};
use webtest_plan::TestPlan;
use webtest_runtime::{RunControl, Runner, TestOutcome};

pub struct PreparedWorkspaceRun {
    pub(crate) lifecycle: Option<Arc<dyn crate::RunLifecycle>>,
    pub workspace_revision: u64,
    pub(crate) plans: Vec<Arc<TestPlan>>,
    pub(crate) runner: Runner,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct WorkspaceTestResult {
    pub declaration_id: TestDeclarationId,
    pub variant_id: TestVariantId,
    pub outcome: TestOutcomeKind,
    pub duration_ms: u64,
    pub message: Option<String>,
    pub failure_code: Option<webtest_observation::RuntimeFailureCode>,
    pub failure_class: Option<webtest_feedback::FailureClass>,
    pub location: Option<SourceLocation>,
    pub related_locations: Vec<SourceLocation>,
    pub artifacts: Vec<Artifact>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct WorkspaceRunSummary {
    pub workspace_revision: u64,
    pub tests: Vec<WorkspaceTestResult>,
    pub failures: Vec<WorkspaceRunFailure>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct WorkspaceRunFailure {
    pub variants: Vec<TestVariantId>,
    pub file: webtest_text::FileId,
    pub message: String,
    pub failure: webtest_observation::ExecutionFailure,
}
impl PreparedWorkspaceRun {
    /// The caller must keep this future owned until cancellation cleanup completes.
    pub async fn execute(
        self,
        browser: &dyn BrowserHost,
        control: Option<&dyn RunControl>,
    ) -> WorkspaceRunSummary {
        let mut tests = Vec::new();
        let mut failures = Vec::new();
        let lifecycle_failure = |failure: webtest_provider::ProviderError| {
            self.plans.first().map(|plan| {
                let failure = webtest_runtime::RunError::Provider(failure);
                WorkspaceRunFailure {
                    variants: self
                        .plans
                        .iter()
                        .flat_map(|plan| plan.tests.iter().map(|test| test.variant_id))
                        .collect(),
                    file: plan.file,
                    message: failure.to_string(),
                    failure: failure.to_execution_failure(),
                }
            })
        };
        let lease = match &self.lifecycle {
            Some(lifecycle) => match lifecycle.begin().await {
                Ok(lease) => Some(lease),
                Err(error) => {
                    failures.extend(lifecycle_failure(error));
                    return WorkspaceRunSummary {
                        workspace_revision: self.workspace_revision,
                        tests,
                        failures,
                    };
                }
            },
            None => None,
        };
        let inputs = self
            .plans
            .iter()
            .map(|plan| webtest_runtime::TestRun {
                runner: &self.runner,
                plan,
                browser,
                control,
            })
            .collect::<Vec<_>>();
        let runs = webtest_runtime::run_jobs(&inputs, webtest_runtime::JobLimit::default()).await;
        for (plan, run) in self.plans.iter().zip(runs) {
            if let webtest_runtime::RunOutcome::Aborted { failure, .. } = &run.outcome {
                failures.push(WorkspaceRunFailure {
                    variants: run.tests.iter().map(|test| test.variant_id).collect(),
                    file: plan.file,
                    message: failure.to_string(),
                    failure: failure.to_execution_failure(),
                });
            }
            for result in run.tests {
                let (failure_code, failure_class) = match &result.outcome {
                    TestOutcome::Failed(failure) => (
                        Some(failure.error.code()),
                        Some(failure.error.failure_class()),
                    ),
                    TestOutcome::TimedOut { .. } => (
                        Some(webtest_observation::RuntimeFailureCode::TestTimeout),
                        Some(webtest_feedback::FailureClass::Test),
                    ),
                    TestOutcome::Aborted { failure, .. } => {
                        (Some(failure.code()), Some(failure.failure_class()))
                    }
                    _ => (None, None),
                };
                let (outcome, message, location, related_locations, artifacts) =
                    match result.outcome {
                        TestOutcome::Verdict { kind, message } => (
                            match kind {
                                webtest_model::VerdictKind::Pass => TestOutcomeKind::Passed,
                                webtest_model::VerdictKind::Fail => TestOutcomeKind::Failed,
                                webtest_model::VerdictKind::Skipped => TestOutcomeKind::Skipped,
                                webtest_model::VerdictKind::Inconclusive => {
                                    TestOutcomeKind::Inconclusive
                                }
                            },
                            Some(message),
                            None,
                            Vec::new(),
                            Vec::new(),
                        ),
                        TestOutcome::Passed => {
                            (TestOutcomeKind::Passed, None, None, Vec::new(), Vec::new())
                        }
                        TestOutcome::Failed(failure) => {
                            let origin = failure.step.origin;
                            let source = plan
                                .source_files
                                .iter()
                                .find(|source| source.file == origin.file);
                            (
                                TestOutcomeKind::Failed,
                                Some(failure.error.to_string()),
                                source.map(|source| SourceLocation {
                                    path: source.path.clone(),
                                    origin,
                                    revision: source.revision,
                                }),
                                failure.step.origin_chain,
                                failure.artifacts,
                            )
                        }
                        TestOutcome::TimedOut { timeout, .. } => (
                            TestOutcomeKind::TimedOut,
                            Some(format!("test exceeded {} ms", timeout.as_millis())),
                            None,
                            Vec::new(),
                            Vec::new(),
                        ),
                        TestOutcome::Cancelled { .. } => (
                            TestOutcomeKind::Cancelled,
                            None,
                            None,
                            Vec::new(),
                            Vec::new(),
                        ),
                        TestOutcome::Skipped { reason, .. } => (
                            if reason == webtest_observation::SkipReason::RunCancelled {
                                TestOutcomeKind::Cancelled
                            } else {
                                TestOutcomeKind::Aborted
                            },
                            Some(format!("{reason:?}")),
                            None,
                            Vec::new(),
                            Vec::new(),
                        ),
                        TestOutcome::Aborted { failure, .. } => (
                            TestOutcomeKind::Aborted,
                            Some(failure.to_string()),
                            None,
                            Vec::new(),
                            Vec::new(),
                        ),
                    };
                tests.push(WorkspaceTestResult {
                    declaration_id: result.test_declaration_id,
                    variant_id: result.variant_id,
                    outcome,
                    duration_ms: result.duration.as_millis().min(u64::MAX as u128) as u64,
                    message,
                    failure_code,
                    failure_class,
                    location,
                    related_locations,
                    artifacts,
                });
            }
        }
        if let Some(lease) = lease
            && let Err(error) = lease.finish().await
        {
            failures.extend(lifecycle_failure(error));
        }
        WorkspaceRunSummary {
            workspace_revision: self.workspace_revision,
            tests,
            failures,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorError, EditorService};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};
    use webtest_analysis::{AnalysisDatabase, WorkspaceConfiguration};
    use webtest_browser::{BrowserError, BrowserSession};
    struct NoBrowser;
    #[async_trait]
    impl BrowserHost for NoBrowser {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            panic!("pure test must not start a browser")
        }
    }
    #[tokio::test]
    async fn selected_files_share_one_captured_workspace_and_reject_stale_discovery() {
        let mut database = AnalysisDatabase::default();
        database.configure_workspace(WorkspaceConfiguration {
            root: "/workspace".into(),
            ..Default::default()
        });
        database.open_file(
            "/workspace/a.webtest",
            "import { verify } from \"./helpers\" test \"same\" { verify() }",
        );
        database.open_file(
            "/workspace/b.webtest",
            "import { verify } from \"./helpers\" test \"same\" { verify() }",
        );
        let helper = database.open_file(
            "/workspace/helpers.webtest",
            "export fn verify() -> Null { expect 1 == 1 }",
        );
        let editor = EditorService::with_analysis_database(
            Default::default(),
            webtest_provider::ProviderRegistry::built_in_schemas(),
            database,
        );
        let discovery = editor.snapshot().discover_tests(100).unwrap();
        let selected = discovery
            .modules
            .iter()
            .flat_map(|module| &module.tests)
            .flat_map(|test| &test.variants)
            .map(|variant| variant.id)
            .collect();
        let prepared = editor
            .prepare_variants(discovery.workspace_revision, &selected)
            .unwrap();
        editor.update_document(helper, "export fn verify() -> Null { expect 1 == 2 }");
        assert!(matches!(
            editor.prepare_variants(discovery.workspace_revision, &selected),
            Err(EditorError::StaleDiscovery)
        ));
        let result = prepared.execute(&NoBrowser, None).await;
        assert_eq!(result.tests.len(), 2);
        assert!(
            result
                .tests
                .iter()
                .all(|test| test.outcome == TestOutcomeKind::Passed)
        );
        let fresh = editor
            .prepare_variants(editor.analysis_revision(), &selected)
            .unwrap()
            .execute(&NoBrowser, None)
            .await;
        assert_eq!(fresh.tests.len(), 2);
        for test in fresh.tests {
            assert_eq!(test.outcome, TestOutcomeKind::Failed);
            assert_eq!(test.location.unwrap().path, "/workspace/helpers.webtest");
            assert_eq!(test.related_locations.len(), 1);
        }
    }
    #[derive(Default)]
    struct Cancel(AtomicBool);
    #[async_trait]
    impl RunControl for Cancel {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
        async fn before_step(&self, _: &webtest_plan::PlannedTest, _: &webtest_plan::PlannedStep) {}
    }
    #[tokio::test]
    async fn cancellation_finishes_active_backoff_and_returns_every_selected_variant() {
        let editor = EditorService::new();
        editor.open_document(
            "cancel.webtest",
            "test \"active\" { retry 64 backoff 1s { expect false } } test \"queued\" {}",
        );
        let discovery = editor.snapshot().discover_tests(100).unwrap();
        let selected = discovery
            .modules
            .iter()
            .flat_map(|module| &module.tests)
            .flat_map(|test| &test.variants)
            .map(|variant| variant.id)
            .collect();
        let prepared = editor
            .prepare_variants(discovery.workspace_revision, &selected)
            .unwrap();
        let control = Arc::new(Cancel::default());
        let cancellation = control.clone();
        let running =
            tokio::spawn(async move { prepared.execute(&NoBrowser, Some(control.as_ref())).await });
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.0.store(true, Ordering::Release);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.tests.len(), 2);
        assert!(
            result
                .tests
                .iter()
                .all(|test| test.outcome == TestOutcomeKind::Cancelled)
        );
    }
}
