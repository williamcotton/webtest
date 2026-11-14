//! Runtime error projection over the canonical portable evaluator.
use crate::{DecodeFailure, EvaluationFailure, EvaluationFailureKind, StepError};
pub(crate) use pure::{display_value, runtime_transferable, value_contains, values_equal};
use std::{cmp::Ordering, collections::HashMap};
use webtest_model::{BindingId, Type, Value};
use webtest_plan::{PlanExpr, evaluation as pure};
fn decode_failure(f: pure::DecodeFailure) -> DecodeFailure {
    DecodeFailure {
        path: f.path,
        expected: f.expected,
        actual: f.actual,
        response_operation: f.response_operation,
    }
}
impl From<pure::PureEvaluationError> for StepError {
    fn from(error: pure::PureEvaluationError) -> Self {
        match error {
            pure::PureEvaluationError::Internal(message) => Self::Internal(message),
            pure::PureEvaluationError::Decode(f) => Self::Decode(decode_failure(f)),
            pure::PureEvaluationError::Evaluation(f) => Self::Evaluation(EvaluationFailure {
                message: f.message,
                kind: match f.kind {
                    pure::EvaluationFailureKind::DivisionByZero => {
                        EvaluationFailureKind::DivisionByZero
                    }
                    pure::EvaluationFailureKind::IntegerOverflow => {
                        EvaluationFailureKind::IntegerOverflow
                    }
                    pure::EvaluationFailureKind::PatternLimitExceeded => {
                        EvaluationFailureKind::PatternLimitExceeded
                    }
                    pure::EvaluationFailureKind::ResponseDecodeFailed => {
                        EvaluationFailureKind::ResponseDecodeFailed
                    }
                },
            }),
        }
    }
}
pub(crate) fn evaluate(
    expression: &PlanExpr,
    environment: &HashMap<BindingId, Value>,
) -> Result<Value, StepError> {
    pure::evaluate(expression, environment).map_err(Into::into)
}
pub(crate) fn compare_values(left: &Value, right: &Value) -> Result<Option<Ordering>, StepError> {
    pure::compare_values(left, right).map_err(Into::into)
}
pub(crate) fn decode_value(
    value: &Value,
    expected: &Type,
    path: &str,
    operation: Option<String>,
) -> Result<Value, DecodeFailure> {
    pure::decode_value(value, expected, path, operation).map_err(decode_failure)
}
pub(crate) fn string_value(value: Value) -> Result<String, StepError> {
    pure::string_value(value).map_err(Into::into)
}
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use webtest_model::{BinaryOperator, UnaryOperator};
#[cfg(test)]
mod tests;
