//! Incremental workspace, static semantics, and deterministic plan construction.

mod callables;
mod compiler;
pub use callables::{DeclarationParameter, DeclarationSignature};
mod database;
mod incremental;
mod workspace;
mod workspace_editor;
pub use incremental::QueryCounts;
pub use workspace::{
    Module, ModuleDiagnostic, ModuleGraph, ResolvedImport, WorkspaceConfiguration,
};
mod description;
mod diagnostic;
mod discovery;
pub use discovery::{
    DiscoveredModule, DiscoveredTest, DiscoveredVariant, StaticStatus, TestDiscovery,
};
mod editor_queries;
mod facts;
mod navigation;
mod semantic_index;
mod semantic_tokens;
pub use navigation::RenameError;
pub use semantic_tokens::{SemanticToken, SemanticTokenKind, SemanticTokenModifiers};
mod syntax_diagnostics;

pub use database::{AnalysisCancellation, AnalysisDatabase, AnalysisError, AnalysisSnapshot};
pub use description::{
    Availability, CategoryDescription, ConstraintDescription, ConstructDescription,
    DESCRIPTION_SCHEMA_VERSION, DescriptionDiagnostic, DescriptionIndex, DescriptionLimits,
    DescriptionProject, DescriptionRequest, DescriptionResponse, GuidanceDescription,
    LanguageDescription, ParameterDescription, ProjectDeclaration, ProjectSemanticDescription,
    Provenance, ResolvedRuntimeConfiguration, SearchDescription, SearchResult, SourceExample,
    SyntaxElement, SyntaxForm, describe,
};
pub use diagnostic::{Diagnostic, DiagnosticSeverity, DiagnosticSource};
pub use facts::{
    CodeAction, Completion, CompletionKind, DocumentEdit, DocumentationFact, FoldingRange,
    InlayHint, LocationLink, QueryResult, Reference, SelectionRange, Signature, SignatureParameter,
    SourceLocation, Symbol, SymbolKind, TextEdit, TypeFact, WorkspaceEdit,
};
pub use webtest_feedback::{RepairHint, RepairHintKind, RepairReplacement};
