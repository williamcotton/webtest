use super::*;
use webtest_runtime::{JobLimit, TestRun, run_jobs};

struct CancelAt(tokio::time::Instant);
#[async_trait]
impl RunControl for CancelAt {
    fn is_cancelled(&self) -> bool {
        tokio::time::Instant::now() >= self.0
    }
    fn cancellation_reason(&self) -> CancellationReason {
        CancellationReason::FailFast
    }
    async fn cancelled(&self) {
        tokio::time::sleep_until(self.0).await;
    }
    async fn before_step(&self, _: &PlannedTest, _: &PlannedStep) {}
}

#[tokio::test(start_paused = true)]
async fn jobs_bound_live_roots_across_files_and_preserve_source_order() {
    let state = Arc::new(LifecycleState::default());
    state.page_delays.lock().unwrap().extend([
        ("slow".into(), Duration::from_millis(100)),
        ("fast".into(), Duration::from_millis(10)),
        ("third".into(), Duration::from_millis(10)),
    ]);
    // The fast test's slot must remain occupied during slow context teardown.
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(1, Duration::from_millis(30));
    let first = compile_source(
        r#"
        test "slow" { browser { evaluate "slow" } }
        test "fast" { browser { evaluate "fast" } }
    "#,
    );
    let second = compile_source(r#"test "third" { browser { evaluate "third" } }"#);
    let sink = Arc::new(RecordingEventSink::default());
    let runner1 = Runner::new(Arc::new(ObservationStore::default())).with_event_sink(sink.clone());
    let runner2 = Runner::new(Arc::new(ObservationStore::default())).with_event_sink(sink.clone());
    let host = LifecycleHost(state.clone());
    let results = run_jobs(
        &[
            TestRun {
                runner: &runner1,
                plan: &first,
                browser: &host,
                control: None,
            },
            TestRun {
                runner: &runner2,
                plan: &second,
                browser: &host,
                control: None,
            },
        ],
        JobLimit::new(2).unwrap(),
    )
    .await;
    assert_eq!(
        results.iter().map(|result| result.passed()).sum::<usize>(),
        3
    );
    assert_eq!(
        results[0]
            .tests
            .iter()
            .map(|test| test.name.as_str())
            .collect::<Vec<_>>(),
        ["slow", "fast"]
    );
    assert_eq!(results[1].tests[0].name, "third");
    let log = state.log();
    let at = |s: &str| log.iter().position(|event| event == s).unwrap();
    assert!(at("session_start:1") < at("session_close:0"));
    assert!(at("context_close_done:1") < at("session_close:1"));
    assert!(at("session_close:1") < at("session_start:2"));
    assert!(at("session_close:2") < at("session_close:0"));
    let mut active = 0;
    let mut peak = 0;
    for entry in &log {
        if entry.starts_with("session_start:") {
            active += 1;
            peak = peak.max(active);
        }
        if entry.starts_with("session_close:") {
            active -= 1;
        }
    }
    assert_eq!((peak, active), (2, 0));
    for result in &results {
        assert_terminal_event_invariants(&result.events);
        assert_scopes_finish_once_after_children(&result.events);
        assert_eq!(result.events.len(), result.journal.len());
        for (ordinal, (event, record)) in result.events.iter().zip(&result.journal).enumerate() {
            assert_eq!(&record.event, event);
            assert_eq!(
                record.identity.execution_id,
                result.execution_id.expect("run started")
            );
            let plan = if result.execution_id.expect("run started")
                == results[0].execution_id.expect("run started")
            {
                &first
            } else {
                &second
            };
            assert_eq!(record.metadata.source_revision, Some(plan.source_revision));
            if let Some(origin) = record.metadata.origin {
                assert_eq!(origin.file, plan.file);
            }
            assert_eq!(record.identity.event_sequence.0, ordinal as u64);
            assert_eq!(
                record.schema_version,
                webtest_observation::EVENT_JOURNAL_SCHEMA_VERSION
            );
        }
    }
    let events = sink.events.lock().unwrap();
    let finished: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::TestFinished {
                execution_id,
                test_id,
                ..
            } => Some((*execution_id, *test_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        [
            (
                results[0].execution_id.expect("run started"),
                first.tests[1].id
            ),
            (
                results[1].execution_id.expect("run started"),
                second.tests[0].id
            ),
            (
                results[0].execution_id.expect("run started"),
                first.tests[0].id
            ),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn jobs_keep_bindings_and_failure_observations_isolated_and_publish_atomically() {
    struct ReverseCompletion;
    #[async_trait]
    impl RunControl for ReverseCompletion {
        async fn before_step(&self, test: &PlannedTest, _: &PlannedStep) {
            if test.name == "first" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    let plan = compile_source(
        r#"
        test "first" { let value = 101 expect value == 102 }
        test "second" { let value = 201 expect value == 202 }
        test "third" { let value = 301 expect value == 301 }
    "#,
    );
    let observations = Arc::new(ObservationStore::default());
    let runner = Runner::new(observations.clone());
    let host = LifecycleHost(Arc::new(LifecycleState::default()));
    let inputs = [TestRun {
        runner: &runner,
        plan: &plan,
        browser: &host,
        control: Some(&ReverseCompletion),
    }];
    let (results, ()) = tokio::join!(run_jobs(&inputs, JobLimit::new(2).unwrap()), async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert!(
            observations
                .observations_for(plan.file, plan.source_revision)
                .is_empty(),
            "a completed sibling must not publish a partial file batch"
        );
    });
    let result = &results[0];
    assert_eq!((result.failed(), result.passed()), (2, 1));
    for (index, value) in [101, 201, 301].into_iter().enumerate() {
        assert_eq!(result.tests[index].bindings["value"], Value::Int(value));
    }
    let batch = observations.observations_for(plan.file, plan.source_revision);
    assert_eq!(batch.len(), 2);
    assert_eq!(
        batch
            .iter()
            .map(|observation| observation.test_id)
            .collect::<Vec<_>>(),
        [plan.tests[0].id, plan.tests[1].id]
    );
    assert!(
        batch.iter().all(
            |observation| observation.execution_id == result.execution_id.expect("run started")
        )
    );
}

#[tokio::test(start_paused = true)]
async fn jobs_do_not_cancel_healthy_roots_on_abort_and_retain_all_infrastructure_failures() {
    let plan = compile_source(
        r#"
        test "first" { browser { evaluate "slow failure" } }
        test "second" { browser { evaluate "fast failure" } }
        test "skipped" { browser { evaluate "never" } }
    "#,
    );
    let state = Arc::new(LifecycleState::default());
    state
        .page_delays
        .lock()
        .unwrap()
        .insert("slow failure".into(), Duration::from_millis(30));
    state.page_errors.lock().unwrap().extend([
        (
            "slow failure".into(),
            BrowserError::Protocol {
                method: "Runtime.evaluate".into(),
                message: "first failure".into(),
            },
        ),
        ("fast failure".into(), BrowserError::BrowserDisconnected),
    ]);
    let runner = Runner::new(Arc::new(ObservationStore::default()));
    let host = LifecycleHost(state.clone());
    let results = run_jobs(
        &[TestRun {
            runner: &runner,
            plan: &plan,
            browser: &host,
            control: None,
        }],
        JobLimit::new(2).unwrap(),
    )
    .await;
    let result = &results[0];
    assert_eq!(
        (result.aborted(), result.skipped(), result.cancelled()),
        (2, 1, 0)
    );
    assert!(
        matches!(&result.tests[0].outcome, TestOutcome::Aborted { failure, .. } if failure.to_string().contains("first failure"))
    );
    assert!(
        matches!(&result.outcome, RunOutcome::Aborted { failure, .. } if failure.to_string().contains("first failure"))
    );
    assert_eq!(state.next_session.load(Ordering::SeqCst), 2);
    assert_terminal_event_invariants(&result.events);
    assert_scopes_finish_once_after_children(&result.events);
}

#[tokio::test(start_paused = true)]
async fn jobs_session_teardown_finishes_before_terminal_test_and_retains_primary_failure() {
    struct SessionClosedBeforeFinish(Arc<LifecycleState>);
    impl RunEventSink for SessionClosedBeforeFinish {
        fn publish(&self, event: &ExecutionEvent) {
            if matches!(event, ExecutionEvent::TestFinished { .. }) {
                assert!(self.0.log().contains(&"session_close:0".into()));
            }
        }
    }
    let plan = compile_source(r#"test "failure" { browser { evaluate "ok" expect 1 == 2 } }"#);
    let state = Arc::new(LifecycleState::default());
    state.session_close_failures.lock().unwrap().insert(0);
    let runner = Runner::new(Arc::new(ObservationStore::default()))
        .with_event_sink(Arc::new(SessionClosedBeforeFinish(state.clone())));
    let host = LifecycleHost(state);
    let results = run_jobs(
        &[TestRun {
            runner: &runner,
            plan: &plan,
            browser: &host,
            control: None,
        }],
        JobLimit::new(2).unwrap(),
    )
    .await;
    assert!(
        matches!(&results[0].tests[0].outcome, TestOutcome::Aborted {
        failure: RunError::Cleanup(CleanupFailure { resource: CleanupResource::BrowserSession, .. }),
        prior_outcome: Some(prior),
    } if matches!(prior.as_ref(), PriorTestOutcome::Failed(_)))
    );
    assert_terminal_event_invariants(&results[0].events);
}

#[tokio::test(start_paused = true)]
async fn jobs_preserve_typed_explicit_cancellation_and_await_all_admitted_cleanup() {
    let plan = compile_source(
        r#"
        test "first" { browser { evaluate "slow" } }
        test "second" { browser { evaluate "slow" } }
        test "skipped" { browser { evaluate "never" } }
    "#,
    );
    let state = Arc::new(LifecycleState::default());
    state
        .page_delays
        .lock()
        .unwrap()
        .insert("slow".into(), Duration::from_secs(10));
    state.context_close_delays.lock().unwrap().extend([
        (0, Duration::from_millis(20)),
        (1, Duration::from_millis(30)),
    ]);
    let runner = Runner::new(Arc::new(ObservationStore::default()));
    let host = LifecycleHost(state.clone());
    let control = CancelAt(tokio::time::Instant::now() + Duration::from_millis(10));
    let results = run_jobs(
        &[TestRun {
            runner: &runner,
            plan: &plan,
            browser: &host,
            control: Some(&control),
        }],
        JobLimit::new(2).unwrap(),
    )
    .await;
    let result = &results[0];
    assert_eq!((result.cancelled(), result.skipped()), (2, 1));
    assert!(matches!(
        result.outcome,
        RunOutcome::Cancelled {
            reason: CancellationReason::FailFast
        }
    ));
    for test in &result.tests[..2] {
        assert!(matches!(
            test.outcome,
            TestOutcome::Cancelled {
                reason: CancellationReason::FailFast
            }
        ));
    }
    for id in 0..2 {
        assert_eq!(
            state
                .log()
                .iter()
                .filter(|entry| *entry == &format!("context_close_done:{id}"))
                .count(),
            1
        );
        assert_eq!(
            state
                .log()
                .iter()
                .filter(|entry| *entry == &format!("session_close:{id}"))
                .count(),
            1
        );
    }
    assert_scopes_finish_once_after_children(&result.events);
}

#[tokio::test]
async fn jobs_one_preserves_sequential_session_reuse() {
    let plan = compile_source(
        r#"test "one" { browser { evaluate "one" } } test "two" { browser { evaluate "two" } }"#,
    );
    let state = Arc::new(LifecycleState::default());
    let runner = Runner::new(Arc::new(ObservationStore::default()));
    let host = LifecycleHost(state.clone());
    let results = run_jobs(
        &[TestRun {
            runner: &runner,
            plan: &plan,
            browser: &host,
            control: None,
        }],
        JobLimit::default(),
    )
    .await;
    assert_eq!(results[0].passed(), 2);
    assert_eq!(state.next_session.load(Ordering::SeqCst), 1);
    assert_eq!(state.next_context.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn jobs_stop_admission_on_primary_infrastructure_failure_before_slow_teardown() {
    let plan = compile_source(
        r#"
        test "broken" { browser { evaluate "broken" } }
        test "healthy" { browser { evaluate "healthy" } }
        test "never admitted" { browser { evaluate "never" } }
    "#,
    );
    let state = Arc::new(LifecycleState::default());
    state
        .page_errors
        .lock()
        .unwrap()
        .insert("broken".into(), BrowserError::BrowserDisconnected);
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(0, Duration::from_millis(100));
    state
        .page_delays
        .lock()
        .unwrap()
        .insert("healthy".into(), Duration::from_millis(10));
    let runner = Runner::new(Arc::new(ObservationStore::default()));
    let host = LifecycleHost(state.clone());
    let results = run_jobs(
        &[TestRun {
            runner: &runner,
            plan: &plan,
            browser: &host,
            control: None,
        }],
        JobLimit::new(2).unwrap(),
    )
    .await;
    let result = &results[0];
    assert_eq!(
        (result.aborted(), result.passed(), result.skipped()),
        (1, 1, 1)
    );
    assert_eq!(state.next_session.load(Ordering::SeqCst), 2);
    let log = state.log();
    assert!(
        log.iter()
            .position(|entry| entry == "session_close:1")
            .unwrap()
            < log
                .iter()
                .position(|entry| entry == "context_close_done:0")
                .unwrap()
    );
    let skipped = result
        .events
        .iter()
        .position(|event| matches!(event, ExecutionEvent::TestSkipped { .. }))
        .unwrap();
    let broken_finished = result.events.iter().position(|event| matches!(event, ExecutionEvent::TestFinished { test_id, .. } if *test_id == plan.tests[0].id)).unwrap();
    assert!(skipped < broken_finished);
    assert_scopes_finish_once_after_children(&result.events);
}

#[tokio::test(start_paused = true)]
async fn jobs_do_not_charge_nested_parallel_branches_against_root_slots() {
    let plan = compile_source(
        r#"
        test "first" { parallel { browser { evaluate "slow" } browser { evaluate "slow" } } }
        test "second" { parallel { browser { evaluate "slow" } browser { evaluate "slow" } } }
    "#,
    );
    let state = Arc::new(LifecycleState::default());
    state
        .page_delays
        .lock()
        .unwrap()
        .insert("slow".into(), Duration::from_millis(10));
    let runner = Runner::new(Arc::new(ObservationStore::default()));
    let host = LifecycleHost(state.clone());
    let results = run_jobs(
        &[TestRun {
            runner: &runner,
            plan: &plan,
            browser: &host,
            control: None,
        }],
        JobLimit::new(2).unwrap(),
    )
    .await;
    assert_eq!(results[0].passed(), 2);
    assert!(results[0].tests.iter().all(|test| test.branches.len() == 2));
    let log = state.log();
    let first_close = log
        .iter()
        .position(|entry| entry.starts_with("session_close:"))
        .unwrap();
    assert_eq!(
        log[..first_close]
            .iter()
            .filter(|entry| entry.starts_with("session_start:"))
            .count(),
        4
    );
    assert_scopes_finish_once_after_children(&results[0].events);
}

#[tokio::test]
async fn jobs_keep_artifacts_distinct_even_when_files_have_the_same_local_ids() {
    let directory = tempfile::tempdir().unwrap();
    let state = Arc::new(LifecycleState::default());
    state.record_waits.store(true, Ordering::SeqCst);
    state.page_evidence.lock().unwrap().dom_snapshot = Some("<p>evidence</p>".into());
    state
        .locator_outcomes
        .lock()
        .unwrap()
        .extend((0..2).map(|_| {
            Err(BrowserError::AssertionFailed {
                locator: Locator::Text("ready".into()),
                expected: LocatorState::Visible,
                actual: "missing".into(),
            })
        }));
    let plan = compile_source(r#"test "evidence" { browser { expect text("ready").visible } }"#);
    let options = RunnerOptions {
        evidence: webtest_runtime::EvidenceOptions {
            artifact_directory: directory.path().to_owned(),
            dom_snapshot_on_failure: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let runner1 = Runner::new(Arc::new(ObservationStore::default())).with_options(options.clone());
    let runner2 = Runner::new(Arc::new(ObservationStore::default())).with_options(options);
    let host = LifecycleHost(state);
    let results = run_jobs(
        &[
            TestRun {
                runner: &runner1,
                plan: &plan,
                browser: &host,
                control: None,
            },
            TestRun {
                runner: &runner2,
                plan: &plan,
                browser: &host,
                control: None,
            },
        ],
        JobLimit::new(2).unwrap(),
    )
    .await;
    let mut paths = BTreeSet::new();
    for result in &results {
        let TestOutcome::Failed(failure) = &result.tests[0].outcome else {
            panic!("{:?}", result.tests[0].outcome);
        };
        assert_eq!(failure.artifacts.len(), 2);
        for artifact in &failure.artifacts {
            assert!(
                paths.insert(artifact.path.clone()),
                "evidence was overwritten"
            );
            assert!(artifact.path.is_file());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn jobs_run_summary_preserves_cancellation_before_or_after_an_independent_abort() {
    for cleanup in [Duration::ZERO, Duration::from_millis(30)] {
        let plan = compile_source(
            r#"
            test "broken" { browser { evaluate "broken" } }
            test "cancelled" { browser { evaluate "slow" } }
        "#,
        );
        let state = Arc::new(LifecycleState::default());
        state
            .page_errors
            .lock()
            .unwrap()
            .insert("broken".into(), BrowserError::BrowserDisconnected);
        state
            .page_delays
            .lock()
            .unwrap()
            .insert("slow".into(), Duration::from_secs(10));
        state
            .context_close_delays
            .lock()
            .unwrap()
            .insert(0, cleanup);
        let runner = Runner::new(Arc::new(ObservationStore::default()));
        let host = LifecycleHost(state);
        let control = CancelAt(tokio::time::Instant::now() + Duration::from_millis(10));
        let results = run_jobs(
            &[TestRun {
                runner: &runner,
                plan: &plan,
                browser: &host,
                control: Some(&control),
            }],
            JobLimit::new(2).unwrap(),
        )
        .await;
        assert_eq!((results[0].aborted(), results[0].cancelled()), (1, 1));
        assert!(matches!(
            results[0].outcome,
            RunOutcome::Aborted {
                prior_outcome: Some(webtest_runtime::PriorRunOutcome::Cancelled {
                    reason: CancellationReason::FailFast
                }),
                ..
            }
        ));
    }
}
