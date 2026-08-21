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
        self.block();
        self.finish();
    }

    fn block(&mut self) {
        self.start(SyntaxKind::Block);
        if !self.expect(
            SyntaxKind::LBrace,
            "syntax.expected_lbrace",
            "expected `{` to start test body",
        ) {
            self.finish();
            return;
        }
        loop {
            self.eat_trivia();
            match self.current() {
                SyntaxKind::BrowserKw => self.browser_block(),
                SyntaxKind::RBrace => {
                    self.bump();
                    break;
                }
                SyntaxKind::Eof => {
                    self.error_here("syntax.expected_rbrace", "expected `}` to close test body");
                    break;
                }
                _ => self.unexpected(
                    "syntax.expected_browser",
                    "expected `browser` block in test body",
                ),
            }
        }
        self.finish();
    }

    fn browser_block(&mut self) {
        self.start(SyntaxKind::BrowserBlock);
        self.bump();
        if !self.expect(
            SyntaxKind::LBrace,
            "syntax.expected_lbrace",
            "expected `{` to start browser block",
        ) {
            self.finish();
            return;
        }
        loop {
            self.eat_trivia();
            match self.current() {
                SyntaxKind::OpenKw => self.open_statement(),
                SyntaxKind::ClickKw => self.locator_action(SyntaxKind::ClickStmt),
                SyntaxKind::FillKw => {
                    self.value_action(SyntaxKind::FillStmt, SyntaxKind::WithKw, "with")
                }
                SyntaxKind::TypeKw => {
                    self.value_action(SyntaxKind::TypeStmt, SyntaxKind::WithKw, "with")
                }
                SyntaxKind::PressKw => {
                    self.value_action(SyntaxKind::PressStmt, SyntaxKind::KeyKw, "key")
                }
                SyntaxKind::CheckKw => self.locator_action(SyntaxKind::CheckStmt),
                SyntaxKind::UncheckKw => self.locator_action(SyntaxKind::UncheckStmt),
                SyntaxKind::SelectKw => {
                    self.value_action(SyntaxKind::SelectStmt, SyntaxKind::OptionKw, "option")
                }
                SyntaxKind::HoverKw => self.locator_action(SyntaxKind::HoverStmt),
                SyntaxKind::WaitKw => self.wait_statement(),
                SyntaxKind::ExpectKw => self.expect_statement(),
                SyntaxKind::RBrace => {
                    self.bump();
                    break;
                }
                SyntaxKind::Eof => {
                    self.error_here(
                        "syntax.expected_rbrace",
                        "expected `}` to close browser block",
                    );
                    break;
                }
                _ => self.unexpected(
                    "syntax.expected_browser_statement",
                    "expected a browser action, wait, or assertion in browser block",
                ),
            }
        }
        self.finish();
    }

    fn open_statement(&mut self) {
        self.start(SyntaxKind::OpenStmt);
        self.bump();
        self.expect(
            SyntaxKind::String,
            "syntax.expected_url",
            "expected URL string after `open`",
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
                "with" => "expected `with` and a value string after locator",
                "key" => "expected `key` and a key string after locator",
                _ => "expected `option` and an option string after locator",
            },
        );
        self.expect(
            SyntaxKind::String,
            "syntax.expected_action_value",
            "expected action value string",
        );
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

    fn expect_statement(&mut self) {
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
        self.expect(
            SyntaxKind::String,
            "syntax.expected_url",
            "expected URL string",
        );
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
