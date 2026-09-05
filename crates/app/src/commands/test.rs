use std::{io, path::PathBuf, sync::Arc, time::Instant};

use webtest_feedback::FailureClass;
use webtest_observation::ObservationStore;
use webtest_runtime::{RunError, RunOutcome, Runner, TestOutcome};

use crate::{
    chrome::LazyChromeHost,
    cli::TestReporter,
    error::AppError,
    project_analysis::analyze_file,
    project_context::{display_path, nanos, project},
    provider_composition::{runtime_application, runtime_provider_registry},
    report::{
        ExecutionFailureReport, ExitClass, FailureReport, FileReport, RunReportOutcome, TestReport,
        TestReportOutcome, WarningReport, base_report, write_human_report_after_progress,
        write_report,
    },
    runtime_configuration::runner_options,
    runtime_output::{
        aborted_run_failure_report, aborted_test_failure_report, event_reports, step_failure_report,
    },
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
    let options = runner_options(&project);
    let runtime_providers = runtime_provider_registry(&project, &options)?;
    let application = runtime_application(&project, runtime_providers.app.clone());
    let providers = runtime_providers.registry;
    let progress = (reporter == TestReporter::Human).then(|| Arc::new(HumanTestProgress::stdout()));
    let browser = LazyChromeHost::new(project.clone(), chrome_path, show_browser, progress.clone());
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
                execution_error: None,
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
            execution_error: None,
            events: Vec::new(),
        };
        if let Some(application) = &application {
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
            if let Err(error) = application.start(&project).await {
                if first_attempt && let Some(progress) = &progress {
                    record_progress_error(progress.application_started(false), &mut progress_error);
                }
                file_report.execution_error = Some(ExecutionFailureReport {
                    class: FailureClass::Infrastructure,
                    failure: FailureReport {
                        diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                        repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                        code: webtest_observation::RuntimeFailureCode::from(&error)
                            .diagnostic_code()
                            .into(),
                        message: error.to_string(),
                        span: None,
                        diff: None,
                        artifacts: Vec::new(),
                        semantic_details: None,
                        repair_hints: Vec::new(),
                        page: None,
                        secondary: Vec::new(),
                    },
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
        let file_browser = browser.for_file(file_report.path.clone());
        let result = runner.run(&analyzed.plan, &file_browser).await;
        file_report.duration_nanos = nanos(result.duration);
        file_report.events = event_reports(&file_report.path, &result.events);
        let run_exit_class = match &result.outcome {
            RunOutcome::Completed => ExitClass::Success,
            RunOutcome::Cancelled { reason } => {
                file_report.outcome = Some(RunReportOutcome::Cancelled);
                file_report.reason = Some(cancellation_reason_name(*reason).into());
                ExitClass::TestFailure
            }
            RunOutcome::Aborted {
                failure,
                prior_outcome,
            } => {
                file_report.outcome = Some(RunReportOutcome::Aborted);
                file_report.reason = Some(failure.to_string());
                if result.aborted() == 0 {
                    file_report.execution_error = Some(ExecutionFailureReport {
                        class: failure.failure_class(),
                        failure: aborted_run_failure_report(failure, *prior_outcome),
                    });
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
                let test_id = test.test_id;
                let (outcome, failure_class, reason, timeout_nanos, failure, exit_class) =
                    match test.outcome {
                        TestOutcome::Passed => (
                            TestReportOutcome::Passed,
                            None,
                            None,
                            None,
                            None,
                            ExitClass::Success,
                        ),
                        TestOutcome::Failed(failure) => (
                            TestReportOutcome::Failed,
                            Some(FailureClass::Test),
                            None,
                            None,
                            Some(step_failure_report(*failure, &analyzed.source)),
                            ExitClass::TestFailure,
                        ),
                        TestOutcome::TimedOut {
                            timeout,
                            active_step,
                        } => (
                            TestReportOutcome::TimedOut,
                            Some(FailureClass::Test),
                            Some(format!("timed out after {}ms", timeout.as_millis())),
                            Some(nanos(timeout)),
                            analyzed
                                .plan
                                .tests
                                .iter()
                                .find(|planned| planned.id == test_id)
                                .map(|planned| {
                                    let origin = active_step
                                        .and_then(|id| {
                                            planned
                                                .steps()
                                                .iter()
                                                .find(|step| step.id == id)
                                                .map(|step| step.origin)
                                        })
                                        .unwrap_or(planned.origin);
                                    FailureReport {
                                        diagnostic_schema_version:
                                            webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                                        repair_hint_schema_version:
                                            webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                                        code: webtest_observation::RuntimeFailureCode::TestTimeout
                                            .diagnostic_code()
                                            .into(),
                                        message: format!(
                                            "test timed out after {}ms",
                                            timeout.as_millis()
                                        ),
                                        span: Some(crate::source_output::source_span(
                                            &analyzed.source,
                                            origin.range,
                                        )),
                                        diff: None,
                                        artifacts: Vec::new(),
                                        semantic_details: Some(serde_json::json!({
                                            "test_id": test_id.0,
                                            "active_step_id": active_step.map(|step| step.0),
                                            "timeout_ms": timeout.as_millis(),
                                        })),
                                        repair_hints: Vec::new(),
                                        page: None,
                                        secondary: Vec::new(),
                                    }
                                }),
                            ExitClass::TestFailure,
                        ),
                        TestOutcome::Cancelled { reason } => (
                            TestReportOutcome::Cancelled,
                            None,
                            Some(cancellation_reason_name(reason).into()),
                            None,
                            None,
                            ExitClass::TestFailure,
                        ),
                        TestOutcome::Skipped {
                            reason,
                            failure_class,
                        } => (
                            TestReportOutcome::Skipped,
                            failure_class,
                            Some(skip_reason_name(reason).into()),
                            None,
                            None,
                            failure_class.map_or(ExitClass::TestFailure, |class| {
                                ExitClass::from_failure_class(class)
                            }),
                        ),
                        TestOutcome::Aborted {
                            failure,
                            prior_outcome,
                        } => {
                            let exit_class = run_error_exit_class(&failure);
                            (
                                TestReportOutcome::Aborted,
                                Some(failure.failure_class()),
                                Some(failure.to_string()),
                                None,
                                Some(aborted_test_failure_report(
                                    &failure,
                                    prior_outcome,
                                    &analyzed.source,
                                )),
                                exit_class,
                            )
                        }
                    };
                TestReport {
                    name: test.name,
                    exit_class,
                    outcome,
                    failure_class,
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
        if file_report.duration_nanos == 0 {
            file_report.duration_nanos = nanos(analysis_duration.saturating_add(started.elapsed()));
        }
        report.files.push(file_report);
    }
    if let Some(application) = application {
        if app_started && let Some(progress) = &progress {
            record_progress_error(progress.stopping_application(), &mut progress_error);
        }
        let shutdown = application.shutdown().await;
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
                key: application.configuration_key().into(),
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
    ExitClass::from_failure_class(error.failure_class())
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
