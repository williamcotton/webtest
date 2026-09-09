use super::*;
use webtest_runtime::{
    ExecutionIdentityError, ExecutionIdentitySource, JobLimit, TestRun, TestWorker, run_jobs,
    run_jobs_on_workers,
};

struct IdSource {
    allocations: AtomicUsize,
    fail_at: usize,
}

impl ExecutionIdentitySource for IdSource {
    fn allocate(&self) -> Result<ExecutionId, ExecutionIdentityError> {
        let ordinal = self.allocations.fetch_add(1, Ordering::SeqCst);
        if ordinal == self.fail_at {
            Err(ExecutionIdentityError {
                source: getrandom::Error::UNSUPPORTED,
            })
        } else {
            Ok(ExecutionId::from_u128(ordinal as u128 + 100))
        }
    }
}

#[tokio::test]
async fn allocation_failure_clears_observations_without_execution_or_fabricated_events() {
    let plan = compile_source(r#"test "first" { browser { evaluate "never" } } test "second" {}"#);
    let store = Arc::new(ObservationStore::default());
    store.begin_execution(plan.file, ExecutionId::from_u128(1));
    store.record(RuntimeObservation {
        execution_id: ExecutionId::from_u128(1),
        file: plan.file,
        source_revision: plan.source_revision,
        test_id: TestId(0),
        step_id: None,
        range: TextRange::default(),
        kind: RuntimeObservationKind::ValueFailure {
            code: webtest_observation::RuntimeFailureCode::AssertionFailed,
            message: "stale".into(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        },
    });
    assert_eq!(
        store
            .observations_for(plan.file, plan.source_revision)
            .len(),
        1
    );
    let state = Arc::new(LifecycleState::default());
    let source = Arc::new(IdSource {
        allocations: AtomicUsize::new(0),
        fail_at: 0,
    });
    let result = Runner::new(store.clone())
        .with_execution_identity_source(source.clone())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_eq!(source.allocations.load(Ordering::SeqCst), 1);
    assert!(state.log().is_empty());
    assert!(
        store
            .observations_for(plan.file, plan.source_revision)
            .is_empty()
    );
    assert!(!store.complete_execution(
        plan.file,
        plan.source_revision,
        ExecutionId::from_u128(1),
        vec![]
    ));
    assert!(result.execution_id.is_none());
    assert!(result.events.is_empty());
    assert!(result.journal.is_empty());
    let RunOutcome::Aborted {
        failure,
        prior_outcome: None,
    } = result.outcome
    else {
        panic!("expected pre-execution infrastructure failure");
    };
    assert!(matches!(failure, RunError::ExecutionIdentity(_)));
    assert_eq!(
        failure.code(),
        webtest_observation::RuntimeFailureCode::ExecutionIdentityUnavailable
    );
    assert_eq!(failure.failure_class(), FailureClass::Infrastructure);
    assert_eq!(
        result
            .tests
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(result.tests.iter().all(|test| matches!(
        test.outcome,
        TestOutcome::Skipped {
            reason: SkipReason::RunAborted,
            failure_class: Some(FailureClass::Infrastructure)
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn failed_file_identity_preserves_healthy_files_and_source_order_in_every_scheduler() {
    for (jobs, workers) in [(1, false), (3, false), (1, true), (3, true)] {
        let source = Arc::new(IdSource {
            allocations: AtomicUsize::new(0),
            fail_at: 1,
        });
        let runners: Vec<_> = (0..3)
            .map(|_| {
                Runner::new(Arc::new(ObservationStore::default()))
                    .with_execution_identity_source(source.clone())
            })
            .collect();
        let plans = [
            compile_source(r#"test "first" { browser { evaluate "first" } }"#),
            compile_source(r#"test "failed" { browser { evaluate "never" } }"#),
            compile_source(r#"test "third" { browser { evaluate "third" } }"#),
        ];
        let state = Arc::new(LifecycleState::default());
        let host = LifecycleHost(state.clone());
        let inputs: Vec<_> = runners
            .iter()
            .zip(&plans)
            .map(|(runner, plan)| TestRun {
                runner,
                plan,
                browser: &host,
                control: None,
            })
            .collect();
        let results = if workers {
            let services: Vec<_> = (0..jobs)
                .map(|_| TestWorker {
                    options: RunnerOptions::default(),
                    providers: ProviderRegistry::default(),
                })
                .collect();
            run_jobs_on_workers(&inputs, &services).await.unwrap()
        } else {
            run_jobs(&inputs, JobLimit::new(jobs).unwrap()).await
        };
        assert_eq!(source.allocations.load(Ordering::SeqCst), 3);
        assert_eq!(
            results
                .iter()
                .map(|r| r.tests[0].name.as_str())
                .collect::<Vec<_>>(),
            ["first", "failed", "third"]
        );
        assert_eq!(
            results.iter().map(|r| r.execution_id).collect::<Vec<_>>(),
            [
                Some(ExecutionId::from_u128(100)),
                None,
                Some(ExecutionId::from_u128(102))
            ]
        );
        assert!(results[1].journal.is_empty());
        for index in [0, 2] {
            assert_eq!(results[index].passed(), 1);
            journal::assert_serialized_journal(&results[index]);
        }
        assert!(!state.log().iter().any(|entry| entry.contains("never")));
    }
}

#[tokio::test]
async fn each_run_allocates_once_and_shares_identity_across_branches_and_attempts() {
    let source = Arc::new(IdSource {
        allocations: AtomicUsize::new(0),
        fail_at: usize::MAX,
    });
    let runner = Runner::new(Arc::new(ObservationStore::default()))
        .with_execution_identity_source(source.clone());
    let plan = compile_source(
        r#"test "nested" {
        parallel { server { retry 2 backoff 0ms { expect 1 == 2 } } server { expect true } }
    }"#,
    );
    let host = LifecycleHost(Arc::new(LifecycleState::default()));
    for ordinal in 0..2 {
        let result = runner.run(&plan, &host).await;
        let expected = ExecutionId::from_u128(ordinal + 100);
        assert_eq!(result.execution_id, Some(expected));
        assert_eq!(result.failed(), 1);
        assert_eq!(result.journal[0].identity.event_sequence.0, 0);
        assert!(
            result
                .journal
                .iter()
                .all(|record| record.identity.execution_id == expected
                    && record.event.execution_id() == expected)
        );
        journal::assert_serialized_journal(&result);
    }
    assert_eq!(source.allocations.load(Ordering::SeqCst), 2);
}
