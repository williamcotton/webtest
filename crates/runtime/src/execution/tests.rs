use super::*;
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Mutex};
use tokio::sync::Barrier;
use webtest_model::{BindingId, Value};
use webtest_observation::{ScopeEvent, ScopeOutcome};
use webtest_plan::PlanNodeKind;

struct SiblingBarrier {
    ready: Barrier,
    snapshots: Mutex<Vec<BTreeMap<String, Value>>>,
}

#[async_trait]
impl RunControl for SiblingBarrier {
    async fn before_step(&self, _: &PlannedTest, _: &PlannedStep) {}
    async fn before_step_with_bindings(
        &self,
        _: &PlannedTest,
        step: &PlannedStep,
        bindings: BTreeMap<String, Value>,
    ) {
        if matches!(step.operation, TestOperation::Assertion(_)) {
            let left = bindings.contains_key("left");
            self.snapshots.lock().unwrap().push(bindings);
            self.ready.wait().await;
            if left {
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn sibling_futures_have_independent_bindings_active_operations_and_explicit_parentage() {
    let mut database = webtest_analysis::AnalysisDatabase::default();
    let file = database.open_file(
        "siblings.webtest",
        r#"test "siblings" {
        let seed = 7
        timeout 10ms { server { let left = seed expect left == 7 } }
        timeout 100ms { server { let right = seed expect right == 7 } }
    }"#,
    );
    assert!(database.diagnostics(file).unwrap().is_empty());
    let plan = database.test_plan(file).unwrap();
    let test = &plan.tests[0];
    let PlanNodeKind::Sequence { children } = &test.body.kind else {
        panic!("sequence")
    };
    let options = RunnerOptions::default();
    let events = EventBuffer::default();
    let providers = ProviderRegistry::default();
    let resources = crate::ResourceRegistry::default();
    let waits = crate::WaitRegistry::default();
    let scopes = scopes::ScopeFactory::new(scopes::ExecutionIds::default(), test.id);
    let deadline = TestDeadline::new(options.test_timeout);
    let root = scopes.root(&test.body, deadline.at);
    let control = SiblingBarrier {
        ready: Barrier::new(2),
        snapshots: Mutex::default(),
    };
    struct NoBrowser;
    #[async_trait]
    impl BrowserHost for NoBrowser {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, webtest_browser::BrowserError> {
            panic!("pure branches do not acquire a browser")
        }
    }
    let observations = ObservationStore::default();
    let services = ExecutionServices {
        plan: &plan,
        observations: &observations,
        test,
        execution_id: ExecutionId(1),
        events: &events,
        event_sink: None,
        control: Some(&control),
        options: &options,
        providers: &providers,
        deadline: &deadline,
        waits: &waits,
        resources: &resources,
        scopes: &scopes,
        browser: &NoBrowser,
    };
    let mut parent = branch::BranchState::new(&options);
    parent
        .scopes
        .start(&root, services.execution_id, &events, None);
    let result = TreeExecution {
        services: &services,
        branch: &mut parent,
    }
    .node(&children[0], &root)
    .await;
    assert!(matches!(
        result,
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
    ));
    let mut left = parent.fork();
    let mut right = parent.fork();
    let mut left_execution = TreeExecution {
        services: &services,
        branch: &mut left,
    };
    let mut right_execution = TreeExecution {
        services: &services,
        branch: &mut right,
    };
    // The barrier requires both futures to advance; serial execution would hang.
    // Each borrows a different BranchState and only immutable services are shared.
    let (left_result, right_result) = tokio::join!(
        left_execution.node(&children[1], &root),
        right_execution.node(&children[2], &root),
    );
    assert!(matches!(
        left_result,
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut { .. })
    ));
    assert!(matches!(
        right_result,
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
    ));
    assert_ne!(left.active_step, right.active_step);
    assert_ne!(parent.active_step, left.active_step);
    assert!(root.context.cancellation.cause().is_none());
    assert!(left.scopes.subtree_ids(&root).is_empty());
    assert!(right.scopes.subtree_ids(&root).is_empty());
    assert!(left.cleanup_failures.is_empty() && right.cleanup_failures.is_empty());
    let snapshots = control.snapshots.lock().unwrap();
    assert_eq!(snapshots.len(), 2);
    assert!(
        snapshots
            .iter()
            .all(|bindings| bindings.get("seed") == Some(&Value::Int(7)))
    );
    assert!(
        snapshots
            .iter()
            .all(|bindings| bindings.contains_key("left") != bindings.contains_key("right"))
    );
    parent.scopes.finish(
        &root,
        ScopeOutcome::Failed,
        services.execution_id,
        &events,
        None,
    );
    let events = events.into_events();
    let facts: Vec<&ScopeEvent> = events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Scope { event, .. } => Some(event),
            _ => None,
        })
        .collect();
    let left = facts
        .iter()
        .find(|fact| fact.execution_context.task_path == vec![1] && fact.outcome.is_none())
        .unwrap();
    let right = facts
        .iter()
        .find(|fact| fact.execution_context.task_path == vec![2] && fact.outcome.is_none())
        .unwrap();
    assert_eq!(left.execution_context.parent_scope_id, Some(root.id()));
    assert_eq!(right.execution_context.parent_scope_id, Some(root.id()));
    assert_ne!(
        left.execution_context.scope_id,
        right.execution_context.scope_id
    );
    assert!(
        facts
            .iter()
            .filter(|fact| fact.execution_context.task_path.starts_with(&[2]))
            .all(|fact| fact.cancellation.is_none())
    );
    for started in facts.iter().filter(|fact| fact.outcome.is_none()) {
        assert_eq!(
            facts
                .iter()
                .filter(|fact| fact.execution_context.scope_id
                    == started.execution_context.scope_id
                    && fact.outcome.is_some())
                .count(),
            1
        );
    }
}

#[test]
fn sibling_snapshots_do_not_copy_native_handles_or_resource_ownership() {
    let options = RunnerOptions::default();
    let mut parent = branch::BranchState::new(&options);
    parent.bindings.bind(
        BindingId(0),
        Some("shared"),
        Value::List(vec![Value::Int(1)]),
    );
    let directory = Value::TempDirectory(options.project_root.join("owned"));
    parent
        .bindings
        .bind(BindingId(1), Some("directory"), directory.clone());
    parent.bindings.accept_provider_resources(&directory);
    parent.bindings.bind(
        BindingId(2),
        Some("nested_handle"),
        Value::List(vec![directory]),
    );
    let mut left = parent.fork();
    let right = parent.fork();
    left.bindings
        .bind(BindingId(0), Some("shared"), Value::Int(2));
    assert_eq!(
        right.bindings.environment().get(&BindingId(0)),
        Some(&Value::List(vec![Value::Int(1)]))
    );
    assert_eq!(
        parent.bindings.environment().get(&BindingId(0)),
        right.bindings.environment().get(&BindingId(0))
    );
    assert_eq!(left.bindings.environment().len(), 1);
    assert_eq!(right.bindings.environment().len(), 1);
    assert!(left.bindings.temporary_directories().is_empty());
    assert!(right.bindings.temporary_directories().is_empty());
    assert_eq!(parent.bindings.temporary_directories().len(), 1);
    assert!(left.page.is_none() && right.page.is_none());
}
