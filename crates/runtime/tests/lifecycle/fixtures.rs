use super::*;

#[tokio::test]
async fn shared_fixture_setup_is_cached_and_teardown_keeps_setup_locals() {
    let plan = compile_source(
        r#"
        fixture seed(value: Int) -> Int scope suite shareable {
            setup { let saved = value + 1 provide saved }
            teardown { expect saved == value + 1 }
        }
        test "first" { let value = use seed(7) expect value == 8 }
        test "second" { let value = use seed(7) expect value == 8 }
    "#,
    );
    let host = LifecycleHost(Arc::default());
    let result = Runner::new(Arc::default()).run(&plan, &host).await;
    assert!(
        matches!(result.outcome, RunOutcome::Completed),
        "{:?}",
        result.outcome
    );
    assert!(
        result
            .tests
            .iter()
            .all(|test| matches!(test.outcome, TestOutcome::Passed)),
        "{:?}",
        result.tests
    );
    let starts = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Scope { event, .. }
                if event.outcome.is_none() && event.execution_context.task_path.is_empty() =>
            {
                event.execution_context.fixture
            }
            _ => None,
        })
        .filter(|fixture| fixture.phase == webtest_observation::FixturePhase::Lifetime)
        .count();
    assert_eq!(starts, 1);
    let acquisitions = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::FixtureAcquired {
                fixture,
                acquisition_execution_id,
                test_id,
                ..
            } => {
                assert!(
                    test_id.is_some(),
                    "consumption retains its actual test identity"
                );
                Some((fixture.instance, *acquisition_execution_id))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(acquisitions.len(), 2);
    assert_eq!(
        acquisitions[0], acquisitions[1],
        "cached consumers link to one acquisition"
    );

    let mut replay =
        webtest_observation::ReplayJournal::new(std::num::NonZeroUsize::new(1000).unwrap());
    for event in &result.journal {
        replay.insert(event.clone()).unwrap();
    }
}

#[tokio::test]
async fn fixture_setup_failure_is_projected_to_each_consumer_without_fabricated_test_ids() {
    let plan = compile_source(
        r#"
        fixture seed() -> Int scope suite shareable {
            setup { expect 1 == 2 provide 7 }
        }
        test "first" { let value = use seed() }
        test "second" { let value = use seed() }
    "#,
    );
    let host = LifecycleHost(Arc::default());
    let result = Runner::new(Arc::default()).run(&plan, &host).await;
    assert!(
        result
            .tests
            .iter()
            .all(|test| matches!(test.outcome, TestOutcome::Failed(_))),
        "{:?}",
        result.tests
    );
    for test in &result.tests {
        let TestOutcome::Failed(failure) = &test.outcome else {
            unreachable!()
        };
        assert!(matches!(
            failure.step.operation,
            TestOperation::AcquireFixture(_)
        ));
        assert!(!failure.step.origin_chain.is_empty());
    }
    assert!(
        result
            .journal
            .iter()
            .map(|record| &record.metadata.execution_context)
            .filter(|context| context.fixture.is_some())
            .all(|context| context.test_id.is_none() && context.test_execution_id.is_none())
    );
}

#[tokio::test]
async fn shared_fixture_teardown_failure_aborts_the_run_after_passing_tests() {
    let plan = compile_source(
        r#"
        fixture seed() -> Int scope file shareable {
            setup { provide 7 }
            teardown { expect 1 == 2 }
        }
        test "first" { let value = use seed() expect value == 7 }
    "#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(matches!(result.tests[0].outcome, TestOutcome::Passed));
    assert!(
        matches!(
            result.outcome,
            RunOutcome::Aborted {
                failure: RunError::Cleanup(CleanupFailure {
                    resource: CleanupResource::Fixture(_),
                    cause: CleanupCause::Runtime { .. }
                }),
                ..
            }
        ),
        "{:?}",
        result.outcome
    );
}

#[tokio::test]
async fn fixture_browser_context_is_owned_through_teardown_and_not_borrowed_from_consumer() {
    let plan = compile_source(
        r#"
        fixture page_value() -> Int scope test {
            setup browser { evaluate "setup" provide 7 }
            teardown browser { evaluate "teardown" }
        }
        test "consumer" { let value = use page_value() browser { evaluate "consumer" } }
    "#,
    );
    let state = Arc::new(LifecycleState::default());
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert!(
        matches!(result.tests[0].outcome, TestOutcome::Passed),
        "{:?}\n{:?}\n{:?}",
        result.tests,
        state.log(),
        result.events
    );
    let log = state.log();
    assert!(log.iter().any(|event| event.ends_with(":setup")), "{log:?}");
    assert!(
        log.iter().any(|event| event.ends_with(":teardown")),
        "{log:?}"
    );
    assert!(
        log.iter().any(|event| event.ends_with(":consumer")),
        "{log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|event| event.starts_with("context_close:"))
            .count(),
        2,
        "{log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|event| event.starts_with("session_close:"))
            .count(),
        2,
        "{log:?}"
    );
}

#[tokio::test]
async fn test_fixture_teardown_failure_retains_the_primary_assertion() {
    let plan = compile_source(
        r#"
        fixture value() -> Int scope test { setup { provide 7 } teardown { expect 2 == 3 } }
        test "consumer" { let value = use value() expect value == 8 }
    "#,
    );
    let result = Runner::new(Arc::default())
        .run(&plan, &LifecycleHost(Arc::default()))
        .await;
    assert!(
        matches!(
            result.tests[0].outcome,
            TestOutcome::Aborted {
                prior_outcome: Some(_),
                ..
            }
        ),
        "{:?}",
        result.tests
    );
}

#[tokio::test]
async fn concurrent_files_share_one_suite_fixture_and_close_file_fixtures_first() {
    let mut db = webtest_analysis::AnalysisDatabase::default();
    db.configure_workspace(webtest_analysis::WorkspaceConfiguration {
        root: "/workspace".into(),
        ..Default::default()
    });
    db.open_file("/workspace/fixtures.webtest", r#"
        export fixture seed() -> Int scope suite shareable { setup { provide 7 } teardown { expect 1 == 1 } }
        export fixture local() -> Int scope file shareable { setup { let seed = use seed() provide seed } teardown { expect 2 == 2 } }
    "#);
    let a = db.open_file(
        "/workspace/a.webtest",
        "import { local } from \"./fixtures\" test \"a\" { let n = use local() expect n == 7 }",
    );
    let b = db.open_file(
        "/workspace/b.webtest",
        "import { local } from \"./fixtures\" test \"b\" { let n = use local() expect n == 7 }",
    );
    let plans = [a, b].map(|file| {
        assert!(
            db.diagnostics(file).unwrap().is_empty(),
            "{:?}",
            db.diagnostics(file)
        );
        db.test_plan(file).unwrap()
    });
    let runner = Runner::new(Arc::default());
    let host = LifecycleHost(Arc::default());
    let inputs = plans
        .iter()
        .map(|plan| webtest_runtime::TestRun {
            runner: &runner,
            plan,
            browser: &host,
            control: None,
        })
        .collect::<Vec<_>>();
    let results =
        webtest_runtime::run_jobs(&inputs, webtest_runtime::JobLimit::new(2).unwrap()).await;
    assert!(
        results.iter().all(|result| result.passed() == 1),
        "{results:?}"
    );
    let fixtures = results
        .iter()
        .flat_map(|result| &result.events)
        .filter_map(|event| match event {
            ExecutionEvent::Scope { event, .. }
                if event.outcome.is_none() && event.execution_context.task_path.is_empty() =>
            {
                event.execution_context.fixture
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        fixtures
            .iter()
            .filter(|fixture| matches!(
                fixture.owner,
                webtest_observation::FixtureOwnerKey::Suite { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        fixtures
            .iter()
            .filter(|fixture| matches!(
                fixture.owner,
                webtest_observation::FixtureOwnerKey::File { .. }
            ))
            .count(),
        2
    );
}

#[tokio::test]
async fn retry_attempts_reacquire_test_fixtures_and_reuse_shared_fixtures() {
    for (lifetime, count) in [("test", 2), ("file shareable", 1)] {
        let source = format!(
            r#"
            fixture seed() -> Int scope {lifetime} {{ setup {{ provide 7 }} teardown {{ expect 1 == 1 }} }}
            test "retry" {{ retry 2 {{ let seed = use seed() browser {{ expect text("ready").visible }} }} }}
        "#
        );
        let plan = compile_source(&source);
        let state = Arc::new(LifecycleState::default());
        state.record_waits.store(true, Ordering::SeqCst);
        state.locator_outcomes.lock().unwrap().extend([
            Err(BrowserError::AssertionFailed {
                locator: Locator::Text("ready".into()),
                expected: LocatorState::Visible,
                actual: "first".into(),
            }),
            Ok(()),
        ]);
        let result = Runner::new(Arc::default())
            .run(&plan, &LifecycleHost(state))
            .await;
        assert_eq!(result.passed(), 1, "{:?}", result.tests);
        let starts = result.events.iter().filter(|event| matches!(event, ExecutionEvent::Scope { event, .. } if event.outcome.is_none() && event.execution_context.task_path.is_empty() && event.execution_context.fixture.is_some())).count();
        assert_eq!(starts, count);
    }
}

#[tokio::test]
async fn test_fixture_keeps_native_temporary_directory_alive_until_its_teardown() {
    let root = tempfile::tempdir().unwrap();
    let plan = compile_source(
        r#"
        fixture directory() -> TempDirectory scope test {
            setup server { let directory = fs.temp_dir() provide directory }
            teardown server { expect 1 == 1 }
        }
        test "consumer" { let directory = use directory() expect 1 == 1 }
    "#,
    );
    struct CheckAlive(PathBuf, AtomicUsize);
    impl CheckAlive {
        fn check(&self) {
            assert_eq!(
                std::fs::read_dir(self.0.join(".webtest/tmp"))
                    .unwrap()
                    .count(),
                1
            );
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl RunControl for CheckAlive {
        async fn before_step(&self, _: &PlannedTest, step: &PlannedStep) {
            if matches!(step.operation, TestOperation::Assertion(_)) {
                self.check();
            }
        }
        async fn before_fixture_step(
            &self,
            _: &webtest_plan::FixtureDefinition,
            step: &PlannedStep,
            _: &webtest_runtime::DebugContext,
            _: Option<BTreeMap<String, Value>>,
        ) {
            if matches!(step.operation, TestOperation::Assertion(_)) {
                self.check();
            }
        }
    }
    let control = CheckAlive(root.path().into(), AtomicUsize::new(0));
    let result = Runner::new(Arc::default())
        .with_options(RunnerOptions {
            project_root: root.path().into(),
            ..Default::default()
        })
        .run_with_control(&plan, &LifecycleHost(Arc::default()), Some(&control))
        .await;
    assert_eq!(control.1.load(Ordering::SeqCst), 2);
    assert_eq!(result.passed(), 1, "{:?}", result.tests);
    assert_eq!(
        std::fs::read_dir(root.path().join(".webtest/tmp"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test(start_paused = true)]
async fn timed_out_fixture_setup_is_joined_and_its_partial_browser_is_closed() {
    let plan = compile_source(
        r#"
        fixture page_value() -> Int scope test {
            setup browser { evaluate "slow" provide 7 }
            teardown browser { evaluate "must-not-run" }
        }
        test "consumer" { let value = use page_value() }
    "#,
    );
    let state = Arc::new(LifecycleState::default());
    state
        .page_delays
        .lock()
        .unwrap()
        .insert("slow".into(), Duration::from_secs(100));
    let result = Runner::new(Arc::default())
        .with_options(RunnerOptions {
            test_timeout: Duration::from_millis(10),
            cleanup_timeout: Duration::from_secs(1),
            ..Default::default()
        })
        .run(&plan, &LifecycleHost(state.clone()))
        .await;
    assert!(
        matches!(result.tests[0].outcome, TestOutcome::TimedOut { .. }),
        "{:?}",
        result.tests
    );
    let log = state.log();
    assert!(
        log.iter().any(|event| event.starts_with("context_close:")),
        "{log:?}"
    );
    assert!(
        log.iter().any(|event| event.starts_with("session_close:")),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|event| event.contains("must-not-run")),
        "{log:?}"
    );
}
