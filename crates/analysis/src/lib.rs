//! Incremental workspace, static semantics, and deterministic plan construction.

mod compiler;
mod database;
mod description;
mod diagnostic;
mod editor_queries;
mod facts;
mod syntax_diagnostics;

pub use database::{AnalysisDatabase, AnalysisError};
pub use description::{
    Availability, CategoryDescription, ConstraintDescription, ConstructDescription,
    DescriptionDiagnostic, DescriptionIndex, DescriptionLimits, DescriptionProject,
    DescriptionRequest, DescriptionResponse, GuidanceDescription, LanguageDescription,
    ParameterDescription, Provenance, ResolvedRuntimeConfiguration, SearchDescription,
    SearchResult, SourceExample, SyntaxElement, SyntaxForm, describe,
};
pub use diagnostic::{Diagnostic, DiagnosticSeverity, DiagnosticSource};
pub use facts::{
    Completion, CompletionKind, DocumentationFact, Signature, SignatureParameter, TypeFact,
};
pub use webtest_feedback::{RepairHint, RepairHintKind, RepairReplacement};
