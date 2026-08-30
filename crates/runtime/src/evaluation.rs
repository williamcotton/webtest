use std::collections::{BTreeMap, HashMap};

use webtest_hir::{BinaryOperator, BindingId, UnaryOperator};
use webtest_plan::PlanExpr;
use webtest_provider::{Type, Value};

use crate::{DecodeFailure, EvaluationFailure, StepError};

pub(crate) fn evaluate(
    expression: &PlanExpr,
    environment: &HashMap<BindingId, Value>,
) -> Result<Value, StepError> {
    match expression {
        PlanExpr::Literal(value) => Ok(value.clone()),
        PlanExpr::Binding(binding) => environment.get(binding).cloned().ok_or_else(|| {
            StepError::Internal(format!("binding {} has no runtime value", binding.0))
        }),
        PlanExpr::List(items) => items
            .iter()
            .map(|item| evaluate(item, environment))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        PlanExpr::Record(fields) => fields
            .iter()
            .map(|(name, value)| Ok((name.clone(), evaluate(value, environment)?)))
            .collect::<Result<BTreeMap<_, _>, StepError>>()
            .map(Value::Record),
        PlanExpr::Type(_) => Err(StepError::Internal(
            "type pattern cannot be evaluated as a value".into(),
        )),
        PlanExpr::Member { receiver, member } => {
            let receiver = evaluate(receiver, environment)?;
            receiver.member(member).ok_or_else(|| {
                if matches!(receiver, Value::Response(_))
                    && matches!(member.as_str(), "json" | "text")
                {
                    StepError::Evaluation(EvaluationFailure {
                        code: "response_decode_failed",
                        message: format!(
                            "response body is not available as `{member}` for this operation"
                        ),
                    })
                } else {
                    StepError::Internal(format!("runtime value has no member `{member}`"))
                }
            })
        }
        PlanExpr::Unary { operator, operand } => {
            let operand = evaluate(operand, environment)?;
            match (operator, operand) {
                (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                (UnaryOperator::Negate, Value::Int(value)) => Ok(Value::Int(-value)),
                (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                _ => Err(StepError::Internal("invalid typed unary operation".into())),
            }
        }
        PlanExpr::Binary {
            operator,
            left,
            right,
        } => {
            let left = evaluate(left, environment)?;
            match (operator, &left) {
                (BinaryOperator::And, Value::Bool(false)) => Ok(Value::Bool(false)),
                (BinaryOperator::Or, Value::Bool(true)) => Ok(Value::Bool(true)),
                _ => evaluate_binary(*operator, left, evaluate(right, environment)?),
            }
        }
        PlanExpr::Decode {
            value,
            target,
            response_operation,
        } => {
            let value = evaluate(value, environment)?;
            decode_value(&value, target, "$", response_operation.clone()).map_err(StepError::Decode)
        }
    }
}

fn evaluate_binary(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, StepError> {
    let value = match operator {
        BinaryOperator::Equal => Value::Bool(values_equal(&left, &right)),
        BinaryOperator::NotEqual => Value::Bool(!values_equal(&left, &right)),
        BinaryOperator::Less => Value::Bool(compare_values(&left, &right).is_some_and(|it| it < 0)),
        BinaryOperator::LessEqual => {
            Value::Bool(compare_values(&left, &right).is_some_and(|it| it <= 0))
        }
        BinaryOperator::Greater => {
            Value::Bool(compare_values(&left, &right).is_some_and(|it| it > 0))
        }
        BinaryOperator::GreaterEqual => {
            Value::Bool(compare_values(&left, &right).is_some_and(|it| it >= 0))
        }
        BinaryOperator::Add => match (left, right) {
            (Value::Int(left), Value::Int(right)) => Value::Int(left + right),
            (Value::Float(left), Value::Float(right)) => Value::Float(left + right),
            (Value::Int(left), Value::Float(right)) => Value::Float(left as f64 + right),
            (Value::Float(left), Value::Int(right)) => Value::Float(left + right as f64),
            (Value::String(left), Value::String(right)) => Value::String(left + &right),
            _ => return Err(StepError::Internal("invalid typed addition".into())),
        },
        BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide => {
            numeric_binary(operator, left, right)?
        }
        BinaryOperator::And => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Value::Bool(left && right),
            _ => {
                return Err(StepError::Internal(
                    "invalid typed boolean operation".into(),
                ));
            }
        },
        BinaryOperator::Or => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Value::Bool(left || right),
            _ => {
                return Err(StepError::Internal(
                    "invalid typed boolean operation".into(),
                ));
            }
        },
        BinaryOperator::Contains => Value::Bool(value_contains(&left, &right)),
        BinaryOperator::Matches => {
            return Err(StepError::Internal(
                "matches is evaluated by assertion execution".into(),
            ));
        }
    };
    Ok(value)
}

fn numeric_binary(operator: BinaryOperator, left: Value, right: Value) -> Result<Value, StepError> {
    if let (Value::Int(left), Value::Int(right)) = (&left, &right)
        && operator != BinaryOperator::Divide
    {
        return Ok(Value::Int(match operator {
            BinaryOperator::Subtract => left - right,
            BinaryOperator::Multiply => left * right,
            _ => unreachable!(),
        }));
    }
    let left = number(&left).ok_or_else(|| StepError::Internal("expected number".into()))?;
    let right = number(&right).ok_or_else(|| StepError::Internal("expected number".into()))?;
    if operator == BinaryOperator::Divide && right == 0.0 {
        return Err(StepError::Evaluation(EvaluationFailure {
            code: "division_by_zero",
            message: "division by zero".into(),
        }));
    }
    Ok(Value::Float(match operator {
        BinaryOperator::Subtract => left - right,
        BinaryOperator::Multiply => left * right,
        BinaryOperator::Divide => left / right,
        _ => unreachable!(),
    }))
}

pub(crate) fn decode_value(
    value: &Value,
    expected: &Type,
    path: &str,
    response_operation: Option<String>,
) -> Result<Value, DecodeFailure> {
    let failure = || DecodeFailure {
        path: path.into(),
        expected: expected.clone(),
        actual: value.type_name().into(),
        response_operation: response_operation.clone(),
    };
    match expected {
        Type::Json | Type::Unknown => Ok(value.clone()),
        Type::Null if matches!(value, Value::Null) => Ok(Value::Null),
        Type::Bool if matches!(value, Value::Bool(_)) => Ok(value.clone()),
        Type::Int if matches!(value, Value::Int(_)) => Ok(value.clone()),
        Type::Float => match value {
            Value::Float(_) => Ok(value.clone()),
            Value::Int(value) => Ok(Value::Float(*value as f64)),
            _ => Err(failure()),
        },
        Type::String | Type::Url if matches!(value, Value::String(_)) => Ok(value.clone()),
        Type::Duration if matches!(value, Value::DurationMillis(_)) => Ok(value.clone()),
        Type::Bytes if matches!(value, Value::Bytes(_)) => Ok(value.clone()),
        Type::StatusCode if matches!(value, Value::Int(_)) => Ok(value.clone()),
        Type::Option(_) if matches!(value, Value::Null) => Ok(Value::Null),
        Type::Option(inner) => decode_value(value, inner, path, response_operation),
        Type::List(inner) => {
            let Value::List(values) = value else {
                return Err(failure());
            };
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    decode_value(
                        value,
                        inner,
                        &format!("{path}[{index}]"),
                        response_operation.clone(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List)
        }
        Type::Record(expected_fields) => {
            let Value::Record(values) = value else {
                return Err(failure());
            };
            let mut decoded = BTreeMap::new();
            for (name, field) in expected_fields {
                match values.get(name) {
                    Some(value) => {
                        decoded.insert(
                            name.clone(),
                            decode_value(
                                value,
                                &field.ty,
                                &format!("{path}.{name}"),
                                response_operation.clone(),
                            )?,
                        );
                    }
                    None if field.optional => {
                        decoded.insert(name.clone(), Value::Null);
                    }
                    None => {
                        return Err(DecodeFailure {
                            path: format!("{path}.{name}"),
                            expected: field.ty.clone(),
                            actual: "missing field".into(),
                            response_operation,
                        });
                    }
                }
            }
            Ok(Value::Record(decoded))
        }
        Type::FilePath if matches!(value, Value::FilePath(_)) => Ok(value.clone()),
        Type::TempDirectory if matches!(value, Value::TempDirectory(_)) => Ok(value.clone()),
        Type::ProcessResult if matches!(value, Value::ProcessResult(_)) => Ok(value.clone()),
        Type::Response(_) if matches!(value, Value::Response(_)) => Ok(value.clone()),
        Type::Headers if matches!(value, Value::Headers(_)) => Ok(value.clone()),
        _ => Err(failure()),
    }
}

pub(crate) fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Int(left), Value::Float(right)) => *left as f64 == *right,
        (Value::Float(left), Value::Int(right)) => *left == *right as f64,
        _ => left == right,
    }
}

pub(crate) fn compare_values(left: &Value, right: &Value) -> Option<i8> {
    match (left, right) {
        (Value::String(left), Value::String(right)) => Some(match left.cmp(right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        _ => {
            let left = number(left)?;
            let right = number(right)?;
            Some(if left < right {
                -1
            } else if left > right {
                1
            } else {
                0
            })
        }
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(value) => Some(*value as f64),
        Value::Float(value) => Some(*value),
        _ => None,
    }
}

pub(crate) fn value_contains(container: &Value, value: &Value) -> bool {
    match (container, value) {
        (Value::String(container), Value::String(value)) => container.contains(value),
        (Value::List(values), value) => values.iter().any(|item| values_equal(item, value)),
        _ => false,
    }
}

pub(crate) fn display_value(value: &Value) -> String {
    webtest_provider::value_to_json(value)
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| format!("<{:?}>", value.type_name()))
}

pub(crate) fn runtime_transferable(value: &Value) -> bool {
    match value {
        Value::Null
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DurationMillis(_) => true,
        Value::List(values) => values.iter().all(runtime_transferable),
        Value::Record(values) => values.values().all(runtime_transferable),
        Value::Headers(_)
        | Value::Bytes(_)
        | Value::Response(_)
        | Value::ProcessResult(_)
        | Value::FilePath(_)
        | Value::TempDirectory(_) => false,
    }
}

pub(crate) fn string_value(value: Value) -> Result<String, StepError> {
    if let Value::String(value) = value {
        Ok(value)
    } else {
        Err(StepError::Internal(format!(
            "typed expression produced {}, expected string",
            value.type_name()
        )))
    }
}

#[cfg(test)]
mod tests;
