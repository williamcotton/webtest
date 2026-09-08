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
