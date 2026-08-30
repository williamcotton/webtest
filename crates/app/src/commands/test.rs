use std::{path::PathBuf, sync::Arc, time::Instant};

use webtest_browser_cdp::ChromeHost;
use webtest_observation::ObservationStore;
use webtest_provider::Capability;
use webtest_runtime::Runner;

use crate::{
    chrome::resolve_chrome,
    cli::TestReporter,
    error::AppError,
    project_analysis::analyze_file,
    project_context::{display_path, nanos, project},
    provider_composition::runtime_provider_registry,
    report::{
        ExitClass, FailureReport, FileReport, TestReport, WarningReport, base_report, write_report,
    },
    runtime_configuration::runner_options,
    runtime_output::{event_reports, run_error_code, run_error_message, step_failure_report},
};

pub(crate) async fn run_test(
    paths: Vec<PathBuf>,
    chrome_path: Option<PathBuf>,
    headed: bool,
    reporter: TestReporter,
) -> Result<ExitClass, AppError> {
    let project = project(&paths)?;
    let mut report = base_report("test", &project);
    let show_browser = headed || !project.config.browser.headless;
    let mut browser = None;
    let options = runner_options(&project);
    let runtime_providers = runtime_provider_registry(&project, &options)?;
    let providers = runtime_providers.registry;
    let app_provider = runtime_providers.app;

    for file in &project.files {
        let started = Instant::now();
        let analyzed = analyze_file(&project, file)?;
        let has_static_errors = analyzed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error");
        if has_static_errors {
            report.exit_class = report.exit_class.combine(ExitClass::TestFailure);
            report.files.push(FileReport {
                path: display_path(file),
                exit_class: ExitClass::TestFailure,
                source_revision: analyzed.source_revision,
                duration_nanos: nanos(started.elapsed()),
                diagnostics: analyzed.diagnostics,
                tests: Vec::new(),
                infrastructure_error: None,
                events: Vec::new(),
            });
            continue;
        }

        let observations = Arc::new(ObservationStore::default());
        let runner = Runner::new(observations)
            .with_options(options.clone())
            .with_provider_registry(providers.clone());
        let mut file_report = FileReport {
            path: display_path(file),
            exit_class: ExitClass::Success,
            source_revision: analyzed.source_revision,
            duration_nanos: 0,
            diagnostics: analyzed.diagnostics,
            tests: Vec::new(),
            infrastructure_error: None,
            events: Vec::new(),
        };
        if let Some(provider) = &app_provider
            && let Err(error) = provider.start(&project.root).await
        {
            file_report.infrastructure_error = Some(FailureReport {
                diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                code: format!("runtime.{}", error.code()),
                message: error.to_string(),
                span: None,
                diff: None,
                artifacts: Vec::new(),
                semantic_details: None,
                repair_hints: Vec::new(),
                page: None,
                secondary: Vec::new(),
            });
            file_report.duration_nanos = nanos(started.elapsed());
            file_report.exit_class = ExitClass::Infrastructure;
            report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
            report.files.push(file_report);
            continue;
        }
        let needs_browser = analyzed
            .plan
            .required_host_capabilities
            .contains(&Capability::Browser);
        if needs_browser && browser.is_none() {
            match resolve_chrome(&project, chrome_path.clone()) {
                Ok(resolved) => {
                    browser = Some(
                        ChromeHost::new(Some(resolved.path))
                            .with_headed(show_browser)
                            .with_timeouts(
                                project.config.timeouts.browser_command,
                                project.config.timeouts.navigation,
                            ),
                    );
                }
                Err(error) => {
                    file_report.infrastructure_error = Some(FailureReport {
                        diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                        repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                        code: "runtime.browser_launch".into(),
                        message: error.message,
                        span: None,
                        diff: None,
                        artifacts: Vec::new(),
                        semantic_details: None,
                        repair_hints: Vec::new(),
                        page: None,
                        secondary: Vec::new(),
                    });
                    file_report.duration_nanos = nanos(started.elapsed());
                    report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                    file_report.exit_class = ExitClass::Infrastructure;
                    report.files.push(file_report);
                    continue;
                }
            }
        }
        let inactive_browser = ChromeHost::new(None);
        let browser = browser.as_ref().unwrap_or(&inactive_browser);
        let run = tokio::time::timeout(
            project.config.timeouts.test,
            runner.run(&analyzed.plan, browser),
        )
        .await;
        match run {
            Err(_) => {
                file_report.infrastructure_error = Some(FailureReport {
                    diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                    repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                    code: "runtime.test_timeout".into(),
                    message: format!(
                        "test file exceeded its {}ms timeout",
                        project.config.timeouts.test.as_millis()
                    ),
                    span: None,
                    diff: None,
                    artifacts: Vec::new(),
                    semantic_details: Some(serde_json::json!({
                        "timeout_ms": project.config.timeouts.test.as_millis(),
                    })),
                    repair_hints: Vec::new(),
                    page: None,
                    secondary: Vec::new(),
                });
                report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                file_report.exit_class = ExitClass::Infrastructure;
            }
            Ok(Err(error)) => {
                file_report.infrastructure_error = Some(FailureReport {
                    diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                    repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                    code: run_error_code(&error),
                    message: run_error_message(&error),
                    span: None,
                    diff: None,
                    artifacts: Vec::new(),
                    semantic_details: Some(serde_json::json!({
                        "failure_class": "infrastructure",
                    })),
                    repair_hints: Vec::new(),
                    page: None,
                    secondary: Vec::new(),
                });
                report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                file_report.exit_class = ExitClass::Infrastructure;
            }
            Ok(Ok(result)) => {
                file_report.duration_nanos = nanos(result.duration);
                file_report.events = event_reports(&file_report.path, &result.events);
                file_report.tests = result
                    .tests
                    .into_iter()
                    .map(|test| {
                        let failure = test
                            .failure
                            .map(|failure| step_failure_report(failure, &analyzed.source));
                        TestReport {
                            name: test.name,
                            exit_class: if test.passed {
                                ExitClass::Success
                            } else {
                                ExitClass::TestFailure
                            },
                            passed: test.passed,
                            duration_nanos: nanos(test.duration),
                            failure,
                        }
                    })
                    .collect();
                if file_report.tests.iter().any(|test| !test.passed) {
                    report.exit_class = report.exit_class.combine(ExitClass::TestFailure);
                    file_report.exit_class = ExitClass::TestFailure;
                }
            }
        }
        if file_report.duration_nanos == 0 {
            file_report.duration_nanos = nanos(started.elapsed());
        }
        report.files.push(file_report);
    }
    if let Some(provider) = app_provider
        && let Err(error) = provider.shutdown().await
    {
        report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
        report.warnings.push(WarningReport {
            code: "app.teardown".into(),
            key: "server.app".into(),
            message: error.to_string(),
        });
    }
    report.finish();
    write_report(&report, reporter.into())?;
    Ok(report.exit_class)
}
