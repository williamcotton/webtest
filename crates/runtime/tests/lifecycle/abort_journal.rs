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
use super::*;
use webtest_observation::{ExecutionFailure, PriorScopeOutcome};

fn assert_terminal_failure(result: &webtest_runtime::RunResult) {
    journal::assert_serialized_journal(result);
    let RunOutcome::Aborted { failure, .. } = &result.outcome else {
        panic!("aborted run")
    };
    let ExecutionEvent::RunFinished {
        failure: Some(fact),
        failure_class,
        ..
    } = &result.events.last().unwrap()
    else {
        panic!("typed terminal failure")
    };
    assert_eq!(**fact, failure.to_execution_failure());
    assert_eq!(*failure_class, Some(fact.failure_class()));
}

#[tokio::test]
async fn launch_failure_is_replayable_before_scope_and_test_terminal_records() {
    let plan = compile_source(r#"test "launch" { browser { open "/" } }"#);
    let result = Runner::new(Arc::default())
        .run(&plan, &FailingStartHost)
        .await;
    assert_terminal_failure(&result);
    let (position, abort) = result
        .journal
        .iter()
        .enumerate()
        .find(|(_, r)| matches!(r.event, ExecutionEvent::ScopeAborted { .. }))
        .unwrap();
    let ExecutionEvent::ScopeAborted {
        scope,
        failure,
        prior_outcome,
        ..
    } = &abort.event
    else {
        unreachable!()
    };
    assert_eq!(
        **failure,
        ExecutionFailure::Browser(BrowserError::Launch("cannot launch".into()))
    );
    assert!(prior_outcome.is_none());
    assert_eq!(abort.metadata.origin, Some(plan.tests[0].body.origin));
    assert_eq!(abort.metadata.source_revision, Some(plan.source_revision));
    let scope_id = scope.execution_context.scope_id;
    assert!(
        result
            .journal
            .iter()
            .skip(position + 1)
            .any(|r| matches!(&r.event,
        ExecutionEvent::Scope { event, .. } if event.execution_context.scope_id == scope_id
        && event.outcome.is_some()))
    );
    assert!(
        result
            .journal
            .iter()
            .skip(position + 1)
            .any(|r| matches!(r.event, ExecutionEvent::TestFinished { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn sibling_cleanup_aborts_remain_distinct_and_keep_source_parentage() {
    let plan = compile_source(
        r#"test "siblings" { parallel {
        browser { evaluate "one" } browser { evaluate "two" }
    } }"#,
    );
    let state = Arc::new(LifecycleState::default());
    state.page_delays.lock().unwrap().extend([
        ("one".into(), Duration::from_millis(10)),
        ("two".into(), Duration::from_millis(10)),
    ]);
    state.context_close_failures.lock().unwrap().extend([0, 1]);
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_terminal_failure(&result);
    assert_eq!(result.tests[0].branches.len(), 2);
    for branch in &result.tests[0].branches {
        let TestOutcome::Aborted { failure, .. } = &branch.outcome else {
            panic!("child abort")
        };
        let records: Vec<_> = result
            .journal
            .iter()
            .filter(|r| {
                matches!(&r.event,
            ExecutionEvent::ScopeAborted { scope, .. } if
            scope.execution_context.scope_id == branch.scope.execution_context.scope_id)
            })
            .collect();
        assert_eq!(records.len(), 1);
        let ExecutionEvent::ScopeAborted { failure: fact, .. } = &records[0].event else {
            unreachable!()
        };
        assert_eq!(**fact, failure.to_execution_failure());
        assert_eq!(
            records[0].metadata.execution_context.parent_scope_id,
            branch.scope.execution_context.parent_scope_id
        );
        assert_eq!(
            records[0].metadata.execution_context.task_path,
            Some(branch.scope.execution_context.task_path.clone())
        );
    }
    assert_eq!(
        state
            .log()
            .iter()
            .filter(|s| s.starts_with("context_close:"))
            .count(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn cleanup_abort_references_the_original_assertion_or_timeout_without_erasing_it() {
    for source in [
        r#"test "assertion" { browser { evaluate "ok" expect 1 == 2 } }"#,
        r#"test "timeout" { browser { evaluate "slow" } }"#,
    ] {
        let plan = compile_source(source);
        let state = Arc::new(LifecycleState::default());
        state.context_close_failures.lock().unwrap().insert(0);
        state
            .page_delays
            .lock()
            .unwrap()
            .insert("slow".into(), Duration::from_secs(1));
        let runner = Runner::new(Arc::default()).with_options(RunnerOptions {
            test_timeout: Duration::from_millis(10),
            ..Default::default()
        });
        let result = runner.run(&plan, &LifecycleHost(state)).await;
        assert_terminal_failure(&result);
        let abort = result
            .events
            .iter()
            .find_map(|event| match event {
                ExecutionEvent::ScopeAborted {
                    prior_outcome: Some(prior),
                    ..
                } => Some(prior),
                _ => None,
            })
            .unwrap();
        if source.contains("assertion") {
            let PriorScopeOutcome::Failed {
                step_id,
                origin,
                failure,
            } = abort
            else {
                panic!("original assertion")
            };
            assert_eq!(
                *origin,
                plan.tests[0]
                    .steps()
                    .into_iter()
                    .find(|step| step.id == *step_id)
                    .unwrap()
                    .origin
            );
            assert_eq!(
                failure.code(),
                webtest_observation::RuntimeFailureCode::AssertionFailed
            );

            assert!(result.events.iter().any(|event| matches!(event,
                ExecutionEvent::StepFailed { step_id: id, failure: fact, .. } if id == step_id && fact == failure.as_ref())));
        } else {
            assert!(matches!(
                abort,
                PriorScopeOutcome::TimedOut { timeout_ms: 10, .. }
            ));
        }
    }
}

#[tokio::test]
async fn invalid_plan_records_internal_failure_without_starting_a_scope() {
    let mut plan = compile_source(r#"test "invalid" {}"#);
    plan.tests[0].body.source_revision = SourceRevision::of("drift");
    let state = Arc::new(LifecycleState::default());
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_terminal_failure(&result);
    assert!(matches!(&result.events.last().unwrap(),
        ExecutionEvent::RunFinished { failure: Some(fact), .. }
            if matches!(**fact, ExecutionFailure::Internal { .. })));
    assert!(state.log().is_empty());
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, ExecutionEvent::ScopeAborted { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_run_terminal_preserves_fail_fast_before_and_after_cleanup_abort() {
    let plan = compile_source(r#"test "cancel" { browser { evaluate "slow" } }"#);
    for cleanup_fails in [false, true] {
        let state = Arc::new(LifecycleState::default());
        state
            .page_delays
            .lock()
            .unwrap()
            .insert("slow".into(), Duration::from_secs(1));
        if cleanup_fails {
            state.context_close_failures.lock().unwrap().insert(0);
        }
        let control = CancelAt(tokio::time::Instant::now() + Duration::from_millis(10));
        let result = Runner::new(Arc::default())
            .run_with_control(&plan, &LifecycleHost(state), Some(&control))
            .await;
        journal::assert_serialized_journal(&result);
        assert!(matches!(
            result.events.last().unwrap(),
            ExecutionEvent::RunFinished {
                cancellation: Some(CancellationReason::FailFast),
                ..
            }
        ));
        if cleanup_fails {
            assert_terminal_failure(&result);
            assert!(result.events.iter().any(|event| matches!(
                event,
                ExecutionEvent::ScopeAborted {
                    prior_outcome: Some(PriorScopeOutcome::Cancelled {
                        reason: CancellationReason::FailFast
                    }),
                    ..
                }
            )));
        }
    }
}

#[tokio::test]
async fn abort_publication_uses_branch_known_secrets_after_context_teardown() {
    let mut providers = ProviderRegistry::default();
    providers.register(Arc::new(RecordingProvider::new(Ok(Value::Record(
        BTreeMap::new(),
    )))));
    let plan = plan_with_tests(
        vec![Capability::Server, Capability::Browser],
        vec![vec![
            secret_binding(),
            provider_operation(None),
            browser_evaluate("ok"),
        ]],
    );
    let state = Arc::new(LifecycleState::default());
    state.context_close_errors.lock().unwrap().insert(
        0,
        BrowserError::Protocol {
            method: "dispose private".into(),
            message: "cleanup private".into(),
        },
    );
    let result = Runner::new(Arc::default())
        .with_provider_registry(providers)
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_terminal_failure(&result);
    let wire = serde_json::to_string(&result.journal).unwrap();
    assert!(!wire.contains("private"), "{wire}");
    assert!(wire.contains("[redacted]"));
    assert!(!format!("{result:?}").contains("private"));
    assert!(state.log().contains(&"context_close:0".into()));
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_failed_assertion_cleanup_survives_run_summary_promotion() {
    let plan = compile_source(r#"test "prior" { browser { evaluate "ok" expect 1 == 2 } }"#);
    let state = Arc::new(LifecycleState::default());
    state.context_close_failures.lock().unwrap().insert(0);
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(0, Duration::from_millis(20));
    let control = CancelAt(tokio::time::Instant::now() + Duration::from_millis(10));
    let result = Runner::new(Arc::default())
        .run_with_control(&plan, &LifecycleHost(state), Some(&control))
        .await;
    assert_terminal_failure(&result);
    assert!(matches!(
        result.events.last().unwrap(),
        ExecutionEvent::RunFinished {
            cancellation: Some(CancellationReason::FailFast),
            ..
        }
    ));
    assert!(result.events.iter().any(|event| matches!(
        event,
        ExecutionEvent::ScopeAborted {
            prior_outcome: Some(PriorScopeOutcome::Failed { .. }),
            ..
        }
    )));
}
