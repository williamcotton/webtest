use std::{io, path::PathBuf, sync::Arc, time::Instant};

use webtest_browser_cdp::ChromeHost;
use webtest_observation::ObservationStore;
use webtest_provider::Capability;
use webtest_runtime::{RunError, RunOutcome, Runner, TestOutcome};

use crate::{
    chrome::resolve_chrome,
    cli::TestReporter,
    error::AppError,
    project_analysis::analyze_file,
    project_context::{display_path, nanos, project},
    provider_composition::runtime_provider_registry,
    report::{
        ExitClass, FailureReport, FileReport, RunReportOutcome, TestReport, TestReportOutcome,
        WarningReport, base_report, write_human_report_after_progress, write_report,
    },
    runtime_configuration::runner_options,
    runtime_output::{event_reports, run_failure_report, step_failure_report},
    test_progress::HumanTestProgress,
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
    let progress = (reporter == TestReporter::Human).then(|| Arc::new(HumanTestProgress::stdout()));
    let mut progress_error = None;

    if let Some(progress) = &progress {
        record_progress_error(progress.checking(project.files.len()), &mut progress_error);
    }
    let mut prepared = Vec::with_capacity(project.files.len());
    for file in &project.files {
        let started = Instant::now();
        let analyzed = match analyze_file(&project, file) {
            Ok(analyzed) => analyzed,
            Err(error) => {
                if let Some(progress) = &progress {
                    let _ = progress.checking_failed();
                }
                return Err(error);
            }
        };
        let has_static_errors = analyzed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error");
        prepared.push((file, analyzed, started.elapsed(), has_static_errors));
    }
    let files_with_errors = prepared
        .iter()
        .filter(|(_, _, _, has_static_errors)| *has_static_errors)
        .count();
    let runnable_files = prepared.len() - files_with_errors;
    let runnable_tests = prepared
        .iter()
        .filter(|(_, _, _, has_static_errors)| !has_static_errors)
        .map(|(_, analyzed, _, _)| analyzed.plan.tests.len())
        .sum();
    if let Some(progress) = &progress {
        record_progress_error(progress.checked(files_with_errors), &mut progress_error);
        record_progress_error(
            progress.running(runnable_tests, runnable_files),
            &mut progress_error,
        );
    }

    let mut app_start_attempted = false;
    let mut app_started = false;
    let application_progress_message = application_progress_message(&project);

    for (file, analyzed, analysis_duration, has_static_errors) in prepared {
        let started = Instant::now();
        if has_static_errors {
            report.exit_class = report.exit_class.combine(ExitClass::TestFailure);
            report.files.push(FileReport {
                path: display_path(file),
                exit_class: ExitClass::TestFailure,
                source_revision: analyzed.source_revision,
                duration_nanos: nanos(analysis_duration),
                diagnostics: analyzed.diagnostics,
                tests: Vec::new(),
                outcome: None,
                reason: None,
                infrastructure_error: None,
                events: Vec::new(),
            });
            continue;
        }

        let observations = Arc::new(ObservationStore::default());
        let mut runner = Runner::new(observations)
            .with_options(options.clone())
            .with_provider_registry(providers.clone());
        if let Some(progress) = &progress {
            runner = runner.with_event_sink(progress.clone());
        }
        let mut file_report = FileReport {
            path: display_path(file),
            exit_class: ExitClass::Success,
            source_revision: analyzed.source_revision,
            duration_nanos: 0,
            diagnostics: analyzed.diagnostics,
            tests: Vec::new(),
            outcome: None,
            reason: None,
            infrastructure_error: None,
            events: Vec::new(),
        };
        if let Some(provider) = &app_provider {
            let first_attempt = !app_start_attempted;
            if first_attempt {
                app_start_attempted = true;
                if let Some(progress) = &progress {
                    record_progress_error(
                        progress.starting_application(application_progress_message),
                        &mut progress_error,
                    );
                }
            }
            if let Err(error) = provider.start(&project.root).await {
                if first_attempt && let Some(progress) = &progress {
                    record_progress_error(progress.application_started(false), &mut progress_error);
                }
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
                file_report.duration_nanos =
                    nanos(analysis_duration.saturating_add(started.elapsed()));
                file_report.exit_class = ExitClass::Infrastructure;
                report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                report.files.push(file_report);
                continue;
            }
            if first_attempt {
                app_started = true;
                if let Some(progress) = &progress {
                    record_progress_error(progress.application_started(true), &mut progress_error);
                }
            }
        }
        let needs_browser = analyzed
            .plan
            .required_host_capabilities
            .contains(&Capability::Browser);
        if needs_browser && let Some(progress) = &progress {
            record_progress_error(
                progress.starting_browser(&file_report.path, show_browser),
                &mut progress_error,
            );
        }
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
                    if let Some(progress) = &progress {
                        record_progress_error(
                            progress.browser_run_finished(false),
                            &mut progress_error,
                        );
                    }
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
                    file_report.duration_nanos =
                        nanos(analysis_duration.saturating_add(started.elapsed()));
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
        if let Some(progress) = &progress {
            record_progress_error(
                progress.browser_run_finished(run.is_ok()),
                &mut progress_error,
            );
        }
        match run {
            Err(_) => {
                file_report.outcome = Some(RunReportOutcome::Aborted);
                file_report.reason = Some("test file timeout".into());
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
            Ok(result) => {
                file_report.duration_nanos = nanos(result.duration);
                file_report.events = event_reports(&file_report.path, &result.events);
                let run_exit_class = match &result.outcome {
                    RunOutcome::Completed => ExitClass::Success,
                    RunOutcome::Cancelled { reason } => {
                        file_report.outcome = Some(RunReportOutcome::Cancelled);
                        file_report.reason = Some(cancellation_reason_name(*reason).into());
                        ExitClass::TestFailure
                    }
                    RunOutcome::Aborted { failure } => {
                        file_report.outcome = Some(RunReportOutcome::Aborted);
                        file_report.reason = Some(failure.to_string());
                        if result.aborted() == 0 {
                            file_report.infrastructure_error = Some(run_failure_report(failure));
                        }
                        run_error_exit_class(failure)
                    }
                };
                if matches!(result.outcome, RunOutcome::Completed) {
                    file_report.outcome = Some(RunReportOutcome::Completed);
                }
                file_report.tests = result
                    .tests
                    .into_iter()
                    .map(|test| {
                        let (outcome, reason, timeout_nanos, failure, exit_class) =
                            match test.outcome {
                                TestOutcome::Passed => (
                                    TestReportOutcome::Passed,
                                    None,
                                    None,
                                    None,
                                    ExitClass::Success,
                                ),
                                TestOutcome::Failed(failure) => (
                                    TestReportOutcome::Failed,
                                    None,
                                    None,
                                    Some(step_failure_report(*failure, &analyzed.source)),
                                    ExitClass::TestFailure,
                                ),
                                TestOutcome::TimedOut { timeout } => (
                                    TestReportOutcome::TimedOut,
                                    Some(format!("timed out after {}ms", timeout.as_millis())),
                                    Some(nanos(timeout)),
                                    None,
                                    ExitClass::TestFailure,
                                ),
                                TestOutcome::Cancelled { reason } => (
                                    TestReportOutcome::Cancelled,
                                    Some(cancellation_reason_name(reason).into()),
                                    None,
                                    None,
                                    ExitClass::TestFailure,
                                ),
                                TestOutcome::Skipped { reason } => (
                                    TestReportOutcome::Skipped,
                                    Some(skip_reason_name(reason).into()),
                                    None,
                                    None,
                                    match reason {
                                        webtest_runtime::SkipReason::RunCancelled => {
                                            ExitClass::TestFailure
                                        }
                                        webtest_runtime::SkipReason::RunAborted => {
                                            ExitClass::Infrastructure
                                        }
                                    },
                                ),
                                TestOutcome::Aborted { failure } => {
                                    let exit_class = run_error_exit_class(&failure);
                                    (
                                        TestReportOutcome::Aborted,
                                        Some(failure.to_string()),
                                        None,
                                        Some(run_failure_report(&failure)),
                                        exit_class,
                                    )
                                }
                            };
                        TestReport {
                            name: test.name,
                            exit_class,
                            outcome,
                            reason,
                            timeout_nanos,
                            duration_nanos: nanos(test.duration),
                            failure,
                        }
                    })
                    .collect();
                let tests_exit_class = file_report
                    .tests
                    .iter()
                    .fold(ExitClass::Success, |class, test| {
                        class.combine(test.exit_class)
                    });
                file_report.exit_class = run_exit_class.combine(tests_exit_class);
                if file_report.exit_class != ExitClass::Success {
                    report.exit_class = report.exit_class.combine(file_report.exit_class);
                }
            }
        }
        if file_report.duration_nanos == 0 {
            file_report.duration_nanos = nanos(analysis_duration.saturating_add(started.elapsed()));
        }
        report.files.push(file_report);
    }
    if let Some(provider) = app_provider {
        if app_started && let Some(progress) = &progress {
            record_progress_error(progress.stopping_application(), &mut progress_error);
        }
        let shutdown = provider.shutdown().await;
        if app_started && let Some(progress) = &progress {
            record_progress_error(
                progress.application_stopped(shutdown.is_ok()),
                &mut progress_error,
            );
        }
        if let Err(error) = shutdown {
            report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
            report.warnings.push(WarningReport {
                code: "app.teardown".into(),
                key: "server.app".into(),
                message: error.to_string(),
            });
        }
    }
    report.finish();
    if let Some(progress) = &progress {
        record_progress_error(progress.check_error(), &mut progress_error);
    }
    if let Some(error) = progress_error {
        return Err(AppError::infrastructure(error));
    }
    if reporter == TestReporter::Human {
        write_human_report_after_progress(&report)?;
    } else {
        write_report(&report, reporter.into())?;
    }
    Ok(report.exit_class)
}

fn run_error_exit_class(error: &RunError) -> ExitClass {
    match error {
        RunError::Browser(_) | RunError::Provider(_) => ExitClass::Infrastructure,
        RunError::Internal(_) => ExitClass::Internal,
    }
}

fn cancellation_reason_name(reason: webtest_runtime::CancellationReason) -> &'static str {
    match reason {
        webtest_runtime::CancellationReason::Requested => "requested",
    }
}

fn skip_reason_name(reason: webtest_runtime::SkipReason) -> &'static str {
    match reason {
        webtest_runtime::SkipReason::RunCancelled => "run_cancelled",
        webtest_runtime::SkipReason::RunAborted => "run_aborted",
    }
}

fn record_progress_error(result: io::Result<()>, first_error: &mut Option<io::Error>) {
    if let Err(error) = result
        && first_error.is_none()
    {
        *first_error = Some(error);
    }
}

fn application_progress_message(project: &webtest_project::Project) -> &'static str {
    let bridge = project
        .config
        .server
        .app
        .as_ref()
        .is_some_and(|app| app.adapter == webtest_project::ServerAppAdapter::Bridge);
    let owned = project
        .config
        .app
        .as_ref()
        .is_some_and(|application| application.owned);
    let health = project
        .config
        .app
        .as_ref()
        .and_then(|application| application.health.as_ref())
        .is_some();
    match (bridge, owned, health) {
        (true, true, true) => {
            "starting application, waiting for health check, and verifying app bridge"
        }
        (true, true, false) => "starting application and verifying app bridge",
        (true, false, true) => "waiting for application health check and verifying app bridge",
        (true, false, false) => "connecting to application bridge",
        (false, true, true) => "starting application and waiting for health check",
        (false, true, false) => "starting application",
        (false, false, true) => "waiting for application health check",
        (false, false, false) => "preparing application provider",
    }
}
