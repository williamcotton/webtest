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
    EvaluateKw,
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
    EvaluateStmt,
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
                | Self::EvaluateKw
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
            15 => SyntaxKind::EvaluateKw,
            16 => SyntaxKind::ClickKw,
            17 => SyntaxKind::FillKw,
            18 => SyntaxKind::TypeKw,
            19 => SyntaxKind::PressKw,
            20 => SyntaxKind::KeyKw,
            21 => SyntaxKind::WithKw,
            22 => SyntaxKind::CheckKw,
            23 => SyntaxKind::UncheckKw,
            24 => SyntaxKind::SelectKw,
            25 => SyntaxKind::OptionKw,
            26 => SyntaxKind::HoverKw,
            27 => SyntaxKind::WaitKw,
            28 => SyntaxKind::ExpectKw,
            29 => SyntaxKind::WithinKw,
            30 => SyntaxKind::UrlKw,
            31 => SyntaxKind::IdKw,
            32 => SyntaxKind::RoleKw,
            33 => SyntaxKind::NameKw,
            34 => SyntaxKind::LabelKw,
            35 => SyntaxKind::TextKw,
            36 => SyntaxKind::PlaceholderKw,
            37 => SyntaxKind::TestIdKw,
            38 => SyntaxKind::CssKw,
            39 => SyntaxKind::XPathKw,
            40 => SyntaxKind::VisibleKw,
            41 => SyntaxKind::HiddenKw,
            42 => SyntaxKind::AttachedKw,
            43 => SyntaxKind::DetachedKw,
            44 => SyntaxKind::EnabledKw,
            45 => SyntaxKind::DisabledKw,
            46 => SyntaxKind::CheckedKw,
            47 => SyntaxKind::UncheckedKw,
            48 => SyntaxKind::Root,
            49 => SyntaxKind::TestDecl,
            50 => SyntaxKind::Block,
            51 => SyntaxKind::BrowserBlock,
            52 => SyntaxKind::OpenStmt,
            53 => SyntaxKind::EvaluateStmt,
            54 => SyntaxKind::ClickStmt,
            55 => SyntaxKind::FillStmt,
            56 => SyntaxKind::TypeStmt,
            57 => SyntaxKind::PressStmt,
            58 => SyntaxKind::CheckStmt,
            59 => SyntaxKind::UncheckStmt,
            60 => SyntaxKind::SelectStmt,
            61 => SyntaxKind::HoverStmt,
            62 => SyntaxKind::WaitLocatorStmt,
            63 => SyntaxKind::WaitUrlStmt,
            64 => SyntaxKind::ExpectLocatorStmt,
            65 => SyntaxKind::ExpectUrlStmt,
            66 => SyntaxKind::IdLocator,
            67 => SyntaxKind::RoleLocator,
            68 => SyntaxKind::LabelLocator,
            69 => SyntaxKind::TextLocator,
            70 => SyntaxKind::PlaceholderLocator,
            71 => SyntaxKind::TestIdLocator,
            72 => SyntaxKind::CssLocator,
            73 => SyntaxKind::XPathLocator,
            74 => SyntaxKind::Error,
            75 => SyntaxKind::Eof,
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
