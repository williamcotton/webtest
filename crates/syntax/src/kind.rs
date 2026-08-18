use rowan::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    Whitespace,
    LineComment,
    Ident,
    String,
    LBrace,
    RBrace,
    LParen,
    RParen,
    TestKw,
    BrowserKw,
    OpenKw,
    ClickKw,
    IdKw,
    Root,
    TestDecl,
    Block,
    BrowserBlock,
    OpenStmt,
    ClickStmt,
    IdLocator,
    Error,
    Eof,
}

impl SyntaxKind {
    pub const fn is_trivia(self) -> bool {
        matches!(self, Self::Whitespace | Self::LineComment)
    }

    pub const fn is_keyword(self) -> bool {
        matches!(
            self,
            Self::TestKw | Self::BrowserKw | Self::OpenKw | Self::ClickKw | Self::IdKw
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebtestLanguage {}

impl Language for WebtestLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        match raw.0 {
            0 => SyntaxKind::Whitespace,
            1 => SyntaxKind::LineComment,
            2 => SyntaxKind::Ident,
            3 => SyntaxKind::String,
            4 => SyntaxKind::LBrace,
            5 => SyntaxKind::RBrace,
            6 => SyntaxKind::LParen,
            7 => SyntaxKind::RParen,
            8 => SyntaxKind::TestKw,
            9 => SyntaxKind::BrowserKw,
            10 => SyntaxKind::OpenKw,
            11 => SyntaxKind::ClickKw,
            12 => SyntaxKind::IdKw,
            13 => SyntaxKind::Root,
            14 => SyntaxKind::TestDecl,
            15 => SyntaxKind::Block,
            16 => SyntaxKind::BrowserBlock,
            17 => SyntaxKind::OpenStmt,
            18 => SyntaxKind::ClickStmt,
            19 => SyntaxKind::IdLocator,
            20 => SyntaxKind::Error,
            21 => SyntaxKind::Eof,
            _ => SyntaxKind::Error,
        }
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<WebtestLanguage>;
pub type SyntaxToken = rowan::SyntaxToken<WebtestLanguage>;
pub type SyntaxElement = rowan::SyntaxElement<WebtestLanguage>;
