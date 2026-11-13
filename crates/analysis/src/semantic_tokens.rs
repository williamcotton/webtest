//! CST tokenization decorated by the shared resolved semantic index.
use crate::{
    facts::SymbolKind,
    incremental::{Db, SourceInput},
};
use webtest_syntax::SyntaxKind;
use webtest_text::TextRange;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticTokenKind {
    Keyword,
    String,
    Comment,
    Function,
    Variable,
    Property,
    Type,
    Parameter,
    Namespace,
    Fixture,
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub modifiers: SemanticTokenModifiers,
    pub range: TextRange,
    pub kind: SemanticTokenKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SemanticTokenModifiers {
    pub declaration: bool,
    pub readonly: bool,
    pub deprecated: bool,
}
pub(crate) fn tokens(db: &dyn Db, file: SourceInput) -> Vec<SemanticToken> {
    let parse = crate::incremental::parsed(db, file);
    let index = crate::semantic_index::symbols(db, file);
    let resolved = index
        .references
        .iter()
        .filter_map(|reference| {
            let symbol = index.symbols.get(&reference.resolved_symbol)?;
            let range = reference.location.origin.range;
            Some((
                (u32::from(range.start()), u32::from(range.end())),
                (
                    symbol.kind,
                    SemanticTokenModifiers {
                        declaration: reference.declaration,
                        readonly: symbol.read_only
                            || matches!(
                                symbol.kind,
                                SymbolKind::Binding
                                    | SymbolKind::Parameter
                                    | SymbolKind::Function
                                    | SymbolKind::Fixture
                                    | SymbolKind::Test
                            ),
                        deprecated: false,
                    },
                ),
            ))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    parse
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter_map(|token| {
            if token.kind() == SyntaxKind::String
                && let Some((_, modifiers)) =
                    resolved
                        .iter()
                        .find_map(|((start, end), (kind, modifiers))| {
                            (*kind == SymbolKind::Test
                                && token
                                    .text_range()
                                    .contains_range(TextRange::new((*start).into(), (*end).into())))
                            .then_some((kind, modifiers))
                        })
            {
                return Some(SemanticToken {
                    modifiers: *modifiers,
                    range: token.text_range(),
                    kind: SemanticTokenKind::Test,
                });
            }
            if !matches!(token.kind(), SyntaxKind::String | SyntaxKind::LineComment)
                && let Some((kind, modifiers)) = resolved.get(&(
                    u32::from(token.text_range().start()),
                    u32::from(token.text_range().end()),
                ))
            {
                let kind = match kind {
                    SymbolKind::Function | SymbolKind::ProviderFunction => {
                        SemanticTokenKind::Function
                    }
                    SymbolKind::Fixture => SemanticTokenKind::Fixture,
                    SymbolKind::Parameter => SemanticTokenKind::Parameter,
                    SymbolKind::ProviderNamespace | SymbolKind::Module => {
                        SemanticTokenKind::Namespace
                    }
                    SymbolKind::RecordField => SemanticTokenKind::Property,
                    SymbolKind::Type => SemanticTokenKind::Type,
                    SymbolKind::Binding | SymbolKind::Import => SemanticTokenKind::Variable,
                    SymbolKind::Test => return None,
                };
                return Some(SemanticToken {
                    modifiers: *modifiers,
                    range: token.text_range(),
                    kind,
                });
            }
            let parent = token.parent().map(|node| node.kind());
            let kind = match token.kind() {
                SyntaxKind::RetryKw | SyntaxKind::BackoffKw | SyntaxKind::MaxKw
                    if parent == Some(SyntaxKind::RetryStmt) =>
                {
                    SemanticTokenKind::Keyword
                }
                SyntaxKind::RaceKw if parent == Some(SyntaxKind::RaceStmt) => {
                    SemanticTokenKind::Keyword
                }
                SyntaxKind::ProvideKw if parent == Some(SyntaxKind::ProvideStmt) => {
                    SemanticTokenKind::Keyword
                }
                SyntaxKind::ParallelKw if parent == Some(SyntaxKind::ParallelStmt) => {
                    SemanticTokenKind::Keyword
                }
                SyntaxKind::TimeoutKw if parent == Some(SyntaxKind::TimeoutStmt) => {
                    SemanticTokenKind::Keyword
                }
                SyntaxKind::ImportKw
                | SyntaxKind::ExportKw
                | SyntaxKind::FnKw
                | SyntaxKind::FixtureKw
                | SyntaxKind::UseKw
                | SyntaxKind::ScopeKw
                | SyntaxKind::FileKw
                | SyntaxKind::WorkerKw
                | SyntaxKind::SuiteKw
                | SyntaxKind::SetupKw
                | SyntaxKind::TeardownKw
                | SyntaxKind::ShareableKw
                | SyntaxKind::FromKw
                | SyntaxKind::AsKw
                | SyntaxKind::TestKw
                | SyntaxKind::BrowserKw
                | SyntaxKind::ServerKw
                | SyntaxKind::LetKw
                | SyntaxKind::TrueKw
                | SyntaxKind::FalseKw
                | SyntaxKind::NullKw
                | SyntaxKind::ContainsKw
                | SyntaxKind::MatchesKw
                | SyntaxKind::OpenKw
                | SyntaxKind::ClickKw
                | SyntaxKind::FillKw
                | SyntaxKind::TypeKw
                | SyntaxKind::PressKw
                | SyntaxKind::KeyKw
                | SyntaxKind::WithKw
                | SyntaxKind::CheckKw
                | SyntaxKind::UncheckKw
                | SyntaxKind::SelectKw
                | SyntaxKind::OptionKw
                | SyntaxKind::HoverKw
                | SyntaxKind::WaitKw
                | SyntaxKind::ExpectKw
                | SyntaxKind::WithinKw
                | SyntaxKind::VisibleKw
                | SyntaxKind::HiddenKw
                | SyntaxKind::AttachedKw
                | SyntaxKind::DetachedKw
                | SyntaxKind::EnabledKw
                | SyntaxKind::DisabledKw
                | SyntaxKind::CheckedKw
                | SyntaxKind::UncheckedKw => SemanticTokenKind::Keyword,
                SyntaxKind::IdKw
                | SyntaxKind::RoleKw
                | SyntaxKind::LabelKw
                | SyntaxKind::TextKw
                | SyntaxKind::PlaceholderKw
                | SyntaxKind::TestIdKw
                | SyntaxKind::CssKw
                | SyntaxKind::XPathKw
                | SyntaxKind::UrlKw => SemanticTokenKind::Function,
                SyntaxKind::String => SemanticTokenKind::String,
                SyntaxKind::LineComment => SemanticTokenKind::Comment,
                SyntaxKind::Ident
                | SyntaxKind::TimeoutKw
                | SyntaxKind::ParallelKw
                | SyntaxKind::RaceKw
                | SyntaxKind::ProvideKw
                | SyntaxKind::RetryKw
                | SyntaxKind::BackoffKw
                | SyntaxKind::MaxKw
                    if matches!(parent, Some(SyntaxKind::LetStmt | SyntaxKind::NameExpr)) =>
                {
                    SemanticTokenKind::Variable
                }
                SyntaxKind::Ident
                | SyntaxKind::TimeoutKw
                | SyntaxKind::ParallelKw
                | SyntaxKind::RaceKw
                | SyntaxKind::ProvideKw
                | SyntaxKind::RetryKw
                | SyntaxKind::BackoffKw
                | SyntaxKind::MaxKw
                    if parent == Some(SyntaxKind::MemberExpr) =>
                {
                    SemanticTokenKind::Property
                }
                SyntaxKind::Ident
                    if matches!(
                        parent,
                        Some(SyntaxKind::NamedType | SyntaxKind::GenericType)
                    ) =>
                {
                    SemanticTokenKind::Type
                }
                _ => return None,
            };
            Some(SemanticToken {
                modifiers: SemanticTokenModifiers::default(),
                range: token.text_range(),
                kind,
            })
        })
        .collect()
}
