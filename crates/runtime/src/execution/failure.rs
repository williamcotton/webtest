use webtest_browser::{
    BrowserError, EvidenceRequest, Page, PageEvidence, PageInspection, RepairHint, RepairHintKind,
    locator_repair_hints,
};
use webtest_feedback::ByteRange;
use webtest_model::TestId;
use webtest_observation::{
    ExecutionEvent, ExecutionId, ObservationStore, RuntimeFailure, RuntimeObservation,
    RuntimeObservationKind,
};
use webtest_plan::{PlannedStep, ServerProviderCall, TestOperation, TestPlan};
use webtest_provider::ProviderRegistry;

use crate::{
    Artifact, FailureClass, RunError, RunEventSink, RunnerOptions, StepError, StepFailure,
    artifacts::write_artifacts, evaluation::display_value, events::emit_event,
};

use super::browser::step_browser_locator;

pub(super) struct PrepareFailureInput<'a> {
    pub(super) step: &'a PlannedStep,
    pub(super) error: StepError,
    pub(super) page: &'a mut Option<Box<dyn Page>>,
    pub(super) options: &'a RunnerOptions,
    pub(super) elapsed_ms: u64,
    pub(super) secrets: &'a [String],
}

pub(super) struct PendingFailure {
    step: PlannedStep,
    error: StepError,
    evidence: PageEvidence,
    inspection: Option<PageInspection>,
    secondary_failures: Vec<String>,
    elapsed_ms: u64,
}

pub(super) struct FailureInput<'a> {
    pub(super) plan: &'a TestPlan,
    pub(super) test_id: TestId,
    pub(super) execution_id: ExecutionId,
    pub(super) pending: PendingFailure,
    pub(super) artifact_deadline: tokio::time::Instant,
    pub(super) options: &'a RunnerOptions,
    pub(super) providers: &'a ProviderRegistry,
    pub(super) observations: &'a ObservationStore,
    pub(super) events: &'a mut Vec<ExecutionEvent>,
    pub(super) event_sink: Option<&'a dyn RunEventSink>,
}

pub(super) async fn prepare_failure(input: PrepareFailureInput<'_>) -> PendingFailure {
    let PrepareFailureInput {
        step,
        error,
        page,
        options,
        elapsed_ms,
        secrets,
    } = input;
    let eligible_browser_failure =
        matches!(error, StepError::Browser(_)) && error.failure_class() == FailureClass::Test;
    let evidence = if eligible_browser_failure {
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
    let (inspection, secondary_failures) = if eligible_browser_failure {
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
    PendingFailure {
        step: step.clone(),
        error,
        evidence,
        inspection,
        secondary_failures,
        elapsed_ms,
    }
}

pub(super) async fn process_failure(input: FailureInput<'_>) -> Result<StepFailure, RunError> {
    let FailureInput {
        plan,
        test_id,
        execution_id,
        pending,
        artifact_deadline,
        options,
        providers,
        observations,
        events,
        event_sink,
    } = input;
    let PendingFailure {
        step,
        error,
        evidence,
        inspection,
        secondary_failures,
        elapsed_ms,
    } = pending;
    finish_failure(FinishFailureInput {
        plan,
        test_id,
        step: &step,
        execution_id,
        error,
        evidence,
        inspection,
        secondary_failures,
        options,
        providers,
        observations,
        events,
        event_sink,
        elapsed_ms,
        artifact_deadline,
    })
    .await
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
    event_sink: Option<&'a dyn RunEventSink>,
    elapsed_ms: u64,
    artifact_deadline: tokio::time::Instant,
}

async fn finish_failure(input: FinishFailureInput<'_>) -> Result<StepFailure, RunError> {
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
        event_sink,
        elapsed_ms,
        artifact_deadline,
    } = input;
    let failure_class = error.failure_class();
    let eligible_browser_failure =
        matches!(error, StepError::Browser(_)) && failure_class == FailureClass::Test;
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
    let artifacts = if eligible_browser_failure
        && (options.evidence.screenshot_on_failure || options.evidence.dom_snapshot_on_failure)
    {
        write_artifacts(
            &options.evidence.artifact_directory,
            execution_id,
            test_id,
            step.id,
            artifact_deadline,
            &mut evidence,
        )
        .await
    } else {
        Vec::new()
    };
    if let TestOperation::ServerProviderCall(call) = &step.operation {
        emit_event(
            events,
            event_sink,
            provider_failure_event(
                call,
                execution_id,
                test_id,
                step,
                &error,
                elapsed_ms,
                providers,
            ),
        );
    }
    if failure_class == FailureClass::Test {
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
    emit_event(
        events,
        event_sink,
        ExecutionEvent::StepFailed {
            execution_id,
            test_id,
            step_id: step.id,
            failure_class,
            failure: runtime_failure(&error),
            repair_hints: repair_hints.clone(),
            page: inspection
                .as_ref()
                .map(|inspection| inspection.page.clone()),
        },
    );
    match failure_class {
        FailureClass::Test => Ok(StepFailure {
            step: step.clone(),
            error,
            evidence,
            artifacts,
            inspection,
            repair_hints,
            secondary_failures,
        }),
        FailureClass::Infrastructure => match error {
            StepError::Browser(error) => Err(RunError::Browser(error)),
            StepError::Provider(error) => Err(RunError::Provider(error)),
            StepError::Assertion(_)
            | StepError::Decode(_)
            | StepError::Evaluation(_)
            | StepError::Internal(_) => {
                unreachable!("only browser and provider errors classify as infrastructure")
            }
        },
        FailureClass::Internal => match error {
            StepError::Internal(message) => Err(RunError::Internal(message)),
            StepError::Browser(_)
            | StepError::Provider(_)
            | StepError::Assertion(_)
            | StepError::Decode(_)
            | StepError::Evaluation(_) => {
                unreachable!("only internal step errors classify as internal")
            }
        },
    }
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
        code: error.code(),
        message: error.to_string(),
        failure_class: error.failure_class(),
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
            code: error.into(),
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
            code: webtest_observation::RuntimeFailureCode::JsonDecodeFailed,
            message: error.to_string(),
            path: Some(error.path.clone()),
            expected: Some(error.expected.to_string()),
            actual: Some(error.actual.clone()),
            diff: None,
        },
        StepError::Assertion(error) => RuntimeObservationKind::ValueFailure {
            code: webtest_observation::RuntimeFailureCode::AssertionFailed,
            message: error.message.clone(),
            path: None,
            expected: error.expected.as_ref().map(display_value),
            actual: Some(display_value(&error.actual)),
            diff: Some(error.diff.clone()),
        },
        StepError::Provider(error) => RuntimeObservationKind::ValueFailure {
            code: error.into(),
            message: error.to_string(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        },
        StepError::Evaluation(error) => RuntimeObservationKind::ValueFailure {
            code: error.kind.code(),
            message: error.message.clone(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        },
        StepError::Internal(_) => return,
    };
    observations.record(RuntimeObservation {
        execution_id,
        file: plan.file,
        source_revision: plan.source_revision,
        test_id,
        step_id: Some(step.id),
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
            code: error.kind.code(),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use webtest_browser::Locator;
    use webtest_model::StepId;
    use webtest_plan::{BrowserOperation, PlannedStep, TestOperation};
    use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

    use super::*;

    #[tokio::test]
    async fn exhausted_artifact_budget_preserves_the_primary_step_failure() {
        let root = tempfile::tempdir().expect("temporary root");
        let file = FileId::new(91);
        let step = PlannedStep {
            id: StepId(7),
            origin: SyntaxOrigin::new(file, TextRange::new(TextSize::new(4), TextSize::new(12))),
            operation: TestOperation::Browser(BrowserOperation::Click {
                locator: webtest_plan::Locator::Id("missing".into()),
            }),
        };
        let plan = TestPlan {
            file,
            source_revision: SourceRevision::of("failure"),
            required_host_capabilities: Vec::new(),
            tests: Vec::new(),
        };
        let pending = PendingFailure {
            step,
            error: StepError::Browser(BrowserError::LocatorNotFound {
                locator: Locator::Id("missing".into()),
            }),
            evidence: PageEvidence {
                screenshot_png: Some(vec![1, 2, 3]),
                dom_snapshot: Some("<main>later</main>".into()),
                ..PageEvidence::default()
            },
            inspection: None,
            secondary_failures: Vec::new(),
            elapsed_ms: 10,
        };
        let options = RunnerOptions {
            evidence: crate::EvidenceOptions {
                screenshot_on_failure: true,
                dom_snapshot_on_failure: true,
                artifact_directory: root.path().join("artifacts"),
                ..crate::EvidenceOptions::default()
            },
            ..RunnerOptions::default()
        };
        let providers = ProviderRegistry::default();
        let observations = Arc::new(ObservationStore::default());
        let execution_id = ExecutionId::next();
        let mut events = Vec::new();

        let failure = process_failure(FailureInput {
            plan: &plan,
            test_id: TestId(3),
            execution_id,
            pending,
            artifact_deadline: tokio::time::Instant::now(),
            options: &options,
            providers: &providers,
            observations: &observations,
            events: &mut events,
            event_sink: None,
        })
        .await
        .expect("ordinary failure remains primary");

        assert!(matches!(
            failure.error,
            StepError::Browser(BrowserError::LocatorNotFound { .. })
        ));
        assert!(failure.artifacts.is_empty());
        assert_eq!(
            failure.evidence.capture_failures,
            ["artifact persistence exceeded the remaining test budget"]
        );
        assert!(!options.evidence.artifact_directory.exists());
        assert!(events.iter().any(|event| matches!(
            event,
            ExecutionEvent::StepFailed {
                failure: RuntimeFailure::Browser(BrowserError::LocatorNotFound { .. }),
                ..
            }
        )));
        let stored = observations.observations_for(plan.file, plan.source_revision);
        assert_eq!(stored.len(), 1);
        assert!(matches!(
            &stored[0].kind,
            RuntimeObservationKind::BrowserFailure { artifacts, .. } if artifacts.is_empty()
        ));
    }
}
