//! Semantic representation lowered exclusively from the canonical typed AST.

use std::time::Duration;

use rowan::ast::AstNode;
use serde::{Deserialize, Serialize};
use webtest_syntax::ast::{self, BrowserOperation};
use webtest_text::{FileId, SyntaxOrigin};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TestId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StepId(pub u32);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HirFile {
    pub tests: Vec<HirTest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirTest {
    pub id: TestId,
    pub name: String,
    pub body: Vec<HirStmt>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirStmt {
    Browser(HirBrowserBlock),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirBrowserBlock {
    pub operations: Vec<HirBrowserOp>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirBrowserOp {
    Open(HirOpen),
    Evaluate(HirEvaluate),
    Click(HirLocatorAction),
    Fill(HirValueAction),
    Type(HirValueAction),
    Press(HirValueAction),
    Check(HirLocatorAction),
    Uncheck(HirLocatorAction),
    Select(HirValueAction),
    Hover(HirLocatorAction),
    WaitLocator(HirLocatorExpectation),
    WaitUrl(HirUrlExpectation),
    ExpectLocator(HirLocatorExpectation),
    ExpectUrl(HirUrlExpectation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirOpen {
    pub url: HirString,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirEvaluate {
    pub expression: HirString,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirLocatorAction {
    pub locator: HirLocator,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirValueAction {
    pub locator: HirLocator,
    pub value: HirString,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirLocatorExpectation {
    pub locator: HirLocator,
    pub state: LocatorState,
    pub timeout: Option<Duration>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirUrlExpectation {
    pub url: HirString,
    pub timeout: Option<Duration>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirString {
    pub value: String,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirLocator {
    pub kind: HirLocatorKind,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirLocatorKind {
    Id(String),
    Role { role: String, name: Option<String> },
    Label(String),
    Text(String),
    Placeholder(String),
    TestId(String),
    Css(String),
    XPath(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

impl From<ast::LocatorState> for LocatorState {
    fn from(value: ast::LocatorState) -> Self {
        match value {
            ast::LocatorState::Visible => Self::Visible,
            ast::LocatorState::Hidden => Self::Hidden,
            ast::LocatorState::Attached => Self::Attached,
            ast::LocatorState::Detached => Self::Detached,
            ast::LocatorState::Enabled => Self::Enabled,
            ast::LocatorState::Disabled => Self::Disabled,
            ast::LocatorState::Checked => Self::Checked,
            ast::LocatorState::Unchecked => Self::Unchecked,
        }
    }
}

pub fn lower(file: FileId, parsed: &webtest_syntax::Parse) -> HirFile {
    let Some(root) = ast::Root::cast(parsed.syntax()) else {
        return HirFile::default();
    };
    HirFile {
        tests: root
            .tests()
            .enumerate()
            .filter_map(|(index, test)| lower_test(file, index as u32, test))
            .collect(),
    }
}

fn lower_test(file: FileId, id: u32, test: ast::TestDecl) -> Option<HirTest> {
    let name = test.name()?.value()?;
    let body = test
        .browser_blocks()
        .map(|block| HirStmt::Browser(lower_browser_block(file, block)))
        .collect();
    Some(HirTest {
        id: TestId(id),
        name,
        body,
        origin: origin(file, test.syntax()),
    })
}

fn lower_browser_block(file: FileId, block: ast::BrowserBlock) -> HirBrowserBlock {
    let operations = block
        .operations()
        .filter_map(|operation| lower_operation(file, operation))
        .collect();
    HirBrowserBlock {
        operations,
        origin: origin(file, block.syntax()),
    }
}

fn lower_operation(file: FileId, operation: BrowserOperation) -> Option<HirBrowserOp> {
    match operation {
        BrowserOperation::Open(statement) => Some(HirBrowserOp::Open(HirOpen {
            url: lower_string(file, statement.url()?)?,
            origin: origin(file, statement.syntax()),
        })),
        BrowserOperation::Evaluate(statement) => Some(HirBrowserOp::Evaluate(HirEvaluate {
            expression: lower_string(file, statement.expression()?)?,
            origin: origin(file, statement.syntax()),
        })),
        BrowserOperation::Click(statement) => Some(HirBrowserOp::Click(locator_action(
            file,
            statement.syntax(),
            statement.locator()?,
        )?)),
        BrowserOperation::Fill(statement) => Some(HirBrowserOp::Fill(value_action(
            file,
            statement.syntax(),
            statement.locator()?,
            statement.value()?,
        )?)),
        BrowserOperation::Type(statement) => Some(HirBrowserOp::Type(value_action(
            file,
            statement.syntax(),
            statement.locator()?,
            statement.value()?,
        )?)),
        BrowserOperation::Press(statement) => Some(HirBrowserOp::Press(value_action(
            file,
            statement.syntax(),
            statement.locator()?,
            statement.key()?,
        )?)),
        BrowserOperation::Check(statement) => Some(HirBrowserOp::Check(locator_action(
            file,
            statement.syntax(),
            statement.locator()?,
        )?)),
        BrowserOperation::Uncheck(statement) => Some(HirBrowserOp::Uncheck(locator_action(
            file,
            statement.syntax(),
            statement.locator()?,
        )?)),
        BrowserOperation::Select(statement) => Some(HirBrowserOp::Select(value_action(
            file,
            statement.syntax(),
            statement.locator()?,
            statement.option()?,
        )?)),
        BrowserOperation::Hover(statement) => Some(HirBrowserOp::Hover(locator_action(
            file,
            statement.syntax(),
            statement.locator()?,
        )?)),
        BrowserOperation::WaitLocator(statement) => {
            Some(HirBrowserOp::WaitLocator(locator_expectation(
                file,
                statement.syntax(),
                statement.locator()?,
                statement.state()?,
                statement.timeout().and_then(|it| it.value()),
            )?))
        }
        BrowserOperation::ExpectLocator(statement) => {
            Some(HirBrowserOp::ExpectLocator(locator_expectation(
                file,
                statement.syntax(),
                statement.locator()?,
                statement.state()?,
                statement.timeout().and_then(|it| it.value()),
            )?))
        }
        BrowserOperation::WaitUrl(statement) => Some(HirBrowserOp::WaitUrl(url_expectation(
            file,
            statement.syntax(),
            statement.url()?,
            statement.timeout().and_then(|it| it.value()),
        )?)),
        BrowserOperation::ExpectUrl(statement) => Some(HirBrowserOp::ExpectUrl(url_expectation(
            file,
            statement.syntax(),
            statement.url()?,
            statement.timeout().and_then(|it| it.value()),
        )?)),
    }
}

fn locator_action(
    file: FileId,
    syntax: &webtest_syntax::SyntaxNode,
    locator: ast::Locator,
) -> Option<HirLocatorAction> {
    Some(HirLocatorAction {
        locator: lower_locator(file, locator)?,
        origin: origin(file, syntax),
    })
}

fn value_action(
    file: FileId,
    syntax: &webtest_syntax::SyntaxNode,
    locator: ast::Locator,
    value: ast::StringToken,
) -> Option<HirValueAction> {
    Some(HirValueAction {
        locator: lower_locator(file, locator)?,
        value: lower_string(file, value)?,
        origin: origin(file, syntax),
    })
}

fn locator_expectation(
    file: FileId,
    syntax: &webtest_syntax::SyntaxNode,
    locator: ast::Locator,
    state: ast::LocatorState,
    timeout: Option<Duration>,
) -> Option<HirLocatorExpectation> {
    Some(HirLocatorExpectation {
        locator: lower_locator(file, locator)?,
        state: state.into(),
        timeout,
        origin: origin(file, syntax),
    })
}

fn url_expectation(
    file: FileId,
    syntax: &webtest_syntax::SyntaxNode,
    url: ast::StringToken,
    timeout: Option<Duration>,
) -> Option<HirUrlExpectation> {
    Some(HirUrlExpectation {
        url: lower_string(file, url)?,
        timeout,
        origin: origin(file, syntax),
    })
}

fn lower_string(file: FileId, token: ast::StringToken) -> Option<HirString> {
    Some(HirString {
        value: token.value()?,
        origin: SyntaxOrigin::new(file, token.syntax().text_range()),
    })
}

fn lower_locator(file: FileId, locator: ast::Locator) -> Option<HirLocator> {
    let (kind, range) = match locator {
        ast::Locator::Id(locator) => (
            HirLocatorKind::Id(locator.value()?.value()?),
            locator.syntax().text_range(),
        ),
        ast::Locator::Role(locator) => (
            HirLocatorKind::Role {
                role: locator.role()?.value()?,
                name: locator.name().and_then(|name| name.value()),
            },
            locator.syntax().text_range(),
        ),
        ast::Locator::Label(locator) => (
            HirLocatorKind::Label(locator.value()?.value()?),
            locator.syntax().text_range(),
        ),
        ast::Locator::Text(locator) => (
            HirLocatorKind::Text(locator.value()?.value()?),
            locator.syntax().text_range(),
        ),
        ast::Locator::Placeholder(locator) => (
            HirLocatorKind::Placeholder(locator.value()?.value()?),
            locator.syntax().text_range(),
        ),
        ast::Locator::TestId(locator) => (
            HirLocatorKind::TestId(locator.value()?.value()?),
            locator.syntax().text_range(),
        ),
        ast::Locator::Css(locator) => (
            HirLocatorKind::Css(locator.value()?.value()?),
            locator.syntax().text_range(),
        ),
        ast::Locator::XPath(locator) => (
            HirLocatorKind::XPath(locator.value()?.value()?),
            locator.syntax().text_range(),
        ),
    };
    Some(HirLocator {
        kind,
        origin: SyntaxOrigin::new(file, range),
    })
}

fn origin(file: FileId, syntax: &webtest_syntax::SyntaxNode) -> SyntaxOrigin {
    SyntaxOrigin::new(file, syntax.text_range())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_all_milestone_b_operations_with_precise_origins() {
        let source = r#"test "x" { browser {
            fill role("textbox", name: "Email") with "alice"
            evaluate "window.saveDraft()"
            press label("Search") key "Enter"
            expect test_id("saved").visible within 5s
            wait url("/done")
        } }"#;
        let parsed = webtest_syntax::parse(source);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
        let hir = lower(FileId::new(7), &parsed);
        let HirStmt::Browser(block) = &hir.tests[0].body[0];
        assert_eq!(block.operations.len(), 5);
        let HirBrowserOp::Fill(fill) = &block.operations[0] else {
            panic!("fill")
        };
        assert_eq!(
            fill.locator.kind,
            HirLocatorKind::Role {
                role: "textbox".into(),
                name: Some("Email".into())
            }
        );
        let range = fill.locator.origin.range;
        assert_eq!(
            &source[usize::from(range.start())..usize::from(range.end())],
            "role(\"textbox\", name: \"Email\")"
        );
        let HirBrowserOp::Evaluate(evaluate) = &block.operations[1] else {
            panic!("evaluate")
        };
        assert_eq!(evaluate.expression.value, "window.saveDraft()");
        let range = evaluate.expression.origin.range;
        assert_eq!(
            &source[usize::from(range.start())..usize::from(range.end())],
            "\"window.saveDraft()\""
        );
        let HirBrowserOp::ExpectLocator(expectation) = &block.operations[3] else {
            panic!("expect")
        };
        assert_eq!(expectation.state, LocatorState::Visible);
        assert_eq!(expectation.timeout, Some(Duration::from_secs(5)));
    }
}
