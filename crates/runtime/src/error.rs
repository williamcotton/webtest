use thiserror::Error;
use webtest_browser::BrowserError;
use webtest_feedback::FailureClass;
use webtest_observation::ValueDiff;
use webtest_plan::ValueMatcher;
use webtest_provider::{ProviderError, Type, Value};

#[derive(Clone, Debug)]
pub enum StepError {
    Browser(BrowserError),
    Provider(ProviderError),
    Assertion(Box<AssertionFailure>),
    Decode(DecodeFailure),
    Evaluation(EvaluationFailure),
    Internal(String),
}

impl StepError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Browser(error) => error.code(),
            Self::Provider(error) => error.code(),
            Self::Assertion(_) => "assertion_failed",
            Self::Decode(_) => "json_decode_failed",
            Self::Evaluation(error) => error.code,
            Self::Internal(_) => "internal_error",
        }
    }

    pub fn failure_class(&self) -> FailureClass {
        match self {
            Self::Browser(error) if error.is_infrastructure() => FailureClass::Infrastructure,
            Self::Provider(error) if error.is_infrastructure() => FailureClass::Infrastructure,
            Self::Browser(_)
            | Self::Provider(_)
            | Self::Assertion(_)
            | Self::Decode(_)
            | Self::Evaluation(_) => FailureClass::Test,
            Self::Internal(_) => FailureClass::Internal,
        }
    }
}

impl std::fmt::Display for StepError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Browser(error) => error.fmt(formatter),
            Self::Provider(error) => error.fmt(formatter),
            Self::Assertion(error) => error.fmt(formatter),
            Self::Decode(error) => error.fmt(formatter),
            Self::Evaluation(error) => error.fmt(formatter),
            Self::Internal(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for StepError {}

#[derive(Clone, Debug)]
pub struct AssertionFailure {
    pub matcher: ValueMatcher,
    pub expected: Option<Value>,
    pub actual: Value,
    pub message: String,
    pub diff: ValueDiff,
}

impl std::fmt::Display for AssertionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Debug)]
pub struct DecodeFailure {
    pub path: String,
    pub expected: Type,
    pub actual: String,
    pub response_operation: Option<String>,
}

impl std::fmt::Display for DecodeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "JSON decode failed at {}: expected {}, got {}",
            self.path, self.expected, self.actual
        )
    }
}

#[derive(Clone, Debug)]
pub struct EvaluationFailure {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for EvaluationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Debug, Error)]
pub enum RunError {
    #[error(transparent)]
    Browser(#[from] BrowserError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("internal runtime error: {0}")]
    Internal(String),
}

impl RunError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Browser(error) => error.code(),
            Self::Provider(error) => error.code(),
            Self::Internal(_) => "internal_error",
        }
    }

    pub const fn failure_class(&self) -> FailureClass {
        match self {
            Self::Browser(_) | Self::Provider(_) => FailureClass::Infrastructure,
            Self::Internal(_) => FailureClass::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use webtest_browser::Locator;
    use webtest_observation::ValueDiff;
    use webtest_plan::ValueMatcher;

    use super::*;

    fn assertion_failure() -> StepError {
        StepError::Assertion(Box::new(AssertionFailure {
            matcher: ValueMatcher::Equal,
            expected: Some(Value::Int(1)),
            actual: Value::Int(2),
            message: "values differ".into(),
            diff: ValueDiff::Scalar {
                expected: Some("1".into()),
                actual: "2".into(),
            },
        }))
    }

    #[test]
    fn every_step_error_variant_has_an_explicit_failure_class() {
        let cases = [
            (
                StepError::Browser(BrowserError::LocatorNotFound {
                    locator: Locator::Id("missing".into()),
                }),
                FailureClass::Test,
            ),
            (
                StepError::Browser(BrowserError::BrowserDisconnected),
                FailureClass::Infrastructure,
            ),
            (
                StepError::Provider(ProviderError::Application {
                    code: "denied".into(),
                    message: "denied".into(),
                    retryable: false,
                    data: serde_json::Value::Null,
                }),
                FailureClass::Test,
            ),
            (
                StepError::Provider(ProviderError::BridgeTransport {
                    message: "disconnected".into(),
                }),
                FailureClass::Infrastructure,
            ),
            (assertion_failure(), FailureClass::Test),
            (
                StepError::Decode(DecodeFailure {
                    path: "$.id".into(),
                    expected: Type::Int,
                    actual: "string".into(),
                    response_operation: None,
                }),
                FailureClass::Test,
            ),
            (
                StepError::Evaluation(EvaluationFailure {
                    code: "division_by_zero",
                    message: "division by zero".into(),
                }),
                FailureClass::Test,
            ),
            (
                StepError::Internal("missing runtime binding".into()),
                FailureClass::Internal,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.failure_class(), expected, "{error:?}");
        }
    }

    #[test]
    fn every_run_error_variant_has_an_explicit_failure_class() {
        let cases = [
            (
                RunError::Browser(BrowserError::BrowserDisconnected),
                FailureClass::Infrastructure,
            ),
            (
                RunError::Provider(ProviderError::BridgeTransport {
                    message: "disconnected".into(),
                }),
                FailureClass::Infrastructure,
            ),
            (
                RunError::Internal("violated invariant".into()),
                FailureClass::Internal,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.failure_class(), expected, "{error:?}");
        }
    }
}
