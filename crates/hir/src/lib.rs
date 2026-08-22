//! Semantic representation lowered exclusively from the canonical typed AST.

use std::{collections::HashMap, time::Duration};

use rowan::ast::AstNode;
use serde::{Deserialize, Serialize};
use webtest_syntax::{
    SyntaxKind,
    ast::{self, BrowserOperation, DomainStatement, FlowStatement},
};
use webtest_text::{FileId, SyntaxOrigin};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TestId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StepId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BindingId(pub u32);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HirFile {
    pub tests: Vec<HirTest>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirTest {
    pub id: TestId,
    pub name: String,
    pub body: Vec<HirStmt>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirStmt {
    Server(HirServerBlock),
    Browser(HirBrowserBlock),
    Let(HirLet),
    Expression(HirExpressionStmt),
    Expect(HirExpectation),
    BrowserOperation(HirBrowserOp),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirServerBlock {
    pub statements: Vec<HirStmt>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirBrowserBlock {
    pub statements: Vec<HirStmt>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirLet {
    pub id: BindingId,
    pub name: String,
    pub annotation: Option<HirType>,
    pub value: HirExpr,
    pub origin: SyntaxOrigin,
    pub name_origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirExpressionStmt {
    pub expression: HirExpr,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirExpectation {
    pub expression: HirExpr,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct HirOpen {
    pub url: HirExpr,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirEvaluate {
    pub expression: HirString,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirLocatorAction {
    pub locator: HirLocator,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirValueAction {
    pub locator: HirLocator,
    pub value: HirExpr,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirLocatorExpectation {
    pub locator: HirLocator,
    pub state: LocatorState,
    pub timeout: Option<Duration>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirUrlExpectation {
    pub url: HirString,
    pub timeout: Option<Duration>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirString {
    pub value: String,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirLocator {
    pub kind: HirLocatorKind,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct HirExpr {
    pub kind: HirExprKind,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirExprKind {
    Literal(HirLiteral),
    Name(HirNameRef),
    List(Vec<HirExpr>),
    Record(Vec<HirRecordField>),
    Member {
        receiver: Box<HirExpr>,
        member: String,
        member_origin: SyntaxOrigin,
    },
    Call {
        callee: Box<HirExpr>,
        arguments: Vec<HirCallArgument>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<HirExpr>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<HirExpr>,
        right: Box<HirExpr>,
    },
    Missing,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirLiteral {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Duration(Duration),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirNameRef {
    Binding { id: BindingId, name: String },
    Unresolved(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirRecordField {
    pub name: String,
    pub value: HirExpr,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirCallArgument {
    pub name: Option<String>,
    pub value: HirExpr,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOperator {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOperator {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    And,
    Or,
    Contains,
    Matches,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirType {
    pub kind: HirTypeKind,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirTypeKind {
    Named(String),
    Generic {
        name: String,
        argument: Box<HirType>,
    },
    Record(Vec<HirTypeField>),
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirTypeField {
    pub name: String,
    pub ty: HirType,
    pub optional: bool,
    pub origin: SyntaxOrigin,
}

pub fn lower(file: FileId, parsed: &webtest_syntax::Parse) -> HirFile {
    let Some(root) = ast::Root::cast(parsed.syntax()) else {
        return HirFile::default();
    };
    let mut next_binding = 0;
    HirFile {
        tests: root
            .tests()
            .enumerate()
            .filter_map(|(index, test)| lower_test(file, index as u32, test, &mut next_binding))
            .collect(),
    }
}

struct LowerContext {
    file: FileId,
    bindings: HashMap<String, BindingId>,
    next_binding: u32,
}

fn lower_test(
    file: FileId,
    id: u32,
    test: ast::TestDecl,
    next_binding: &mut u32,
) -> Option<HirTest> {
    let name = test.name()?.value()?;
    let mut context = LowerContext {
        file,
        bindings: HashMap::new(),
        next_binding: *next_binding,
    };
    let body = test
        .statements()
        .filter_map(|statement| lower_flow_statement(&mut context, statement))
        .collect();
    *next_binding = context.next_binding;
    Some(HirTest {
        id: TestId(id),
        name,
        body,
        origin: origin(file, test.syntax()),
    })
}

fn lower_flow_statement(context: &mut LowerContext, statement: FlowStatement) -> Option<HirStmt> {
    match statement {
        FlowStatement::Server(block) => {
            let statements = block
                .statements()
                .filter_map(|statement| lower_domain_statement(context, statement))
                .collect();
            Some(HirStmt::Server(HirServerBlock {
                statements,
                origin: origin(context.file, block.syntax()),
            }))
        }
        FlowStatement::Browser(block) => {
            let statements = block
                .statements()
                .filter_map(|statement| lower_domain_statement(context, statement))
                .collect();
            Some(HirStmt::Browser(HirBrowserBlock {
                statements,
                origin: origin(context.file, block.syntax()),
            }))
        }
        FlowStatement::Let(statement) => lower_let(context, statement).map(HirStmt::Let),
        FlowStatement::Expression(statement) => {
            lower_expression_statement(context, statement).map(HirStmt::Expression)
        }
        FlowStatement::Expect(statement) => {
            lower_expectation(context, statement).map(HirStmt::Expect)
        }
    }
}

fn lower_domain_statement(
    context: &mut LowerContext,
    statement: DomainStatement,
) -> Option<HirStmt> {
    match statement {
        DomainStatement::Let(statement) => lower_let(context, statement).map(HirStmt::Let),
        DomainStatement::Expression(statement) => {
            lower_expression_statement(context, statement).map(HirStmt::Expression)
        }
        DomainStatement::Expect(statement) => {
            lower_expectation(context, statement).map(HirStmt::Expect)
        }
        DomainStatement::Browser(operation) => {
            lower_browser_operation(context, operation).map(HirStmt::BrowserOperation)
        }
    }
}

fn lower_let(context: &mut LowerContext, statement: ast::LetStmt) -> Option<HirLet> {
    let name_token = statement.name()?;
    let name = name_token.text().to_string();
    let value = lower_expr(context, statement.value()?)?;
    let id = BindingId(context.next_binding);
    context.next_binding += 1;
    context.bindings.entry(name.clone()).or_insert(id);
    Some(HirLet {
        id,
        name,
        annotation: statement
            .annotation()
            .and_then(|ty| lower_type(context.file, ty)),
        value,
        origin: origin(context.file, statement.syntax()),
        name_origin: SyntaxOrigin::new(context.file, name_token.text_range()),
    })
}

fn lower_expression_statement(
    context: &mut LowerContext,
    statement: ast::ExprStmt,
) -> Option<HirExpressionStmt> {
    Some(HirExpressionStmt {
        expression: lower_expr(context, statement.expression()?)?,
        origin: origin(context.file, statement.syntax()),
    })
}

fn lower_expectation(
    context: &mut LowerContext,
    statement: ast::ExpectExprStmt,
) -> Option<HirExpectation> {
    Some(HirExpectation {
        expression: lower_expr(context, statement.expression()?)?,
        origin: origin(context.file, statement.syntax()),
    })
}

fn lower_browser_operation(
    context: &mut LowerContext,
    operation: BrowserOperation,
) -> Option<HirBrowserOp> {
    let operation = match operation {
        BrowserOperation::Open(statement) => HirBrowserOp::Open(HirOpen {
            url: lower_expr(context, statement.expression()?)?,
            origin: origin(context.file, statement.syntax()),
        }),
        BrowserOperation::Evaluate(statement) => HirBrowserOp::Evaluate(HirEvaluate {
            expression: lower_string(context.file, statement.expression()?)?,
            origin: origin(context.file, statement.syntax()),
        }),
        BrowserOperation::Click(statement) => HirBrowserOp::Click(locator_action(
            context.file,
            statement.syntax(),
            statement.locator()?,
        )?),
        BrowserOperation::Fill(statement) => HirBrowserOp::Fill(value_action(
            context,
            statement.syntax(),
            statement.locator()?,
            statement.value_expression()?,
        )?),
        BrowserOperation::Type(statement) => HirBrowserOp::Type(value_action(
            context,
            statement.syntax(),
            statement.locator()?,
            statement.value_expression()?,
        )?),
        BrowserOperation::Press(statement) => HirBrowserOp::Press(value_action(
            context,
            statement.syntax(),
            statement.locator()?,
            statement.value_expression()?,
        )?),
        BrowserOperation::Check(statement) => HirBrowserOp::Check(locator_action(
            context.file,
            statement.syntax(),
            statement.locator()?,
        )?),
        BrowserOperation::Uncheck(statement) => HirBrowserOp::Uncheck(locator_action(
            context.file,
            statement.syntax(),
            statement.locator()?,
        )?),
        BrowserOperation::Select(statement) => HirBrowserOp::Select(value_action(
            context,
            statement.syntax(),
            statement.locator()?,
            statement.value_expression()?,
        )?),
        BrowserOperation::Hover(statement) => HirBrowserOp::Hover(locator_action(
            context.file,
            statement.syntax(),
            statement.locator()?,
        )?),
        BrowserOperation::WaitLocator(statement) => HirBrowserOp::WaitLocator(locator_expectation(
            context.file,
            statement.syntax(),
            statement.locator()?,
            statement.state()?,
            statement.timeout().and_then(|it| it.value()),
        )?),
        BrowserOperation::ExpectLocator(statement) => {
            HirBrowserOp::ExpectLocator(locator_expectation(
                context.file,
                statement.syntax(),
                statement.locator()?,
                statement.state()?,
                statement.timeout().and_then(|it| it.value()),
            )?)
        }
        BrowserOperation::WaitUrl(statement) => HirBrowserOp::WaitUrl(url_expectation(
            context.file,
            statement.syntax(),
            statement.url()?,
            statement.timeout().and_then(|it| it.value()),
        )?),
        BrowserOperation::ExpectUrl(statement) => HirBrowserOp::ExpectUrl(url_expectation(
            context.file,
            statement.syntax(),
            statement.url()?,
            statement.timeout().and_then(|it| it.value()),
        )?),
    };
    Some(operation)
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
    context: &mut LowerContext,
    syntax: &webtest_syntax::SyntaxNode,
    locator: ast::Locator,
    value: ast::Expr,
) -> Option<HirValueAction> {
    Some(HirValueAction {
        locator: lower_locator(context.file, locator)?,
        value: lower_expr(context, value)?,
        origin: origin(context.file, syntax),
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

fn lower_expr(context: &LowerContext, expression: ast::Expr) -> Option<HirExpr> {
    let expression_origin = origin(context.file, expression.syntax());
    let kind = match expression {
        ast::Expr::Literal(literal) => {
            let token = literal.token()?;
            HirExprKind::Literal(match token.kind() {
                SyntaxKind::String => HirLiteral::String(ast::StringToken::cast(token)?.value()?),
                SyntaxKind::Int => HirLiteral::Int(token.text().parse().ok()?),
                SyntaxKind::Float => HirLiteral::Float(token.text().parse().ok()?),
                SyntaxKind::TrueKw => HirLiteral::Bool(true),
                SyntaxKind::FalseKw => HirLiteral::Bool(false),
                SyntaxKind::NullKw => HirLiteral::Null,
                SyntaxKind::Duration => {
                    HirLiteral::Duration(ast::DurationToken::cast(token)?.value()?)
                }
                _ => return None,
            })
        }
        ast::Expr::Name(name) => {
            let name = name.name()?.text().to_string();
            HirExprKind::Name(if let Some(id) = context.bindings.get(&name).copied() {
                HirNameRef::Binding { id, name }
            } else {
                HirNameRef::Unresolved(name)
            })
        }
        ast::Expr::List(list) => HirExprKind::List(
            list.items()
                .filter_map(|item| lower_expr(context, item))
                .collect(),
        ),
        ast::Expr::Record(record) => HirExprKind::Record(
            record
                .fields()
                .filter_map(|field| {
                    Some(HirRecordField {
                        name: field.name()?,
                        value: lower_expr(context, field.value()?)?,
                        origin: origin(context.file, field.syntax()),
                    })
                })
                .collect(),
        ),
        ast::Expr::Member(member) => {
            let member_token = member.member()?;
            HirExprKind::Member {
                receiver: Box::new(lower_expr(context, member.receiver()?)?),
                member: member_token.text().into(),
                member_origin: SyntaxOrigin::new(context.file, member_token.text_range()),
            }
        }
        ast::Expr::Call(call) => HirExprKind::Call {
            callee: Box::new(lower_expr(context, call.callee()?)?),
            arguments: call
                .arguments()
                .filter_map(|argument| {
                    Some(HirCallArgument {
                        name: argument.name(),
                        value: lower_expr(context, argument.value()?)?,
                        origin: origin(context.file, argument.syntax()),
                    })
                })
                .collect(),
        },
        ast::Expr::Unary(unary) => HirExprKind::Unary {
            operator: match unary.operator()? {
                SyntaxKind::Bang => UnaryOperator::Not,
                SyntaxKind::Minus => UnaryOperator::Negate,
                _ => return None,
            },
            operand: Box::new(lower_expr(context, unary.operand()?)?),
        },
        ast::Expr::Binary(binary) => {
            let mut operands = binary.operands();
            let left = lower_expr(context, operands.next()?)?;
            let right = lower_expr(context, operands.next()?)?;
            HirExprKind::Binary {
                operator: match binary.operator()? {
                    SyntaxKind::EqEq => BinaryOperator::Equal,
                    SyntaxKind::BangEq => BinaryOperator::NotEqual,
                    SyntaxKind::Lt => BinaryOperator::Less,
                    SyntaxKind::LtEq => BinaryOperator::LessEqual,
                    SyntaxKind::Gt => BinaryOperator::Greater,
                    SyntaxKind::GtEq => BinaryOperator::GreaterEqual,
                    SyntaxKind::Plus => BinaryOperator::Add,
                    SyntaxKind::Minus => BinaryOperator::Subtract,
                    SyntaxKind::Star => BinaryOperator::Multiply,
                    SyntaxKind::Slash => BinaryOperator::Divide,
                    SyntaxKind::AndAnd => BinaryOperator::And,
                    SyntaxKind::OrOr => BinaryOperator::Or,
                    SyntaxKind::ContainsKw => BinaryOperator::Contains,
                    SyntaxKind::MatchesKw => BinaryOperator::Matches,
                    _ => return None,
                },
                left: Box::new(left),
                right: Box::new(right),
            }
        }
        ast::Expr::Parenthesized(expression) => {
            return lower_expr(context, expression.expression()?);
        }
    };
    Some(HirExpr {
        kind,
        origin: expression_origin,
    })
}

fn lower_type(file: FileId, ty: ast::TypeExpr) -> Option<HirType> {
    let ty_origin = origin(file, ty.syntax());
    let kind = match ty {
        ast::TypeExpr::Named(ty) => HirTypeKind::Named(ty.name()?),
        ast::TypeExpr::Generic(ty) => HirTypeKind::Generic {
            name: ty.name()?,
            argument: Box::new(lower_type(file, ty.argument()?)?),
        },
        ast::TypeExpr::Record(record) => HirTypeKind::Record(
            record
                .fields()
                .filter_map(|field| {
                    Some(HirTypeField {
                        name: field.name()?,
                        ty: lower_type(file, field.ty()?)?,
                        optional: field.optional(),
                        origin: origin(file, field.syntax()),
                    })
                })
                .collect(),
        ),
    };
    Some(HirType {
        kind,
        origin: ty_origin,
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
    fn lowers_bindings_types_and_references_with_precise_origins() {
        let source = r#"test "x" {
            server {
                let response = http.get("/user")
                let user: { id: Int, email: String } = response.json
                expect user.id == 1
            }
        }"#;
        let parsed = webtest_syntax::parse(source);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
        let hir = lower(FileId::new(7), &parsed);
        let HirStmt::Server(block) = &hir.tests[0].body[0] else {
            panic!("server block")
        };
        let HirStmt::Let(response) = &block.statements[0] else {
            panic!("response binding")
        };
        let HirStmt::Let(user) = &block.statements[1] else {
            panic!("user binding")
        };
        assert_eq!(response.id, BindingId(0));
        assert_eq!(user.id, BindingId(1));
        let HirExprKind::Member { receiver, .. } = &user.value.kind else {
            panic!("member")
        };
        assert!(matches!(
            receiver.kind,
            HirExprKind::Name(HirNameRef::Binding {
                id: BindingId(0),
                ..
            })
        ));
        let annotation = user.annotation.as_ref().expect("annotation");
        let range = annotation.origin.range;
        assert_eq!(
            &source[usize::from(range.start())..usize::from(range.end())],
            "{ id: Int, email: String }"
        );
    }
}
