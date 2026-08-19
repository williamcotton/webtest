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
ast_node!(BrowserBlock, BrowserBlock);
ast_node!(OpenStmt, OpenStmt);
ast_node!(ClickStmt, ClickStmt);
ast_node!(ExpectVisibleStmt, ExpectVisibleStmt);
ast_node!(IdLocator, IdLocator);
ast_node!(TextLocator, TextLocator);

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

impl Root {
    pub fn tests(&self) -> impl Iterator<Item = TestDecl> + '_ {
        self.syntax.children().filter_map(TestDecl::cast)
    }
}

impl TestDecl {
    pub fn name(&self) -> Option<StringToken> {
        self.syntax
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .find_map(StringToken::cast)
    }

    pub fn browser_blocks(&self) -> impl Iterator<Item = BrowserBlock> + '_ {
        self.syntax.descendants().filter_map(BrowserBlock::cast)
    }
}

impl BrowserBlock {
    pub fn operations(&self) -> impl Iterator<Item = BrowserOperation> + '_ {
        self.syntax.children().filter_map(BrowserOperation::cast)
    }
}

impl OpenStmt {
    pub fn url(&self) -> Option<StringToken> {
        string_token(&self.syntax)
    }
}

impl ClickStmt {
    pub fn locator(&self) -> Option<Locator> {
        self.syntax.children().find_map(Locator::cast)
    }
}

impl ExpectVisibleStmt {
    pub fn locator(&self) -> Option<Locator> {
        self.syntax.children().find_map(Locator::cast)
    }
}

impl IdLocator {
    pub fn value(&self) -> Option<StringToken> {
        string_token(&self.syntax)
    }
}

impl TextLocator {
    pub fn value(&self) -> Option<StringToken> {
        string_token(&self.syntax)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BrowserOperation {
    Open(OpenStmt),
    Click(ClickStmt),
    ExpectVisible(ExpectVisibleStmt),
}

impl BrowserOperation {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::OpenStmt => OpenStmt::cast(node).map(Self::Open),
            SyntaxKind::ClickStmt => ClickStmt::cast(node).map(Self::Click),
            SyntaxKind::ExpectVisibleStmt => ExpectVisibleStmt::cast(node).map(Self::ExpectVisible),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Locator {
    Id(IdLocator),
    Text(TextLocator),
}

impl Locator {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::IdLocator => IdLocator::cast(node).map(Self::Id),
            SyntaxKind::TextLocator => TextLocator::cast(node).map(Self::Text),
            _ => None,
        }
    }
}

fn string_token(node: &SyntaxNode) -> Option<StringToken> {
    node.children_with_tokens()
        .filter_map(|element| element.into_token())
        .find_map(StringToken::cast)
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
