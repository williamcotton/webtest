//! Public data-only results returned by semantic and editor queries.

use webtest_model::{Capability, Type};
use webtest_text::TextRange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeFact {
    pub range: TextRange,
    pub ty: Type,
    pub capability: Capability,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    #[default]
    Function,
    Variable,
    Keyword,
    Namespace,
    Module,
    Type,
    Fixture,
    Parameter,
    Property,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Completion {
    pub replacement_range: Option<webtest_feedback::ByteRange>,
    pub sort_text: String,
    pub filter_text: String,
    pub symbol: Option<webtest_model::SymbolId>,
    pub additional_edits: Vec<TextEdit>,
    pub label: String,
    pub detail: String,
    pub documentation: String,
    pub kind: CompletionKind,
    pub insert_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SignatureParameter {
    pub name: String,
    pub ty: Option<Type>,
    pub optional: bool,
    pub default: Option<serde_json::Value>,
    pub label: String,
    pub documentation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Signature {
    pub symbol: Option<webtest_model::SymbolId>,
    pub capability: Option<Capability>,
    pub lifetime: Option<webtest_model::FixtureLifetime>,
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<SignatureParameter>,
    pub active_parameter: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentationFact {
    pub range: TextRange,
    pub contents: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_kind_serialization_is_stable() {
        assert_eq!(
            serde_json::to_value(CompletionKind::Function).expect("serialize completion kind"),
            "function"
        );
        assert_eq!(
            serde_json::to_value(CompletionKind::Parameter).expect("serialize completion kind"),
            "parameter"
        );
        assert_eq!(
            serde_json::to_value(CompletionKind::Property).expect("serialize completion kind"),
            "property"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Guard,
    Pattern,
    Module,
    Import,
    Test,
    Fixture,
    Function,
    Parameter,
    Binding,
    ProviderNamespace,
    ProviderFunction,
    RecordField,
    Type,
}

pub use webtest_text::SourceLocation;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub id: webtest_model::SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub location: Option<SourceLocation>,
    pub full_origin: Option<webtest_text::SyntaxOrigin>,
    pub container: Option<webtest_model::SymbolId>,
    pub ty: Option<Type>,
    pub capability: Capability,
    pub lifetime: Option<webtest_model::FixtureLifetime>,
    pub documentation: String,
    pub exported: bool,
    pub read_only: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Reference {
    pub symbol: webtest_model::SymbolId,
    pub resolved_symbol: webtest_model::SymbolId,
    pub location: SourceLocation,
    pub declaration: bool,
    pub scope: webtest_model::SymbolId,
    pub quoted: bool,
    pub scope_sensitive: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocationLink {
    pub origin: webtest_text::SyntaxOrigin,
    pub target: SourceLocation,
    pub target_range: webtest_text::SyntaxOrigin,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextEdit {
    pub origin: webtest_text::SyntaxOrigin,
    pub replacement: String,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentEdit {
    pub path: String,
    pub file: webtest_text::FileId,
    pub revision: webtest_text::SourceRevision,
    pub version: Option<webtest_text::DocumentVersion>,
    pub edits: Vec<TextEdit>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceEdit {
    pub documents: Vec<DocumentEdit>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FoldingRange {
    pub range: webtest_feedback::ByteRange,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectionRange {
    pub range: webtest_feedback::ByteRange,
    pub parent: Option<Box<SelectionRange>>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InlayHint {
    pub file: webtest_text::FileId,
    pub offset: u32,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CodeAction {
    pub title: String,
    pub diagnostic_code: Option<String>,
    pub edit: WorkspaceEdit,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QueryResult<T> {
    pub items: Vec<T>,
    pub truncated: bool,
}
