use webtest_browser::{BrowserError, Locator};
use webtest_observation::{
    CleanupCause, CleanupFailure, ExecutionEvent, RunOutcomeKind, RuntimeFailure,
    RuntimeFailureCode, SkipReason, TestOutcomeKind,
};
use webtest_provider::ProviderError;
use webtest_runtime::{PriorRunOutcome, PriorTestOutcome, RunError, StepError, StepFailure};

use crate::{
    project_context::normalized_path,
    report::{EventReport, ExitClass, FailureReport, REPORT_SCHEMA_VERSION},
    source_output::source_span,
};

fn runtime_message(error: &BrowserError) -> String {
    match error {
        BrowserError::LocatorNotFound { locator } => {
            format!(
                "No element with {} was found.",
                locator_description(locator)
            )
        }
        BrowserError::LocatorNotVisible { locator } => format!(
            "The element with {} was not visible.",
            locator_description(locator)
        ),
        _ => error.to_string(),
    }
}

fn infrastructure_message(error: &BrowserError) -> String {
    match error {
        BrowserError::Launch(message) if message.contains("Chrome was not found") => format!(
            "{message}. Install the managed browser with `webtest browser install` or configure an explicit path"
        ),
        _ => error.to_string(),
    }
}

pub(crate) fn run_error_code(error: &RunError) -> String {
    error.code().diagnostic_code().into()
}

pub(crate) fn run_error_message(error: &RunError) -> String {
    match error {
        RunError::Browser(error) => infrastructure_message(error),
        RunError::Multiple { primary, .. } => run_error_message(primary),
        _ => error.to_string(),
    }
}

pub(crate) fn run_failure_report(error: &RunError) -> FailureReport {
    FailureReport {
        diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        code: run_error_code(error),
        message: run_error_message(error),
        span: None,
        diff: None,
        artifacts: Vec::new(),
        semantic_details: Some(run_error_semantic_details(error)),
        repair_hints: Vec::new(),
        page: None,
        secondary: Vec::new(),
    }
}

pub(crate) fn aborted_run_failure_report(
    error: &RunError,
    prior_outcome: Option<PriorRunOutcome>,
) -> FailureReport {
    let mut report = run_failure_report(error);
    if let Some(PriorRunOutcome::Cancelled { reason }) = prior_outcome {
        insert_prior_outcome(
            &mut report,
            serde_json::json!({
                "kind": "cancelled",
                "reason": format!("{reason:?}").to_ascii_lowercase(),
            }),
        );
    }
    report
}

pub(crate) fn aborted_test_failure_report(
    error: &RunError,
    prior_outcome: Option<Box<PriorTestOutcome>>,
    source: &str,
) -> FailureReport {
    let mut report = run_failure_report(error);
    let prior = match prior_outcome.map(|prior| *prior) {
        Some(PriorTestOutcome::Failed(failure)) => serde_json::json!({
            "kind": "failed",
            "failure": step_failure_report(*failure, source),
        }),
        Some(PriorTestOutcome::TimedOut {
            timeout,
            active_step,
        }) => serde_json::json!({
            "kind": "timed_out",
            "timeout_nanos": timeout.as_nanos().min(u128::from(u64::MAX)) as u64,
            "active_step_id": active_step.map(|step| step.0),
        }),
        Some(PriorTestOutcome::Cancelled { reason }) => serde_json::json!({
            "kind": "cancelled",
            "reason": format!("{reason:?}").to_ascii_lowercase(),
        }),
        None => return report,
    };
    insert_prior_outcome(&mut report, prior);
    report
}

fn insert_prior_outcome(report: &mut FailureReport, prior: serde_json::Value) {
    let details = report
        .semantic_details
        .get_or_insert_with(|| serde_json::json!({}));
    if let Some(details) = details.as_object_mut() {
        details.insert("prior_outcome".into(), prior);
    }
}

fn run_error_semantic_details(error: &RunError) -> serde_json::Value {
    match error {
        RunError::Cleanup(failure) => cleanup_failure_details(failure),
        RunError::Multiple { primary, secondary } => serde_json::json!({
            "failure_class": error.failure_class(),
            "primary": run_error_semantic_details(primary),
            "secondary": secondary.iter().map(run_error_semantic_details).collect::<Vec<_>>(),
        }),
        RunError::Browser(_) | RunError::Provider(_) | RunError::Internal(_) => {
            serde_json::json!({
                "code": run_error_code(error),
                "failure_class": error.failure_class(),
                "message": run_error_message(error),
            })
        }
    }
}

fn cleanup_failure_details(failure: &CleanupFailure) -> serde_json::Value {
    let cause = match &failure.cause {
        CleanupCause::Browser(error) => serde_json::json!({
            "kind": "browser",
            "code": RuntimeFailureCode::from(error).short_code(),
            "message": error.to_string(),
        }),
        CleanupCause::Io(error) => serde_json::json!({
            "kind": "io",
            "error_kind": error.kind,
            "raw_os_error": error.raw_os_error,
            "message": error.message,
        }),
        CleanupCause::Internal { message } => serde_json::json!({
            "kind": "internal",
            "message": message,
        }),
    };
    serde_json::json!({
        "code": failure.code().diagnostic_code(),
        "failure_class": failure.failure_class(),
        "resource": failure.resource,
        "cause": cause,
    })
}

fn step_error_code(error: &StepError) -> String {
    error.code().diagnostic_code().into()
}

pub(crate) fn step_failure_report(mut failure: StepFailure, source: &str) -> FailureReport {
    let range = failure.step.origin.range;
    for hint in &mut failure.repair_hints {
        if hint.source_range.is_none() {
            hint.source_range = Some(webtest_feedback::ByteRange {
                start: range.start().into(),
                end: range.end().into(),
            });
        }
    }
    let semantic_details = step_semantic_details(&failure);
    let page = failure
        .inspection
        .as_ref()
        .map(|inspection| inspection.page.clone())
        .or_else(|| {
            failure
                .evidence
                .current_url
                .as_ref()
                .map(|url| webtest_browser::PageSummary {
                    url: url.clone(),
                    title: failure.evidence.title.clone().unwrap_or_default(),
                })
        });
    FailureReport {
        diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        code: step_error_code(&failure.error),
        message: failure.error.to_string(),
        span: Some(source_span(source, range)),
        diff: match &failure.error {
            StepError::Assertion(error) => Some(error.diff.clone()),
            _ => None,
        },
        artifacts: failure
            .artifacts
            .into_iter()
            .map(|artifact| normalized_path(&artifact.path))
            .collect(),
        semantic_details,
        repair_hints: failure.repair_hints,
        page,
        secondary: failure.secondary_failures,
    }
}

fn step_semantic_details(failure: &StepFailure) -> Option<serde_json::Value> {
    match &failure.error {
        StepError::Browser(error) => {
            let locator = browser_error_locator(error);
            let requested = locator.map(ToString::to_string);
            let target = locator.and_then(|locator| {
                let source = locator.to_string();
                failure
                    .inspection
                    .as_ref()?
                    .elements
                    .iter()
                    .find(|element| {
                        element.preferred_locator.source == source
                            || element
                                .alternate_locators
                                .iter()
                                .any(|candidate| candidate.source == source)
                    })
            });
            Some(serde_json::json!({
                "code": RuntimeFailureCode::from(error).short_code(),
                "requested": requested.map(|source| serde_json::json!({"source": source})),
                "states": target.map(|target| &target.states),
                "supported_actions": target.map(|target| &target.supported_actions),
                "available_options": target.map(|target| &target.options),
                "actionability": failure.evidence.actionability,
                "nearby_candidates": failure.evidence.candidates,
            }))
        }
        StepError::Provider(error) => Some(provider_error_semantic_details(error)),
        StepError::Assertion(error) => Some(serde_json::json!({
            "matcher": format!("{:?}", error.matcher).to_ascii_lowercase(),
            "expected": error.expected,
            "actual": error.actual,
        })),
        StepError::Decode(error) => Some(serde_json::json!({
            "path": error.path,
            "expected_type": error.expected.to_string(),
            "actual": error.actual,
            "response_operation": error.response_operation,
        })),
        StepError::Evaluation(error) => Some(serde_json::json!({
            "evaluation_code": error.kind.code().short_code(),
        })),
        StepError::Internal(_) => None,
    }
}

fn provider_error_semantic_details(error: &ProviderError) -> serde_json::Value {
    let runtime_code = RuntimeFailureCode::from(error);
    let mut details = serde_json::json!({
        "provider_error_code": runtime_code.short_code(),
    });
    let Some(object) = details.as_object_mut() else {
        return details;
    };
    match error {
        ProviderError::BridgeHandshake { code, .. }
        | ProviderError::BridgeProtocol { code, .. } => {
            object.insert("bridge_code".into(), code.clone().into());
        }
        ProviderError::BridgeSchemaDrift { expected, live } => {
            object.insert("expected_schema_identity".into(), expected.clone().into());
            object.insert("live_schema_identity".into(), live.clone().into());
        }
        _ => {}
    }
    if !runtime_code.default_reference_queries().is_empty() {
        object.insert(
            "reference_queries".into(),
            serde_json::json!(runtime_code.default_reference_queries()),
        );
    }
    details
}

fn browser_error_locator(error: &BrowserError) -> Option<&Locator> {
    match error {
        BrowserError::LocatorNotFound { locator }
        | BrowserError::LocatorAmbiguous { locator, .. }
        | BrowserError::LocatorInvalid { locator, .. }
        | BrowserError::ElementDetached { locator }
        | BrowserError::LocatorNotVisible { locator }
        | BrowserError::ElementUnstable { locator }
        | BrowserError::ElementDisabled { locator }
        | BrowserError::ElementObscured { locator }
        | BrowserError::ElementNotEditable { locator }
        | BrowserError::OptionNotFound { locator, .. }
        | BrowserError::OptionAmbiguous { locator, .. }
        | BrowserError::ActionTimeout { locator, .. }
        | BrowserError::AssertionFailed { locator, .. } => Some(locator),
        _ => None,
    }
}

fn locator_description(locator: &Locator) -> String {
    locator.to_string()
}

pub(crate) fn event_reports(path: &str, events: &[ExecutionEvent]) -> Vec<EventReport> {
    events
        .iter()
        .map(|event| match event {
            ExecutionEvent::RunStarted { execution_id } => {
                event_report(path, "run_started", Some(execution_id.0), None, None)
            }
            ExecutionEvent::TestStarted {
                execution_id,
                test_id,
                name,
            } => {
                let mut event = event_report(
                    path,
                    "test_started",
                    Some(execution_id.0),
                    Some(test_id.0),
                    None,
                );
                event.name = Some(name.clone());
                event
            }
            ExecutionEvent::StepStarted {
                execution_id,
                test_id,
                step_id,
            } => event_report(
                path,
                "step_started",
                Some(execution_id.0),
                Some(test_id.0),
                Some(step_id.0),
            ),
            ExecutionEvent::StepPassed {
                execution_id,
                test_id,
                step_id,
            } => event_report(
                path,
                "step_passed",
                Some(execution_id.0),
                Some(test_id.0),
                Some(step_id.0),
            ),
            ExecutionEvent::ProviderCallStarted {
                execution_id,
                test_id,
                step_id,
                provider,
                operation,
                transport_kind,
                arguments,
            } => {
                let mut event = event_report(
                    path,
                    "provider_call_started",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                event.name = Some(format!("{provider}.{operation}"));
                event.transport_kind = transport_kind.clone();
                event.arguments = arguments.clone();
                event
            }
            ExecutionEvent::ProviderCallFinished {
                execution_id,
                test_id,
                step_id,
                provider,
                operation,
                elapsed_ms,
                transport_kind,
                result,
            } => {
                let mut event = event_report(
                    path,
                    "provider_call_finished",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                event.name = Some(format!("{provider}.{operation}"));
                event.message = Some(format!("completed in {elapsed_ms}ms"));
                event.transport_kind = transport_kind.clone();
                event.result = result.clone();
                event
            }
            ExecutionEvent::ProviderCallFailed {
                execution_id,
                test_id,
                step_id,
                provider,
                operation,
                code,
                message,
                failure_class,
                elapsed_ms,
                transport_kind,
            } => {
                let mut event = event_report(
                    path,
                    "provider_call_failed",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                event.name = Some(format!("{provider}.{operation}"));
                event.code = Some(code.diagnostic_code().into());
                event.message = Some(format!("{message} (failed after {elapsed_ms}ms)"));
                event.failure_class = Some(*failure_class);
                event.exit_class = Some(ExitClass::from_failure_class(*failure_class));
                event.transport_kind = transport_kind.clone();
                event
            }
            ExecutionEvent::StepFailed {
                execution_id,
                test_id,
                step_id,
                failure_class,
                failure,
                repair_hints,
                page,
            } => {
                let mut event = event_report(
                    path,
                    "step_failed",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                let (code, message) = runtime_failure_report(failure);
                event.code = Some(code);
                event.message = Some(message);
                event.failure_class = Some(*failure_class);
                event.exit_class = Some(ExitClass::from_failure_class(*failure_class));
                if let RuntimeFailure::Assertion { diff, .. } = failure {
                    event.diff = Some(diff.clone());
                }
                event.repair_hints = repair_hints.clone();
                event.page = page.clone();
                event.diagnostic_schema_version = Some(webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION);
                event.repair_hint_schema_version =
                    Some(webtest_feedback::REPAIR_HINT_SCHEMA_VERSION);
                event
            }
            ExecutionEvent::TestTimedOut {
                execution_id,
                test_id,
                active_step,
                timeout_ms,
            } => {
                let mut event = event_report(
                    path,
                    "test_timed_out",
                    Some(execution_id.0),
                    Some(test_id.0),
                    active_step.map(|step| step.0),
                );
                event.outcome = Some("timed_out".into());
                event.code = Some(RuntimeFailureCode::TestTimeout.diagnostic_code().into());
                event.message = Some(format!("test timed out after {timeout_ms}ms"));
                event.failure_class = Some(webtest_feedback::FailureClass::Test);
                event.exit_class = Some(ExitClass::TestFailure);
                event
            }
            ExecutionEvent::CleanupFailed {
                execution_id,
                test_id,
                resource,
                failure_class,
                code,
                message,
            } => {
                let mut event = event_report(
                    path,
                    "cleanup_failed",
                    Some(execution_id.0),
                    test_id.map(|test_id| test_id.0),
                    None,
                );
                event.resource = Some(resource.clone());
                event.code = Some(code.diagnostic_code().into());
                event.message = Some(message.clone());
                event.failure_class = Some(*failure_class);
                event.exit_class = Some(ExitClass::from_failure_class(*failure_class));
                event
            }
            ExecutionEvent::TestFinished {
                execution_id,
                test_id,
                outcome,
                failure_class,
            } => {
                let mut event = event_report(
                    path,
                    "test_finished",
                    Some(execution_id.0),
                    Some(test_id.0),
                    None,
                );
                event.outcome = Some(test_outcome_name(*outcome).into());
                event.failure_class = *failure_class;
                event.exit_class = Some(match outcome {
                    TestOutcomeKind::Passed => ExitClass::Success,
                    TestOutcomeKind::Failed
                    | TestOutcomeKind::TimedOut
                    | TestOutcomeKind::Cancelled => ExitClass::TestFailure,
                    TestOutcomeKind::Aborted => failure_class
                        .map_or(ExitClass::Infrastructure, ExitClass::from_failure_class),
                });
                event
            }
            ExecutionEvent::TestSkipped {
                execution_id,
                test_id,
                name,
                reason,
                failure_class,
            } => {
                let mut event = event_report(
                    path,
                    "test_skipped",
                    Some(execution_id.0),
                    Some(test_id.0),
                    None,
                );
                event.name = Some(name.clone());
                event.outcome = Some("skipped".into());
                event.reason = Some(skip_reason_name(*reason).into());
                event.failure_class = *failure_class;
                event.exit_class = Some(failure_class.map_or(ExitClass::TestFailure, |class| {
                    ExitClass::from_failure_class(class)
                }));
                event
            }
            ExecutionEvent::RunFinished {
                execution_id,
                outcome,
                failure_class,
            } => {
                let mut event =
                    event_report(path, "run_finished", Some(execution_id.0), None, None);
                event.outcome = Some(run_outcome_name(*outcome).into());
                event.failure_class = *failure_class;
                match outcome {
                    RunOutcomeKind::Completed => {}
                    RunOutcomeKind::Cancelled => {
                        event.exit_class = Some(ExitClass::TestFailure);
                    }
                    RunOutcomeKind::Aborted => {
                        event.exit_class = Some(
                            failure_class
                                .map_or(ExitClass::Infrastructure, ExitClass::from_failure_class),
                        );
                    }
                }
                event
            }
        })
        .collect()
}

fn runtime_failure_report(failure: &RuntimeFailure) -> (String, String) {
    let message = match failure {
        RuntimeFailure::TestTimeout { timeout_ms, .. } => {
            format!("test timed out after {timeout_ms}ms")
        }
        RuntimeFailure::Browser(error) => runtime_message(error),
        RuntimeFailure::Provider(error) => error.to_string(),
        RuntimeFailure::Assertion { message, .. }
        | RuntimeFailure::Decode { message }
        | RuntimeFailure::Evaluation { message, .. }
        | RuntimeFailure::Internal { message } => message.clone(),
    };
    (failure.code().diagnostic_code().into(), message)
}

fn event_report(
    path: &str,
    kind: &str,
    execution_id: Option<u64>,
    test_id: Option<u32>,
    step_id: Option<u32>,
) -> EventReport {
    EventReport {
        schema_version: REPORT_SCHEMA_VERSION,
        kind: kind.into(),
        file: path.into(),
        execution_id,
        test_id,
        step_id,
        name: None,
        outcome: None,
        reason: None,
        exit_class: None,
        failure_class: None,
        code: None,
        message: None,
        resource: None,
        transport_kind: None,
        arguments: Default::default(),
        result: None,
        diagnostic_schema_version: None,
        repair_hint_schema_version: None,
        diff: None,
        repair_hints: Vec::new(),
        page: None,
    }
}

const fn test_outcome_name(outcome: TestOutcomeKind) -> &'static str {
    match outcome {
        TestOutcomeKind::Passed => "passed",
        TestOutcomeKind::Failed => "failed",
        TestOutcomeKind::TimedOut => "timed_out",
        TestOutcomeKind::Cancelled => "cancelled",
        TestOutcomeKind::Aborted => "aborted",
    }
}

const fn run_outcome_name(outcome: RunOutcomeKind) -> &'static str {
    match outcome {
        RunOutcomeKind::Completed => "completed",
        RunOutcomeKind::Cancelled => "cancelled",
        RunOutcomeKind::Aborted => "aborted",
    }
}

const fn skip_reason_name(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::RunCancelled => "run_cancelled",
        SkipReason::RunAborted => "run_aborted",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use webtest_feedback::FailureClass;
    use webtest_model::{StepId, TestId};
    use webtest_observation::ExecutionId;

    use super::*;

    #[test]
    fn bridge_failures_point_to_targeted_diagnostics() {
        let details = provider_error_semantic_details(&ProviderError::BridgeProtocol {
            code: "unknown_response_id".into(),
            message: "duplicate response".into(),
        });
        assert_eq!(details["bridge_code"], "unknown_response_id");
        assert_eq!(
            details["reference_queries"],
            serde_json::json!(["app.diagnostics", "runtime.configuration"])
        );
    }

    #[test]
    fn browser_error_codes_remain_stable() {
        assert_eq!(
            RuntimeFailureCode::from(&BrowserError::BrowserDisconnected).diagnostic_code(),
            "runtime.browser_disconnected"
        );
        assert_eq!(
            RuntimeFailureCode::from(&BrowserError::LocatorNotFound {
                locator: Locator::Id("missing".into()),
            })
            .diagnostic_code(),
            "runtime.locator_not_found"
        );
    }

    #[test]
    fn typed_runtime_codes_remove_generic_cli_fallbacks() {
        let application = RunError::Provider(ProviderError::Application {
            code: "dynamic_application_code".into(),
            message: "failed".into(),
            retryable: false,
            data: serde_json::Value::Null,
        });
        assert_eq!(run_error_code(&application), "runtime.app_provider_failure");
        let evaluation = StepError::Evaluation(webtest_runtime::EvaluationFailure {
            kind: webtest_runtime::EvaluationFailureKind::IntegerOverflow,
            message: "overflow".into(),
        });
        assert_eq!(step_error_code(&evaluation), "runtime.integer_overflow");
    }

    #[test]
    fn cleanup_reports_preserve_primary_secondary_and_prior_outcome_structure() {
        let cleanup = RunError::Cleanup(CleanupFailure {
            resource: webtest_observation::CleanupResource::BrowserContext,
            cause: CleanupCause::Browser(BrowserError::BrowserDisconnected),
        });
        let aggregate = RunError::Multiple {
            primary: Box::new(RunError::Internal("body invariant".into())),
            secondary: vec![RunError::Cleanup(CleanupFailure {
                resource: webtest_observation::CleanupResource::BrowserSession,
                cause: CleanupCause::Browser(BrowserError::BrowserDisconnected),
            })],
        };

        let cleanup_report = aborted_test_failure_report(
            &cleanup,
            Some(Box::new(PriorTestOutcome::Cancelled {
                reason: webtest_observation::CancellationReason::Requested,
            })),
            "",
        );
        let cleanup_details = cleanup_report.semantic_details.expect("cleanup details");
        assert_eq!(
            cleanup_report.code,
            "runtime.cleanup_browser_context_failed"
        );
        assert_eq!(cleanup_details["resource"]["kind"], "browser_context");
        assert_eq!(cleanup_details["cause"]["kind"], "browser");
        assert_eq!(cleanup_details["prior_outcome"]["kind"], "cancelled");

        let aggregate_details = run_failure_report(&aggregate)
            .semantic_details
            .expect("aggregate details");
        assert_eq!(aggregate_details["failure_class"], "internal");
        assert_eq!(aggregate_details["primary"]["failure_class"], "internal");
        assert_eq!(
            aggregate_details["secondary"][0]["resource"]["kind"],
            "browser_session"
        );
    }

    #[test]
    fn every_execution_event_variant_has_a_stable_report_kind() {
        let execution_id = ExecutionId(7);
        let test_id = TestId(3);
        let step_id = StepId(5);
        let events = vec![
            ExecutionEvent::RunStarted { execution_id },
            ExecutionEvent::TestStarted {
                execution_id,
                test_id,
                name: "test".into(),
            },
            ExecutionEvent::StepStarted {
                execution_id,
                test_id,
                step_id,
            },
            ExecutionEvent::StepPassed {
                execution_id,
                test_id,
                step_id,
            },
            ExecutionEvent::ProviderCallStarted {
                execution_id,
                test_id,
                step_id,
                provider: "app".into(),
                operation: "echo".into(),
                transport_kind: Some("stdio".into()),
                arguments: BTreeMap::from([("message".into(), "<redacted>".into())]),
            },
            ExecutionEvent::ProviderCallFinished {
                execution_id,
                test_id,
                step_id,
                provider: "app".into(),
                operation: "echo".into(),
                elapsed_ms: 1,
                transport_kind: Some("stdio".into()),
                result: Some("<redacted>".into()),
            },
            ExecutionEvent::ProviderCallFailed {
                execution_id,
                test_id,
                step_id,
                provider: "app".into(),
                operation: "echo".into(),
                code: RuntimeFailureCode::AppProviderFailure,
                message: "failed".into(),
                failure_class: FailureClass::Test,
                elapsed_ms: 2,
                transport_kind: Some("stdio".into()),
            },
            ExecutionEvent::StepFailed {
                execution_id,
                test_id,
                step_id,
                failure_class: FailureClass::Internal,
                failure: RuntimeFailure::Internal {
                    message: "failed".into(),
                },
                repair_hints: Vec::new(),
                page: None,
            },
            ExecutionEvent::TestTimedOut {
                execution_id,
                test_id,
                active_step: Some(step_id),
                timeout_ms: 25,
            },
            ExecutionEvent::CleanupFailed {
                execution_id,
                test_id: Some(test_id),
                resource: webtest_observation::CleanupResource::BrowserContext,
                failure_class: FailureClass::Infrastructure,
                code: RuntimeFailureCode::CleanupBrowserContextFailed,
                message: "failed to clean up browser context".into(),
            },
            ExecutionEvent::TestFinished {
                execution_id,
                test_id,
                outcome: TestOutcomeKind::Aborted,
                failure_class: Some(FailureClass::Internal),
            },
            ExecutionEvent::TestSkipped {
                execution_id,
                test_id: TestId(4),
                name: "skipped".into(),
                reason: SkipReason::RunCancelled,
                failure_class: None,
            },
            ExecutionEvent::RunFinished {
                execution_id,
                outcome: RunOutcomeKind::Aborted,
                failure_class: Some(FailureClass::Internal),
            },
        ];
        let reports = event_reports("tests/a.webtest", &events);
        assert_eq!(
            reports
                .iter()
                .map(|report| report.kind.as_str())
                .collect::<Vec<_>>(),
            [
                "run_started",
                "test_started",
                "step_started",
                "step_passed",
                "provider_call_started",
                "provider_call_finished",
                "provider_call_failed",
                "step_failed",
                "test_timed_out",
                "cleanup_failed",
                "test_finished",
                "test_skipped",
                "run_finished",
            ]
        );
        assert_eq!(reports[4].arguments["message"], "<redacted>");
        assert_eq!(reports[8].exit_class, Some(ExitClass::TestFailure));
        assert_eq!(reports[8].code.as_deref(), Some("runtime.test_timeout"));
        assert_eq!(reports[9].exit_class, Some(ExitClass::Infrastructure));
        assert_eq!(
            reports[9].code.as_deref(),
            Some("runtime.cleanup_browser_context_failed")
        );
        assert_eq!(reports[10].exit_class, Some(ExitClass::Internal));
        assert_eq!(reports[10].failure_class, Some(FailureClass::Internal));
        assert_eq!(reports[10].outcome.as_deref(), Some("aborted"));
        assert_eq!(reports[11].outcome.as_deref(), Some("skipped"));
    }
}
