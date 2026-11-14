//! Canonical deterministic value evaluation shared by static expansion and execution.
#[derive(Clone, Debug)]
pub enum PureEvaluationError {
    Internal(String),
    Decode(DecodeFailure),
    Evaluation(EvaluationFailure),
}
#[derive(Clone, Debug)]
pub struct DecodeFailure {
    pub path: String,
    pub expected: webtest_model::Type,
    pub actual: String,
    pub response_operation: Option<String>,
}
#[derive(Clone, Debug)]
pub struct EvaluationFailure {
    pub kind: EvaluationFailureKind,
    pub message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvaluationFailureKind {
    DivisionByZero,
    IntegerOverflow,
    ResponseDecodeFailed,
    PatternLimitExceeded,
}
impl std::fmt::Display for PureEvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Internal(message) => f.write_str(message),
            Self::Decode(failure) => write!(
                f,
                "expected {} at {}, got {}",
                failure.expected, failure.path, failure.actual
            ),
            Self::Evaluation(failure) => f.write_str(&failure.message),
        }
    }
}
impl std::error::Error for PureEvaluationError {}
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
};

use crate::PlanExpr;
use webtest_model::{BinaryOperator, BindingId, Type, UnaryOperator, Value, value_to_json};

/// Immutable value lookup for the canonical pure evaluator. Hosts may overlay
/// one lexical event binding without copying an entire captured environment.
pub trait EvaluationBindings {
    fn get(&self, binding: &BindingId) -> Option<&Value>;
}
impl EvaluationBindings for HashMap<BindingId, Value> {
    fn get(&self, binding: &BindingId) -> Option<&Value> {
        HashMap::get(self, binding)
    }
}
pub fn evaluate(
    expression: &PlanExpr,
    environment: &dyn EvaluationBindings,
) -> Result<Value, PureEvaluationError> {
    match expression {
        PlanExpr::Pattern(_) => Err(PureEvaluationError::Internal(
            "patterns are not runtime values".into(),
        )),
        PlanExpr::PureCall {
            arguments, body, ..
        } => {
            let mut locals = HashMap::new();
            for (parameter, argument) in arguments {
                locals.insert(*parameter, evaluate(argument, environment)?);
            }
            evaluate(body, &locals)
        }
        PlanExpr::Lower { value } => match evaluate(value, environment)? {
            Value::String(value) => Ok(Value::String(value.to_lowercase())),
            _ => Err(PureEvaluationError::Internal(
                "lower requires a typed String".into(),
            )),
        },
        PlanExpr::Literal(value) => Ok(value.clone()),
        PlanExpr::Binding(binding) => environment.get(binding).cloned().ok_or_else(|| {
            PureEvaluationError::Internal(format!("binding {} has no runtime value", binding.0))
        }),
        PlanExpr::List(items) => items
            .iter()
            .map(|item| evaluate(item, environment))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        PlanExpr::Record(fields) => fields
            .iter()
            .map(|(name, value)| Ok((name.clone(), evaluate(value, environment)?)))
            .collect::<Result<BTreeMap<_, _>, PureEvaluationError>>()
            .map(Value::Record),
        PlanExpr::Type(_) => Err(PureEvaluationError::Internal(
            "type pattern cannot be evaluated as a value".into(),
        )),
        PlanExpr::Member {
            receiver,
            member,
            missing_is_null,
        } => {
            let receiver = evaluate(receiver, environment)?;
            receiver.member(member).map_or_else(
                || {
                    if matches!(receiver, Value::Response(_))
                        && matches!(member.as_str(), "json" | "text")
                    {
                        Err(PureEvaluationError::Evaluation(EvaluationFailure {
                            kind: EvaluationFailureKind::ResponseDecodeFailed,
                            message: format!(
                                "response body is not available as `{member}` for this operation"
                            ),
                        }))
                    } else if *missing_is_null {
                        Ok(Value::Null)
                    } else {
                        Err(PureEvaluationError::Internal(format!(
                            "runtime value has no member `{member}`"
                        )))
                    }
                },
                Ok,
            )
        }
        PlanExpr::Unary { operator, operand } => {
            let operand = evaluate(operand, environment)?;
            match (operator, operand) {
                (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                (UnaryOperator::Negate, Value::Int(value)) => value
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or_else(|| integer_overflow("negation")),
                (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                _ => Err(PureEvaluationError::Internal(
                    "invalid typed unary operation".into(),
                )),
            }
        }
        PlanExpr::Binary {
            operator,
            left,
            right,
        } => {
            let left = evaluate(left, environment)?;
            if *operator == BinaryOperator::Matches {
                if let PlanExpr::Pattern(matcher) = right.as_ref() {
                    return Ok(Value::Bool(
                        matcher.matches(&left, environment)?.mismatches.is_empty(),
                    ));
                }
                if let PlanExpr::Type(ty) = right.as_ref() {
                    return Ok(Value::Bool(decode_value(&left, ty, "$", None).is_ok()));
                }
                return Err(PureEvaluationError::Internal(
                    "matches requires a matcher plan".into(),
                ));
            }
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
            decode_value(&value, target, "$", response_operation.clone())
                .map_err(PureEvaluationError::Decode)
        }
    }
}

fn evaluate_binary(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, PureEvaluationError> {
    let value = match operator {
        BinaryOperator::Equal => Value::Bool(values_equal(&left, &right)),
        BinaryOperator::NotEqual => Value::Bool(!values_equal(&left, &right)),
        BinaryOperator::Less => {
            Value::Bool(compare_values(&left, &right)?.is_some_and(|it| it == Ordering::Less))
        }
        BinaryOperator::LessEqual => Value::Bool(
            compare_values(&left, &right)?
                .is_some_and(|it| matches!(it, Ordering::Less | Ordering::Equal)),
        ),
        BinaryOperator::Greater => {
            Value::Bool(compare_values(&left, &right)?.is_some_and(|it| it == Ordering::Greater))
        }
        BinaryOperator::GreaterEqual => Value::Bool(
            compare_values(&left, &right)?
                .is_some_and(|it| matches!(it, Ordering::Greater | Ordering::Equal)),
        ),
        BinaryOperator::Add => match (left, right) {
            (Value::Int(left), Value::Int(right)) => Value::Int(
                left.checked_add(right)
                    .ok_or_else(|| integer_overflow("addition"))?,
            ),
            (Value::Float(left), Value::Float(right)) => Value::Float(left + right),
            (Value::Int(left), Value::Float(right)) => Value::Float(left as f64 + right),
            (Value::Float(left), Value::Int(right)) => Value::Float(left + right as f64),
            (Value::String(left), Value::String(right)) => Value::String(left + &right),
            _ => {
                return Err(PureEvaluationError::Internal(
                    "invalid typed addition".into(),
                ));
            }
        },
        BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide => {
            numeric_binary(operator, left, right)?
        }
        BinaryOperator::And => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Value::Bool(left && right),
            _ => {
                return Err(PureEvaluationError::Internal(
                    "invalid typed boolean operation".into(),
                ));
            }
        },
        BinaryOperator::Or => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Value::Bool(left || right),
            _ => {
                return Err(PureEvaluationError::Internal(
                    "invalid typed boolean operation".into(),
                ));
            }
        },
        BinaryOperator::Contains => Value::Bool(value_contains(&left, &right)),
        BinaryOperator::Matches => {
            return Err(PureEvaluationError::Internal(
                "matches is evaluated by assertion execution".into(),
            ));
        }
    };
    Ok(value)
}

fn numeric_binary(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, PureEvaluationError> {
    if let (Value::Int(left), Value::Int(right)) = (&left, &right)
        && operator != BinaryOperator::Divide
    {
        return Ok(Value::Int(match operator {
            BinaryOperator::Subtract => left
                .checked_sub(*right)
                .ok_or_else(|| integer_overflow("subtraction"))?,
            BinaryOperator::Multiply => left
                .checked_mul(*right)
                .ok_or_else(|| integer_overflow("multiplication"))?,
            _ => unreachable!(),
        }));
    }
    let left =
        number(&left).ok_or_else(|| PureEvaluationError::Internal("expected number".into()))?;
    let right =
        number(&right).ok_or_else(|| PureEvaluationError::Internal("expected number".into()))?;
    if operator == BinaryOperator::Divide && right == 0.0 {
        return Err(PureEvaluationError::Evaluation(EvaluationFailure {
            kind: EvaluationFailureKind::DivisionByZero,
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

pub fn decode_value(
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

fn integer_overflow(operation: &str) -> PureEvaluationError {
    PureEvaluationError::Evaluation(EvaluationFailure {
        kind: EvaluationFailureKind::IntegerOverflow,
        message: format!("integer {operation} overflow"),
    })
}

pub fn values_equal(left: &Value, right: &Value) -> bool {
    numeric_relation(left, right).map_or_else(
        || left == right,
        |relation| relation == Some(Ordering::Equal),
    )
}

pub fn compare_values(
    left: &Value,
    right: &Value,
) -> Result<Option<Ordering>, PureEvaluationError> {
    match (left, right) {
        (Value::String(left), Value::String(right)) => Ok(Some(left.cmp(right))),
        _ => numeric_relation(left, right).ok_or_else(|| {
            PureEvaluationError::Internal("ordered comparison expected numbers or strings".into())
        }),
    }
}

fn numeric_relation(left: &Value, right: &Value) -> Option<Option<Ordering>> {
    Some(match (left, right) {
        (Value::Int(left), Value::Int(right)) => Some(left.cmp(right)),
        (Value::Float(left), Value::Float(right)) => left.partial_cmp(right),
        (Value::Int(left), Value::Float(right)) => compare_int_float(*left, *right),
        (Value::Float(left), Value::Int(right)) => {
            compare_int_float(*right, *left).map(Ordering::reverse)
        }
        _ => return None,
    })
}

fn compare_int_float(integer: i64, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float >= 9_223_372_036_854_775_808.0 {
        return Some(Ordering::Less);
    }
    if float < -9_223_372_036_854_775_808.0 {
        return Some(Ordering::Greater);
    }

    let truncated = float.trunc() as i64;
    match integer.cmp(&truncated) {
        Ordering::Equal if float.fract() > 0.0 => Some(Ordering::Less),
        Ordering::Equal if float.fract() < 0.0 => Some(Ordering::Greater),
        ordering => Some(ordering),
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(value) => Some(*value as f64),
        Value::Float(value) => Some(*value),
        _ => None,
    }
}

pub fn value_contains(container: &Value, value: &Value) -> bool {
    match (container, value) {
        (Value::String(container), Value::String(value)) => container.contains(value),
        (Value::List(values), value) => values.iter().any(|item| values_equal(item, value)),
        _ => false,
    }
}

pub fn display_value(value: &Value) -> String {
    value_to_json(value)
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| format!("<{:?}>", value.type_name()))
}

pub fn runtime_transferable(value: &Value) -> bool {
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

pub fn string_value(value: Value) -> Result<String, PureEvaluationError> {
    if let Value::String(value) = value {
        Ok(value)
    } else {
        Err(PureEvaluationError::Internal(format!(
            "typed expression produced {}, expected string",
            value.type_name()
        )))
    }
}
