use std::{collections::BTreeMap, error::Error, path::PathBuf, sync::Arc, time::Duration};

use webtest_browser::{BrowserError, PageEvidence};
use webtest_model::{StepId, TestId, Type, Value};
use webtest_observation::{
    CancellationReason, ExecutionEvent, ExecutionId, ObservationStore, RunOutcomeKind, SkipReason,
    ValueDiff,
};
use webtest_plan::{PlannedStep, ValueMatcher};
use webtest_runtime::{
    Artifact, ArtifactKind, AssertionFailure, CleanupCause, CleanupFailure, CleanupResource,
    DecodeFailure, EvaluationFailure, EvaluationFailureKind, EvidenceOptions, RunControl, RunError,
    RunEventSink, RunOutcome, RunResult, Runner, RunnerOptions, StepError, StepFailure,
    TestOutcome, TestResult, resolve_browser_url,
};
use webtest_text::{FileId, SyntaxOrigin, TextRange};

fn assert_error<T: Error>() {}

fn accepts_control(_: Option<&dyn RunControl>) {}

fn accepts_event_sink(_: Option<&dyn RunEventSink>) {}

fn test_result(outcome: TestOutcome) -> TestResult {
    TestResult {
        test_id: webtest_model::TestId(0),
        name: "test".into(),
        outcome,
        duration: Duration::ZERO,
        bindings: BTreeMap::new(),
    }
}

fn failed_outcome() -> TestOutcome {
    TestOutcome::Failed(Box::new(StepFailure {
        step: PlannedStep {
            id: StepId(0),
            origin: SyntaxOrigin::new(FileId::new(7), TextRange::default()),
            operation: webtest_plan::TestOperation::EvaluatePure(
                webtest_plan::EvaluatePureOperation {
                    expression: webtest_plan::PlanExpr::Literal(Value::Null),
                    result_binding: None,
                    result_name: None,
                    result_type: Type::Null,
                },
            ),
        },
        error: StepError::Internal("failed".into()),
        evidence: PageEvidence::default(),
        artifacts: Vec::new(),
        inspection: None,
        repair_hints: Vec::new(),
        secondary_failures: Vec::new(),
    }))
}

#[test]
fn root_public_api_remains_importable_and_constructible() {
    let file = FileId::new(7);
    let step = PlannedStep {
        id: StepId(3),
        origin: SyntaxOrigin::new(file, TextRange::default()),
        operation: webtest_plan::TestOperation::EvaluatePure(webtest_plan::EvaluatePureOperation {
            expression: webtest_plan::PlanExpr::Literal(Value::Bool(true)),
            result_binding: None,
            result_name: None,
            result_type: Type::Bool,
        }),
    };
    let artifact = Artifact {
        kind: ArtifactKind::Evidence,
        path: PathBuf::from("evidence.txt"),
    };
    let assertion = AssertionFailure {
        matcher: ValueMatcher::Equal,
        expected: Some(Value::Int(1)),
        actual: Value::Int(2),
        message: "failed".into(),
        diff: ValueDiff::Scalar {
            expected: Some("1".into()),
            actual: "2".into(),
        },
    };
    let decode = DecodeFailure {
        path: "$.id".into(),
        expected: Type::Int,
        actual: "string".into(),
        response_operation: Some("app.load".into()),
    };
    let evaluation = EvaluationFailure {
        kind: EvaluationFailureKind::DivisionByZero,
        message: "division by zero".into(),
    };
    let step_failure = StepFailure {
        step,
        error: StepError::Assertion(Box::new(assertion)),
        evidence: PageEvidence::default(),
        artifacts: vec![artifact],
        inspection: None,
        repair_hints: Vec::new(),
        secondary_failures: Vec::new(),
    };

    assert_error::<StepError>();
    assert_error::<RunError>();
    accepts_control(None);
    accepts_event_sink(None);
    assert_eq!(
        StepError::Decode(decode).code(),
        webtest_observation::RuntimeFailureCode::JsonDecodeFailed
    );
    assert_eq!(
        StepError::Evaluation(evaluation).code(),
        webtest_observation::RuntimeFailureCode::DivisionByZero
    );
    assert_eq!(
        step_failure.error.code(),
        webtest_observation::RuntimeFailureCode::AssertionFailed
    );
    let cleanup = CleanupFailure {
        resource: CleanupResource::BrowserSession,
        cause: CleanupCause::Browser(BrowserError::BrowserDisconnected),
    };
    assert_eq!(
        cleanup.code(),
        webtest_observation::RuntimeFailureCode::CleanupBrowserSessionFailed
    );
    assert!(matches!(
        resolve_browser_url(None, "/relative"),
        Err(BrowserError::NavigationFailed { .. })
    ));

    let store = Arc::new(ObservationStore::default());
    let _runner = Runner::new(store)
        .with_options(RunnerOptions::default())
        .with_provider_registry(webtest_provider::ProviderRegistry::default());
}

#[test]
fn defaults_and_result_counts_are_exact() {
    let evidence = EvidenceOptions::default();
    assert!(!evidence.screenshot_on_failure);
    assert!(!evidence.dom_snapshot_on_failure);
    assert_eq!(evidence.max_dom_bytes, 1_048_576);
    assert_eq!(
        evidence.artifact_directory,
        PathBuf::from(".webtest/artifacts")
    );

    let options = RunnerOptions::default();
    assert_eq!(options.base_url, None);
    assert_eq!(options.action_timeout, Duration::from_secs(5));
    assert_eq!(options.assertion_timeout, Duration::from_secs(5));
    assert_eq!(options.navigation_timeout, Duration::from_secs(30));
    assert_eq!(options.provider_call_timeout, Duration::from_secs(60));
    assert_eq!(options.test_timeout, Duration::from_secs(60));
    assert_eq!(options.project_root, PathBuf::from("."));
    assert_eq!(
        options.redacted_json_fields,
        [
            "password",
            "token",
            "secret",
            "authorization",
            "cookie",
            "set-cookie"
        ]
    );

    let execution_id = ExecutionId::next();
    let result = RunResult {
        execution_id,
        outcome: RunOutcome::Completed,
        tests: vec![
            test_result(TestOutcome::Passed),
            test_result(failed_outcome()),
            test_result(TestOutcome::TimedOut {
                timeout: Duration::from_secs(1),
                active_step: Some(webtest_model::StepId(0)),
            }),
            test_result(TestOutcome::Cancelled {
                reason: CancellationReason::Requested,
            }),
            test_result(TestOutcome::Skipped {
                reason: SkipReason::RunCancelled,
                failure_class: None,
            }),
            test_result(TestOutcome::Aborted {
                failure: RunError::Internal("aborted".into()),
                prior_outcome: None,
            }),
        ],
        events: vec![
            ExecutionEvent::RunStarted { execution_id },
            ExecutionEvent::RunFinished {
                execution_id,
                outcome: RunOutcomeKind::Completed,
                failure_class: None,
            },
        ],
        duration: Duration::ZERO,
    };
    assert_eq!(
        (
            result.passed(),
            result.failed(),
            result.timed_out(),
            result.cancelled(),
            result.skipped(),
            result.aborted(),
        ),
        (1, 1, 1, 1, 1, 1)
    );

    let _ = TestId(0);
}
