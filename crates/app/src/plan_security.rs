use std::collections::{HashMap, HashSet};

use webtest_model::BindingId;
use webtest_plan::{PlanEnvelope, PlanExpr, TestOperation};
use webtest_project::Project;

use crate::error::AppError;

pub(crate) fn reject_literal_secrets(
    envelope: &PlanEnvelope,
    project: &Project,
) -> Result<(), AppError> {
    for test in &envelope.tests {
        let mut bindings = HashMap::new();
        for step in test.steps() {
            if let TestOperation::EvaluatePure(operation) = &step.operation
                && let Some(binding) = operation.result_binding
            {
                bindings.insert(binding, &operation.expression);
            }
            let TestOperation::ServerProviderCall(call) = &step.operation else {
                continue;
            };
            for argument in &call.redacted_arguments {
                if call
                    .arguments
                    .get(argument)
                    .is_some_and(|value| has_literal_value(value, &bindings))
                {
                    return Err(secret_plan_error(&call.provider, &call.operation, argument));
                }
            }
            if call.provider == "http" {
                if call.arguments.get("json").is_some_and(|value| {
                    has_sensitive_record_literal(
                        value,
                        &project.config.redaction.json_fields,
                        &bindings,
                    )
                }) {
                    return Err(secret_plan_error(&call.provider, &call.operation, "json"));
                }
                if call.arguments.get("headers").is_some_and(|value| {
                    has_sensitive_record_literal(
                        value,
                        &project.config.redaction.headers,
                        &bindings,
                    )
                }) {
                    return Err(secret_plan_error(
                        &call.provider,
                        &call.operation,
                        "headers",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn secret_plan_error(provider: &str, operation: &str, argument: &str) -> AppError {
    AppError::usage(format!(
        "cannot emit a plan containing a literal secret in `{provider}.{operation}` argument `{argument}`; use a late-bound secret source"
    ))
}

fn has_literal_value(expression: &PlanExpr, bindings: &HashMap<BindingId, &PlanExpr>) -> bool {
    has_literal_value_inner(expression, bindings, &mut HashSet::new())
}

fn has_literal_value_inner(
    expression: &PlanExpr,
    bindings: &HashMap<BindingId, &PlanExpr>,
    visiting: &mut HashSet<BindingId>,
) -> bool {
    match expression {
        PlanExpr::Literal(_) => true,
        PlanExpr::Binding(binding) => {
            visiting.insert(*binding)
                && bindings
                    .get(binding)
                    .is_some_and(|value| has_literal_value_inner(value, bindings, visiting))
        }
        PlanExpr::List(values) => values
            .iter()
            .any(|value| has_literal_value_inner(value, bindings, visiting)),
        PlanExpr::Record(values) => values
            .values()
            .any(|value| has_literal_value_inner(value, bindings, visiting)),
        PlanExpr::Member { receiver, .. }
        | PlanExpr::Unary {
            operand: receiver, ..
        }
        | PlanExpr::Decode {
            value: receiver, ..
        } => has_literal_value_inner(receiver, bindings, visiting),
        PlanExpr::Binary { left, right, .. } => {
            has_literal_value_inner(left, bindings, visiting)
                || has_literal_value_inner(right, bindings, visiting)
        }
        PlanExpr::Type(_) => false,
    }
}

fn has_sensitive_record_literal(
    expression: &PlanExpr,
    sensitive_fields: &[String],
    bindings: &HashMap<BindingId, &PlanExpr>,
) -> bool {
    has_sensitive_record_literal_inner(expression, sensitive_fields, bindings, &mut HashSet::new())
}

fn has_sensitive_record_literal_inner(
    expression: &PlanExpr,
    sensitive_fields: &[String],
    bindings: &HashMap<BindingId, &PlanExpr>,
    visiting: &mut HashSet<BindingId>,
) -> bool {
    match expression {
        PlanExpr::Binding(binding) => {
            visiting.insert(*binding)
                && bindings.get(binding).is_some_and(|value| {
                    has_sensitive_record_literal_inner(value, sensitive_fields, bindings, visiting)
                })
        }
        PlanExpr::Record(values) => values.iter().any(|(name, value)| {
            (sensitive_fields
                .iter()
                .any(|field| field.eq_ignore_ascii_case(name))
                && has_literal_value(value, bindings))
                || has_sensitive_record_literal_inner(value, sensitive_fields, bindings, visiting)
        }),
        PlanExpr::List(values) => values.iter().any(|value| {
            has_sensitive_record_literal_inner(value, sensitive_fields, bindings, visiting)
        }),
        PlanExpr::Member { receiver, .. }
        | PlanExpr::Unary {
            operand: receiver, ..
        }
        | PlanExpr::Decode {
            value: receiver, ..
        } => has_sensitive_record_literal_inner(receiver, sensitive_fields, bindings, visiting),
        PlanExpr::Binary { left, right, .. } => {
            has_sensitive_record_literal_inner(left, sensitive_fields, bindings, visiting)
                || has_sensitive_record_literal_inner(right, sensitive_fields, bindings, visiting)
        }
        PlanExpr::Literal(_) | PlanExpr::Type(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use webtest_model::{BinaryOperator, Type, UnaryOperator, Value};

    use super::*;

    fn literal() -> PlanExpr {
        PlanExpr::Literal(Value::String("secret".into()))
    }

    #[test]
    fn literal_traversal_covers_every_expression_shape() {
        let leaf = literal();
        let expressions = [
            PlanExpr::List(vec![leaf.clone()]),
            PlanExpr::Record(BTreeMap::from([("value".into(), leaf.clone())])),
            PlanExpr::Member {
                receiver: Box::new(leaf.clone()),
                member: "value".into(),
                missing_is_null: false,
            },
            PlanExpr::Unary {
                operator: UnaryOperator::Not,
                operand: Box::new(leaf.clone()),
            },
            PlanExpr::Binary {
                operator: BinaryOperator::Equal,
                left: Box::new(PlanExpr::Type(Type::String)),
                right: Box::new(leaf.clone()),
            },
            PlanExpr::Decode {
                value: Box::new(leaf),
                target: Type::String,
                response_operation: None,
            },
        ];
        for expression in expressions {
            assert!(has_literal_value(&expression, &HashMap::new()));
        }
        assert!(!has_literal_value(
            &PlanExpr::Type(Type::String),
            &HashMap::new()
        ));
    }

    #[test]
    fn bindings_find_literals_and_cycles_terminate() {
        let literal = literal();
        let reference = PlanExpr::Binding(BindingId(1));
        let bindings = HashMap::from([(BindingId(1), &literal)]);
        assert!(has_literal_value(&reference, &bindings));

        let cycle = PlanExpr::Binding(BindingId(1));
        let bindings = HashMap::from([(BindingId(1), &cycle)]);
        assert!(!has_literal_value(&cycle, &bindings));
    }

    #[test]
    fn sensitive_record_fields_are_case_insensitive_and_recursive() {
        let nested = PlanExpr::List(vec![PlanExpr::Record(BTreeMap::from([(
            "Authorization".into(),
            literal(),
        )]))]);
        assert!(has_sensitive_record_literal(
            &nested,
            &["authorization".into()],
            &HashMap::new()
        ));
        assert!(!has_sensitive_record_literal(
            &nested,
            &["password".into()],
            &HashMap::new()
        ));
    }
}
