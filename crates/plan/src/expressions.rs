//! Structural traversal of portable expressions; consumers never reconstruct source.
use crate::{AssertionOperation, BrowserOperation, PlanExpr, TestOperation};
impl PlanExpr {
    pub fn children(&self) -> Vec<&Self> {
        match self {
            Self::Pattern(matcher) => matcher.expressions(),
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
            Self::Pattern(matcher) => matcher.expressions_mut(),
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
            Self::Reactive(_) => Vec::new(),
            Self::Verdict { message, .. } => message.iter().collect(),
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
            Self::Assertion(AssertionOperation::Locator { locator, .. }) => locator.expressions(),
            Self::Browser(value) => match value {
                BrowserOperation::Navigate { url } | BrowserOperation::WaitForUrl { url, .. } => {
                    vec![url]
                }
                BrowserOperation::Fill { value, locator }
                | BrowserOperation::Type { value, locator } => std::iter::once(value)
                    .chain(locator.expressions())
                    .collect(),
                BrowserOperation::Press { key, locator } => {
                    std::iter::once(key).chain(locator.expressions()).collect()
                }
                BrowserOperation::Select { option, locator } => std::iter::once(option)
                    .chain(locator.expressions())
                    .collect(),
                BrowserOperation::Evaluate { .. } => Vec::new(),
                BrowserOperation::Click { locator }
                | BrowserOperation::Check { locator, .. }
                | BrowserOperation::Hover { locator }
                | BrowserOperation::WaitForLocator { locator, .. } => locator.expressions(),
            },
        }
    }
    pub fn expressions_mut(&mut self) -> Vec<&mut PlanExpr> {
        match self {
            Self::Reactive(_) => Vec::new(),
            Self::Verdict { message, .. } => message.iter_mut().collect(),
            Self::AcquireFixture(call) => call
                .arguments
                .iter_mut()
                .map(|argument| &mut argument.value)
                .collect(),
            Self::EvaluatePure(value) => vec![&mut value.expression],
            Self::Provide(value) | Self::Return(value) => vec![&mut value.expression],
            Self::ServerProviderCall(value) => value.arguments.values_mut().collect(),
            Self::Assertion(AssertionOperation::Value {
                actual, expected, ..
            }) => std::iter::once(actual).chain(expected.iter_mut()).collect(),
            Self::Assertion(AssertionOperation::Url { url, .. }) => vec![url],
            Self::Assertion(AssertionOperation::Locator { locator, .. }) => {
                locator.expressions_mut()
            }
            Self::Browser(value) => match value {
                BrowserOperation::Navigate { url } | BrowserOperation::WaitForUrl { url, .. } => {
                    vec![url]
                }
                BrowserOperation::Fill { value, locator }
                | BrowserOperation::Type { value, locator } => std::iter::once(value)
                    .chain(locator.expressions_mut())
                    .collect(),
                BrowserOperation::Press { key, locator } => std::iter::once(key)
                    .chain(locator.expressions_mut())
                    .collect(),
                BrowserOperation::Select { option, locator } => std::iter::once(option)
                    .chain(locator.expressions_mut())
                    .collect(),
                BrowserOperation::Evaluate { .. } => Vec::new(),
                BrowserOperation::Click { locator }
                | BrowserOperation::Check { locator, .. }
                | BrowserOperation::Hover { locator }
                | BrowserOperation::WaitForLocator { locator, .. } => locator.expressions_mut(),
            },
        }
    }
}

impl crate::Locator {
    pub fn expressions(&self) -> Vec<&PlanExpr> {
        match self {
            Self::Dynamic {
                template,
                value,
                name,
            } => std::iter::once(value.as_ref())
                .chain(name.iter().map(|v| v.as_ref()))
                .chain(template.expressions())
                .collect(),
            _ => vec![],
        }
    }
    pub fn expressions_mut(&mut self) -> Vec<&mut PlanExpr> {
        match self {
            Self::Dynamic {
                template,
                value,
                name,
            } => std::iter::once(value.as_mut())
                .chain(name.iter_mut().map(|v| v.as_mut()))
                .chain(template.expressions_mut())
                .collect(),
            _ => vec![],
        }
    }
}
