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
                SyntaxKind::ClickKw => self.click_statement(),
                SyntaxKind::ExpectKw => self.expect_visible_statement(),
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
                    "expected `open`, `click`, or `expect` in browser block",
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

    fn click_statement(&mut self) {
        self.start(SyntaxKind::ClickStmt);
        self.bump();
        self.eat_trivia();
        if !self.locator() {
            self.error_here("syntax.expected_locator", "expected locator after `click`");
            if !matches!(self.current(), SyntaxKind::RBrace | SyntaxKind::Eof) {
                self.unexpected("syntax.invalid_locator", "invalid locator");
            }
        }
        self.finish();
    }

    fn expect_visible_statement(&mut self) {
        self.start(SyntaxKind::ExpectVisibleStmt);
        self.bump();
        self.eat_trivia();
        if !self.locator() {
            self.error_here("syntax.expected_locator", "expected locator after `expect`");
        }
        self.expect(
            SyntaxKind::Dot,
            "syntax.expected_dot",
            "expected `.` after locator",
        );
        self.expect(
            SyntaxKind::VisibleKw,
            "syntax.expected_visible",
            "expected `visible` after locator",
        );
        self.finish();
    }

    fn locator(&mut self) -> bool {
        let node_kind = match self.current() {
            SyntaxKind::IdKw => SyntaxKind::IdLocator,
            SyntaxKind::TextKw => SyntaxKind::TextLocator,
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

    fn error_here(&mut self, code: &'static str, message: &'static str) {
        self.eat_trivia();
        self.errors
            .push(SyntaxError::new(self.current_range(), code, message));
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
