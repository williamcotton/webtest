use rowan::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    Whitespace,
    LineComment,
    Ident,
    String,
    Duration,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Dot,
    Comma,
    Colon,
    TestKw,
    BrowserKw,
    OpenKw,
    ClickKw,
    FillKw,
    TypeKw,
    PressKw,
    KeyKw,
    WithKw,
    CheckKw,
    UncheckKw,
    SelectKw,
    OptionKw,
    HoverKw,
    WaitKw,
    ExpectKw,
    WithinKw,
    UrlKw,
    IdKw,
    RoleKw,
    NameKw,
    LabelKw,
    TextKw,
    PlaceholderKw,
    TestIdKw,
    CssKw,
    XPathKw,
    VisibleKw,
    HiddenKw,
    AttachedKw,
    DetachedKw,
    EnabledKw,
    DisabledKw,
    CheckedKw,
    UncheckedKw,
    Root,
    TestDecl,
    Block,
    BrowserBlock,
    OpenStmt,
    ClickStmt,
    FillStmt,
    TypeStmt,
    PressStmt,
    CheckStmt,
    UncheckStmt,
    SelectStmt,
    HoverStmt,
    WaitLocatorStmt,
    WaitUrlStmt,
    ExpectLocatorStmt,
    ExpectUrlStmt,
    IdLocator,
    RoleLocator,
    LabelLocator,
    TextLocator,
    PlaceholderLocator,
    TestIdLocator,
    CssLocator,
    XPathLocator,
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
            Self::TestKw
                | Self::BrowserKw
                | Self::OpenKw
                | Self::ClickKw
                | Self::FillKw
                | Self::TypeKw
                | Self::PressKw
                | Self::KeyKw
                | Self::WithKw
                | Self::CheckKw
                | Self::UncheckKw
                | Self::SelectKw
                | Self::OptionKw
                | Self::HoverKw
                | Self::WaitKw
                | Self::ExpectKw
                | Self::WithinKw
                | Self::UrlKw
                | Self::IdKw
                | Self::RoleKw
                | Self::NameKw
                | Self::LabelKw
                | Self::TextKw
                | Self::PlaceholderKw
                | Self::TestIdKw
                | Self::CssKw
                | Self::XPathKw
                | Self::VisibleKw
                | Self::HiddenKw
                | Self::AttachedKw
                | Self::DetachedKw
                | Self::EnabledKw
                | Self::DisabledKw
                | Self::CheckedKw
                | Self::UncheckedKw
        )
    }

    pub const fn is_locator_start(self) -> bool {
        matches!(
            self,
            Self::IdKw
                | Self::RoleKw
                | Self::LabelKw
                | Self::TextKw
                | Self::PlaceholderKw
                | Self::TestIdKw
                | Self::CssKw
                | Self::XPathKw
        )
    }

    pub const fn is_locator_state(self) -> bool {
        matches!(
            self,
            Self::VisibleKw
                | Self::HiddenKw
                | Self::AttachedKw
                | Self::DetachedKw
                | Self::EnabledKw
                | Self::DisabledKw
                | Self::CheckedKw
                | Self::UncheckedKw
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
            4 => SyntaxKind::Duration,
            5 => SyntaxKind::LBrace,
            6 => SyntaxKind::RBrace,
            7 => SyntaxKind::LParen,
            8 => SyntaxKind::RParen,
            9 => SyntaxKind::Dot,
            10 => SyntaxKind::Comma,
            11 => SyntaxKind::Colon,
            12 => SyntaxKind::TestKw,
            13 => SyntaxKind::BrowserKw,
            14 => SyntaxKind::OpenKw,
            15 => SyntaxKind::ClickKw,
            16 => SyntaxKind::FillKw,
            17 => SyntaxKind::TypeKw,
            18 => SyntaxKind::PressKw,
            19 => SyntaxKind::KeyKw,
            20 => SyntaxKind::WithKw,
            21 => SyntaxKind::CheckKw,
            22 => SyntaxKind::UncheckKw,
            23 => SyntaxKind::SelectKw,
            24 => SyntaxKind::OptionKw,
            25 => SyntaxKind::HoverKw,
            26 => SyntaxKind::WaitKw,
            27 => SyntaxKind::ExpectKw,
            28 => SyntaxKind::WithinKw,
            29 => SyntaxKind::UrlKw,
            30 => SyntaxKind::IdKw,
            31 => SyntaxKind::RoleKw,
            32 => SyntaxKind::NameKw,
            33 => SyntaxKind::LabelKw,
            34 => SyntaxKind::TextKw,
            35 => SyntaxKind::PlaceholderKw,
            36 => SyntaxKind::TestIdKw,
            37 => SyntaxKind::CssKw,
            38 => SyntaxKind::XPathKw,
            39 => SyntaxKind::VisibleKw,
            40 => SyntaxKind::HiddenKw,
            41 => SyntaxKind::AttachedKw,
            42 => SyntaxKind::DetachedKw,
            43 => SyntaxKind::EnabledKw,
            44 => SyntaxKind::DisabledKw,
            45 => SyntaxKind::CheckedKw,
            46 => SyntaxKind::UncheckedKw,
            47 => SyntaxKind::Root,
            48 => SyntaxKind::TestDecl,
            49 => SyntaxKind::Block,
            50 => SyntaxKind::BrowserBlock,
            51 => SyntaxKind::OpenStmt,
            52 => SyntaxKind::ClickStmt,
            53 => SyntaxKind::FillStmt,
            54 => SyntaxKind::TypeStmt,
            55 => SyntaxKind::PressStmt,
            56 => SyntaxKind::CheckStmt,
            57 => SyntaxKind::UncheckStmt,
            58 => SyntaxKind::SelectStmt,
            59 => SyntaxKind::HoverStmt,
            60 => SyntaxKind::WaitLocatorStmt,
            61 => SyntaxKind::WaitUrlStmt,
            62 => SyntaxKind::ExpectLocatorStmt,
            63 => SyntaxKind::ExpectUrlStmt,
            64 => SyntaxKind::IdLocator,
            65 => SyntaxKind::RoleLocator,
            66 => SyntaxKind::LabelLocator,
            67 => SyntaxKind::TextLocator,
            68 => SyntaxKind::PlaceholderLocator,
            69 => SyntaxKind::TestIdLocator,
            70 => SyntaxKind::CssLocator,
            71 => SyntaxKind::XPathLocator,
            72 => SyntaxKind::Error,
            73 => SyntaxKind::Eof,
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
