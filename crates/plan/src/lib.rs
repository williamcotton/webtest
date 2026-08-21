//! Runtime-facing, syntax-independent test plan.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use webtest_hir::{HirBrowserOp, HirFile, HirLocatorKind, HirStmt, StepId, TestId};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestPlan {
    pub file: FileId,
    pub source_revision: SourceRevision,
    pub tests: Vec<PlannedTest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedTest {
    pub id: TestId,
    pub name: String,
    pub steps: Vec<PlannedStep>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedStep {
    pub id: StepId,
    pub operation: TestOperation,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestOperation {
    Browser(BrowserOperation),
    Assertion(AssertionOperation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserOperation {
    Navigate {
        url: String,
    },
    Click {
        locator: Locator,
    },
    Fill {
        locator: Locator,
        value: String,
    },
    Type {
        locator: Locator,
        value: String,
    },
    Press {
        locator: Locator,
        key: String,
    },
    Check {
        locator: Locator,
        checked: bool,
    },
    Select {
        locator: Locator,
        option: String,
    },
    Hover {
        locator: Locator,
    },
    WaitForLocator {
        locator: Locator,
        state: LocatorState,
        timeout: Option<Duration>,
    },
    WaitForUrl {
        url: String,
        timeout: Option<Duration>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssertionOperation {
    Locator {
        locator: Locator,
        state: LocatorState,
        timeout: Option<Duration>,
    },
    Url {
        url: String,
        timeout: Option<Duration>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Locator {
    Id(String),
    Role { role: String, name: Option<String> },
    Label(String),
    Text(String),
    Placeholder(String),
    TestId(String),
    Css(String),
    XPath(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LocatorState {
    Visible,
    Hidden,
    Attached,
    Detached,
    Enabled,
    Disabled,
    Checked,
    Unchecked,
}

impl std::fmt::Display for LocatorState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
            Self::Attached => "attached",
            Self::Detached => "detached",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Checked => "checked",
            Self::Unchecked => "unchecked",
        })
    }
}

pub fn lower(file: FileId, source_revision: SourceRevision, hir: &HirFile) -> TestPlan {
    let mut next_step = 0u32;
    let tests = hir
        .tests
        .iter()
        .map(|test| {
            let mut steps = Vec::new();
            for statement in &test.body {
                let HirStmt::Browser(block) = statement;
                for operation in &block.operations {
                    let (operation, origin) = lower_operation(operation);
                    steps.push(PlannedStep {
                        id: StepId(next_step),
                        operation,
                        origin,
                    });
                    next_step += 1;
                }
            }
            PlannedTest {
                id: test.id,
                name: test.name.clone(),
                steps,
                origin: test.origin,
            }
        })
        .collect();
    TestPlan {
        file,
        source_revision,
        tests,
    }
}

fn lower_operation(operation: &HirBrowserOp) -> (TestOperation, SyntaxOrigin) {
    match operation {
        HirBrowserOp::Open(open) => (
            TestOperation::Browser(BrowserOperation::Navigate {
                url: open.url.value.clone(),
            }),
            open.url.origin,
        ),
        HirBrowserOp::Click(action) => {
            locator_browser(action, |locator| BrowserOperation::Click { locator })
        }
        HirBrowserOp::Fill(action) => value_browser(action, |locator, value| {
            BrowserOperation::Fill { locator, value }
        }),
        HirBrowserOp::Type(action) => value_browser(action, |locator, value| {
            BrowserOperation::Type { locator, value }
        }),
        HirBrowserOp::Press(action) => value_browser(action, |locator, key| {
            BrowserOperation::Press { locator, key }
        }),
        HirBrowserOp::Check(action) => locator_browser(action, |locator| BrowserOperation::Check {
            locator,
            checked: true,
        }),
        HirBrowserOp::Uncheck(action) => {
            locator_browser(action, |locator| BrowserOperation::Check {
                locator,
                checked: false,
            })
        }
        HirBrowserOp::Select(action) => value_browser(action, |locator, option| {
            BrowserOperation::Select { locator, option }
        }),
        HirBrowserOp::Hover(action) => {
            locator_browser(action, |locator| BrowserOperation::Hover { locator })
        }
        HirBrowserOp::WaitLocator(wait) => (
            TestOperation::Browser(BrowserOperation::WaitForLocator {
                locator: lower_locator(&wait.locator.kind),
                state: lower_state(wait.state),
                timeout: wait.timeout,
            }),
            wait.locator.origin,
        ),
        HirBrowserOp::WaitUrl(wait) => (
            TestOperation::Browser(BrowserOperation::WaitForUrl {
                url: wait.url.value.clone(),
                timeout: wait.timeout,
            }),
            wait.url.origin,
        ),
        HirBrowserOp::ExpectLocator(expectation) => (
            TestOperation::Assertion(AssertionOperation::Locator {
                locator: lower_locator(&expectation.locator.kind),
                state: lower_state(expectation.state),
                timeout: expectation.timeout,
            }),
            expectation.locator.origin,
        ),
        HirBrowserOp::ExpectUrl(expectation) => (
            TestOperation::Assertion(AssertionOperation::Url {
                url: expectation.url.value.clone(),
                timeout: expectation.timeout,
            }),
            expectation.url.origin,
        ),
    }
}

fn locator_browser(
    action: &webtest_hir::HirLocatorAction,
    build: impl FnOnce(Locator) -> BrowserOperation,
) -> (TestOperation, SyntaxOrigin) {
    (
        TestOperation::Browser(build(lower_locator(&action.locator.kind))),
        action.locator.origin,
    )
}

fn value_browser(
    action: &webtest_hir::HirValueAction,
    build: impl FnOnce(Locator, String) -> BrowserOperation,
) -> (TestOperation, SyntaxOrigin) {
    (
        TestOperation::Browser(build(
            lower_locator(&action.locator.kind),
            action.value.value.clone(),
        )),
        action.locator.origin,
    )
}

fn lower_locator(locator: &HirLocatorKind) -> Locator {
    match locator {
        HirLocatorKind::Id(value) => Locator::Id(value.clone()),
        HirLocatorKind::Role { role, name } => Locator::Role {
            role: role.clone(),
            name: name.clone(),
        },
        HirLocatorKind::Label(value) => Locator::Label(value.clone()),
        HirLocatorKind::Text(value) => Locator::Text(value.clone()),
        HirLocatorKind::Placeholder(value) => Locator::Placeholder(value.clone()),
        HirLocatorKind::TestId(value) => Locator::TestId(value.clone()),
        HirLocatorKind::Css(value) => Locator::Css(value.clone()),
        HirLocatorKind::XPath(value) => Locator::XPath(value.clone()),
    }
}

fn lower_state(state: webtest_hir::LocatorState) -> LocatorState {
    match state {
        webtest_hir::LocatorState::Visible => LocatorState::Visible,
        webtest_hir::LocatorState::Hidden => LocatorState::Hidden,
        webtest_hir::LocatorState::Attached => LocatorState::Attached,
        webtest_hir::LocatorState::Detached => LocatorState::Detached,
        webtest_hir::LocatorState::Enabled => LocatorState::Enabled,
        webtest_hir::LocatorState::Disabled => LocatorState::Disabled,
        webtest_hir::LocatorState::Checked => LocatorState::Checked,
        webtest_hir::LocatorState::Unchecked => LocatorState::Unchecked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_has_deterministic_steps_revision_and_all_operation_shapes() {
        let source = r#"test "x" { browser {
            open "/login"
            fill label("Email") with "alice"
            click role("button", name: "Sign in")
            expect text("Welcome").visible within 5s
            expect url("/dashboard")
        } }"#;
        let file = FileId::new(4);
        let revision = SourceRevision::of(source);
        let parsed = webtest_syntax::parse(source);
        let hir = webtest_hir::lower(file, &parsed);
        let plan = lower(file, revision, &hir);
        assert_eq!(plan.source_revision, revision);
        for (index, step) in plan.tests[0].steps.iter().enumerate() {
            assert_eq!(step.id, StepId(index as u32));
        }
        assert!(matches!(
            plan.tests[0].steps[1].operation,
            TestOperation::Browser(BrowserOperation::Fill {
                locator: Locator::Label(_),
                ..
            })
        ));
        assert!(matches!(plan.tests[0].steps[3].operation,
            TestOperation::Assertion(AssertionOperation::Locator { timeout: Some(value), .. })
                if value == Duration::from_secs(5)));
    }
}
