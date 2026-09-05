use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use webtest_browser::{
    Action, BrowserError, Locator as BrowserLocator, LocatorState as BrowserLocatorState, Page,
};
use webtest_model::Value;
use webtest_plan::{BrowserOperation, Locator, LocatorState, PlanExpr};

use crate::RunnerOptions;

use super::{browser_locator, browser_state, execute_browser};

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Open(String),
    Evaluate(String),
    Action(Action, Duration),
    WaitForLocator(BrowserLocator, BrowserLocatorState, Duration),
    WaitForUrl(String, Duration),
}

#[derive(Default)]
struct RecordingPage {
    calls: Vec<Call>,
}

#[async_trait]
impl Page for RecordingPage {
    async fn open(&mut self, url: &str) -> Result<(), BrowserError> {
        self.calls.push(Call::Open(url.into()));
        Ok(())
    }

    async fn click(&mut self, _locator: &BrowserLocator) -> Result<(), BrowserError> {
        unreachable!("execute_browser uses perform")
    }

    async fn expect_visible(&mut self, _locator: &BrowserLocator) -> Result<(), BrowserError> {
        unreachable!("execute_browser uses wait_for_locator")
    }

    async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError> {
        self.calls.push(Call::Evaluate(expression.into()));
        Ok(())
    }

    async fn perform(&mut self, action: &Action, timeout: Duration) -> Result<(), BrowserError> {
        self.calls.push(Call::Action(action.clone(), timeout));
        Ok(())
    }

    async fn wait_for_locator(
        &mut self,
        locator: &BrowserLocator,
        state: BrowserLocatorState,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        self.calls
            .push(Call::WaitForLocator(locator.clone(), state, timeout));
        Ok(())
    }

    async fn wait_for_url(
        &mut self,
        expected: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        self.calls.push(Call::WaitForUrl(expected.into(), timeout));
        Ok(())
    }
}

fn literal(value: &str) -> PlanExpr {
    PlanExpr::Literal(Value::String(value.into()))
}

#[tokio::test]
async fn every_browser_operation_maps_to_the_page_with_current_timeouts() {
    let locator = Locator::Id("target".into());
    let operations = vec![
        BrowserOperation::Navigate {
            url: literal("/login"),
        },
        BrowserOperation::Evaluate {
            expression: "document.title".into(),
        },
        BrowserOperation::Click {
            locator: locator.clone(),
        },
        BrowserOperation::Fill {
            locator: locator.clone(),
            value: literal("fill"),
        },
        BrowserOperation::Type {
            locator: locator.clone(),
            value: literal("type"),
        },
        BrowserOperation::Press {
            locator: locator.clone(),
            key: literal("Enter"),
        },
        BrowserOperation::Check {
            locator: locator.clone(),
            checked: true,
        },
        BrowserOperation::Select {
            locator: locator.clone(),
            option: literal("UTC"),
        },
        BrowserOperation::Hover {
            locator: locator.clone(),
        },
        BrowserOperation::WaitForLocator {
            locator: locator.clone(),
            state: LocatorState::Enabled,
            timeout: Some(Duration::from_secs(9)),
        },
        BrowserOperation::WaitForUrl {
            url: literal("next"),
            timeout: None,
        },
    ];
    let options = RunnerOptions {
        base_url: Some("http://example.test/base".into()),
        action_timeout: Duration::from_secs(3),
        assertion_timeout: Duration::from_secs(5),
        test_timeout: Duration::from_secs(4),
        ..RunnerOptions::default()
    };
    let mut page = RecordingPage::default();
    for operation in &operations {
        execute_browser(
            &mut page,
            operation,
            &HashMap::new(),
            &options,
            options.test_timeout,
        )
        .await
        .expect("browser operation");
    }

    let target = BrowserLocator::Id("target".into());
    assert_eq!(
        page.calls,
        [
            Call::Open("http://example.test/login".into()),
            Call::Evaluate("document.title".into()),
            Call::Action(
                Action::Click {
                    locator: target.clone()
                },
                Duration::from_secs(3)
            ),
            Call::Action(
                Action::Fill {
                    locator: target.clone(),
                    value: "fill".into()
                },
                Duration::from_secs(3)
            ),
            Call::Action(
                Action::Type {
                    locator: target.clone(),
                    value: "type".into()
                },
                Duration::from_secs(3)
            ),
            Call::Action(
                Action::Press {
                    locator: target.clone(),
                    key: "Enter".into()
                },
                Duration::from_secs(3)
            ),
            Call::Action(
                Action::Check {
                    locator: target.clone(),
                    checked: true
                },
                Duration::from_secs(3)
            ),
            Call::Action(
                Action::Select {
                    locator: target.clone(),
                    option: "UTC".into()
                },
                Duration::from_secs(3)
            ),
            Call::Action(
                Action::Hover {
                    locator: target.clone()
                },
                Duration::from_secs(3)
            ),
            Call::WaitForLocator(target, BrowserLocatorState::Enabled, Duration::from_secs(4)),
            Call::WaitForUrl(
                "http://example.test/base/next".into(),
                Duration::from_secs(4)
            ),
        ]
    );
}

#[test]
fn every_locator_and_state_variant_is_translated_explicitly() {
    let locators = [
        (Locator::Id("id".into()), BrowserLocator::Id("id".into())),
        (
            Locator::Role {
                role: "button".into(),
                name: Some("Save".into()),
            },
            BrowserLocator::Role {
                role: "button".into(),
                name: Some("Save".into()),
            },
        ),
        (
            Locator::Label("Email".into()),
            BrowserLocator::Label("Email".into()),
        ),
        (
            Locator::Text("Welcome".into()),
            BrowserLocator::Text("Welcome".into()),
        ),
        (
            Locator::Placeholder("Search".into()),
            BrowserLocator::Placeholder("Search".into()),
        ),
        (
            Locator::TestId("submit".into()),
            BrowserLocator::TestId("submit".into()),
        ),
        (
            Locator::Css("main".into()),
            BrowserLocator::Css("main".into()),
        ),
        (
            Locator::XPath("//main".into()),
            BrowserLocator::XPath("//main".into()),
        ),
    ];
    for (plan, browser) in locators {
        assert_eq!(browser_locator(&plan), browser);
    }

    for (plan, browser) in [
        (LocatorState::Visible, BrowserLocatorState::Visible),
        (LocatorState::Hidden, BrowserLocatorState::Hidden),
        (LocatorState::Attached, BrowserLocatorState::Attached),
        (LocatorState::Detached, BrowserLocatorState::Detached),
        (LocatorState::Enabled, BrowserLocatorState::Enabled),
        (LocatorState::Disabled, BrowserLocatorState::Disabled),
        (LocatorState::Checked, BrowserLocatorState::Checked),
        (LocatorState::Unchecked, BrowserLocatorState::Unchecked),
    ] {
        assert_eq!(browser_state(plan), browser);
    }
}
