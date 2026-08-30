use thiserror::Error;
use webtest_browser::BrowserError;
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

    pub fn is_infrastructure(&self) -> bool {
        match self {
            Self::Browser(error) => error.is_infrastructure(),
            Self::Provider(error) => error.is_infrastructure(),
            Self::Assertion(_) | Self::Decode(_) | Self::Evaluation(_) | Self::Internal(_) => false,
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

#[derive(Debug, Error)]
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
}
