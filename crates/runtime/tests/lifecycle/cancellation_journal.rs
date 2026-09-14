use super::*;
use webtest_observation::{RecordedEvent, ScopeCancellation};

fn requests(result: &webtest_runtime::RunResult) -> Vec<(&RecordedEvent, ScopeCancellation)> {
    result
        .journal
        .iter()
        .filter_map(|record| match &record.event {
            ExecutionEvent::CancellationRequested { cause, .. } => Some((record, *cause)),
            _ => None,
        })
        .collect()
}

pub(super) fn assert_requests(result: &webtest_runtime::RunResult) {
    journal::assert_serialized_journal(result);
    let mut targets = BTreeSet::new();
    for (record, cause) in requests(result) {
        let scope = record.event.scope().unwrap();
        let id = scope.execution_context.scope_id;
        assert!(
            targets.insert(id),
            "first accepted request is published once"
        );
        assert_eq!(record.metadata, scope.into());
        assert!(scope.outcome.is_none());
        assert!(scope.cancellation.is_none());
        let terminal = result.journal.iter().find(|record| matches!(&record.event,
            ExecutionEvent::Scope { event, .. } if event.execution_context.scope_id == id && event.outcome.is_some()
        )).unwrap();
        assert!(record.identity.event_sequence < terminal.identity.event_sequence);
        assert_eq!(terminal.event.scope().unwrap().cancellation, Some(cause));
    }
    assert!(!targets.is_empty());
}

#[tokio::test(start_paused = true)]
async fn primary_failure_request_is_published_before_sibling_interruption_and_slow_teardown() {
    let plan = compile_source(
        r#"test "early request" { parallel {
        browser { evaluate "fail" } browser { evaluate "slow" }
    } }"#,
    );
    let state = Arc::new(LifecycleState::default());
    state.page_delays.lock().unwrap().extend([
        ("fail".into(), Duration::from_millis(1)),
        ("slow".into(), Duration::from_secs(10)),
    ]);
    state
        .page_errors
        .lock()
        .unwrap()
        .insert("fail".into(), BrowserError::BrowserDisconnected);
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(0, Duration::from_millis(500));
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state))
        .await;
    assert_requests(&result);
    let requests = requests(&result);
    assert_eq!(requests.len(), 1);
    let (request, cause) = requests[0];
    let loser = &result.tests[0].branches[1].scope;
    assert_eq!(
        request.event.scope().unwrap().execution_context,
        loser.execution_context
    );
    assert_eq!(cause.reason, CancellationReason::ParentFailed);
    assert_eq!(
        Some(cause.causing_scope_id),
        loser.execution_context.parent_scope_id
    );
    assert_eq!(request.timestamp.elapsed, Duration::from_millis(1));
    let interrupted = result.journal.iter().find(|record| matches!(&record.event,
        ExecutionEvent::Wait { scope, event, .. } if scope.execution_context.scope_id == loser.execution_context.scope_id
        && event.kind == webtest_observation::WaitEventKind::Cancelled
    )).unwrap();
    assert!(request.identity.event_sequence < interrupted.identity.event_sequence);
    assert!(result.journal.last().unwrap().timestamp.elapsed >= Duration::from_millis(501));
}

#[tokio::test(start_paused = true)]
async fn race_loss_is_not_reissued_when_an_ancestor_times_out_during_loser_cleanup() {
    let plan = compile_source(
        r#"test "race requests" { timeout 20ms { race {
        browser { evaluate "winner" } browser { evaluate "loser" }
    } } }"#,
    );
    let state = Arc::new(LifecycleState::default());
    state.page_delays.lock().unwrap().extend([
        ("winner".into(), Duration::from_millis(10)),
        ("loser".into(), Duration::from_secs(10)),
    ]);
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(1, Duration::from_millis(100));
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state))
        .await;
    assert_requests(&result);
    let requests = requests(&result);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1.reason, CancellationReason::RaceLost);
    assert_eq!(requests[0].0.timestamp.elapsed, Duration::from_millis(10));
    assert_eq!(requests[1].1.reason, CancellationReason::Timeout);
    assert_eq!(requests[1].0.timestamp.elapsed, Duration::from_millis(20));
    assert_ne!(
        requests[0].0.metadata.execution_context.scope_id,
        requests[1].0.metadata.execution_context.scope_id
    );
    assert_eq!(
        result.tests[0].branches[1].scope.cancellation,
        Some(requests[0].1)
    );
    assert!(result.journal.last().unwrap().timestamp.elapsed >= Duration::from_millis(110));
}

struct CancelAt {
    at: tokio::time::Instant,
    reason: CancellationReason,
}
#[async_trait]
impl RunControl for CancelAt {
    fn is_cancelled(&self) -> bool {
        tokio::time::Instant::now() >= self.at
    }
    fn cancellation_reason(&self) -> CancellationReason {
        self.reason
    }
    async fn cancelled(&self) {
        tokio::time::sleep_until(self.at).await;
    }
    async fn before_step(&self, _: &PlannedTest, _: &PlannedStep) {}
}

#[tokio::test(start_paused = true)]
async fn run_control_requests_preserve_each_typed_reason_without_duplicating_propagation() {
    let plan = compile_source(
        r#"test "control" { parallel {
        browser { evaluate "slow" } browser { evaluate "slow" }
    } }"#,
    );
    for reason in [
        CancellationReason::UserCancelled,
        CancellationReason::DebugDisconnect,
        CancellationReason::FailFast,
        CancellationReason::RunnerShutdown,
        CancellationReason::ParentFailed,
        CancellationReason::RaceLost,
        CancellationReason::Timeout,
    ] {
        let state = Arc::new(LifecycleState::default());
        state
            .page_delays
            .lock()
            .unwrap()
            .insert("slow".into(), Duration::from_secs(10));
        let control = CancelAt {
            at: tokio::time::Instant::now() + Duration::from_millis(10),
            reason,
        };
        let result = Runner::new(Arc::default())
            .run_with_control(&plan, &LifecycleHost(state), Some(&control))
            .await;
        assert_requests(&result);
        let requests = requests(&result);
        assert_eq!(requests.len(), 1);
        let (record, cause) = requests[0];
        assert_eq!(cause.reason, reason);
        assert_eq!(
            record.metadata.execution_context.scope_id,
            Some(cause.causing_scope_id)
        );
        assert_eq!(record.metadata.execution_context.parent_scope_id, None);
        for branch in &result.tests[0].branches {
            assert_eq!(branch.scope.cancellation, Some(cause));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn deadline_request_during_retry_backoff_retains_the_lexical_owner_and_stops_attempts() {
    let plan = compile_source(
        r#"test "backoff" { timeout 10ms {
        retry 3 backoff 100ms max 100ms { expect 1 == 2 }
    } }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_requests(&result);
    let requests = requests(&result);
    assert_eq!(requests.len(), 1);
    let (record, cause) = requests[0];
    assert_eq!(cause.reason, CancellationReason::Timeout);
    assert_eq!(
        record.metadata.execution_context.scope_id,
        Some(cause.causing_scope_id)
    );
    assert_eq!(result.tests[0].branches.len(), 1);
    let attempt_finished = result
        .journal
        .iter()
        .find(|record| {
            matches!(&record.event,
                ExecutionEvent::Attempt { scope, .. } if scope.outcome.is_some()
            )
        })
        .unwrap();
    assert!(attempt_finished.identity.event_sequence < record.identity.event_sequence);
}

#[tokio::test(start_paused = true)]
async fn same_poll_primary_notice_does_not_request_cancellation_after_sibling_completion() {
    let plan = compile_source(
        r#"test "completed sibling" { parallel {
        browser { evaluate "fail" } server { expect 1 == 1 }
    } }"#,
    );
    let state = Arc::new(LifecycleState::default());
    state
        .page_errors
        .lock()
        .unwrap()
        .insert("fail".into(), BrowserError::BrowserDisconnected);
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(0, Duration::from_millis(100));
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state))
        .await;
    journal::assert_serialized_journal(&result);
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::Aborted { .. }
    ));
    assert!(matches!(
        result.tests[0].branches[1].outcome,
        TestOutcome::Passed
    ));
    assert!(
        requests(&result).is_empty(),
        "the successful sibling has already published its terminal"
    );
    assert_scopes_finish_once_after_children(&result.events);
}

#[tokio::test]
async fn race_request_inside_retry_keeps_its_attempt_identity_even_for_an_unpolled_loser() {
    let plan = compile_source(
        r#"test "attempt request" { retry 2 { race {
        server { expect 1 == 1 } server { expect 2 == 2 }
    } } }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert_requests(&result);
    let requests = requests(&result);
    assert_eq!(requests.len(), 1);
    let (request, cause) = requests[0];
    assert_eq!(cause.reason, CancellationReason::RaceLost);
    let attempt = result
        .journal
        .iter()
        .find_map(|record| match &record.event {
            ExecutionEvent::Attempt { scope, .. } if scope.outcome.is_none() => Some(scope),
            _ => None,
        })
        .unwrap();
    assert!(attempt.execution_context.attempt_id.is_some());
    assert_eq!(
        request.metadata.execution_context.attempt_id,
        attempt.execution_context.attempt_id
    );
    let target = request.metadata.execution_context.scope_id;
    let entry = result
        .journal
        .iter()
        .find(|record| {
            matches!(&record.event,
                ExecutionEvent::Scope { event, .. }
                    if Some(event.execution_context.scope_id) == target && event.outcome.is_none()
            )
        })
        .unwrap();
    assert!(
        request.identity.event_sequence < entry.identity.event_sequence,
        "request targets the explicitly allocated loser before its owned future is first polled"
    );
}
