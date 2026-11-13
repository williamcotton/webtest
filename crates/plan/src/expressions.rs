//! Structural traversal of portable expressions; consumers never reconstruct source.
use crate::{AssertionOperation, BrowserOperation, PlanExpr, TestOperation};
impl PlanExpr {
    pub fn children(&self) -> Vec<&Self> {
        match self {
            Self::PureCall {
                arguments, body, ..
            } => arguments
                .iter()
                .map(|(_, value)| value)
                .chain(std::iter::once(body.as_ref()))
                .collect(),
            Self::Lower { value }
            | Self::Decode { value, .. }
            | Self::Member {
                receiver: value, ..
            }
            | Self::Unary { operand: value, .. } => vec![value],
            Self::Binary { left, right, .. } => vec![left, right],
            Self::List(values) => values.iter().collect(),
            Self::Record(fields) => fields.values().collect(),
            Self::Literal(_) | Self::Binding(_) | Self::Type(_) => Vec::new(),
        }
    }
    pub fn children_mut(&mut self) -> Vec<&mut Self> {
        match self {
            Self::PureCall {
                arguments, body, ..
            } => arguments
                .iter_mut()
                .map(|(_, value)| value)
                .chain(std::iter::once(body.as_mut()))
                .collect(),
            Self::Lower { value }
            | Self::Decode { value, .. }
            | Self::Member {
                receiver: value, ..
            }
            | Self::Unary { operand: value, .. } => vec![value],
            Self::Binary { left, right, .. } => vec![left, right],
            Self::List(values) => values.iter_mut().collect(),
            Self::Record(fields) => fields.values_mut().collect(),
            Self::Literal(_) | Self::Binding(_) | Self::Type(_) => Vec::new(),
        }
    }
}
impl TestOperation {
    pub fn expressions(&self) -> Vec<&PlanExpr> {
        match self {
            Self::AcquireFixture(call) => call
                .arguments
                .iter()
                .map(|argument| &argument.value)
                .collect(),
            Self::EvaluatePure(value) => vec![&value.expression],
            Self::Provide(value) | Self::Return(value) => vec![&value.expression],
            Self::ServerProviderCall(value) => value.arguments.values().collect(),
            Self::Assertion(AssertionOperation::Value {
                actual, expected, ..
            }) => std::iter::once(actual).chain(expected.iter()).collect(),
            Self::Assertion(AssertionOperation::Url { url, .. }) => vec![url],
            Self::Assertion(AssertionOperation::Locator { .. }) => Vec::new(),
            Self::Browser(value) => match value {
                BrowserOperation::Navigate { url } | BrowserOperation::WaitForUrl { url, .. } => {
                    vec![url]
                }
                BrowserOperation::Fill { value, .. } | BrowserOperation::Type { value, .. } => {
                    vec![value]
                }
                BrowserOperation::Press { key, .. } => vec![key],
                BrowserOperation::Select { option, .. } => vec![option],
                BrowserOperation::Evaluate { .. }
                | BrowserOperation::Click { .. }
                | BrowserOperation::Check { .. }
                | BrowserOperation::Hover { .. }
                | BrowserOperation::WaitForLocator { .. } => Vec::new(),
            },
        }
    }
    pub fn expressions_mut(&mut self) -> Vec<&mut PlanExpr> {
        match self {
            Self::AcquireFixture(call) => call
                .arguments
                .iter_mut()
                .map(|argument| &mut argument.value)
                .collect(),
            Self::EvaluatePure(value) => vec![&mut value.expression],
            Self::Provide(value) | Self::Return(value) => vec![&mut value.expression],
            Self::ServerProviderCall(value) => value.arguments.values_mut().collect(),
            Self::Assertion(AssertionOperation::Value {
                actual,
                matcher: crate::ValueMatcher::Matches,
                ..
            }) => vec![actual],
            Self::Assertion(AssertionOperation::Value {
                actual, expected, ..
            }) => std::iter::once(actual).chain(expected.iter_mut()).collect(),
            Self::Assertion(AssertionOperation::Url { url, .. }) => vec![url],
            Self::Assertion(AssertionOperation::Locator { .. }) => Vec::new(),
            Self::Browser(value) => match value {
                BrowserOperation::Navigate { url } | BrowserOperation::WaitForUrl { url, .. } => {
                    vec![url]
                }
                BrowserOperation::Fill { value, .. } | BrowserOperation::Type { value, .. } => {
                    vec![value]
                }
                BrowserOperation::Press { key, .. } => vec![key],
                BrowserOperation::Select { option, .. } => vec![option],
                BrowserOperation::Evaluate { .. }
                | BrowserOperation::Click { .. }
                | BrowserOperation::Check { .. }
                | BrowserOperation::Hover { .. }
                | BrowserOperation::WaitForLocator { .. } => Vec::new(),
            },
        }
    }
}
