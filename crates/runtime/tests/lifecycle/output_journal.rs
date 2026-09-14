use super::*;
use webtest_model::ProcessResultValue;
use webtest_observation::{CapturedOutput, OutputStream, RecordedEvent};

fn outputs(result: &webtest_runtime::RunResult) -> Vec<(&RecordedEvent, &CapturedOutput)> {
    result
        .journal
        .iter()
        .filter_map(|record| match &record.event {
            ExecutionEvent::OutputCaptured { output, .. } => Some((record, output)),
            _ => None,
        })
        .collect()
}

fn assert_output_context(result: &webtest_runtime::RunResult, plan: &TestPlan) {
    journal::assert_serialized_journal(result);
    for (record, output) in outputs(result) {
        let step_id = record.event.step_id().unwrap();
        let step = plan.tests[0]
            .steps()
            .into_iter()
            .find(|step| step.id == step_id)
            .unwrap();
        assert_eq!(record.metadata.origin, Some(step.origin));
        assert_eq!(record.metadata.source_revision, Some(plan.source_revision));
        assert!(
            record
                .metadata
                .execution_context
                .operation_execution_id
                .is_some()
        );
        assert!(output.text.len() <= 65_536);
        let terminal = result.journal.iter().find(|candidate| matches!(&candidate.event,
            ExecutionEvent::ProviderCallFinished { step_id: id, .. }
            if *id == step_id && candidate.metadata.execution_context.operation_execution_id == record.metadata.execution_context.operation_execution_id
        )).unwrap();
        assert!(record.identity.event_sequence < terminal.identity.event_sequence);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn completed_native_output_is_redacted_and_retained_for_bound_and_discarded_nonzero_results()
{
    let plan = compile_source(
        r#"test "captured process" { server {
        process.run("/bin/sh", args: ["-c", "printf '%s' \"$TOKEN\"; printf diagnostic >&2; exit 7"], env: { TOKEN: "very-private" })
        let result = process.run("/bin/sh", args: ["-c", "printf '%s' \"$TOKEN\"; printf diagnostic >&2; exit 7"], env: { TOKEN: "very-private" })
        expect result.exit_code == 7
    } }"#,
    );
    let state = Arc::new(LifecycleState::default());
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert!(
        matches!(result.tests[0].outcome, TestOutcome::Passed),
        "{:?}",
        result.outcome
    );
    assert_output_context(&result, &plan);
    assert!(
        state.log().is_empty(),
        "process-only work does not start Chrome"
    );
    let captured = outputs(&result);
    assert_eq!(captured.len(), 4);
    for pair in captured.chunks_exact(2) {
        assert_eq!(pair[0].1.stream, OutputStream::Stdout);
        assert_eq!(pair[0].1.text, "[redacted]");
        assert_eq!(pair[0].1.captured_byte_length, 12);
        assert!(pair[0].1.redacted);
        assert_eq!(pair[1].1.stream, OutputStream::Stderr);
        assert_eq!(pair[1].1.text, "diagnostic");
    }
    assert!(
        !serde_json::to_string(&result.journal)
            .unwrap()
            .contains("very-private")
    );
}

struct OutputProvider(AtomicUsize);

#[async_trait]
impl ServerProvider for OutputProvider {
    fn schema(&self) -> ProviderSchema {
        ProviderSchema {
            name: ProviderName("output".into()),
            operations: ["fast", "slow"]
                .into_iter()
                .map(|name| {
                    (
                        name.into(),
                        OperationSchema {
                            name: OperationName(name.into()),
                            parameters: vec![],
                            result: Type::ProcessResult,
                            capability: Capability::Server,
                            documentation: String::new(),
                            retry_safe: true,
                        },
                    )
                })
                .collect(),
            schema_identity: None,
        }
    }
    async fn call(
        &self,
        call: ProviderCall,
        _: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        let slow = call.operation.0 == "slow";
        let ordinal = if slow {
            self.0.fetch_add(1, Ordering::SeqCst)
        } else {
            0
        };
        tokio::time::sleep(Duration::from_millis(if slow { 20 } else { 5 })).await;
        let stdout = format!("{}-{ordinal}", call.operation.0);
        Ok(ProviderResult {
            value: Value::ProcessResult(ProcessResultValue {
                exit_code: ordinal as i64,
                stdout_bytes: stdout.as_bytes().to_vec(),
                stdout,
                stderr: String::new(),
                stderr_bytes: vec![],
            }),
        })
    }
}

fn fixture(source: &str) -> (TestPlan, ProviderRegistry) {
    let mut providers = ProviderRegistry::default();
    providers.register(Arc::new(OutputProvider(AtomicUsize::new(0))));
    let mut db = webtest_analysis::AnalysisDatabase::with_provider_registry(providers.clone());
    let file = db.open_file("output.webtest", source);
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file)
    );
    (db.test_plan(file).unwrap().as_ref().clone(), providers)
}

#[tokio::test(start_paused = true)]
async fn sibling_and_retry_output_preserves_occurrences_even_for_recovered_attempts() {
    let (plan, providers) = fixture(
        r#"test "output occurrences" { parallel {
        retry 2 backoff 1ms max 1ms { server { let result = output.slow() expect result.exit_code == 1 } }
        server { output.fast() }
    } }"#,
    );
    let store = Arc::new(ObservationStore::default());
    let result = Runner::new(store.clone())
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(
        matches!(result.tests[0].outcome, TestOutcome::Passed),
        "{:?}",
        result.outcome
    );
    assert_output_context(&result, &plan);
    assert!(
        store
            .observations_for(plan.file, plan.source_revision)
            .is_empty()
    );
    let captured = outputs(&result);
    assert_eq!(captured.len(), 6);
    assert_eq!(
        captured[0].1.text, "fast-0",
        "observed order differs from source order"
    );
    let slow: Vec<_> = captured
        .into_iter()
        .filter(|(_, output)| output.text.starts_with("slow"))
        .collect();
    assert_eq!(slow.len(), 2);
    assert_eq!(slow[0].0.event.step_id(), slow[1].0.event.step_id());
    assert_ne!(
        slow[0].0.metadata.execution_context.operation_execution_id,
        slow[1].0.metadata.execution_context.operation_execution_id
    );
    let first = slow[0].0.metadata.execution_context.attempt_id;
    assert!(first.is_some());
    assert_ne!(first, slow[1].0.metadata.execution_context.attempt_id);
}

#[tokio::test(start_paused = true)]
async fn output_records_share_the_authoritative_budget_and_retained_terminal_gap() {
    let source = r#"test "bounded output" { server { output.fast() } }"#;
    let (plan, providers) = fixture(source);
    let baseline = Runner::new(Arc::default())
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    let first = outputs(&baseline)[0].0.identity.event_sequence.0;
    let (plan, providers) = fixture(source);
    let capacity = first as usize + 1;
    let result = Runner::new(Arc::default())
        .with_provider_registry(providers)
        .with_options(RunnerOptions {
            journal_max_events: std::num::NonZeroUsize::new(capacity).unwrap(),
            ..Default::default()
        })
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    let RunOutcome::Aborted { failure, .. } = &result.outcome else {
        panic!("overflow abort")
    };
    let overflow = failure.journal_overflow().unwrap();
    assert_eq!(overflow.first_rejected.event_sequence.0, first);
    assert!(overflow.rejected_events >= 2);
    assert_eq!(result.journal.len(), capacity);
    assert!(outputs(&result).is_empty());
    journal::assert_serialized_journal(&result);
}

#[tokio::test]
async fn failed_process_start_claims_no_completed_output_streams() {
    let plan = compile_source(
        r#"test "no capture" { server { process.run("/webtest-missing-output-executable") } }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(!matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(outputs(&result).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn interrupted_or_over_limit_capture_does_not_claim_completed_stream_snapshots() {
    struct ProcessBegun(PathBuf);
    #[async_trait]
    impl RunControl for ProcessBegun {
        fn is_cancelled(&self) -> bool {
            self.0.exists()
        }
        fn cancellation_reason(&self) -> CancellationReason {
            CancellationReason::DebugDisconnect
        }
        async fn before_step(&self, _: &PlannedTest, _: &PlannedStep) {}
    }
    let directory = tempfile::tempdir().unwrap();
    let control = ProcessBegun(directory.path().join("begun"));
    let plan = compile_source(
        r#"test "partial" { server {
        process.run("/bin/sh", args: ["-c", "printf partial; echo ready > begun; exec sleep 30"])
    } }"#,
    );
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        Runner::new(Arc::default())
            .with_options(RunnerOptions {
                project_root: directory.path().into(),
                ..Default::default()
            })
            .run_with_control(&plan, &LifecycleHost(Arc::default()), Some(&control)),
    )
    .await
    .expect("bounded native interruption");
    assert!(matches!(
        result.outcome,
        RunOutcome::Cancelled {
            reason: CancellationReason::DebugDisconnect
        }
    ));
    assert!(outputs(&result).is_empty());
    let plan = compile_source(
        r#"test "too much" { server {
        process.run("/bin/sh", args: ["-c", "printf 123456789"])
    } }"#,
    );
    let mut options = RunnerOptions::default();
    options.provider_config.process.max_output_bytes = 8;
    let result = Runner::new(Arc::default())
        .with_options(options)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(!matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(outputs(&result).is_empty());
}
