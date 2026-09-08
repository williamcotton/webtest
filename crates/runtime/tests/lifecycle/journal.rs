use super::*;
use std::num::NonZeroUsize;
use webtest_observation::{EventSequence, RunOutcomeKind};
use webtest_runtime::{JobLimit, SubscriptionGapReason, SubscriptionItem, TestRun, run_jobs};

fn bounded_runner(capacity: usize) -> Runner {
    Runner::new(Arc::new(ObservationStore::default())).with_options(RunnerOptions {
        journal_max_events: NonZeroUsize::new(capacity).unwrap(),
        ..Default::default()
    })
}

fn assert_overflow(result: &webtest_runtime::RunResult, capacity: usize) {
    let RunOutcome::Aborted { failure, .. } = &result.outcome else {
        panic!("collector exhaustion did not abort: {:?}", result.outcome);
    };
    let overflow = failure.journal_overflow().expect("typed gap");
    assert_eq!(overflow.capacity, capacity);
    assert_eq!(result.journal.len(), capacity);
    assert_eq!(overflow.first_rejected.execution_id, result.execution_id);
    assert_eq!(
        overflow.first_rejected.event_sequence,
        EventSequence((capacity - 1) as u64)
    );
    assert_eq!(
        overflow.last_rejected.event_sequence.0 - overflow.first_rejected.event_sequence.0 + 1,
        overflow.rejected_events
    );
    let terminal = result.journal.last().unwrap();
    assert_eq!(
        terminal.identity.event_sequence.0,
        overflow.last_rejected.event_sequence.0 + 1
    );
    assert!(matches!(
        terminal.event,
        ExecutionEvent::RunFinished {
            outcome: RunOutcomeKind::Aborted,
            failure_class: Some(FailureClass::Infrastructure),
            ..
        }
    ));
    assert_eq!(
        result.events,
        result
            .journal
            .iter()
            .map(|r| r.event.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test(start_paused = true)]
async fn journal_exhaustion_at_every_event_boundary_is_explicit_and_retains_the_terminal() {
    let plan = compile_source(r#"test "one" { browser { evaluate "ok" } }"#);
    let host = LifecycleHost(Arc::new(LifecycleState::default()));
    let baseline = bounded_runner(1000).run(&plan, &host).await;
    for capacity in 1..baseline.journal.len() {
        let state = Arc::new(LifecycleState::default());
        let result = bounded_runner(capacity)
            .run(&plan, &LifecycleHost(state.clone()))
            .await;
        assert_overflow(&result, capacity);
        let log = state.log();
        assert_eq!(
            log.iter()
                .filter(|s| s.starts_with("session_start:"))
                .count(),
            log.iter()
                .filter(|s| s.starts_with("session_close:"))
                .count(),
            "{capacity}: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|s| s.starts_with("context_create:"))
                .count(),
            log.iter()
                .filter(|s| s.starts_with("context_close:"))
                .count(),
            "{capacity}: {log:?}"
        );
        if capacity == 1 {
            assert!(log.is_empty());
        }
    }
    let exact = bounded_runner(baseline.journal.len())
        .run(&plan, &host)
        .await;
    assert!(matches!(exact.outcome, RunOutcome::Completed));
}

#[tokio::test(start_paused = true)]
async fn journal_exhaustion_cancels_active_jobs_before_slow_teardown_and_stops_admission() {
    let plan = compile_source(
        r#"
        test "slow" { browser { evaluate "slow" } }
        test "fast" { browser { evaluate "fast" } }
        test "never admitted" { browser { evaluate "third" } }
    "#,
    );
    let state = || {
        let state = Arc::new(LifecycleState::default());
        state.page_delays.lock().unwrap().extend([
            ("slow".into(), Duration::from_secs(1)),
            ("fast".into(), Duration::from_millis(10)),
        ]);
        state
            .context_close_delays
            .lock()
            .unwrap()
            .insert(0, Duration::from_millis(30));
        state
    };
    let run = async |runner: &Runner, state: Arc<LifecycleState>| {
        run_jobs(
            &[TestRun {
                runner,
                plan: &plan,
                browser: &LifecycleHost(state),
                control: None,
            }],
            JobLimit::new(2).unwrap(),
        )
        .await
        .remove(0)
    };
    let baseline = run(&bounded_runner(1000), state()).await;
    let cutoff = baseline
        .journal
        .iter()
        .position(|r| {
            matches!(
                r.event,
                ExecutionEvent::StepPassed {
                    test_id: TestId(1),
                    ..
                }
            )
        })
        .unwrap();
    let state = state();
    let started = tokio::time::Instant::now();
    let result = run(&bounded_runner(cutoff + 1), state.clone()).await;
    assert_overflow(&result, cutoff + 1);
    assert_eq!(started.elapsed(), Duration::from_millis(40));
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Cancelled {
            reason: CancellationReason::RunnerShutdown
        }
    ));
    assert!(matches!(
        result.tests[2].outcome,
        TestOutcome::Skipped { .. }
    ));
    assert_eq!(state.next_session.load(Ordering::SeqCst), 2);
    let log = state.log();
    assert!(log.contains(&"context_close_done:0".into()));
    assert!(log.contains(&"session_close:0".into()));
    assert!(log.contains(&"session_close:1".into()));
}

#[tokio::test(start_paused = true)]
async fn journal_gap_does_not_replace_primary_or_cleanup_failure_details() {
    let plan = compile_source(r#"test "failed" { browser { evaluate "ok" expect 1 == 2 } }"#);
    let state = || {
        let state = Arc::new(LifecycleState::default());
        state.context_close_failures.lock().unwrap().insert(0);
        state.session_close_failures.lock().unwrap().insert(0);
        state
    };
    let baseline = bounded_runner(1000)
        .run(&plan, &LifecycleHost(state()))
        .await;
    // Exhaustion at the final TestFinished emission preserves the already-known
    // assertion and both independent cleanup failures, while aborting the run.
    let cutoff = baseline
        .journal
        .iter()
        .position(|r| matches!(r.event, ExecutionEvent::TestFinished { .. }))
        .unwrap();
    let result = bounded_runner(cutoff + 1)
        .run(&plan, &LifecycleHost(state()))
        .await;
    assert_overflow(&result, cutoff + 1);
    let TestOutcome::Aborted {
        failure: RunError::Cleanup(_),
        prior_outcome: Some(prior),
    } = &result.tests[0].outcome
    else {
        panic!("{:?}", result.tests[0].outcome)
    };
    assert!(matches!(**prior, PriorTestOutcome::Failed(_)));
    let RunOutcome::Aborted {
        failure: RunError::Multiple { primary, secondary },
        ..
    } = &result.outcome
    else {
        panic!("{:?}", result.outcome)
    };
    let RunError::Multiple {
        primary,
        secondary: cleanup,
    } = primary.as_ref()
    else {
        panic!("missing cleanup aggregate")
    };
    assert!(matches!(
        primary.as_ref(),
        RunError::Cleanup(CleanupFailure {
            resource: CleanupResource::BrowserContext,
            ..
        })
    ));
    assert!(matches!(
        cleanup.as_slice(),
        [RunError::Cleanup(CleanupFailure {
            resource: CleanupResource::BrowserSession,
            ..
        })]
    ));
    assert!(matches!(
        secondary.as_slice(),
        [RunError::JournalOverflow(_)]
    ));
}

#[tokio::test]
async fn slow_and_dropped_subscribers_do_not_affect_execution_or_authoritative_retention() {
    let plan = compile_source(r#"test "one" { server { expect 1 == 1 } }"#);
    let mut runner = bounded_runner(1000);
    let mut slow = runner.subscribe(NonZeroUsize::new(2).unwrap());
    let dropped = runner.subscribe(NonZeroUsize::new(1).unwrap());
    drop(dropped);
    let result = runner.run(&plan, &LifecycleHost(Arc::default())).await;
    assert!(matches!(result.outcome, RunOutcome::Completed));
    for record in result.journal.iter().take(2) {
        assert_eq!(
            slow.next().await,
            Some(SubscriptionItem::Event(Arc::new(record.clone())))
        );
    }
    let Some(SubscriptionItem::Overflow(gap)) = slow.next().await else {
        panic!("missing overflow marker")
    };
    assert_eq!(gap.reason, SubscriptionGapReason::SubscriberCapacity);
    assert_eq!(gap.first_rejected, result.journal[2].identity);
    assert!(slow.next().await.is_none());
    assert!(matches!(
        result.journal.last().unwrap().event,
        ExecutionEvent::RunFinished { .. }
    ));
}

#[tokio::test]
async fn healthy_subscriber_gets_ordered_records_across_runs_and_ends_when_runner_drops() {
    let plan = compile_source(r#"test "one" { server { expect 1 == 1 } }"#);
    let mut runner = bounded_runner(1000);
    let mut subscriber = runner.subscribe(NonZeroUsize::new(1000).unwrap());
    for _ in 0..2 {
        let result = runner.run(&plan, &LifecycleHost(Arc::default())).await;
        for record in result.journal {
            assert_eq!(
                subscriber.next().await,
                Some(SubscriptionItem::Event(Arc::new(record)))
            );
        }
    }
    drop(runner);
    assert!(subscriber.next().await.is_none());
}

#[tokio::test]
async fn collector_failure_closes_even_a_fast_projection_with_a_typed_journal_gap() {
    let plan = compile_source(r#"test "one" { server { expect 1 == 1 } }"#);
    let mut runner = bounded_runner(2);
    let mut subscriber = runner.subscribe(NonZeroUsize::new(1000).unwrap());
    let result = runner.run(&plan, &LifecycleHost(Arc::default())).await;
    assert_overflow(&result, 2);
    assert_eq!(
        subscriber.next().await,
        Some(SubscriptionItem::Event(Arc::new(result.journal[0].clone())))
    );
    let Some(SubscriptionItem::Overflow(gap)) = subscriber.next().await else {
        panic!("missing gap")
    };
    assert_eq!(gap.reason, SubscriptionGapReason::JournalCapacity);
    assert_eq!(gap.capacity, 2);
    assert!(subscriber.next().await.is_none());
}

fn assert_source_and_operation_metadata(plan: &TestPlan, result: &webtest_runtime::RunResult) {
    use webtest_observation::{EventMetadata, ReplayJournal, ReplayOutcome};
    let mut replay = ReplayJournal::new(NonZeroUsize::new(result.journal.len()).unwrap());
    for record in result.journal.iter().rev() {
        assert_eq!(
            record.metadata.source_revision,
            Some(plan.source_revision),
            "{:?}",
            record.event
        );
        assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Inserted));
        if let Some(scope) = record.event.scope() {
            assert_eq!(record.metadata, EventMetadata::from(scope));
        }
        if matches!(
            record.event,
            ExecutionEvent::TestStarted { .. } | ExecutionEvent::TestFinished { .. }
        ) {
            let context = &record.metadata.execution_context;
            assert!(context.test_execution_id.is_some());
            assert!(context.scope_id.is_some());
            assert!(context.parent_scope_id.is_none());
            assert!(context.operation_execution_id.is_none());
        }
        if let ExecutionEvent::StepStarted {
            test_id, step_id, ..
        }
        | ExecutionEvent::StepPassed {
            test_id, step_id, ..
        }
        | ExecutionEvent::StepFailed {
            test_id, step_id, ..
        }
        | ExecutionEvent::ProviderCallStarted {
            test_id, step_id, ..
        }
        | ExecutionEvent::ProviderCallFinished {
            test_id, step_id, ..
        }
        | ExecutionEvent::ProviderCallFailed {
            test_id, step_id, ..
        } = &record.event
        {
            let step = plan
                .tests
                .iter()
                .find(|test| test.id == *test_id)
                .unwrap()
                .steps()
                .into_iter()
                .find(|step| step.id == *step_id)
                .unwrap();
            assert_eq!(record.metadata.origin, Some(step.origin));
            let context = &record.metadata.execution_context;
            assert!(
                context.operation_execution_id.is_some(),
                "{:?}",
                record.event
            );
            let scope = result
                .journal
                .iter()
                .find_map(|candidate| match &candidate.event {
                    ExecutionEvent::Scope { event, .. }
                        if event.outcome.is_none()
                            && event.execution_context.operation_execution_id
                                == context.operation_execution_id =>
                    {
                        Some(event)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(record.metadata, EventMetadata::from(scope));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn journal_metadata_keeps_interleaved_siblings_and_deferred_failures_in_their_own_scopes() {
    let source = r#"test "méta" { parallel {
        browser { evaluate "slow" expect 1 == 2 }
        browser { evaluate "fast" expect 3 == 3 }
    } }"#;
    let plan = compile_source(source);
    let state = Arc::new(LifecycleState::default());
    state.page_delays.lock().unwrap().extend([
        ("slow".into(), Duration::from_millis(10)),
        ("fast".into(), Duration::from_millis(1)),
    ]);
    let result = bounded_runner(1000).run(&plan, &LifecycleHost(state)).await;
    assert_eq!(result.failed(), 1);
    assert_source_and_operation_metadata(&plan, &result);
    let failed = result
        .journal
        .iter()
        .find(|record| matches!(record.event, ExecutionEvent::StepFailed { .. }))
        .unwrap();
    let range = failed.metadata.origin.unwrap().range;
    assert_eq!(
        &source[u32::from(range.start()) as usize..u32::from(range.end()) as usize],
        "1 == 2 "
    );
    let started: Vec<_> = result
        .journal
        .iter()
        .filter(|r| matches!(r.event, ExecutionEvent::StepStarted { .. }))
        .collect();
    assert_ne!(
        started[0].metadata.execution_context.parent_scope_id,
        started[1].metadata.execution_context.parent_scope_id
    );
}

#[tokio::test(start_paused = true)]
async fn journal_metadata_distinguishes_retry_occurrences_and_retains_provider_and_resource_ownership()
 {
    let plan = compile_source(
        r#"test "retry" {
        server { fs.temp_dir() }
        retry 2 { server { expect 1 == 2 } }
    }"#,
    );
    let result = bounded_runner(1000)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(result.failed(), 1);
    assert_source_and_operation_metadata(&plan, &result);
    let failures: Vec<_> = result
        .journal
        .iter()
        .filter(|r| matches!(r.event, ExecutionEvent::StepFailed { .. }))
        .collect();
    assert_eq!(failures.len(), 2);
    let first = &failures[0].metadata.execution_context;
    let second = &failures[1].metadata.execution_context;
    assert_eq!(first.plan_node_id, second.plan_node_id);
    assert_eq!(first.task_path, second.task_path);
    assert!(first.attempt_id.is_some());
    assert_ne!(first.attempt_id, second.attempt_id);
    assert_ne!(first.operation_execution_id, second.operation_execution_id);
    assert!(
        result
            .journal
            .iter()
            .any(|r| matches!(r.event, ExecutionEvent::ProviderCallFinished { .. }))
    );
    assert!(
        result
            .journal
            .iter()
            .any(|r| matches!(r.event, ExecutionEvent::Resource { .. }))
    );
}

#[tokio::test]
async fn journal_metadata_maps_skipped_tests_without_fabricating_runtime_occurrences() {
    let mut plan = compile_source(r#"test "skipped" { server { expect 1 == 1 } }"#);
    plan.required_host_capabilities.clear(); // A rejected plan never creates test roots.
    let result = bounded_runner(1000)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(result.skipped(), 1);
    for record in &result.journal {
        assert_eq!(record.metadata.source_revision, Some(plan.source_revision));
        assert!(
            record
                .metadata
                .execution_context
                .test_execution_id
                .is_none()
        );
        assert!(record.metadata.execution_context.scope_id.is_none());
        if matches!(record.event, ExecutionEvent::TestSkipped { .. }) {
            assert_eq!(record.metadata.origin, Some(plan.tests[0].origin));
            assert_eq!(
                record.metadata.execution_context.test_id,
                Some(plan.tests[0].id)
            );
        } else {
            assert!(record.metadata.origin.is_none());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn journal_timeout_summary_keeps_the_timeout_range_and_test_occurrence() {
    let source = r#"test "deadline" { timeout 5ms { browser { evaluate "slow" } } }"#;
    let plan = compile_source(source);
    let state = Arc::new(LifecycleState::default());
    state
        .page_delays
        .lock()
        .unwrap()
        .insert("slow".into(), Duration::from_secs(1));
    let result = bounded_runner(1000).run(&plan, &LifecycleHost(state)).await;
    assert_eq!(result.timed_out(), 1);
    assert_source_and_operation_metadata(&plan, &result);
    let timeout = result
        .journal
        .iter()
        .find(|r| matches!(r.event, ExecutionEvent::TestTimedOut { .. }))
        .unwrap();
    let range = timeout.metadata.origin.unwrap().range;
    assert_eq!(
        &source[u32::from(range.start()) as usize..u32::from(range.end()) as usize],
        "timeout 5ms { browser { evaluate \"slow\" } }"
    );
    assert!(
        timeout
            .metadata
            .execution_context
            .test_execution_id
            .is_some()
    );
    assert!(
        timeout
            .metadata
            .execution_context
            .operation_execution_id
            .is_none()
    );
}
