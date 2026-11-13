use super::*;

struct HelperStops(Mutex<Vec<(String, BTreeMap<String, Value>)>>);
#[async_trait]
impl RunControl for HelperStops {
    async fn before_step(&self, _: &PlannedTest, _: &PlannedStep) {}
    async fn before_step_in_context(
        &self,
        _: &PlannedTest,
        step: &PlannedStep,
        context: &webtest_runtime::DebugContext,
        bindings: Option<BTreeMap<String, Value>>,
    ) {
        if matches!(step.operation, TestOperation::Return(_)) {
            let helper = context
                .frames
                .iter()
                .find(|frame| frame.kind == webtest_runtime::DebugScopeKind::Helper)
                .unwrap();
            self.0
                .lock()
                .unwrap()
                .push((helper.display_name.clone().unwrap(), bindings.unwrap()));
        }
    }
}
#[tokio::test]
async fn pure_helpers_have_explicit_safe_points_without_changing_lazy_or_error_order() {
    let plan = compile_source(
        r#"
        fn probe() -> Bool = true
        fn next(value: Int) -> Int = value + 1
        test "and skipped" { let value = false && probe() expect value == false }
        test "or skipped" { let value = true || probe() expect value == true }
        test "and called" { let value = true && probe() expect value == true }
        test "nested" { let value = next(next(1)) expect value == 3 }
        test "earlier failure" { let value = (1 / 0) + next(1) }
    "#,
    );
    plan.validate_tree().unwrap();
    let stops = HelperStops(Mutex::default());
    let result = Runner::new(Arc::default())
        .run_with_control(&plan, &LifecycleHost(Arc::default()), Some(&stops))
        .await;
    assert_eq!(result.passed(), 4, "{:?}", result.tests);
    assert_eq!(result.failed(), 1);
    let stops = stops.0.lock().unwrap();
    assert_eq!(stops.len(), 3, "{stops:?}");
    assert_eq!(stops[1].1.get("value"), Some(&Value::Int(1)));
    assert_eq!(stops[2].1.get("value"), Some(&Value::Int(2)));
    assert!(
        stops
            .iter()
            .flat_map(|(_, bindings)| bindings.keys())
            .all(|name| !name.starts_with('$'))
    );
}
