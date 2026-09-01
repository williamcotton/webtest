use std::{collections::HashMap, time::Duration};

use webtest_browser::{
    Action, Locator as BrowserLocator, LocatorState as BrowserLocatorState, Page,
};
use webtest_hir::BindingId;
use webtest_plan::{
    AssertionOperation, BrowserOperation, Locator, LocatorState, PlannedStep, TestOperation,
};
use webtest_provider::Value;

use crate::{
    RunnerOptions, StepError,
    evaluation::{evaluate, string_value},
    url::resolve_browser_url,
};

pub(super) async fn execute_browser(
    page: &mut dyn Page,
    operation: &BrowserOperation,
    environment: &HashMap<BindingId, Value>,
    options: &RunnerOptions,
    remaining: Duration,
) -> Result<(), StepError> {
    match operation {
        BrowserOperation::Navigate { url } => {
            let url = string_value(evaluate(url, environment)?)?;
            page.open_with_timeout(
                &resolve_url(options.base_url.as_deref(), &url)?,
                bounded_timeout(options.navigation_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Evaluate { expression } => page
            .evaluate_with_timeout(expression, remaining)
            .await
            .map_err(StepError::Browser),
        BrowserOperation::Click { locator } => page
            .perform(
                &Action::Click {
                    locator: browser_locator(locator),
                },
                bounded_timeout(options.action_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::Fill { locator, value } => {
            let value = string_value(evaluate(value, environment)?)?;
            page.perform(
                &Action::Fill {
                    locator: browser_locator(locator),
                    value,
                },
                bounded_timeout(options.action_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Type { locator, value } => {
            let value = string_value(evaluate(value, environment)?)?;
            page.perform(
                &Action::Type {
                    locator: browser_locator(locator),
                    value,
                },
                bounded_timeout(options.action_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Press { locator, key } => {
            let key = string_value(evaluate(key, environment)?)?;
            page.perform(
                &Action::Press {
                    locator: browser_locator(locator),
                    key,
                },
                bounded_timeout(options.action_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Check { locator, checked } => page
            .perform(
                &Action::Check {
                    locator: browser_locator(locator),
                    checked: *checked,
                },
                bounded_timeout(options.action_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::Select { locator, option } => {
            let option = string_value(evaluate(option, environment)?)?;
            page.perform(
                &Action::Select {
                    locator: browser_locator(locator),
                    option,
                },
                bounded_timeout(options.action_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser)
        }
        BrowserOperation::Hover { locator } => page
            .perform(
                &Action::Hover {
                    locator: browser_locator(locator),
                },
                bounded_timeout(options.action_timeout, remaining),
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::WaitForLocator {
            locator,
            state,
            timeout,
        } => page
            .wait_for_locator(
                &browser_locator(locator),
                browser_state(*state),
                bounded_timeout(timeout.unwrap_or(options.assertion_timeout), remaining),
            )
            .await
            .map_err(StepError::Browser),
        BrowserOperation::WaitForUrl { url, timeout } => {
            let url = string_value(evaluate(url, environment)?)?;
            let expected = resolve_url(options.base_url.as_deref(), &url)?;
            page.wait_for_url(
                &expected,
                bounded_timeout(timeout.unwrap_or(options.assertion_timeout), remaining),
            )
            .await
            .map_err(StepError::Browser)
        }
    }
}

pub(crate) fn bounded_timeout(timeout: Duration, remaining: Duration) -> Duration {
    timeout.min(remaining)
}

pub(crate) fn browser_locator(locator: &Locator) -> BrowserLocator {
    match locator {
        Locator::Id(value) => BrowserLocator::Id(value.clone()),
        Locator::Role { role, name } => BrowserLocator::Role {
            role: role.clone(),
            name: name.clone(),
        },
        Locator::Label(value) => BrowserLocator::Label(value.clone()),
        Locator::Text(value) => BrowserLocator::Text(value.clone()),
        Locator::Placeholder(value) => BrowserLocator::Placeholder(value.clone()),
        Locator::TestId(value) => BrowserLocator::TestId(value.clone()),
        Locator::Css(value) => BrowserLocator::Css(value.clone()),
        Locator::XPath(value) => BrowserLocator::XPath(value.clone()),
    }
}

pub(crate) fn browser_state(state: LocatorState) -> BrowserLocatorState {
    match state {
        LocatorState::Visible => BrowserLocatorState::Visible,
        LocatorState::Hidden => BrowserLocatorState::Hidden,
        LocatorState::Attached => BrowserLocatorState::Attached,
        LocatorState::Detached => BrowserLocatorState::Detached,
        LocatorState::Enabled => BrowserLocatorState::Enabled,
        LocatorState::Disabled => BrowserLocatorState::Disabled,
        LocatorState::Checked => BrowserLocatorState::Checked,
        LocatorState::Unchecked => BrowserLocatorState::Unchecked,
    }
}

pub(crate) fn step_browser_locator(step: &PlannedStep) -> Option<BrowserLocator> {
    match &step.operation {
        TestOperation::Browser(BrowserOperation::Click { locator })
        | TestOperation::Browser(BrowserOperation::Fill { locator, .. })
        | TestOperation::Browser(BrowserOperation::Type { locator, .. })
        | TestOperation::Browser(BrowserOperation::Press { locator, .. })
        | TestOperation::Browser(BrowserOperation::Check { locator, .. })
        | TestOperation::Browser(BrowserOperation::Select { locator, .. })
        | TestOperation::Browser(BrowserOperation::Hover { locator })
        | TestOperation::Browser(BrowserOperation::WaitForLocator { locator, .. })
        | TestOperation::Assertion(AssertionOperation::Locator { locator, .. }) => {
            Some(browser_locator(locator))
        }
        _ => None,
    }
}

fn resolve_url(base_url: Option<&str>, value: &str) -> Result<String, StepError> {
    resolve_browser_url(base_url, value).map_err(StepError::Browser)
}

#[cfg(test)]
mod tests;
