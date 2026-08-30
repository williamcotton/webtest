use webtest_provider::RecordField;

use super::*;

#[test]
fn typed_decode_reports_the_exact_json_path() {
    let value = Value::Record(
        [
            ("id".into(), Value::String("wrong".into())),
            ("email".into(), Value::String("a@example.test".into())),
        ]
        .into_iter()
        .collect(),
    );
    let expected = Type::Record(
        [(
            "id".into(),
            RecordField {
                ty: Type::Int,
                optional: false,
                documentation: String::new(),
                secret: false,
            },
        )]
        .into_iter()
        .collect(),
    );
    let error = decode_value(&value, &expected, "$", Some("http.post".into()))
        .expect_err("decode should fail");
    assert_eq!(error.path, "$.id");
    assert_eq!(error.expected, Type::Int);
    assert_eq!(error.actual, "string");
}

#[test]
fn dynamic_expression_errors_are_test_failures_not_internal_invariants() {
    let expression = PlanExpr::Binary {
        operator: BinaryOperator::Divide,
        left: Box::new(PlanExpr::Literal(Value::Int(1))),
        right: Box::new(PlanExpr::Literal(Value::Int(0))),
    };
    let error = evaluate(&expression, &HashMap::new()).expect_err("division should fail");
    assert_eq!(error.code(), "division_by_zero");
    assert!(!error.is_infrastructure());
    assert!(matches!(error, StepError::Evaluation(_)));
}

fn binary(operator: BinaryOperator, left: Value, right: Value) -> Value {
    evaluate(
        &PlanExpr::Binary {
            operator,
            left: Box::new(PlanExpr::Literal(left)),
            right: Box::new(PlanExpr::Literal(right)),
        },
        &HashMap::new(),
    )
    .expect("binary expression")
}

#[test]
fn operators_preserve_numeric_string_boolean_and_containment_semantics() {
    assert_eq!(
        binary(BinaryOperator::Equal, Value::Int(2), Value::Float(2.0)),
        Value::Bool(true)
    );
    assert_eq!(
        binary(BinaryOperator::NotEqual, Value::Int(2), Value::Int(3)),
        Value::Bool(true)
    );
    assert_eq!(
        binary(BinaryOperator::Less, Value::Int(2), Value::Float(3.0)),
        Value::Bool(true)
    );
    assert_eq!(
        binary(
            BinaryOperator::LessEqual,
            Value::String("a".into()),
            Value::String("a".into())
        ),
        Value::Bool(true)
    );
    assert_eq!(
        binary(BinaryOperator::Greater, Value::Int(3), Value::Int(2)),
        Value::Bool(true)
    );
    assert_eq!(
        binary(BinaryOperator::GreaterEqual, Value::Int(3), Value::Int(3)),
        Value::Bool(true)
    );
    assert_eq!(
        binary(BinaryOperator::Add, Value::Int(2), Value::Float(0.5)),
        Value::Float(2.5)
    );
    assert_eq!(
        binary(
            BinaryOperator::Add,
            Value::String("web".into()),
            Value::String("test".into())
        ),
        Value::String("webtest".into())
    );
    assert_eq!(
        binary(BinaryOperator::Subtract, Value::Int(5), Value::Int(2)),
        Value::Int(3)
    );
    assert_eq!(
        binary(BinaryOperator::Multiply, Value::Int(3), Value::Int(2)),
        Value::Int(6)
    );
    assert_eq!(
        binary(BinaryOperator::Divide, Value::Int(5), Value::Int(2)),
        Value::Float(2.5)
    );
    assert_eq!(
        binary(BinaryOperator::And, Value::Bool(true), Value::Bool(false)),
        Value::Bool(false)
    );
    assert_eq!(
        binary(BinaryOperator::Or, Value::Bool(false), Value::Bool(true)),
        Value::Bool(true)
    );
    assert_eq!(
        binary(
            BinaryOperator::Contains,
            Value::String("webtest".into()),
            Value::String("test".into())
        ),
        Value::Bool(true)
    );
    assert_eq!(
        binary(
            BinaryOperator::Contains,
            Value::List(vec![Value::Int(1), Value::Float(2.0)]),
            Value::Int(2)
        ),
        Value::Bool(true)
    );

    let missing = BindingId(99);
    for (operator, left) in [
        (BinaryOperator::And, Value::Bool(false)),
        (BinaryOperator::Or, Value::Bool(true)),
    ] {
        let result = evaluate(
            &PlanExpr::Binary {
                operator,
                left: Box::new(PlanExpr::Literal(left.clone())),
                right: Box::new(PlanExpr::Binding(missing)),
            },
            &HashMap::new(),
        )
        .expect("short circuit");
        assert_eq!(result, left);
    }

    assert_eq!(
        evaluate(
            &PlanExpr::Unary {
                operator: UnaryOperator::Not,
                operand: Box::new(PlanExpr::Literal(Value::Bool(false))),
            },
            &HashMap::new(),
        )
        .expect("not"),
        Value::Bool(true)
    );
    assert_eq!(
        evaluate(
            &PlanExpr::Unary {
                operator: UnaryOperator::Negate,
                operand: Box::new(PlanExpr::Literal(Value::Int(2))),
            },
            &HashMap::new(),
        )
        .expect("negate"),
        Value::Int(-2)
    );
}

#[test]
fn typed_decode_projects_records_and_populates_optional_fields() {
    let expected = Type::Record(
        [
            (
                "score".into(),
                RecordField {
                    ty: Type::Float,
                    optional: false,
                    documentation: String::new(),
                    secret: false,
                },
            ),
            (
                "nickname".into(),
                RecordField {
                    ty: Type::String,
                    optional: true,
                    documentation: String::new(),
                    secret: false,
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    let actual = Value::Record(
        [
            ("score".into(), Value::Int(7)),
            ("ignored".into(), Value::Bool(true)),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        decode_value(&actual, &expected, "$", None).expect("decode"),
        Value::Record(
            [
                ("score".into(), Value::Float(7.0)),
                ("nickname".into(), Value::Null),
            ]
            .into_iter()
            .collect()
        )
    );
}
