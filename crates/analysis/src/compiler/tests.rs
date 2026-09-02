use std::collections::BTreeMap;

use crate::{AnalysisDatabase, Diagnostic};
use webtest_feedback::RepairReplacement;
use webtest_hir::{BinaryOperator, BindingId, StepId};
use webtest_plan::{EvaluatePureOperation, PlanExpr, TestOperation, TestPlan};
use webtest_provider::{
    Capability, OperationName, OperationSchema, ParameterSchema, ProviderName, ProviderRegistry,
    ProviderSchema, RecordField, Type,
};

fn analyze(source: &str) -> (Vec<Diagnostic>, TestPlan) {
    let mut database = AnalysisDatabase::default();
    let file = database.open_file("file:///test.webtest", source);
    (
        database
            .diagnostics(file)
            .expect("diagnostics")
            .as_ref()
            .clone(),
        database.test_plan(file).expect("plan").as_ref().clone(),
    )
}

fn analyze_with_registry(source: &str, providers: ProviderRegistry) -> (Vec<Diagnostic>, TestPlan) {
    let mut database = AnalysisDatabase::with_provider_registry(providers);
    let file = database.open_file("file:///test.webtest", source);
    (
        database
            .diagnostics(file)
            .expect("diagnostics")
            .as_ref()
            .clone(),
        database.test_plan(file).expect("plan").as_ref().clone(),
    )
}

#[test]
fn compiles_typed_server_to_browser_flow() {
    let source = r#"test "created user can sign in" {
        server {
            let response = http.post("/api/test/users", json: { email: "alice@example.com" })
            expect response.status == 201
            let user: { id: Int, email: String } = response.json
        }
        browser {
            open "/login"
            fill label("Email") with user.email
            click role("button", name: "Sign in")
            expect text("Welcome").visible
        }
    }"#;
    let (diagnostics, plan) = analyze(source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert!(matches!(
        plan.tests[0].steps[0].operation,
        TestOperation::ServerProviderCall(_)
    ));
    assert!(matches!(
        plan.tests[0].steps[2].operation,
        TestOperation::EvaluatePure(EvaluatePureOperation {
            expression: PlanExpr::Decode { .. },
            ..
        })
    ));
    assert_eq!(
        plan.required_host_capabilities,
        vec![Capability::Server, Capability::Browser, Capability::Test]
    );
    assert_eq!(
        plan.tests[0].required_host_capabilities,
        vec![Capability::Server, Capability::Browser, Capability::Test]
    );
    assert!(plan.validate_capabilities().is_ok());
}

#[test]
fn optional_assignments_and_member_access_lower_the_runtime_presence_fact() {
    let source = r#"test "optional values" {
        let literal: { required: String, optional?: String, nullable: Option<String> } = {
            required: "hello",
            nullable: null,
        }
        expect literal.optional == null
        expect literal.nullable == null
        let present: Option<String> = "hello"
        expect present == "hello"
    }"#;
    let (diagnostics, plan) = analyze(source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let TestOperation::Assertion(assertion) = &plan.tests[0].steps[1].operation else {
        panic!("optional member assertion")
    };
    let webtest_plan::AssertionOperation::Value { actual, .. } = assertion else {
        panic!("value assertion")
    };
    assert!(matches!(
        actual,
        PlanExpr::Member {
            member,
            missing_is_null: true,
            ..
        } if member == "optional"
    ));
    let TestOperation::Assertion(assertion) = &plan.tests[0].steps[2].operation else {
        panic!("required nullable member assertion")
    };
    let webtest_plan::AssertionOperation::Value { actual, .. } = assertion else {
        panic!("value assertion")
    };
    assert!(matches!(
        actual,
        PlanExpr::Member {
            member,
            missing_is_null: false,
            ..
        } if member == "nullable"
    ));
}

#[test]
fn provider_optional_results_lower_presence_but_required_records_reject_optional_fields() {
    let result = Type::Record(BTreeMap::from([(
        "nickname".into(),
        RecordField {
            ty: Type::String,
            optional: true,
            documentation: String::new(),
            secret: false,
        },
    )]));
    let schema = ProviderSchema {
        name: ProviderName("app".into()),
        operations: BTreeMap::from([(
            "fetch".into(),
            OperationSchema {
                name: OperationName("fetch".into()),
                parameters: Vec::new(),
                result,
                capability: Capability::Server,
                documentation: String::new(),
                retry_safe: false,
            },
        )]),
        schema_identity: Some("schema:optional".into()),
    };
    let mut providers = ProviderRegistry::built_in_schemas();
    providers.register_schema(schema);
    let (diagnostics, plan) = analyze_with_registry(
        r#"test "provider optional" { server {
            let user = app.fetch()
            expect user.nickname == null
        } }"#,
        providers,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let TestOperation::Assertion(webtest_plan::AssertionOperation::Value { actual, .. }) =
        &plan.tests[0].steps[1].operation
    else {
        panic!("provider optional assertion")
    };
    assert!(matches!(
        actual,
        PlanExpr::Member {
            missing_is_null: true,
            ..
        }
    ));

    let (diagnostics, _) = analyze(
        r#"test "presence mismatch" {
            let optional: { name?: String } = {}
            let required: { name: String } = optional
        }"#,
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "semantic.type_mismatch")
    );
}

#[test]
fn capabilities_are_exact_per_test_and_the_plan_is_their_sorted_union() {
    let source = r#"test "server" {
    server { let response = http.get("http://example.test") }
}
test "browser" {
    browser { open "/" }
}
test "mixed" {
    server { let response = http.get("http://example.test") }
    browser { open "/account" }
    expect true
}
test "pure" {
    let answer = 42
}
test "value assertion" {
    expect 1 == 1
}"#;
    let (diagnostics, plan) = analyze(source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert_eq!(
        plan.tests
            .iter()
            .map(|test| test.required_host_capabilities.clone())
            .collect::<Vec<_>>(),
        [
            vec![Capability::Server],
            vec![Capability::Browser],
            vec![Capability::Server, Capability::Browser, Capability::Test],
            vec![],
            vec![Capability::Test],
        ]
    );
    assert_eq!(
        plan.required_host_capabilities,
        vec![Capability::Server, Capability::Browser, Capability::Test]
    );
    assert_eq!(
        plan.required_host_capabilities,
        TestPlan::required_capability_union(&plan.tests)
    );
    assert!(plan.validate_capabilities().is_ok());
}

#[test]
fn reports_provider_type_capability_and_transfer_errors() {
    let source = r#"test "bad" {
        server { let result = process.run("seed", args: [1]) }
        browser {
            let nope = http.get("/inside-browser")
            fill label("Output") with result.stdout
        }
    }"#;
    let (diagnostics, _) = analyze(source);
    let codes: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(
        codes.contains(&"semantic.type_mismatch"),
        "{diagnostics:#?}"
    );
    assert!(
        codes.contains(&"semantic.capability_mismatch"),
        "{diagnostics:#?}"
    );
    assert!(
        codes.contains(&"semantic.non_transferable_value"),
        "{diagnostics:#?}"
    );
}

#[test]
fn distinguishes_use_before_definition_from_unknown_names() {
    let source = r#"test "names" {
        let first = later
        let later = 1
        expect missing == 1
    }"#;
    let (diagnostics, _) = analyze(source);
    let later = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "semantic.use_before_definition")
        .expect("use-before diagnostic");
    assert_eq!(
        &source[u32::from(later.range.start()) as usize..u32::from(later.range.end()) as usize],
        "later"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "semantic.unknown_name")
    );
}

#[test]
fn expression_precedence_is_preserved_in_the_typed_plan() {
    let (diagnostics, plan) = analyze(r#"test "math" { let value = 1 + 2 * 3 }"#);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let TestOperation::EvaluatePure(operation) = &plan.tests[0].steps[0].operation else {
        panic!("pure evaluation")
    };
    assert!(matches!(
        operation.expression,
        PlanExpr::Binary {
            operator: BinaryOperator::Add,
            ref right,
            ..
        } if matches!(
            right.as_ref(),
            PlanExpr::Binary {
                operator: BinaryOperator::Multiply,
                ..
            }
        )
    ));
}

#[test]
fn machine_diagnostics_preserve_typed_details_references_and_bounded_corrections() {
    let source = r#"test "machine" {
        server {
            let user: { id: Int, email: String } = { id: 1, email: "a@example.test" }
            let typo = user.emial
            let response = htp.get("http://example.test")
            let other = http.gte("http://example.test")
            let final = http.get("http://example.test", heders: {})
        }
    }"#;
    let (diagnostics, _) = analyze(source);
    let member = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "semantic.unknown_member")
        .expect("member diagnostic");
    assert_eq!(
        member.semantic_details.as_ref().expect("details")["requested"],
        "emial"
    );
    assert!(
        member
            .repair_hints
            .iter()
            .any(|hint| hint.replacement == RepairReplacement::text("email"))
    );
    assert!(member.reference_queries.contains(&"type.Record".into()));

    let provider = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "semantic.unknown_provider")
        .expect("provider diagnostic");
    assert!(
        provider
            .repair_hints
            .iter()
            .any(|hint| hint.replacement == RepairReplacement::text("http"))
    );
    let operation = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "semantic.unknown_provider_operation")
        .expect("operation diagnostic");
    assert!(
        operation
            .repair_hints
            .iter()
            .any(|hint| hint.replacement == RepairReplacement::text("get"))
    );
    let argument = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "semantic.unknown_argument")
        .expect("argument diagnostic");
    assert!(
        argument
            .repair_hints
            .iter()
            .any(|hint| hint.replacement == RepairReplacement::text("headers"))
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.repair_hints.len() <= 5)
    );
}

#[test]
fn step_ids_are_file_global_and_plans_are_deterministic() {
    let source = r#"test "first" { let one = 1 expect one == 1 }
test "second" { browser { open "/" click id("submit") } }"#;
    let (first_diagnostics, first) = analyze(source);
    let (second_diagnostics, second) = analyze(source);
    assert!(first_diagnostics.is_empty(), "{first_diagnostics:#?}");
    assert!(second_diagnostics.is_empty(), "{second_diagnostics:#?}");
    assert_eq!(first, second);
    assert_eq!(first.tests[0].steps[0].id, StepId(0));
    assert_eq!(first.tests[0].steps[1].id, StepId(1));
    assert_eq!(first.tests[1].steps[0].id, StepId(2));
    assert_eq!(first.tests[1].steps[1].id, StepId(3));
    assert_eq!(first.tests[0].id.0, 0);
    assert_eq!(first.tests[1].id.0, 1);
    assert!(
        first.tests[0].steps[0].origin.range.start() < first.tests[1].steps[0].origin.range.start()
    );
}

#[test]
fn provider_argument_errors_remain_in_encounter_order() {
    let source = r#"test "arguments" { server {
        http.post("/", json: {}, json: {}, text: "x", unknown: true)
        http.post()
    } }"#;
    let (diagnostics, _) = analyze(source);
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .filter(|code| code.starts_with("semantic."))
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        vec![
            "semantic.duplicate_argument",
            "semantic.conflicting_arguments",
            "semantic.unknown_argument",
            "semantic.missing_argument",
        ]
    );
}

#[test]
fn provider_plan_metadata_and_direct_or_bound_results_are_preserved_exactly() {
    let result_type = Type::Record(
        [
            (
                "id".into(),
                RecordField {
                    ty: Type::Int,
                    optional: false,
                    documentation: String::new(),
                    secret: false,
                },
            ),
            (
                "credentials".into(),
                RecordField {
                    ty: Type::Record(
                        [(
                            "token".into(),
                            RecordField {
                                ty: Type::String,
                                optional: false,
                                documentation: String::new(),
                                secret: true,
                            },
                        )]
                        .into(),
                    ),
                    optional: false,
                    documentation: String::new(),
                    secret: false,
                },
            ),
        ]
        .into(),
    );
    let schema = ProviderSchema {
        name: ProviderName("app".into()),
        operations: [(
            "create".into(),
            OperationSchema {
                name: OperationName("create".into()),
                parameters: vec![ParameterSchema {
                    name: "token".into(),
                    ty: Type::String,
                    required: true,
                    positional: false,
                    secret: true,
                    documentation: "Authentication token.".into(),
                    default: None,
                }],
                result: result_type.clone(),
                capability: Capability::Server,
                documentation: "Create a fixture.".into(),
                retry_safe: true,
            },
        )]
        .into(),
        schema_identity: Some("schema:test-app-v1".into()),
    };
    let schema_hash = schema.hash();
    let mut providers = ProviderRegistry::built_in_schemas();
    providers.register_schema(schema);
    let (diagnostics, plan) = analyze_with_registry(
        r#"test "metadata" { server {
            app.create(token: "first")
            let created = app.create(token: "second")
        } }"#,
        providers,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert_eq!(plan.tests[0].steps.len(), 2);
    for (index, step) in plan.tests[0].steps.iter().enumerate() {
        let TestOperation::ServerProviderCall(call) = &step.operation else {
            panic!("provider call")
        };
        assert_eq!(call.provider, "app");
        assert_eq!(call.operation, "create");
        assert_eq!(call.schema_hash, schema_hash);
        assert_eq!(call.result_type, result_type);
        assert_eq!(call.redacted_arguments, vec!["token"]);
        assert_eq!(call.redacted_result_fields, vec!["token"]);
        assert!(call.retry_safe);
        assert_eq!(step.id, StepId(u32::try_from(index).expect("step id")));
        assert_eq!(call.arguments.len(), 1);
        assert!(call.arguments.contains_key("token"));
        if index == 0 {
            assert_eq!(call.result_binding, None);
            assert_eq!(call.result_name, None);
        } else {
            assert_eq!(call.result_binding, Some(BindingId(0)));
            assert_eq!(call.result_name.as_deref(), Some("created"));
        }
    }
}
