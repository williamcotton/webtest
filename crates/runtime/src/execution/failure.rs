use webtest_browser::{
    BrowserError, EvidenceRequest, Page, PageEvidence, PageInspection, RepairHint, RepairHintKind,
    locator_repair_hints,
};
use webtest_feedback::ByteRange;
use webtest_hir::TestId;
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeFailure, RuntimeObservation,
    RuntimeObservationKind,
};
use webtest_plan::{PlannedStep, ServerProviderCall, TestOperation, TestPlan};
use webtest_provider::ProviderRegistry;

use crate::{
    Artifact, RunError, RunnerOptions, StepError, StepFailure, artifacts::write_artifacts,
    evaluation::display_value,
};

use super::browser::step_browser_locator;

pub(super) struct FailureInput<'a> {
    pub(super) plan: &'a TestPlan,
    pub(super) test_id: TestId,
    pub(super) step: &'a PlannedStep,
    pub(super) execution_id: ExecutionId,
    pub(super) error: StepError,
    pub(super) page: &'a mut Option<Box<dyn Page>>,
    pub(super) options: &'a RunnerOptions,
    pub(super) providers: &'a ProviderRegistry,
    pub(super) observations: &'a ObservationStore,
    pub(super) events: &'a mut Vec<ExecutionEvent>,
    pub(super) elapsed_ms: u64,
    pub(super) secrets: &'a [String],
}

pub(super) async fn process_failure(input: FailureInput<'_>) -> Result<StepFailure, RunError> {
    let FailureInput {
        plan,
        test_id,
        step,
        execution_id,
        error,
        page,
        options,
        providers,
        observations,
        events,
        elapsed_ms,
        secrets,
    } = input;
    let evidence = if matches!(error, StepError::Browser(_)) && !error.is_infrastructure() {
        if let Some(page) = page.as_deref_mut() {
            page.capture_evidence(&EvidenceRequest {
                locator: step_browser_locator(step),
                include_screenshot: options.evidence.screenshot_on_failure,
                include_dom: options.evidence.dom_snapshot_on_failure,
                max_dom_bytes: options.evidence.max_dom_bytes,
                redactions: secrets.to_vec(),
                redacted_query_parameters: options.inspection.redacted_query_parameters.clone(),
            })
            .await
        } else {
            PageEvidence::default()
        }
    } else {
        PageEvidence::default()
    };
    let (inspection, secondary_failures) =
        if matches!(error, StepError::Browser(_)) && !error.is_infrastructure() {
            if let Some(page) = page.as_deref_mut() {
                let mut inspection_options = options.inspection.clone();
                inspection_options.redacted_values.extend(secrets.to_vec());
                match page.inspect(&inspection_options).await {
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
    finish_failure(FinishFailureInput {
        plan,
        test_id,
        step,
        execution_id,
        error,
        evidence,
        inspection,
        secondary_failures,
        options,
        providers,
        observations,
        events,
        elapsed_ms,
    })
}

struct FinishFailureInput<'a> {
    plan: &'a TestPlan,
    test_id: TestId,
    step: &'a PlannedStep,
    execution_id: ExecutionId,
    error: StepError,
    evidence: PageEvidence,
    inspection: Option<PageInspection>,
    secondary_failures: Vec<String>,
    options: &'a RunnerOptions,
    providers: &'a ProviderRegistry,
    observations: &'a ObservationStore,
    events: &'a mut Vec<ExecutionEvent>,
    elapsed_ms: u64,
}

fn finish_failure(input: FinishFailureInput<'_>) -> Result<StepFailure, RunError> {
    let FinishFailureInput {
        plan,
        test_id,
        step,
        execution_id,
        error,
        mut evidence,
        inspection,
        secondary_failures,
        options,
        providers,
        observations,
        events,
        elapsed_ms,
    } = input;
    let mut repair_hints = inspection
        .as_ref()
        .map(|inspection| repair_hints_for_error(&error, inspection))
        .unwrap_or_default();
    for hint in &mut repair_hints {
        hint.source_range = Some(ByteRange {
            start: step.origin.range.start().into(),
            end: step.origin.range.end().into(),
        });
    }
    if !options.evidence.screenshot_on_failure {
        evidence.screenshot_png = None;
    }
    let artifacts =
        if options.evidence.screenshot_on_failure || options.evidence.dom_snapshot_on_failure {
            write_artifacts(
                &options.evidence.artifact_directory,
                execution_id,
                test_id,
                step.id,
                &mut evidence,
            )
        } else {
            Vec::new()
        };
    if let TestOperation::ServerProviderCall(call) = &step.operation {
        events.push(provider_failure_event(
            call,
            execution_id,
            test_id,
            step,
            &error,
            elapsed_ms,
            providers,
        ));
    }
    if !error.is_infrastructure() {
        record_observation(
            observations,
            plan,
            test_id,
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
        test_id,
        step_id: step.id,
        failure: runtime_failure(&error),
        repair_hints: repair_hints.clone(),
        page: inspection
            .as_ref()
            .map(|inspection| inspection.page.clone()),
    });
    if error.is_infrastructure() {
        return Err(match error {
            StepError::Browser(error) => RunError::Browser(error),
            StepError::Provider(error) => RunError::Provider(error),
            other => RunError::Internal(other.to_string()),
        });
    }
    Ok(StepFailure {
        step: step.clone(),
        error,
        evidence,
        artifacts,
        inspection,
        repair_hints,
        secondary_failures,
    })
}

fn provider_failure_event(
    call: &ServerProviderCall,
    execution_id: ExecutionId,
    test_id: TestId,
    step: &PlannedStep,
    error: &StepError,
    elapsed_ms: u64,
    providers: &ProviderRegistry,
) -> ExecutionEvent {
    ExecutionEvent::ProviderCallFailed {
        execution_id,
        test_id,
        step_id: step.id,
        provider: call.provider.clone(),
        operation: call.operation.clone(),
        code: error.code().into(),
        message: error.to_string(),
        elapsed_ms,
        transport_kind: providers.transport_kind(&call.provider),
    }
}

#[allow(clippy::too_many_arguments)]
fn record_observation(
    observations: &ObservationStore,
    plan: &TestPlan,
    test_id: TestId,
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
    observations.record(RuntimeObservation {
        execution_id,
        file: plan.file,
        source_revision: plan.source_revision,
        test_id,
        step_id: step.id,
        range: step.origin.range,
        kind,
    });
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

pub(crate) fn repair_hints_for_error(
    error: &StepError,
    inspection: &PageInspection,
) -> Vec<RepairHint> {
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
