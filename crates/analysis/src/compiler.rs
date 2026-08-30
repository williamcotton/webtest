//! Shared semantic compiler state and deterministic result assembly.

mod browser_operations;
mod expressions;
mod provider_calls;
mod statements;
mod type_system;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{
    diagnostic::{Diagnostic, DiagnosticSeverity, DiagnosticSource, default_reference_queries},
    facts::TypeFact,
};
use webtest_feedback::RepairHint;
use webtest_hir::{BindingId, HirFile};
use webtest_plan::{PlanExpr, TestPlan};
use webtest_provider::{Capability, ProviderRegistry, Type};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange};

pub(crate) struct CompileResult {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) type_facts: Vec<TypeFact>,
    pub(crate) plan: TestPlan,
}

#[derive(Clone)]
struct BindingState {
    name: String,
    ty: Type,
    domain: Capability,
    provider_operation: Option<String>,
}

struct TypedExpr {
    expression: PlanExpr,
    ty: Type,
    capability: Capability,
}

struct CompiledProviderCall {
    provider: String,
    operation: String,
    arguments: BTreeMap<String, PlanExpr>,
    result_type: Type,
    schema_hash: String,
    redacted_arguments: Vec<String>,
    redacted_result_fields: Vec<String>,
    retry_safe: bool,
}

struct Compiler<'a> {
    file: FileId,
    revision: SourceRevision,
    providers: &'a ProviderRegistry,
    diagnostics: Vec<Diagnostic>,
    type_facts: Vec<TypeFact>,
    bindings: HashMap<BindingId, BindingState>,
    names: HashMap<String, SyntaxOrigin>,
    declared_names: HashSet<String>,
    required: BTreeSet<Capability>,
    next_step: u32,
}

pub(crate) fn compile(
    file: FileId,
    revision: SourceRevision,
    providers: &ProviderRegistry,
    diagnostics: Vec<Diagnostic>,
    hir: &HirFile,
) -> CompileResult {
    Compiler::new(file, revision, providers, diagnostics).compile(hir)
}

impl<'a> Compiler<'a> {
    fn new(
        file: FileId,
        revision: SourceRevision,
        providers: &'a ProviderRegistry,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        Self {
            file,
            revision,
            providers,
            diagnostics,
            type_facts: Vec::new(),
            bindings: HashMap::new(),
            names: HashMap::new(),
            declared_names: HashSet::new(),
            required: BTreeSet::new(),
            next_step: 0,
        }
    }

    fn compile(mut self, hir: &HirFile) -> CompileResult {
        let tests = hir
            .tests
            .iter()
            .map(|test| {
                self.bindings.clear();
                self.names.clear();
                self.declared_names.clear();
                for statement in &test.body {
                    statements::collect_binding_names(statement, &mut self.declared_names);
                }
                let mut steps = Vec::new();
                for statement in &test.body {
                    self.compile_statement(statement, Capability::Pure, &mut steps);
                }
                webtest_plan::PlannedTest {
                    id: test.id,
                    name: test.name.clone(),
                    steps,
                    origin: test.origin,
                }
            })
            .collect();
        CompileResult {
            diagnostics: self.diagnostics,
            type_facts: self.type_facts,
            plan: TestPlan {
                file: self.file,
                source_revision: self.revision,
                required_host_capabilities: self.required.into_iter().collect(),
                tests,
            },
        }
    }

    fn type_fact(&mut self, range: TextRange, ty: Type, capability: Capability) {
        self.type_facts.push(TypeFact {
            range,
            ty,
            capability,
        });
    }

    fn error(&mut self, range: TextRange, code: &'static str, message: String) {
        self.diagnostics.push(Diagnostic {
            range,
            severity: DiagnosticSeverity::Error,
            code,
            message,
            source: DiagnosticSource::Semantic,
            semantic_details: None,
            repair_hints: Vec::new(),
            reference_queries: default_reference_queries(code),
        });
    }

    fn error_with_details(
        &mut self,
        range: TextRange,
        code: &'static str,
        message: String,
        semantic_details: serde_json::Value,
        repair_hints: Vec<RepairHint>,
        reference_queries: Vec<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            range,
            severity: DiagnosticSeverity::Error,
            code,
            message,
            source: DiagnosticSource::Semantic,
            semantic_details: Some(semantic_details),
            repair_hints,
            reference_queries,
        });
    }
}
