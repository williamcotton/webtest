use rowan::{GreenNode, GreenNodeBuilder};
use webtest_text::{TextRange, TextSize};

use crate::{SyntaxError, SyntaxKind, SyntaxNode, lexer};

#[derive(Clone, Debug)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }
}

pub fn parse(source: &str) -> Parse {
    Parser::new(source).parse()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockDomain {
    Test,
    Server,
    Browser,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<lexer::Token>,
    position: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<SyntaxError>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        let tokens = lexer::lex(source);
        let errors = tokens
            .iter()
            .filter_map(|token| {
                token
                    .error
                    .map(|(code, message)| SyntaxError::new(token.range, code, message))
            })
            .collect();
        Self {
            source,
            tokens,
            position: 0,
            builder: GreenNodeBuilder::new(),
            errors,
        }
    }

    fn parse(mut self) -> Parse {
        self.start(SyntaxKind::Root);
        loop {
            self.eat_trivia();
            match self.current() {
                SyntaxKind::Eof => break,
                SyntaxKind::TestKw => self.test_decl(),
                _ => self.unexpected("syntax.expected_test", "expected `test` declaration"),
            }
        }
        self.finish();
        Parse {
            green: self.builder.finish(),
            errors: self.errors,
        }
    }

    fn test_decl(&mut self) {
        self.start(SyntaxKind::TestDecl);
        self.bump();
        self.expect(
            SyntaxKind::String,
            "syntax.expected_test_name",
            "expected test name string after `test`",
        );
        self.braced_block(SyntaxKind::Block, BlockDomain::Test);
        self.finish();
    }

    fn braced_block(&mut self, kind: SyntaxKind, domain: BlockDomain) {
        self.start(kind);
        if !self.expect(
            SyntaxKind::LBrace,
            "syntax.expected_lbrace",
            "expected `{` to start block",
        ) {
            self.finish();
            return;
        }
        loop {
            self.eat_trivia();
            match self.current() {
                SyntaxKind::RBrace => {
                    self.bump();
                    break;
                }
                SyntaxKind::Eof => {
                    self.error_here("syntax.expected_rbrace", "expected `}` to close block");
                    break;
                }
                _ => self.statement(domain),
            }
        }
        self.finish();
    }

    fn statement(&mut self, domain: BlockDomain) {
        match (domain, self.current()) {
            (BlockDomain::Test, SyntaxKind::ServerKw) => {
                self.capability_block(SyntaxKind::ServerBlock, BlockDomain::Server)
            }
            (BlockDomain::Test, SyntaxKind::BrowserKw) => {
                self.capability_block(SyntaxKind::BrowserBlock, BlockDomain::Browser)
            }
            (_, SyntaxKind::LetKw) => self.let_statement(),
            (BlockDomain::Browser, SyntaxKind::OpenKw) => self.open_statement(),
            (BlockDomain::Browser, SyntaxKind::EvaluateKw) => self.evaluate_statement(),
            (BlockDomain::Browser, SyntaxKind::ClickKw) => {
                self.locator_action(SyntaxKind::ClickStmt)
            }
            (BlockDomain::Browser, SyntaxKind::FillKw) => {
                self.value_action(SyntaxKind::FillStmt, SyntaxKind::WithKw, "with")
            }
            (BlockDomain::Browser, SyntaxKind::TypeKw) => {
                self.value_action(SyntaxKind::TypeStmt, SyntaxKind::WithKw, "with")
            }
            (BlockDomain::Browser, SyntaxKind::PressKw) => {
                self.value_action(SyntaxKind::PressStmt, SyntaxKind::KeyKw, "key")
            }
            (BlockDomain::Browser, SyntaxKind::CheckKw) => {
                self.locator_action(SyntaxKind::CheckStmt)
            }
            (BlockDomain::Browser, SyntaxKind::UncheckKw) => {
                self.locator_action(SyntaxKind::UncheckStmt)
            }
            (BlockDomain::Browser, SyntaxKind::SelectKw) => {
                self.value_action(SyntaxKind::SelectStmt, SyntaxKind::OptionKw, "option")
            }
            (BlockDomain::Browser, SyntaxKind::HoverKw) => {
                self.locator_action(SyntaxKind::HoverStmt)
            }
            (BlockDomain::Browser, SyntaxKind::WaitKw) => self.wait_statement(),
            (BlockDomain::Browser, SyntaxKind::ExpectKw)
                if self.nth_non_trivia(1) == SyntaxKind::UrlKw
                    || self.nth_non_trivia(1).is_locator_start() =>
            {
                self.browser_expect_statement()
            }
            (_, SyntaxKind::ExpectKw) => self.expression_expect_statement(),
            (_, kind) if self.expression_start(kind) => self.expression_statement(),
            (BlockDomain::Test, _) => self.unexpected(
                "syntax.expected_flow_statement",
                "expected `server`, `browser`, `let`, or assertion in test body",
            ),
            (BlockDomain::Server, _) => self.unexpected(
                "syntax.expected_server_statement",
                "expected binding, provider call, or assertion in server block",
            ),
            (BlockDomain::Browser, _) => self.unexpected(
                "syntax.expected_browser_statement",
                "expected browser action, binding, wait, or assertion in browser block",
            ),
        }
    }

    fn capability_block(&mut self, kind: SyntaxKind, domain: BlockDomain) {
        self.start(kind);
        self.bump();
        if !self.expect(
            SyntaxKind::LBrace,
            "syntax.expected_lbrace",
            "expected `{` to start capability block",
        ) {
            self.finish();
            return;
        }
        loop {
            self.eat_trivia();
            match self.current() {
                SyntaxKind::RBrace => {
                    self.bump();
                    break;
                }
                SyntaxKind::Eof => {
                    self.error_here("syntax.expected_rbrace", "expected `}` to close block");
                    break;
                }
                _ => self.statement(domain),
            }
        }
        self.finish();
    }

    fn let_statement(&mut self) {
        self.start(SyntaxKind::LetStmt);
        self.bump();
        self.expect(
            SyntaxKind::Ident,
            "syntax.expected_binding_name",
            "expected binding name after `let`",
        );
        self.eat_trivia();
        if self.current() == SyntaxKind::Colon {
            self.bump();
            self.type_expression();
        }
        self.expect(
            SyntaxKind::Equal,
            "syntax.expected_equal",
            "expected `=` in binding",
        );
        self.require_expression("expected expression after `=`");
        self.finish();
    }

    fn expression_statement(&mut self) {
        self.start(SyntaxKind::ExprStmt);
        self.require_expression("expected expression");
        self.finish();
    }

    fn expression_expect_statement(&mut self) {
        self.start(SyntaxKind::ExpectExprStmt);
        self.bump();
        self.require_expression("expected assertion expression after `expect`");
        self.finish();
    }

    fn open_statement(&mut self) {
        self.start(SyntaxKind::OpenStmt);
        self.bump();
        self.require_expression("expected URL expression after `open`");
        self.finish();
    }

    fn evaluate_statement(&mut self) {
        self.start(SyntaxKind::EvaluateStmt);
        self.bump();
        self.expect(
            SyntaxKind::String,
            "syntax.expected_expression",
            "expected JavaScript string after `evaluate`",
        );
        self.finish();
    }

    fn locator_action(&mut self, kind: SyntaxKind) {
        self.start(kind);
        self.bump();
        self.require_locator("after action");
        self.finish();
    }

    fn value_action(&mut self, kind: SyntaxKind, separator: SyntaxKind, spelling: &'static str) {
        self.start(kind);
        self.bump();
        self.require_locator("after action");
        self.expect(
            separator,
            "syntax.expected_action_argument",
            match spelling {
                "with" => "expected `with` and a value after locator",
                "key" => "expected `key` and a key after locator",
                _ => "expected `option` and an option after locator",
            },
        );
        self.require_expression("expected action value expression");
        self.finish();
    }

    fn wait_statement(&mut self) {
        let kind = if self.nth_non_trivia(1) == SyntaxKind::UrlKw {
            SyntaxKind::WaitUrlStmt
        } else {
            SyntaxKind::WaitLocatorStmt
        };
        self.start(kind);
        self.bump();
        if kind == SyntaxKind::WaitUrlStmt {
            self.url_expression();
        } else {
            self.require_locator("after `wait`");
            self.locator_state();
        }
        self.optional_within();
        self.finish();
    }

    fn browser_expect_statement(&mut self) {
        let kind = if self.nth_non_trivia(1) == SyntaxKind::UrlKw {
            SyntaxKind::ExpectUrlStmt
        } else {
            SyntaxKind::ExpectLocatorStmt
        };
        self.start(kind);
        self.bump();
        if kind == SyntaxKind::ExpectUrlStmt {
            self.url_expression();
        } else {
            self.require_locator("after `expect`");
            self.locator_state();
        }
        self.optional_within();
        self.finish();
    }

    fn url_expression(&mut self) {
        self.expect(
            SyntaxKind::UrlKw,
            "syntax.expected_url",
            "expected `url` expression",
        );
        self.expect(
            SyntaxKind::LParen,
            "syntax.expected_lparen",
            "expected `(` after `url`",
        );
        self.require_expression("expected URL expression");
        self.expect(
            SyntaxKind::RParen,
            "syntax.expected_rparen",
            "expected `)` after URL",
        );
    }

    fn locator_state(&mut self) {
        self.expect(
            SyntaxKind::Dot,
            "syntax.expected_dot",
            "expected `.` after locator",
        );
        self.eat_trivia();
        if self.current().is_locator_state() {
            self.bump();
        } else {
            self.error_here(
                "syntax.expected_locator_state",
                "expected locator state after `.`",
            );
        }
    }

    fn optional_within(&mut self) {
        self.eat_trivia();
        if self.current() == SyntaxKind::WithinKw {
            self.bump();
            self.expect(
                SyntaxKind::Duration,
                "syntax.expected_duration",
                "expected duration after `within`",
            );
        }
    }

    fn require_locator(&mut self, context: &'static str) {
        self.eat_trivia();
        if !self.locator() {
            self.error_here(
                "syntax.expected_locator",
                format!("expected locator {context}"),
            );
        }
    }

    fn locator(&mut self) -> bool {
        let node_kind = match self.current() {
            SyntaxKind::IdKw => SyntaxKind::IdLocator,
            SyntaxKind::RoleKw => SyntaxKind::RoleLocator,
            SyntaxKind::LabelKw => SyntaxKind::LabelLocator,
            SyntaxKind::TextKw => SyntaxKind::TextLocator,
            SyntaxKind::PlaceholderKw => SyntaxKind::PlaceholderLocator,
            SyntaxKind::TestIdKw => SyntaxKind::TestIdLocator,
            SyntaxKind::CssKw => SyntaxKind::CssLocator,
            SyntaxKind::XPathKw => SyntaxKind::XPathLocator,
            _ => return false,
        };
        self.start(node_kind);
        self.bump();
        self.expect(
            SyntaxKind::LParen,
            "syntax.expected_lparen",
            "expected `(` after locator name",
        );
        self.expect(
            SyntaxKind::String,
            "syntax.expected_locator_string",
            "expected string in locator",
        );
        self.eat_trivia();
        if node_kind == SyntaxKind::RoleLocator && self.current() == SyntaxKind::Comma {
            self.bump();
            self.expect(
                SyntaxKind::NameKw,
                "syntax.expected_role_name",
                "expected `name` after `,`",
            );
            self.expect(
                SyntaxKind::Colon,
                "syntax.expected_colon",
                "expected `:` after `name`",
            );
            self.expect(
                SyntaxKind::String,
                "syntax.expected_role_name_value",
                "expected role name string",
            );
        }
        self.expect(
            SyntaxKind::RParen,
            "syntax.expected_rparen",
            "expected `)` after locator",
        );
        self.finish();
        true
    }

    fn require_expression(&mut self, message: &'static str) {
        self.eat_trivia();
        if !self.expression_start(self.current()) {
            self.error_here("syntax.expected_expression", message);
            return;
        }
        self.expression_bp(0);
    }

    fn expression_bp(&mut self, min_binding_power: u8) {
        self.eat_trivia();
        let checkpoint = self.builder.checkpoint();
        match self.current() {
            SyntaxKind::Bang | SyntaxKind::Minus => {
                self.start(SyntaxKind::UnaryExpr);
                self.bump();
                self.expression_bp(13);
                self.finish();
            }
            SyntaxKind::String
            | SyntaxKind::Int
            | SyntaxKind::Float
            | SyntaxKind::Duration
            | SyntaxKind::TrueKw
            | SyntaxKind::FalseKw
            | SyntaxKind::NullKw => {
                self.start(SyntaxKind::LiteralExpr);
                self.bump();
                self.finish();
            }
            kind if self.name_token(kind) => {
                self.start(SyntaxKind::NameExpr);
                self.bump();
                self.finish();
            }
            SyntaxKind::LBracket => self.list_expression(),
            SyntaxKind::LBrace => self.record_expression(),
            SyntaxKind::LParen => {
                self.start(SyntaxKind::ParenExpr);
                self.bump();
                self.require_expression("expected expression after `(`");
                self.expect(SyntaxKind::RParen, "syntax.expected_rparen", "expected `)`");
                self.finish();
            }
            _ => {
                self.error_here("syntax.expected_expression", "expected expression");
                return;
            }
        }

        loop {
            self.eat_trivia();
            if self.current() == SyntaxKind::Dot {
                self.builder
                    .start_node_at(checkpoint, rowan::SyntaxKind(SyntaxKind::MemberExpr as u16));
                self.bump();
                self.eat_trivia();
                if self.name_token(self.current()) {
                    self.bump();
                } else {
                    self.error_here("syntax.expected_member", "expected member name after `.`");
                }
                self.finish();
                continue;
            }
            if self.current() == SyntaxKind::LParen {
                self.builder
                    .start_node_at(checkpoint, rowan::SyntaxKind(SyntaxKind::CallExpr as u16));
                self.call_arguments();
                self.finish();
                continue;
            }
            let Some((left, right)) = self.binary_binding_power(self.current()) else {
                break;
            };
            if left < min_binding_power {
                break;
            }
            self.builder
                .start_node_at(checkpoint, rowan::SyntaxKind(SyntaxKind::BinaryExpr as u16));
            self.bump();
            self.eat_trivia();
            if self.expression_start(self.current()) {
                self.expression_bp(right);
            } else {
                self.error_here(
                    "syntax.expected_operand",
                    "expected expression after operator",
                );
            }
            self.finish();
        }
    }

    fn list_expression(&mut self) {
        self.start(SyntaxKind::ListExpr);
        self.bump();
        loop {
            self.eat_trivia();
            if self.current() == SyntaxKind::RBracket {
                self.bump();
                break;
            }
            if self.current() == SyntaxKind::Eof || self.current() == SyntaxKind::RBrace {
                self.error_here("syntax.expected_rbracket", "expected `]` to close list");
                break;
            }
            self.require_expression("expected list item");
            self.eat_trivia();
            if self.current() == SyntaxKind::Comma {
                self.bump();
            } else if self.current() != SyntaxKind::RBracket {
                self.error_here("syntax.expected_comma", "expected `,` between list items");
                break;
            }
        }
        self.finish();
    }

    fn record_expression(&mut self) {
        self.start(SyntaxKind::RecordExpr);
        self.bump();
        loop {
            self.eat_trivia();
            if self.current() == SyntaxKind::RBrace {
                self.bump();
                break;
            }
            if self.current() == SyntaxKind::Eof {
                self.error_here("syntax.expected_rbrace", "expected `}` to close record");
                break;
            }
            self.start(SyntaxKind::RecordField);
            if self.current() == SyntaxKind::String || self.name_token(self.current()) {
                self.bump();
            } else {
                self.error_here("syntax.expected_field_name", "expected record field name");
            }
            self.expect(
                SyntaxKind::Colon,
                "syntax.expected_colon",
                "expected `:` after field name",
            );
            self.require_expression("expected record field value");
            self.finish();
            self.eat_trivia();
            if self.current() == SyntaxKind::Comma {
                self.bump();
            } else if self.current() != SyntaxKind::RBrace {
                self.error_here(
                    "syntax.expected_comma",
                    "expected `,` between record fields",
                );
                break;
            }
        }
        self.finish();
    }

    fn call_arguments(&mut self) {
        self.bump();
        loop {
            self.eat_trivia();
            if self.current() == SyntaxKind::RParen {
                self.bump();
                break;
            }
            if self.current() == SyntaxKind::Eof || self.current() == SyntaxKind::RBrace {
                self.error_here("syntax.expected_rparen", "expected `)` to close call");
                break;
            }
            self.start(SyntaxKind::CallArg);
            if self.name_token(self.current()) && self.nth_non_trivia(1) == SyntaxKind::Colon {
                self.bump();
                self.expect(
                    SyntaxKind::Colon,
                    "syntax.expected_colon",
                    "expected `:` after argument name",
                );
            }
            self.require_expression("expected call argument");
            self.finish();
            self.eat_trivia();
            if self.current() == SyntaxKind::Comma {
                self.bump();
            } else if self.current() != SyntaxKind::RParen {
                self.error_here(
                    "syntax.expected_comma",
                    "expected `,` between call arguments",
                );
                break;
            }
        }
    }

    fn type_expression(&mut self) {
        self.eat_trivia();
        if self.current() == SyntaxKind::LBrace {
            self.record_type();
            return;
        }
        if !self.name_token(self.current()) {
            self.error_here("syntax.expected_type", "expected type");
            return;
        }
        let kind = if self.nth_non_trivia(1) == SyntaxKind::Lt {
            SyntaxKind::GenericType
        } else {
            SyntaxKind::NamedType
        };
        self.start(kind);
        self.bump();
        if kind == SyntaxKind::GenericType {
            self.expect(
                SyntaxKind::Lt,
                "syntax.expected_lt",
                "expected `<` after type name",
            );
            self.type_expression();
            self.expect(
                SyntaxKind::Gt,
                "syntax.expected_gt",
                "expected `>` after type argument",
            );
        }
        self.finish();
    }

    fn record_type(&mut self) {
        self.start(SyntaxKind::RecordType);
        self.bump();
        loop {
            self.eat_trivia();
            if self.current() == SyntaxKind::RBrace {
                self.bump();
                break;
            }
            if self.current() == SyntaxKind::Eof {
                self.error_here(
                    "syntax.expected_rbrace",
                    "expected `}` to close record type",
                );
                break;
            }
            self.start(SyntaxKind::TypeField);
            if self.name_token(self.current()) || self.current() == SyntaxKind::String {
                self.bump();
            } else {
                self.error_here("syntax.expected_field_name", "expected type field name");
            }
            self.eat_trivia();
            if self.current() == SyntaxKind::Question {
                self.bump();
            }
            self.expect(
                SyntaxKind::Colon,
                "syntax.expected_colon",
                "expected `:` after type field",
            );
            self.type_expression();
            self.finish();
            self.eat_trivia();
            if self.current() == SyntaxKind::Comma {
                self.bump();
            } else if self.current() != SyntaxKind::RBrace {
                self.error_here("syntax.expected_comma", "expected `,` between type fields");
                break;
            }
        }
        self.finish();
    }

    fn binary_binding_power(&self, kind: SyntaxKind) -> Option<(u8, u8)> {
        Some(match kind {
            SyntaxKind::OrOr => (1, 2),
            SyntaxKind::AndAnd => (3, 4),
            SyntaxKind::EqEq
            | SyntaxKind::BangEq
            | SyntaxKind::ContainsKw
            | SyntaxKind::MatchesKw => (5, 6),
            SyntaxKind::Lt | SyntaxKind::LtEq | SyntaxKind::Gt | SyntaxKind::GtEq => (7, 8),
            SyntaxKind::Plus | SyntaxKind::Minus => (9, 10),
            SyntaxKind::Star | SyntaxKind::Slash => (11, 12),
            _ => return None,
        })
    }

    fn expression_start(&self, kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::String
                | SyntaxKind::Int
                | SyntaxKind::Float
                | SyntaxKind::Duration
                | SyntaxKind::TrueKw
                | SyntaxKind::FalseKw
                | SyntaxKind::NullKw
                | SyntaxKind::Bang
                | SyntaxKind::Minus
                | SyntaxKind::LBracket
                | SyntaxKind::LBrace
                | SyntaxKind::LParen
        ) || self.name_token(kind)
    }

    fn name_token(&self, kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::Ident
                | SyntaxKind::NameKw
                | SyntaxKind::IdKw
                | SyntaxKind::RoleKw
                | SyntaxKind::LabelKw
                | SyntaxKind::TextKw
                | SyntaxKind::PlaceholderKw
                | SyntaxKind::TestIdKw
                | SyntaxKind::CssKw
                | SyntaxKind::XPathKw
                | SyntaxKind::UrlKw
                | SyntaxKind::OptionKw
        )
    }

    fn expect(&mut self, kind: SyntaxKind, code: &'static str, message: &'static str) -> bool {
        self.eat_trivia();
        if self.current() == kind {
            self.bump();
            true
        } else {
            self.error_here(code, message);
            false
        }
    }

    fn unexpected(&mut self, code: &'static str, message: &'static str) {
        self.error_here(code, message);
        if self.current() != SyntaxKind::Eof {
            self.start(SyntaxKind::Error);
            self.bump();
            self.finish();
        }
    }

    fn error_here(&mut self, code: &'static str, message: impl Into<String>) {
        self.eat_trivia();
        self.errors
            .push(SyntaxError::new(self.current_range(), code, message.into()));
    }

    fn eat_trivia(&mut self) {
        while self.current().is_trivia() {
            self.bump();
        }
    }

    fn current(&self) -> SyntaxKind {
        self.tokens
            .get(self.position)
            .map_or(SyntaxKind::Eof, |token| token.kind)
    }

    fn nth_non_trivia(&self, nth: usize) -> SyntaxKind {
        self.tokens
            .iter()
            .skip(self.position)
            .filter(|token| !token.kind.is_trivia())
            .nth(nth)
            .map_or(SyntaxKind::Eof, |token| token.kind)
    }

    fn current_range(&self) -> TextRange {
        self.tokens.get(self.position).map_or_else(
            || {
                let end = TextSize::from(self.source.len() as u32);
                TextRange::empty(end)
            },
            |token| token.range,
        )
    }

    fn bump(&mut self) {
        if let Some(token) = self.tokens.get(self.position) {
            let start = u32::from(token.range.start()) as usize;
            let end = u32::from(token.range.end()) as usize;
            self.builder.token(
                rowan::SyntaxKind(token.kind as u16),
                &self.source[start..end],
            );
            self.position += 1;
        }
    }

    fn start(&mut self, kind: SyntaxKind) {
        self.builder.start_node(rowan::SyntaxKind(kind as u16));
    }

    fn finish(&mut self) {
        self.builder.finish_node();
    }
}
