use super::*;
use webtest_trace::{CapturePolicy, SourceInput, TraceInput, TraceLimits, TraceMetadata};

fn roundtrip(
    source: &str,
    plan: &TestPlan,
    result: &webtest_runtime::RunResult,
    include_source: bool,
) -> Vec<u8> {
    let bytes = webtest_trace::write(
        TraceInput {
            metadata: TraceMetadata {
                compiler_version: env!("CARGO_PKG_VERSION").into(),
                project_identity: [1; 32],
                configuration_fingerprint: [2; 32],
                browser_version: None,
                provider_versions: BTreeMap::new(),
                capture: CapturePolicy {
                    sources: include_source,
                    attachments: false,
                },
            },
            records: &result.journal,
            sources: &[SourceInput {
                execution_id: result.execution_id.unwrap(),
                file_id: plan.file,
                revision: plan.source_revision,
                project_path: "tests/structured.webtest",
                text: include_source.then_some(source),
            }],
            attachments: &[],
        },
        TraceLimits::default(),
    )
    .unwrap();
    let trace = webtest_trace::read(&bytes, TraceLimits::default()).unwrap();
    assert_eq!(trace.records(), result.journal);
    assert_eq!(
        trace.source(result.execution_id.unwrap(), plan.file),
        include_source.then_some(source)
    );
    bytes
}

fn budgeted_roundtrip(source: &str, plan: &TestPlan, result: &webtest_runtime::RunResult) {
    let bytes = webtest_trace::write_budgeted(
        TraceInput {
            metadata: TraceMetadata {
                compiler_version: env!("CARGO_PKG_VERSION").into(),
                project_identity: [1; 32],
                configuration_fingerprint: [2; 32],
                browser_version: None,
                provider_versions: BTreeMap::new(),
                capture: CapturePolicy {
                    sources: true,
                    attachments: false,
                },
            },
            records: &result.journal,
            sources: &[SourceInput {
                execution_id: result.execution_id.unwrap(),
                file_id: plan.file,
                revision: plan.source_revision,
                project_path: "tests/structured.webtest",
                text: Some(source),
            }],
            attachments: &[],
        },
        TraceLimits {
            max_entries: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let trace = webtest_trace::read(&bytes, TraceLimits::default()).unwrap();
    assert_eq!(trace.records(), result.journal);
    assert_eq!(trace.source(result.execution_id.unwrap(), plan.file), None);
    let report = trace.manifest().capture_report.as_ref().unwrap();
    assert_eq!(report.events.len(), 1);
    let webtest_trace::CaptureEvent::Truncated { file } = &report.events[0];
    assert_eq!(&file.blake3, plan.source_revision.as_bytes());
    assert_eq!(file.byte_length, source.len() as u64);
}

#[tokio::test(start_paused = true)]
async fn trace_roundtrip_preserves_parallel_race_retry_resource_and_failure_facts() {
    let source = r#"test "trace" { timeout 1s { parallel {
        retry 2 { browser { expect text("é").visible } expect 1 == 2 }
        race { server { expect 1 == 2 } server { expect 1 == 1 } }
    } } }"#;
    let plan = compile_source(source);
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Failed { .. }
    ));
    assert!(
        result
            .journal
            .iter()
            .any(|r| matches!(r.event, ExecutionEvent::Attempt { .. }))
    );
    assert!(
        result
            .journal
            .iter()
            .any(|r| matches!(r.event, ExecutionEvent::Resource { .. }))
    );
    assert!(
        result
            .journal
            .iter()
            .any(|r| matches!(r.event, ExecutionEvent::StepFailed { .. }))
    );
    roundtrip(source, &plan, &result, true);
    budgeted_roundtrip(source, &plan, &result);
}

#[tokio::test(start_paused = true)]
async fn trace_accepts_every_explicit_authoritative_overflow_boundary() {
    let source = r#"test "budget" { browser { evaluate "ok" } }"#;
    let plan = compile_source(source);
    let runner = |capacity| {
        Runner::new(Arc::default()).with_options(RunnerOptions {
            journal_max_events: std::num::NonZeroUsize::new(capacity).unwrap(),
            ..Default::default()
        })
    };
    let result = runner(1000)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    for capacity in 1..=result.journal.len() {
        let result = runner(capacity)
            .run(&plan, &LifecycleHost(Arc::default()))
            .await;
        roundtrip(source, &plan, &result, false);
        budgeted_roundtrip(source, &plan, &result);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn trace_retains_process_output_redaction_without_capturing_source_secrets() {
    let source = r#"test "output" { server {
        process.run("/bin/sh", args: ["-c", "printf '%s' \"$TOKEN\""], env: { TOKEN: "trace-private-token" })
    } }"#;
    let plan = compile_source(source);
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    let bytes = roundtrip(source, &plan, &result, false);
    assert!(
        !bytes
            .windows(b"trace-private-token".len())
            .any(|w| w == b"trace-private-token")
    );
    assert!(result.journal.iter().any(|record| matches!(&record.event,
        ExecutionEvent::OutputCaptured { output, .. } if output.text == "[redacted]" && output.redacted)));
}
