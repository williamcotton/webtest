use super::*;
use webtest_plan::PlanNodeKind;

struct AttemptProvider {
    values: Mutex<VecDeque<Result<Value, ProviderError>>>,
    calls: Mutex<Vec<tokio::time::Instant>>,
}

impl AttemptProvider {
    fn new(values: impl IntoIterator<Item = Result<Value, ProviderError>>) -> Self {
        Self {
            values: Mutex::new(values.into_iter().collect()),
            calls: Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl ServerProvider for AttemptProvider {
    fn schema(&self) -> ProviderSchema {
        ProviderSchema {
            name: ProviderName("attempt".into()),
            operations: BTreeMap::from([(
                "value".into(),
                OperationSchema {
                    name: OperationName("value".into()),
                    parameters: vec![],
                    result: Type::Int,
                    capability: Capability::Server,
                    documentation: String::new(),
                    retry_safe: true,
                },
            )]),
            schema_identity: None,
        }
    }
    async fn call(&self, _: ProviderCall, _: CallContext) -> Result<ProviderResult, ProviderError> {
        self.calls.lock().unwrap().push(tokio::time::Instant::now());
        self.values
            .lock()
            .unwrap()
            .pop_front()
            .expect("bounded attempts")
            .map(|value| ProviderResult { value })
    }
}

fn provider_plan(source: &str, provider: Arc<AttemptProvider>) -> (TestPlan, ProviderRegistry) {
    let mut registry = ProviderRegistry::default();
    registry.register(provider);
    let mut db = webtest_analysis::AnalysisDatabase::with_provider_registry(registry.clone());
    let file = db.open_file("attempts.webtest", source);
    assert!(
        db.diagnostics(file).unwrap().is_empty(),
        "{:?}",
        db.diagnostics(file)
    );
    (db.test_plan(file).unwrap().as_ref().clone(), registry)
}

fn assertion_error(actual: &str) -> BrowserError {
    BrowserError::AssertionFailed {
        locator: Locator::Text("ready".into()),
        expected: LocatorState::Visible,
        actual: actual.into(),
    }
}

#[tokio::test(start_paused = true)]
async fn public_retry_lexical_contexts_coexist_with_an_enclosing_native_context() {
    let state = Arc::new(LifecycleState::default());
    state.record_waits.store(true, Ordering::SeqCst);
    state
        .locator_outcomes
        .lock()
        .unwrap()
        .extend([Err(assertion_error("first")), Ok(()), Ok(())]);
    let plan = compile_source(
        r#"test "distinct owners" {
        browser { open "https://example.test/before" }
        retry 3 { browser { expect text("ready").visible } }
        browser { expect text("after").visible }
    }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests[0].outcome);
    assert_eq!(result.tests[0].branches.len(), 2);
    let log = state.log();
    assert_eq!(
        log.iter()
            .filter(|entry| entry.starts_with("wait:"))
            .cloned()
            .collect::<Vec<_>>(),
        ["wait:1", "wait:2", "wait:0"]
    );
    for id in 0..3 {
        assert_eq!(
            log.iter()
                .filter(|entry| *entry == &format!("context_close:{id}"))
                .count(),
            1
        );
    }
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_exhaustion_keeps_all_attempts_and_does_not_backoff_after_the_last() {
    let plan = compile_source(
        r#"test "exhausted" { retry 3 backoff 10ms max 25ms { expect 1 == 2 } expect 9 == 9 }"#,
    );
    let observations = Arc::new(ObservationStore::default());
    let started = tokio::time::Instant::now();
    let result = Runner::new(observations.clone())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(started.elapsed(), Duration::from_millis(30));
    assert_eq!(result.failed(), 1);
    assert_eq!(result.tests[0].branches.len(), 3);
    assert!(
        result.tests[0]
            .branches
            .iter()
            .all(|attempt| matches!(attempt.outcome, TestOutcome::Failed(_)))
    );
    assert_eq!(
        observations
            .observations_for(plan.file, plan.source_revision)
            .len(),
        3
    );
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, ExecutionEvent::StepStarted { .. }))
            .count(),
        3
    );
    let registered: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Wait { event, .. }
                if event.kind == webtest_observation::WaitEventKind::Registered =>
            {
                Some(event.registration_id)
            }
            _ => None,
        })
        .collect();
    let terminal: Vec<_> = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Wait { event, .. }
                if event.kind != webtest_observation::WaitEventKind::Registered =>
            {
                Some(event.registration_id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(registered.len(), terminal.len());
    assert_eq!(registered, terminal.into_iter().collect());
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn nested_retry_attempt_identity_is_distinct_from_its_parent_and_static_path() {
    let provider = Arc::new(AttemptProvider::new(
        [1, 2, 3, 4, 5].map(|n| Ok(Value::Int(n))),
    ));
    let (plan, registry) = provider_plan(
        r#"test "nested attempts" { retry 3 backoff 10ms max 25ms { server {
        retry 3 backoff 10ms max 25ms { let inner = attempt.value() expect inner >= 2 }
        let outer = attempt.value()
        expect outer == 5
    } } }"#,
        provider,
    );
    let result = Runner::new(Arc::default())
        .with_provider_registry(registry)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests[0].outcome);
    let outer = &result.tests[0].branches;
    assert_eq!(outer.len(), 2);
    assert_eq!(outer[0].branches.len(), 2);
    assert_eq!(outer[1].branches.len(), 1);
    let ids: BTreeSet<_> = outer
        .iter()
        .chain(outer.iter().flat_map(|a| &a.branches))
        .map(|a| a.scope.execution_context.attempt_id.unwrap())
        .collect();
    assert_eq!(ids.len(), 5);
    assert_eq!(
        outer[0].branches[0].scope.execution_context.plan_node_id,
        outer[1].branches[0].scope.execution_context.plan_node_id
    );
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_cleanup_expiry_is_terminal_and_never_grants_another_attempt() {
    let state = Arc::new(LifecycleState::default());
    state.record_waits.store(true, Ordering::SeqCst);
    state
        .locator_outcomes
        .lock()
        .unwrap()
        .push_back(Err(assertion_error("primary")));
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(0, Duration::from_secs(1));
    let plan = compile_source(
        r#"test "cleanup expires" { parallel { retry 3 backoff 10ms max 25ms { browser { expect text("ready").visible } } } }"#,
    );
    let started = tokio::time::Instant::now();
    let result = Runner::new(Arc::default())
        .with_options(RunnerOptions {
            cleanup_timeout: Duration::from_millis(5),
            ..Default::default()
        })
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_eq!(started.elapsed(), Duration::from_millis(5));
    assert_eq!(result.tests[0].branches[0].branches.len(), 1);
    assert_eq!(state.next_context.load(Ordering::SeqCst), 1);
    assert!(
        matches!(&result.tests[0].branches[0].branches[0].outcome, TestOutcome::Aborted { prior_outcome: Some(prior), .. } if matches!(prior.as_ref(), PriorTestOutcome::Failed(_)))
    );
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_attempts_have_distinct_identity_fresh_bindings_and_capped_backoff() {
    let provider = Arc::new(AttemptProvider::new(
        [1, 2, 3, 4, 5].map(|n| Ok(Value::Int(n))),
    ));
    let (plan, registry) = provider_plan(
        r#"test "attempts" {
        let seed = 5
        retry 5 backoff 10ms max 25ms { server { let local = attempt.value() expect local == seed } }
        expect seed == 5
    }"#,
        provider.clone(),
    );
    let observations = Arc::new(ObservationStore::default());
    let started = tokio::time::Instant::now();
    let result = Runner::new(observations.clone())
        .with_provider_registry(registry)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests[0].outcome);
    assert_eq!(started.elapsed(), Duration::from_millis(80));
    assert_eq!(
        provider
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|at| at.duration_since(started).as_millis())
            .collect::<Vec<_>>(),
        [0, 10, 30, 55, 80]
    );
    assert_eq!(
        result.tests[0].bindings,
        BTreeMap::from([("seed".into(), Value::Int(5))])
    );
    let attempts = &result.tests[0].branches;
    assert_eq!(attempts.len(), 5);
    let ids: BTreeSet<_> = attempts
        .iter()
        .map(|a| a.scope.execution_context.attempt_id.unwrap())
        .collect();
    assert_eq!(ids.len(), 5);
    assert!(
        attempts[..4]
            .iter()
            .all(|a| matches!(a.outcome, TestOutcome::Failed(_)))
    );
    assert!(matches!(attempts[4].outcome, TestOutcome::Passed));
    assert!(
        attempts
            .iter()
            .all(|a| a.scope.execution_context.plan_node_id
                == attempts[0].scope.execution_context.plan_node_id)
    );
    let operations: Vec<_> = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Scope { event, .. }
                if event.outcome.is_none()
                    && event.execution_context.operation_execution_id.is_some()
                    && event.execution_context.attempt_id.is_some() =>
            {
                Some(&event.execution_context)
            }
            _ => None,
        })
        .collect();
    assert_eq!(operations.len(), 10);
    assert_eq!(
        operations
            .iter()
            .map(|e| e.operation_execution_id)
            .collect::<BTreeSet<_>>()
            .len(),
        10
    );
    assert!(
        operations
            .iter()
            .all(|e| ids.contains(&e.attempt_id.unwrap()))
    );
    assert!(
        observations
            .observations_for(plan.file, plan.source_revision)
            .is_empty()
    );
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_reacquires_lexical_contexts_only_after_terminal_teardown_and_backoff() {
    let state = Arc::new(LifecycleState::default());
    state.record_waits.store(true, Ordering::SeqCst);
    state.locator_outcomes.lock().unwrap().extend([
        Err(assertion_error("first")),
        Err(assertion_error("second")),
        Ok(()),
    ]);
    state.context_close_delays.lock().unwrap().extend([
        (0, Duration::from_millis(40)),
        (1, Duration::from_millis(50)),
    ]);
    let plan = compile_source(
        r#"test "fresh contexts" { retry 3 backoff 10ms max 25ms { browser { expect text("ready").visible } } }"#,
    );
    let started = tokio::time::Instant::now();
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests[0].outcome);
    assert_eq!(started.elapsed(), Duration::from_millis(120));
    let timings: Vec<_> = result
        .journal
        .iter()
        .filter_map(|record| match &record.event {
            ExecutionEvent::Attempt { scope, event, .. } => Some((
                event.ordinal,
                scope.outcome.is_some(),
                record.timestamp.elapsed.as_millis(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        timings,
        [
            (1, false, 0),
            (1, true, 40),
            (2, false, 50),
            (2, true, 100),
            (3, false, 120),
            (3, true, 120)
        ]
    );

    let log = state.log();
    assert!(
        log.iter()
            .position(|v| v == "context_close_done:0")
            .unwrap()
            < log.iter().position(|v| v == "context_create:1:1").unwrap()
    );
    assert!(
        log.iter()
            .position(|v| v == "context_close_done:1")
            .unwrap()
            < log.iter().position(|v| v == "context_create:2:2").unwrap()
    );
    for id in 0..3 {
        assert_eq!(
            log.iter()
                .filter(|v| *v == &format!("context_close:{id}"))
                .count(),
            1
        );
        assert_eq!(
            log.iter()
                .filter(|v| *v == &format!("session_close:{id}"))
                .count(),
            1
        );
    }
    let acquired: Vec<_> = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Resource { scope, event, .. }
                if event.kind == webtest_observation::ResourceEventKind::Ready =>
            {
                Some((
                    scope.execution_context.attempt_id,
                    event.resource.key.generation_id,
                ))
            }
            _ => None,
        })
        .collect();
    assert_eq!(acquired.len(), 3);
    assert_eq!(
        acquired
            .iter()
            .map(|(_, generation)| generation)
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    assert!(acquired.iter().all(|(attempt, _)| attempt.is_some()));
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_observations_can_reuse_an_enclosing_context_without_releasing_it() {
    let state = Arc::new(LifecycleState::default());
    state.record_waits.store(true, Ordering::SeqCst);
    state.locator_outcomes.lock().unwrap().extend([
        Err(assertion_error("not yet")),
        Ok(()),
        Ok(()),
    ]);
    let plan = compile_source(
        r#"test "same context" { browser {
        retry 3 backoff 10ms max 25ms { expect text("ready").visible }
        expect text("still ready").visible
    } }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests[0].outcome);
    assert_eq!(state.next_context.load(Ordering::SeqCst), 1);
    let log = state.log();
    assert_eq!(log.iter().filter(|v| *v == "wait:0").count(), 3);
    assert_eq!(log.iter().filter(|v| *v == "context_close:0").count(), 1);
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_cleanup_failure_stops_attempts_and_preserves_the_primary_failure() {
    let state = Arc::new(LifecycleState::default());
    state.record_waits.store(true, Ordering::SeqCst);
    state
        .locator_outcomes
        .lock()
        .unwrap()
        .push_back(Err(assertion_error("primary")));
    state.context_close_failures.lock().unwrap().insert(0);
    let plan = compile_source(
        r#"test "cleanup" { parallel { retry 3 backoff 10ms max 25ms { browser { expect text("ready").visible } } } }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_eq!(result.tests[0].branches[0].branches.len(), 1);
    assert_eq!(state.next_context.load(Ordering::SeqCst), 1);
    assert!(
        matches!(&result.tests[0].branches[0].branches[0].outcome, TestOutcome::Aborted { prior_outcome: Some(prior), .. } if matches!(prior.as_ref(), PriorTestOutcome::Failed(_)))
    );
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_deadline_cancels_registered_backoff_without_starting_another_attempt() {
    let plan = compile_source(
        r#"test "deadline" { timeout 5ms { retry 3 backoff 10ms max 25ms { expect 1 == 2 } } }"#,
    );
    let started = tokio::time::Instant::now();
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(started.elapsed(), Duration::from_millis(5));
    assert!(matches!(
        result.tests[0].outcome,
        TestOutcome::TimedOut { .. }
    ));
    assert_eq!(result.tests[0].branches.len(), 1);
    assert!(matches!(
        result.tests[0].branches[0].outcome,
        TestOutcome::Failed(_)
    ));
    assert!(result.events.iter().any(|event| matches!(event, ExecutionEvent::Wait { event, .. } if event.cancellation.is_some_and(|cause| cause.reason == CancellationReason::Timeout))));
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test(start_paused = true)]
async fn retry_only_recovers_provider_errors_explicitly_marked_retryable() {
    for (error, expected) in [
        (
            ProviderError::Application {
                code: "busy".into(),
                message: "try again".into(),
                retryable: true,
                data: serde_json::Value::Null,
            },
            2,
        ),
        (
            ProviderError::Application {
                code: "denied".into(),
                message: "stop".into(),
                retryable: false,
                data: serde_json::Value::Null,
            },
            1,
        ),
        (
            ProviderError::InvalidArgument {
                message: "bad request".into(),
            },
            1,
        ),
        (
            ProviderError::BridgeSchemaDrift {
                expected: "old".into(),
                live: "new".into(),
            },
            1,
        ),
        (
            ProviderError::BridgeHandshake {
                code: "authentication".into(),
                message: "denied".into(),
            },
            1,
        ),
        (
            ProviderError::HttpTransport {
                message: "disconnected".into(),
            },
            1,
        ),
    ] {
        let provider = Arc::new(AttemptProvider::new([Err(error), Ok(Value::Int(1))]));
        let (plan, registry) = provider_plan(
            r#"test "policy" { retry 3 backoff 10ms max 25ms { server { let value = attempt.value() } } }"#,
            provider.clone(),
        );
        let result = Runner::new(Arc::default())
            .with_provider_registry(registry)
            .run(&plan, &LifecycleHost(Arc::default()))
            .await;
        assert_eq!(provider.calls.lock().unwrap().len(), expected);
        assert_eq!(result.tests[0].branches.len(), expected);
        assert_eq!(result.passed(), usize::from(expected == 2));
        assert_scopes_finish_once_after_children(&result.events);
        assert_attempt_events(&result);
    }
}

#[tokio::test(start_paused = true)]
async fn retry_checks_every_parallel_failure_instead_of_only_the_severity_summary() {
    let plan = compile_source(
        r#"test "mixed failures" { retry 3 backoff 10ms max 25ms { parallel {
        server { expect 1 == 2 }
        server { let value = 1 / 0 }
    } } }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(result.tests[0].branches.len(), 1);
    let failures = &result.tests[0].branches[0].branches;
    assert_eq!(failures.len(), 2);
    assert!(
        matches!(&failures[0].outcome, TestOutcome::Failed(f) if matches!(f.error, StepError::Assertion(_)))
    );
    assert!(
        matches!(&failures[1].outcome, TestOutcome::Failed(f) if matches!(f.error, StepError::Evaluation(_)))
    );
}

#[tokio::test(start_paused = true)]
async fn retry_can_provide_only_its_successful_attempt_to_an_enclosing_race() {
    let provider = Arc::new(AttemptProvider::new([Ok(Value::Int(1)), Ok(Value::Int(2))]));
    let (plan, registry) = provider_plan(
        r#"test "provided" {
        let selected = race { server { retry 3 backoff 10ms max 25ms {
            let value = attempt.value()
            expect value == 2
            provide value
        } } }
        expect selected == 2
    }"#,
        provider,
    );
    let result = Runner::new(Arc::default())
        .with_provider_registry(registry)
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(result.passed(), 1, "{:?}", result.tests[0].outcome);
    assert_eq!(
        result.tests[0].bindings,
        BTreeMap::from([("selected".into(), Value::Int(2))])
    );
    assert_eq!(result.tests[0].branches[0].branches.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn retry_preserves_every_cancellation_reason_during_attempts_and_backoff() {
    struct CancelAt {
        at: tokio::time::Instant,
        reason: CancellationReason,
        cancelled: AtomicBool,
        hold_step: bool,
    }
    #[async_trait]
    impl RunControl for CancelAt {
        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
        fn cancellation_reason(&self) -> CancellationReason {
            self.reason
        }
        async fn cancelled(&self) {
            tokio::time::sleep_until(self.at).await;
            self.cancelled.store(true, Ordering::SeqCst);
        }
        async fn before_step(&self, _: &PlannedTest, _: &PlannedStep) {
            if self.hold_step {
                std::future::pending::<()>().await;
            }
        }
    }
    let plan =
        compile_source(r#"test "cancel" { retry 3 backoff 10ms max 25ms { expect 1 == 2 } }"#);
    for hold_step in [false, true] {
        for reason in [
            CancellationReason::ParentFailed,
            CancellationReason::RaceLost,
            CancellationReason::Timeout,
            CancellationReason::FailFast,
            CancellationReason::DebugDisconnect,
            CancellationReason::UserCancelled,
            CancellationReason::RunnerShutdown,
        ] {
            let started = tokio::time::Instant::now();
            let control = CancelAt {
                at: started + Duration::from_millis(5),
                reason,
                cancelled: AtomicBool::new(false),
                hold_step,
            };
            let result = Runner::new(Arc::default())
                .run_with_control(&plan, &LifecycleHost(Arc::default()), Some(&control))
                .await;
            assert_eq!(started.elapsed(), Duration::from_millis(5));
            assert_eq!(result.tests[0].branches.len(), 1);
            let attempt = &result.tests[0].branches[0];
            if hold_step {
                assert!(
                    matches!(attempt.outcome, TestOutcome::Cancelled { reason: actual } if actual == reason)
                );
                assert_eq!(attempt.scope.cancellation.unwrap().reason, reason);
            } else {
                assert!(matches!(attempt.outcome, TestOutcome::Failed(_)));
            }
            assert!(result.events.iter().any(|event| matches!(event,
                ExecutionEvent::Scope { event, .. } if event.outcome.is_some()
                    && event.execution_context.plan_node_id == match &plan.tests[0].body.kind { PlanNodeKind::Sequence { children } => children[0].id, _ => unreachable!() }
                    && event.cancellation.is_some_and(|cause| cause.reason == reason)
            )));
            assert_scopes_finish_once_after_children(&result.events);
            assert_attempt_events(&result);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn retry_infrastructure_failure_notifies_siblings_before_attempt_teardown() {
    let state = Arc::new(LifecycleState::default());
    state.page_creation_failures.lock().unwrap().insert(0);
    state
        .context_close_delays
        .lock()
        .unwrap()
        .insert(0, Duration::from_millis(100));
    state
        .page_delays
        .lock()
        .unwrap()
        .insert("slow".into(), Duration::from_secs(10));
    let plan = compile_source(
        r#"test "unhealthy attempt" { parallel {
        retry 3 backoff 10ms max 25ms { browser { expect text("ready").visible } }
        browser { evaluate "slow" }
    } }"#,
    );
    let started = tokio::time::Instant::now();
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert_eq!(started.elapsed(), Duration::from_millis(100));
    assert_eq!(result.tests[0].branches[0].branches.len(), 1);
    assert!(matches!(
        result.tests[0].branches[1].outcome,
        TestOutcome::Cancelled {
            reason: CancellationReason::ParentFailed
        }
    ));
    assert_scopes_finish_once_after_children(&result.events);
    assert_attempt_events(&result);
}

#[tokio::test]
async fn retry_preserves_separate_artifact_files_for_every_failed_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let state = Arc::new(LifecycleState::default());
    state.record_waits.store(true, Ordering::SeqCst);
    state.locator_outcomes.lock().unwrap().extend([
        Err(assertion_error("first")),
        Err(assertion_error("second")),
    ]);
    state.page_evidence.lock().unwrap().dom_snapshot = Some("<p>bounded evidence</p>".into());
    let plan = compile_source(
        r#"test "evidence" { retry 2 backoff 10ms max 25ms { browser { expect text("ready").visible } } }"#,
    );
    let result = Runner::new(Arc::default())
        .with_options(RunnerOptions {
            evidence: webtest_runtime::EvidenceOptions {
                artifact_directory: directory.path().to_owned(),
                dom_snapshot_on_failure: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .run(&plan, &LifecycleHost(state))
        .await;
    assert_eq!(result.failed(), 1);
    let attempts = &result.tests[0].branches;
    assert_eq!(attempts.len(), 2);
    let mut paths = BTreeSet::new();
    for attempt in attempts {
        let TestOutcome::Failed(failure) = &attempt.outcome else {
            panic!("{:?}", attempt.outcome);
        };
        assert_eq!(failure.artifacts.len(), 2);
        for artifact in &failure.artifacts {
            assert!(
                paths.insert(artifact.path.clone()),
                "attempt overwrote evidence"
            );
            assert!(artifact.path.is_file());
            let owner = format!(
                "execution-{}-attempt-{}",
                result.execution_id.0,
                attempt.scope.execution_context.attempt_id.unwrap().0
            );
            assert!(artifact.path.parent().unwrap().ends_with(owner));
        }
    }
}

/// Attempt facts surround the same explicit owned scope, including cancellation
/// and teardown failures. Descendants must not masquerade as extra attempts.
fn assert_attempt_events(result: &webtest_runtime::RunResult) {
    use webtest_observation::{AttemptEvent, EventMetadata, ReplayJournal, ScopeEvent};
    let mut replay = ReplayJournal::new(std::num::NonZeroUsize::new(result.journal.len()).unwrap());
    for record in result.journal.iter().rev() {
        replay.insert(record.clone()).unwrap();
    }
    let mut active = BTreeMap::new();
    let mut ordinals = BTreeMap::new();
    let mut completed = BTreeSet::new();
    for (index, record) in result.journal.iter().enumerate() {
        let ExecutionEvent::Attempt {
            scope,
            event: attempt,
            ..
        } = &record.event
        else {
            continue;
        };
        assert_eq!(record.metadata, EventMetadata::from(scope));
        assert_eq!(
            serde_json::from_value::<AttemptEvent>(serde_json::to_value(attempt).unwrap()).unwrap(),
            *attempt
        );
        let id = scope.execution_context.scope_id;
        assert!(scope.execution_context.attempt_id.is_some());
        if scope.outcome.is_none() {
            assert!(!completed.contains(&id));
            assert!(active.insert(id, (index, scope, attempt)).is_none());
            let ordinal = ordinals
                .entry(scope.execution_context.parent_scope_id.unwrap())
                .or_insert((0, attempt.max_attempts));
            ordinal.0 += 1;
            assert_eq!(*ordinal, (attempt.ordinal, attempt.max_attempts));
            assert!(attempt.ordinal <= attempt.max_attempts);
        } else {
            let (start, started, settings) = active.remove(&id).expect("attempt started once");
            assert_eq!(settings, attempt);
            assert_eq!(started.execution_context, scope.execution_context);
            assert_eq!(started.origin, scope.origin);
            assert!(completed.insert(id));
            let scopes: Vec<&ScopeEvent> = result.journal[start + 1..index]
                .iter()
                .filter_map(|record| match &record.event {
                    ExecutionEvent::Scope { event, .. }
                        if event.execution_context.scope_id == id =>
                    {
                        Some(event)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(scopes, [started, scope]);
            // Every fact belonging to this attempt's inherited identity, including
            // wait and resource release terminals, falls inside its lifecycle.
            for (position, fact) in result.journal.iter().enumerate() {
                if !matches!(fact.event, ExecutionEvent::Attempt { .. })
                    && fact.metadata.execution_context.attempt_id
                        == scope.execution_context.attempt_id
                {
                    assert!(
                        position > start && position < index,
                        "fact outside attempt lifecycle: {:?}",
                        fact.event
                    );
                }
            }
        }
    }
    assert!(
        active.is_empty(),
        "attempts missing terminal facts: {active:?}"
    );
    assert!(!completed.is_empty());
    fn verify_results(
        branches: &[webtest_runtime::BranchResult],
        completed: &mut BTreeSet<webtest_model::ExecutionScopeId>,
        events: &[ExecutionEvent],
    ) {
        for branch in branches {
            let context = &branch.scope.execution_context;
            let parent_attempt = events
                .iter()
                .find_map(|event| match event {
                    ExecutionEvent::Scope { event, .. }
                        if Some(event.execution_context.scope_id) == context.parent_scope_id =>
                    {
                        Some(event.execution_context.attempt_id)
                    }
                    _ => None,
                })
                .expect("explicit parent scope");
            if context.attempt_id.is_some() && context.attempt_id != parent_attempt {
                assert!(
                    completed.remove(&context.scope_id),
                    "attempt result missing lifecycle events"
                );
                let terminal = events
                    .iter()
                    .find_map(|event| match event {
                        ExecutionEvent::Attempt { scope, .. }
                            if scope.outcome.is_some()
                                && scope.execution_context.scope_id
                                    == branch.scope.execution_context.scope_id =>
                        {
                            Some(scope)
                        }
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(terminal, &branch.scope);
            }
            verify_results(&branch.branches, completed, events);
        }
    }
    for test in &result.tests {
        verify_results(&test.branches, &mut completed, &result.events);
    }
    assert!(
        completed.is_empty(),
        "attempt outcomes missing from results"
    );
}

#[tokio::test(start_paused = true)]
async fn concurrent_retries_keep_local_attempt_ordinals_despite_interleaved_completion() {
    let plan = compile_source(
        r#"test "interleaved attempts" { parallel {
        retry 2 backoff 20ms { expect 1 == 2 }
        retry 3 backoff 5ms { expect 3 == 4 }
    } }"#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert_eq!(result.failed(), 1);
    assert_eq!(result.tests[0].branches[0].branches.len(), 2);
    assert_eq!(result.tests[0].branches[1].branches.len(), 3);
    assert_attempt_events(&result);
    let mut starts = BTreeMap::<_, Vec<_>>::new();
    for record in &result.journal {
        if let ExecutionEvent::Attempt { scope, .. } = &record.event
            && scope.outcome.is_none()
        {
            starts
                .entry(scope.execution_context.parent_scope_id)
                .or_default()
                .push(record.timestamp.elapsed.as_millis());
        }
    }
    let mut timings: Vec<_> = starts.into_values().collect();
    timings.sort();
    assert_eq!(timings, [vec![0, 5, 10], vec![0, 20]]);
}
