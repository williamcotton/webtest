use std::time::Duration;

use rowan::ast::AstNode;

use crate::{SyntaxKind, SyntaxNode, SyntaxToken, WebtestLanguage};

macro_rules! ast_node {
    ($name:ident, $kind:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name {
            syntax: SyntaxNode,
        }
        impl AstNode for $name {
            type Language = WebtestLanguage;
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }
            fn cast(syntax: SyntaxNode) -> Option<Self> {
                Self::can_cast(syntax.kind()).then_some(Self { syntax })
            }
            fn syntax(&self) -> &SyntaxNode {
                &self.syntax
            }
        }
    };
}

ast_node!(Root, Root);
ast_node!(TestDecl, TestDecl);
ast_node!(Block, Block);
ast_node!(ServerBlock, ServerBlock);
ast_node!(BrowserBlock, BrowserBlock);
ast_node!(LetStmt, LetStmt);
ast_node!(ExprStmt, ExprStmt);
ast_node!(ExpectExprStmt, ExpectExprStmt);
ast_node!(OpenStmt, OpenStmt);
ast_node!(EvaluateStmt, EvaluateStmt);
ast_node!(ClickStmt, ClickStmt);
ast_node!(FillStmt, FillStmt);
ast_node!(TypeStmt, TypeStmt);
ast_node!(PressStmt, PressStmt);
ast_node!(CheckStmt, CheckStmt);
ast_node!(UncheckStmt, UncheckStmt);
ast_node!(SelectStmt, SelectStmt);
ast_node!(HoverStmt, HoverStmt);
ast_node!(WaitLocatorStmt, WaitLocatorStmt);
ast_node!(WaitUrlStmt, WaitUrlStmt);
ast_node!(ExpectLocatorStmt, ExpectLocatorStmt);
ast_node!(ExpectUrlStmt, ExpectUrlStmt);
ast_node!(IdLocator, IdLocator);
ast_node!(RoleLocator, RoleLocator);
ast_node!(LabelLocator, LabelLocator);
ast_node!(TextLocator, TextLocator);
ast_node!(PlaceholderLocator, PlaceholderLocator);
ast_node!(TestIdLocator, TestIdLocator);
ast_node!(CssLocator, CssLocator);
ast_node!(XPathLocator, XPathLocator);
ast_node!(LiteralExpr, LiteralExpr);
ast_node!(NameExpr, NameExpr);
ast_node!(ListExpr, ListExpr);
ast_node!(RecordExpr, RecordExpr);
ast_node!(RecordField, RecordField);
ast_node!(MemberExpr, MemberExpr);
ast_node!(CallExpr, CallExpr);
ast_node!(CallArg, CallArg);
ast_node!(UnaryExpr, UnaryExpr);
ast_node!(BinaryExpr, BinaryExpr);
ast_node!(ParenExpr, ParenExpr);
ast_node!(NamedType, NamedType);
ast_node!(GenericType, GenericType);
ast_node!(RecordType, RecordType);
ast_node!(TypeField, TypeField);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StringToken(SyntaxToken);

impl StringToken {
    pub fn cast(token: SyntaxToken) -> Option<Self> {
        (token.kind() == SyntaxKind::String).then_some(Self(token))
    }
    pub fn syntax(&self) -> &SyntaxToken {
        &self.0
    }
    pub fn value(&self) -> Option<String> {
        decode_string(self.0.text())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DurationToken(SyntaxToken);

impl DurationToken {
    pub fn cast(token: SyntaxToken) -> Option<Self> {
        (token.kind() == SyntaxKind::Duration).then_some(Self(token))
    }
    pub fn syntax(&self) -> &SyntaxToken {
        &self.0
    }
    pub fn value(&self) -> Option<Duration> {
        parse_duration(self.0.text())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

impl LocatorState {
    fn from_kind(kind: SyntaxKind) -> Option<Self> {
        match kind {
            SyntaxKind::VisibleKw => Some(Self::Visible),
            SyntaxKind::HiddenKw => Some(Self::Hidden),
            SyntaxKind::AttachedKw => Some(Self::Attached),
            SyntaxKind::DetachedKw => Some(Self::Detached),
            SyntaxKind::EnabledKw => Some(Self::Enabled),
            SyntaxKind::DisabledKw => Some(Self::Disabled),
            SyntaxKind::CheckedKw => Some(Self::Checked),
            SyntaxKind::UncheckedKw => Some(Self::Unchecked),
            _ => None,
        }
    }
}

impl Root {
    pub fn tests(&self) -> impl Iterator<Item = TestDecl> + '_ {
        self.syntax.children().filter_map(TestDecl::cast)
    }
}

impl TestDecl {
    pub fn name(&self) -> Option<StringToken> {
        direct_string(&self.syntax)
    }
    pub fn browser_blocks(&self) -> impl Iterator<Item = BrowserBlock> + '_ {
        self.syntax.descendants().filter_map(BrowserBlock::cast)
    }
    pub fn body(&self) -> Option<Block> {
        self.syntax.children().find_map(Block::cast)
    }
    pub fn statements(&self) -> impl Iterator<Item = FlowStatement> + '_ {
        self.body().into_iter().flat_map(|block| {
            block
                .syntax
                .children()
                .filter_map(FlowStatement::cast)
                .collect::<Vec<_>>()
        })
    }
}

impl BrowserBlock {
    pub fn operations(&self) -> impl Iterator<Item = BrowserOperation> + '_ {
        self.syntax.children().filter_map(BrowserOperation::cast)
    }
    pub fn statements(&self) -> impl Iterator<Item = DomainStatement> + '_ {
        self.syntax.children().filter_map(DomainStatement::cast)
    }
}

impl ServerBlock {
    pub fn statements(&self) -> impl Iterator<Item = DomainStatement> + '_ {
        self.syntax.children().filter_map(DomainStatement::cast)
    }
}

impl LetStmt {
    pub fn name(&self) -> Option<SyntaxToken> {
        self.syntax
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|token| token.kind() == SyntaxKind::Ident)
    }
    pub fn annotation(&self) -> Option<TypeExpr> {
        self.syntax.children().find_map(TypeExpr::cast)
    }
    pub fn value(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

impl ExprStmt {
    pub fn expression(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

impl ExpectExprStmt {
    pub fn expression(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

impl OpenStmt {
    pub fn url(&self) -> Option<StringToken> {
        self.expression()?.literal_string()
    }
    pub fn expression(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

impl EvaluateStmt {
    pub fn expression(&self) -> Option<StringToken> {
        direct_string(&self.syntax)
    }
}

macro_rules! locator_action {
    ($name:ident) => {
        impl $name {
            pub fn locator(&self) -> Option<Locator> {
                child_locator(&self.syntax)
            }
        }
    };
}
locator_action!(ClickStmt);
locator_action!(FillStmt);
locator_action!(TypeStmt);
locator_action!(PressStmt);
locator_action!(CheckStmt);
locator_action!(UncheckStmt);
locator_action!(SelectStmt);
locator_action!(HoverStmt);
locator_action!(WaitLocatorStmt);
locator_action!(ExpectLocatorStmt);

macro_rules! value_action {
    ($name:ident, $method:ident) => {
        impl $name {
            pub fn $method(&self) -> Option<StringToken> {
                self.value_expression()?.literal_string()
            }
            pub fn value_expression(&self) -> Option<Expr> {
                self.syntax.children().find_map(Expr::cast)
            }
        }
    };
}
value_action!(FillStmt, value);
value_action!(TypeStmt, value);
value_action!(PressStmt, key);
value_action!(SelectStmt, option);

impl WaitLocatorStmt {
    pub fn state(&self) -> Option<LocatorState> {
        direct_state(&self.syntax)
    }
    pub fn timeout(&self) -> Option<DurationToken> {
        direct_duration(&self.syntax)
    }
}
impl ExpectLocatorStmt {
    pub fn state(&self) -> Option<LocatorState> {
        direct_state(&self.syntax)
    }
    pub fn timeout(&self) -> Option<DurationToken> {
        direct_duration(&self.syntax)
    }
}
impl WaitUrlStmt {
    pub fn url(&self) -> Option<StringToken> {
        descendant_string(&self.syntax)
    }
    pub fn timeout(&self) -> Option<DurationToken> {
        direct_duration(&self.syntax)
    }
}
impl ExpectUrlStmt {
    pub fn url(&self) -> Option<StringToken> {
        descendant_string(&self.syntax)
    }
    pub fn timeout(&self) -> Option<DurationToken> {
        direct_duration(&self.syntax)
    }
}

macro_rules! simple_locator_value {
    ($name:ident) => {
        impl $name {
            pub fn value(&self) -> Option<StringToken> {
                direct_string(&self.syntax)
            }
        }
    };
}
simple_locator_value!(IdLocator);
simple_locator_value!(LabelLocator);
simple_locator_value!(TextLocator);
simple_locator_value!(PlaceholderLocator);
simple_locator_value!(TestIdLocator);
simple_locator_value!(CssLocator);
simple_locator_value!(XPathLocator);

impl RoleLocator {
    pub fn role(&self) -> Option<StringToken> {
        direct_strings(&self.syntax).next()
    }
    pub fn name(&self) -> Option<StringToken> {
        direct_strings(&self.syntax).nth(1)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BrowserOperation {
    Open(OpenStmt),
    Evaluate(EvaluateStmt),
    Click(ClickStmt),
    Fill(FillStmt),
    Type(TypeStmt),
    Press(PressStmt),
    Check(CheckStmt),
    Uncheck(UncheckStmt),
    Select(SelectStmt),
    Hover(HoverStmt),
    WaitLocator(WaitLocatorStmt),
    WaitUrl(WaitUrlStmt),
    ExpectLocator(ExpectLocatorStmt),
    ExpectUrl(ExpectUrlStmt),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlowStatement {
    Server(ServerBlock),
    Browser(BrowserBlock),
    Let(LetStmt),
    Expression(ExprStmt),
    Expect(ExpectExprStmt),
}

impl FlowStatement {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::ServerBlock => ServerBlock::cast(node).map(Self::Server),
            SyntaxKind::BrowserBlock => BrowserBlock::cast(node).map(Self::Browser),
            SyntaxKind::LetStmt => LetStmt::cast(node).map(Self::Let),
            SyntaxKind::ExprStmt => ExprStmt::cast(node).map(Self::Expression),
            SyntaxKind::ExpectExprStmt => ExpectExprStmt::cast(node).map(Self::Expect),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DomainStatement {
    Let(LetStmt),
    Expression(ExprStmt),
    Expect(ExpectExprStmt),
    Browser(BrowserOperation),
}

impl DomainStatement {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::LetStmt => LetStmt::cast(node).map(Self::Let),
            SyntaxKind::ExprStmt => ExprStmt::cast(node).map(Self::Expression),
            SyntaxKind::ExpectExprStmt => ExpectExprStmt::cast(node).map(Self::Expect),
            _ => BrowserOperation::cast(node).map(Self::Browser),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Expr {
    Literal(LiteralExpr),
    Name(NameExpr),
    List(ListExpr),
    Record(RecordExpr),
    Member(MemberExpr),
    Call(CallExpr),
    Unary(UnaryExpr),
    Binary(BinaryExpr),
    Parenthesized(ParenExpr),
}

impl Expr {
    pub fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::LiteralExpr => LiteralExpr::cast(node).map(Self::Literal),
            SyntaxKind::NameExpr => NameExpr::cast(node).map(Self::Name),
            SyntaxKind::ListExpr => ListExpr::cast(node).map(Self::List),
            SyntaxKind::RecordExpr => RecordExpr::cast(node).map(Self::Record),
            SyntaxKind::MemberExpr => MemberExpr::cast(node).map(Self::Member),
            SyntaxKind::CallExpr => CallExpr::cast(node).map(Self::Call),
            SyntaxKind::UnaryExpr => UnaryExpr::cast(node).map(Self::Unary),
            SyntaxKind::BinaryExpr => BinaryExpr::cast(node).map(Self::Binary),
            SyntaxKind::ParenExpr => ParenExpr::cast(node).map(Self::Parenthesized),
            _ => None,
        }
    }

    pub fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Literal(value) => value.syntax(),
            Self::Name(value) => value.syntax(),
            Self::List(value) => value.syntax(),
            Self::Record(value) => value.syntax(),
            Self::Member(value) => value.syntax(),
            Self::Call(value) => value.syntax(),
            Self::Unary(value) => value.syntax(),
            Self::Binary(value) => value.syntax(),
            Self::Parenthesized(value) => value.syntax(),
        }
    }

    pub fn literal_string(&self) -> Option<StringToken> {
        let Self::Literal(literal) = self else {
            return None;
        };
        literal.token().and_then(StringToken::cast)
    }
}

impl LiteralExpr {
    pub fn token(&self) -> Option<SyntaxToken> {
        first_meaningful_token(&self.syntax)
    }
}

impl NameExpr {
    pub fn name(&self) -> Option<SyntaxToken> {
        first_meaningful_token(&self.syntax)
    }
}

impl ListExpr {
    pub fn items(&self) -> impl Iterator<Item = Expr> + '_ {
        self.syntax.children().filter_map(Expr::cast)
    }
}

impl RecordExpr {
    pub fn fields(&self) -> impl Iterator<Item = RecordField> + '_ {
        self.syntax.children().filter_map(RecordField::cast)
    }
}

impl RecordField {
    pub fn name(&self) -> Option<String> {
        let token = first_meaningful_token(&self.syntax)?;
        if token.kind() == SyntaxKind::String {
            decode_string(token.text())
        } else {
            Some(token.text().into())
        }
    }
    pub fn value(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

impl MemberExpr {
    pub fn receiver(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
    pub fn member(&self) -> Option<SyntaxToken> {
        self.syntax
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| !token.kind().is_trivia())
            .find(|token| token.kind() != SyntaxKind::Dot)
    }
}

impl CallExpr {
    pub fn callee(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
    pub fn arguments(&self) -> impl Iterator<Item = CallArg> + '_ {
        self.syntax.children().filter_map(CallArg::cast)
    }
}

impl CallArg {
    pub fn name(&self) -> Option<String> {
        let has_colon = self
            .syntax
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .any(|token| token.kind() == SyntaxKind::Colon);
        has_colon.then(|| first_meaningful_token(&self.syntax).map(|token| token.text().into()))?
    }
    pub fn value(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

impl UnaryExpr {
    pub fn operator(&self) -> Option<SyntaxKind> {
        first_meaningful_token(&self.syntax).map(|token| token.kind())
    }
    pub fn operand(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

impl BinaryExpr {
    pub fn operands(&self) -> impl Iterator<Item = Expr> + '_ {
        self.syntax.children().filter_map(Expr::cast)
    }
    pub fn operator(&self) -> Option<SyntaxKind> {
        self.syntax
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .map(|token| token.kind())
            .find(|kind| {
                matches!(
                    kind,
                    SyntaxKind::EqEq
                        | SyntaxKind::BangEq
                        | SyntaxKind::Lt
                        | SyntaxKind::LtEq
                        | SyntaxKind::Gt
                        | SyntaxKind::GtEq
                        | SyntaxKind::Plus
                        | SyntaxKind::Minus
                        | SyntaxKind::Star
                        | SyntaxKind::Slash
                        | SyntaxKind::AndAnd
                        | SyntaxKind::OrOr
                        | SyntaxKind::ContainsKw
                        | SyntaxKind::MatchesKw
                )
            })
    }
}

impl ParenExpr {
    pub fn expression(&self) -> Option<Expr> {
        self.syntax.children().find_map(Expr::cast)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeExpr {
    Named(NamedType),
    Generic(GenericType),
    Record(RecordType),
}

impl TypeExpr {
    pub fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::NamedType => NamedType::cast(node).map(Self::Named),
            SyntaxKind::GenericType => GenericType::cast(node).map(Self::Generic),
            SyntaxKind::RecordType => RecordType::cast(node).map(Self::Record),
            _ => None,
        }
    }

    pub fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Named(value) => value.syntax(),
            Self::Generic(value) => value.syntax(),
            Self::Record(value) => value.syntax(),
        }
    }
}

impl NamedType {
    pub fn name(&self) -> Option<String> {
        first_meaningful_token(&self.syntax).map(|token| token.text().into())
    }
}

impl GenericType {
    pub fn name(&self) -> Option<String> {
        first_meaningful_token(&self.syntax).map(|token| token.text().into())
    }
    pub fn argument(&self) -> Option<TypeExpr> {
        self.syntax.children().find_map(TypeExpr::cast)
    }
}

impl RecordType {
    pub fn fields(&self) -> impl Iterator<Item = TypeField> + '_ {
        self.syntax.children().filter_map(TypeField::cast)
    }
}

impl TypeField {
    pub fn name(&self) -> Option<String> {
        let token = first_meaningful_token(&self.syntax)?;
        if token.kind() == SyntaxKind::String {
            decode_string(token.text())
        } else {
            Some(token.text().into())
        }
    }
    pub fn optional(&self) -> bool {
        self.syntax
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .any(|token| token.kind() == SyntaxKind::Question)
    }
    pub fn ty(&self) -> Option<TypeExpr> {
        self.syntax.children().find_map(TypeExpr::cast)
    }
}

impl BrowserOperation {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::OpenStmt => OpenStmt::cast(node).map(Self::Open),
            SyntaxKind::EvaluateStmt => EvaluateStmt::cast(node).map(Self::Evaluate),
            SyntaxKind::ClickStmt => ClickStmt::cast(node).map(Self::Click),
            SyntaxKind::FillStmt => FillStmt::cast(node).map(Self::Fill),
            SyntaxKind::TypeStmt => TypeStmt::cast(node).map(Self::Type),
            SyntaxKind::PressStmt => PressStmt::cast(node).map(Self::Press),
            SyntaxKind::CheckStmt => CheckStmt::cast(node).map(Self::Check),
            SyntaxKind::UncheckStmt => UncheckStmt::cast(node).map(Self::Uncheck),
            SyntaxKind::SelectStmt => SelectStmt::cast(node).map(Self::Select),
            SyntaxKind::HoverStmt => HoverStmt::cast(node).map(Self::Hover),
            SyntaxKind::WaitLocatorStmt => WaitLocatorStmt::cast(node).map(Self::WaitLocator),
            SyntaxKind::WaitUrlStmt => WaitUrlStmt::cast(node).map(Self::WaitUrl),
            SyntaxKind::ExpectLocatorStmt => ExpectLocatorStmt::cast(node).map(Self::ExpectLocator),
            SyntaxKind::ExpectUrlStmt => ExpectUrlStmt::cast(node).map(Self::ExpectUrl),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Locator {
    Id(IdLocator),
    Role(RoleLocator),
    Label(LabelLocator),
    Text(TextLocator),
    Placeholder(PlaceholderLocator),
    TestId(TestIdLocator),
    Css(CssLocator),
    XPath(XPathLocator),
}

impl Locator {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::IdLocator => IdLocator::cast(node).map(Self::Id),
            SyntaxKind::RoleLocator => RoleLocator::cast(node).map(Self::Role),
            SyntaxKind::LabelLocator => LabelLocator::cast(node).map(Self::Label),
            SyntaxKind::TextLocator => TextLocator::cast(node).map(Self::Text),
            SyntaxKind::PlaceholderLocator => PlaceholderLocator::cast(node).map(Self::Placeholder),
            SyntaxKind::TestIdLocator => TestIdLocator::cast(node).map(Self::TestId),
            SyntaxKind::CssLocator => CssLocator::cast(node).map(Self::Css),
            SyntaxKind::XPathLocator => XPathLocator::cast(node).map(Self::XPath),
            _ => None,
        }
    }
}

fn child_locator(node: &SyntaxNode) -> Option<Locator> {
    node.children().find_map(Locator::cast)
}
fn direct_strings(node: &SyntaxNode) -> impl Iterator<Item = StringToken> + '_ {
    node.children_with_tokens()
        .filter_map(|element| element.into_token())
        .filter_map(StringToken::cast)
}
fn direct_string(node: &SyntaxNode) -> Option<StringToken> {
    direct_strings(node).next()
}
fn descendant_string(node: &SyntaxNode) -> Option<StringToken> {
    node.descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .find_map(StringToken::cast)
}
fn first_meaningful_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| !token.kind().is_trivia())
}
fn direct_duration(node: &SyntaxNode) -> Option<DurationToken> {
    node.children_with_tokens()
        .filter_map(|element| element.into_token())
        .find_map(DurationToken::cast)
}
fn direct_state(node: &SyntaxNode) -> Option<LocatorState> {
    node.children_with_tokens()
        .filter_map(|element| element.into_token())
        .find_map(|token| LocatorState::from_kind(token.kind()))
}

fn decode_string(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut result = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match characters.next()? {
            '"' => result.push('"'),
            '\\' => result.push('\\'),
            'n' => result.push('\n'),
            'r' => result.push('\r'),
            't' => result.push('\t'),
            other => {
                result.push('\\');
                result.push(other);
            }
        }
    }
    Some(result)
}

fn parse_duration(text: &str) -> Option<Duration> {
    let (number, multiplier) = if let Some(number) = text.strip_suffix("ms") {
        (number, 1)
    } else if let Some(number) = text.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = text.strip_suffix('m') {
        (number, 60_000)
    } else {
        return None;
    };
    let number = number.parse::<u64>().ok()?;
    (number > 0).then(|| Duration::from_millis(number.saturating_mul(multiplier)))
}
