use webtest_model::RecordField;

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
    assert_eq!(
        error.code(),
        webtest_observation::RuntimeFailureCode::DivisionByZero
    );
    assert_eq!(error.failure_class(), crate::FailureClass::Test);
    assert!(matches!(error, StepError::Evaluation(_)));
}

fn evaluate_unary(operator: UnaryOperator, operand: Value) -> Result<Value, StepError> {
    evaluate(
        &PlanExpr::Unary {
            operator,
            operand: Box::new(PlanExpr::Literal(operand)),
        },
        &HashMap::new(),
    )
}

fn evaluate_binary_result(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, StepError> {
    evaluate(
        &PlanExpr::Binary {
            operator,
            left: Box::new(PlanExpr::Literal(left)),
            right: Box::new(PlanExpr::Literal(right)),
        },
        &HashMap::new(),
    )
}

#[test]
fn all_integer_overflow_boundaries_are_structured_test_failures() {
    let operations = [
        evaluate_unary(UnaryOperator::Negate, Value::Int(i64::MIN)),
        evaluate_binary_result(BinaryOperator::Add, Value::Int(i64::MAX), Value::Int(1)),
        evaluate_binary_result(
            BinaryOperator::Subtract,
            Value::Int(i64::MIN),
            Value::Int(1),
        ),
        evaluate_binary_result(
            BinaryOperator::Multiply,
            Value::Int(i64::MAX),
            Value::Int(2),
        ),
    ];
    for error in operations.map(|result| result.expect_err("integer operation must overflow")) {
        assert_eq!(
            error.code(),
            webtest_observation::RuntimeFailureCode::IntegerOverflow
        );
        assert_eq!(error.failure_class(), crate::FailureClass::Test);
        assert!(matches!(error, StepError::Evaluation(_)));
        assert!(error.to_string().starts_with("integer "));
        assert!(error.to_string().ends_with(" overflow"));
    }
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
fn numeric_relations_are_exact_across_integer_and_float_boundaries() {
    let bool_result = |operator, left, right| {
        assert_eq!(binary(operator, left, right), Value::Bool(true));
    };
    bool_result(
        BinaryOperator::Less,
        Value::Int(9_007_199_254_740_992),
        Value::Int(9_007_199_254_740_993),
    );
    bool_result(
        BinaryOperator::NotEqual,
        Value::Int(9_007_199_254_740_993),
        Value::Float(9_007_199_254_740_992.0),
    );
    bool_result(
        BinaryOperator::Less,
        Value::Int(i64::MAX),
        Value::Float(9_223_372_036_854_775_808.0),
    );
    bool_result(
        BinaryOperator::Equal,
        Value::Int(i64::MIN),
        Value::Float(-9_223_372_036_854_775_808.0),
    );

    for (integer, float, expected) in [
        (1, 1.5, Ordering::Less),
        (-1, -1.5, Ordering::Greater),
        (0, -0.0, Ordering::Equal),
        (i64::MAX, f64::INFINITY, Ordering::Less),
        (i64::MIN, f64::NEG_INFINITY, Ordering::Greater),
    ] {
        assert_eq!(
            compare_values(&Value::Int(integer), &Value::Float(float)).expect("numeric values"),
            Some(expected)
        );
        assert_eq!(
            compare_values(&Value::Float(float), &Value::Int(integer)).expect("numeric values"),
            Some(expected.reverse())
        );
    }

    assert_eq!(
        compare_values(&Value::Float(f64::NAN), &Value::Int(0)).expect("numeric values"),
        None
    );
    assert_eq!(
        binary(
            BinaryOperator::Equal,
            Value::Float(f64::NAN),
            Value::Float(f64::NAN)
        ),
        Value::Bool(false)
    );
    assert_eq!(
        binary(BinaryOperator::Less, Value::Float(f64::NAN), Value::Int(0)),
        Value::Bool(false)
    );
    assert!(matches!(
        compare_values(&Value::Bool(false), &Value::Bool(true)),
        Err(StepError::Internal(_))
    ));

    assert!(!value_contains(
        &Value::List(vec![Value::Int(9_007_199_254_740_993)]),
        &Value::Float(9_007_199_254_740_992.0)
    ));
}

#[test]
fn optional_member_absence_is_null_only_when_the_plan_marks_it_optional() {
    let member = |missing_is_null| PlanExpr::Member {
        receiver: Box::new(PlanExpr::Literal(Value::Record(BTreeMap::new()))),
        member: "value".into(),
        missing_is_null,
    };
    assert_eq!(
        evaluate(&member(true), &HashMap::new()).expect("optional member"),
        Value::Null
    );
    assert!(matches!(
        evaluate(&member(false), &HashMap::new()),
        Err(StepError::Internal(_))
    ));

    let present = PlanExpr::Member {
        receiver: Box::new(PlanExpr::Literal(Value::Record(BTreeMap::from([(
            "value".into(),
            Value::String("present".into()),
        )])))),
        member: "value".into(),
        missing_is_null: true,
    };
    assert_eq!(
        evaluate(&present, &HashMap::new()).expect("present optional member"),
        Value::String("present".into())
    );
}

#[test]
fn nested_typed_json_populates_optional_members_and_response_decode_failures_stay_structured() {
    let item_type = Type::Record(BTreeMap::from([
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
            "nickname".into(),
            RecordField {
                ty: Type::String,
                optional: true,
                documentation: String::new(),
                secret: false,
            },
        ),
    ]));
    let decoded = decode_value(
        &Value::Record(BTreeMap::from([(
            "items".into(),
            Value::List(vec![Value::Record(BTreeMap::from([(
                "id".into(),
                Value::Int(7),
            )]))]),
        )])),
        &Type::Record(BTreeMap::from([(
            "items".into(),
            RecordField {
                ty: Type::List(Box::new(item_type)),
                optional: false,
                documentation: String::new(),
                secret: false,
            },
        )])),
        "$",
        None,
    )
    .expect("nested decode");
    let Value::Record(root) = decoded else {
        panic!("decoded record")
    };
    let Some(Value::List(items)) = root.get("items") else {
        panic!("decoded items")
    };
    let Value::Record(item) = &items[0] else {
        panic!("decoded item")
    };
    assert_eq!(item.get("nickname"), Some(&Value::Null));

    let response = Value::Response(webtest_model::ResponseValue {
        status: 204,
        headers: BTreeMap::new(),
        body: vec![0xff],
        json: None,
    });
    for member in ["text", "json"] {
        let error = evaluate(
            &PlanExpr::Member {
                receiver: Box::new(PlanExpr::Literal(response.clone())),
                member: member.into(),
                missing_is_null: false,
            },
            &HashMap::new(),
        )
        .expect_err("unavailable response decoding");
        assert_eq!(
            error.code(),
            webtest_observation::RuntimeFailureCode::ResponseDecodeFailed
        );
        assert!(matches!(error, StepError::Evaluation(_)));
    }
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
