//! Shared semantic compiler state and deterministic result assembly.

mod browser_operations;
mod expressions;
mod fixtures;
mod helpers;
mod provider_calls;
mod pure_scopes;
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
use webtest_hir::HirFile;
use webtest_model::{BindingId, Capability, Type};
use webtest_plan::{PlanExpr, TestPlan};
use webtest_provider::{ProviderRegistry, ProviderSchema};

pub(crate) trait ProviderSchemas {
    fn schema(&self, name: &str) -> Option<&ProviderSchema>;
    fn names(&self) -> Vec<String>;
}
impl ProviderSchemas for ProviderRegistry {
    fn schema(&self, name: &str) -> Option<&ProviderSchema> {
        ProviderRegistry::schema(self, name)
    }
    fn names(&self) -> Vec<String> {
        self.schemas().map(|schema| schema.name.0.clone()).collect()
    }
}
use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompileResult {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) type_facts: Vec<TypeFact>,
    pub(crate) plan: TestPlan,
    pub(crate) fixtures: Vec<webtest_plan::FixtureDefinition>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FixtureCompileResult {
    pub definition: Option<webtest_plan::FixtureDefinition>,
    pub dependencies: Vec<webtest_plan::FixtureDefinition>,
    pub diagnostics: Vec<Diagnostic>,
}
pub(crate) fn compile_fixture_template(
    file: FileId,
    revision: SourceRevision,
    providers: &dyn ProviderSchemas,
    hir: &HirFile,
    id: webtest_model::FixtureId,
    workspace: (&dyn crate::incremental::Db, &crate::ModuleGraph),
) -> FixtureCompileResult {
    let mut compiler = Compiler::new(file, revision, providers, Vec::new());
    compiler.workspace = Some(workspace);
    compiler.module = Some(workspace.1.root);
    compiler.local_hir = Some(hir);
    let mut selected = hir.clone();
    selected.fixtures.retain(|fixture| fixture.id == id);
    let definition = compiler
        .compile_fixture_declarations(&selected)
        .into_iter()
        .next();
    FixtureCompileResult {
        definition,
        dependencies: compiler.referenced_fixtures.into_values().collect(),
        diagnostics: compiler.diagnostics,
    }
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
    workspace: Option<(&'a dyn crate::incremental::Db, &'a crate::ModuleGraph)>,
    module: Option<webtest_model::ModuleId>,
    local_hir: Option<&'a HirFile>,
    active_calls: Vec<(webtest_model::SymbolId, SyntaxOrigin)>,
    specialization_budget: usize,
    referenced_fixtures: BTreeMap<webtest_model::FixtureId, webtest_plan::FixtureDefinition>,
    validating_declaration: bool,
    in_fixture_teardown: bool,
    providers: &'a dyn ProviderSchemas,
    diagnostics: Vec<Diagnostic>,
    type_facts: Vec<TypeFact>,
    bindings: HashMap<BindingId, BindingState>,
    names: HashMap<String, SyntaxOrigin>,
    declared_names: HashSet<String>,
    required: BTreeSet<Capability>,
    test_required: BTreeSet<Capability>,
    next_step: u32,
    concurrent_captures: HashSet<BindingId>,
    concurrent_depth: usize,
    retry_depth: usize,
    retry_captures: HashSet<BindingId>,
    provide_types: Option<Vec<(Type, SyntaxOrigin)>>,
    provide_is_return: bool,
    resource_depth: usize,
    return_boundary_depth: usize,
    next_temporary: u32,
}

pub(crate) fn compile_in_workspace(
    file: FileId,
    revision: SourceRevision,
    providers: &dyn ProviderSchemas,
    diagnostics: Vec<Diagnostic>,
    hir: &HirFile,
    source_identity: &str,
    workspace: (&dyn crate::incremental::Db, &crate::ModuleGraph),
) -> CompileResult {
    let mut compiler = Compiler::new(file, revision, providers, diagnostics);
    compiler.workspace = Some(workspace);
    compiler.module = Some(workspace.1.root);
    compiler.compile(hir, source_identity)
}

impl<'a> Compiler<'a> {
    fn new(
        file: FileId,
        revision: SourceRevision,
        providers: &'a dyn ProviderSchemas,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        Self {
            file,
            revision,
            workspace: None,
            module: None,
            local_hir: None,
            active_calls: Vec::new(),
            specialization_budget: 100_000,
            referenced_fixtures: BTreeMap::new(),
            validating_declaration: false,
            in_fixture_teardown: false,
            providers,
            diagnostics,
            type_facts: Vec::new(),
            bindings: HashMap::new(),
            names: HashMap::new(),
            declared_names: HashSet::new(),
            required: BTreeSet::new(),
            test_required: BTreeSet::new(),
            next_step: 0,
            concurrent_captures: HashSet::new(),
            concurrent_depth: 0,
            retry_depth: 0,
            retry_captures: HashSet::new(),
            provide_types: None,
            provide_is_return: false,
            resource_depth: 0,
            return_boundary_depth: 0,
            next_temporary: u32::MAX,
        }
    }

    fn compile(mut self, hir: &'a HirFile, source_identity: &str) -> CompileResult {
        self.local_hir = Some(hir);
        self.validate_expression_helpers(hir);
        let fixtures = self.compile_fixture_declarations(hir);
        for definition in &fixtures {
            self.referenced_fixtures
                .insert(definition.id, definition.clone());
        }
        let mut occurrences = HashMap::<String, u32>::new();
        let tests = hir
            .tests
            .iter()
            .map(|test| {
                self.bindings.clear();
                self.names.clear();
                self.declared_names.clear();
                self.test_required.clear();
                for statement in &test.body {
                    statements::collect_binding_names(statement, &mut self.declared_names);
                }
                let occurrence = occurrences.entry(test.name.clone()).or_default();
                let declaration_id =
                    webtest_plan::declaration_identity(source_identity, &test.name, *occurrence);
                *occurrence += 1;
                let body = self.compile_sequence(
                    declaration_id,
                    test.origin,
                    &test.body,
                    Capability::Pure,
                    Vec::new(),
                );
                let body =
                    if body.required_resources().iter().any(|usage| {
                        usage.resource == webtest_plan::ResourceReference::BrowserContext
                    }) {
                        body.with_browser_resource(declaration_id)
                    } else {
                        body
                    };
                webtest_plan::PlannedTest {
                    test_declaration_id: test.declaration_id,
                    variant_id: test.variant_id,
                    id: test.id,
                    declaration_id,
                    name: test.name.clone(),
                    required_host_capabilities: std::mem::take(&mut self.test_required)
                        .into_iter()
                        .collect(),
                    body,
                    origin: test.origin,
                }
            })
            .collect();
        for definition in &fixtures {
            self.referenced_fixtures
                .entry(definition.id)
                .or_insert_with(|| definition.clone());
        }
        let plan = TestPlan {
            fixtures: self.referenced_fixtures.into_values().collect(),
            source_files: self.workspace.map_or_else(Vec::new, |(_, graph)| {
                graph
                    .modules
                    .values()
                    .map(|module| webtest_plan::PlanSourceFile {
                        file: module.file,
                        path: module.path.clone(),
                        revision: module.revision,
                    })
                    .collect()
            }),
            file: self.file,
            source_revision: self.revision,
            required_host_capabilities: self.required.into_iter().collect(),
            tests,
        };
        debug_assert!(plan.validate_capabilities().is_ok());
        CompileResult {
            diagnostics: self.diagnostics,
            type_facts: self.type_facts,
            plan,
            fixtures,
        }
    }

    fn record_capability(&mut self, capability: Capability) {
        self.test_required.insert(capability);
        self.required.insert(capability);
    }

    fn type_fact(&mut self, range: TextRange, ty: Type, capability: Capability) {
        if self
            .workspace
            .is_some_and(|(_, graph)| self.module != Some(graph.root))
        {
            return;
        }
        self.type_facts.push(TypeFact {
            range,
            ty,
            capability,
        });
    }

    fn diagnostic_location(
        &self,
        range: TextRange,
    ) -> (TextRange, Vec<webtest_text::SourceLocation>) {
        let Some((_, graph)) = self.workspace else {
            return (range, Vec::new());
        };
        let Some(module) = self.module.and_then(|module| graph.modules.get(&module)) else {
            return (range, Vec::new());
        };
        let caller = self
            .active_calls
            .get(usize::from(self.validating_declaration))
            .map(|(_, origin)| *origin);
        let mut related = caller
            .and_then(|_| graph.source_location(SyntaxOrigin::new(module.file, range)))
            .into_iter()
            .collect::<Vec<_>>();
        related.extend(
            self.active_calls
                .iter()
                .skip(usize::from(self.validating_declaration))
                .filter_map(|(_, origin)| graph.source_location(*origin)),
        );
        related.dedup();
        (caller.map_or(range, |origin| origin.range), related)
    }
    fn call_origin_chain(&self) -> Vec<webtest_text::SourceLocation> {
        self.workspace.map_or_else(Vec::new, |(_, graph)| {
            self.active_calls
                .iter()
                .skip(usize::from(self.validating_declaration))
                .filter_map(|(_, origin)| graph.source_location(*origin))
                .collect()
        })
    }
    fn error(&mut self, range: TextRange, code: &'static str, message: String) {
        let declaration_origin = self
            .module
            .and_then(|id| self.workspace.and_then(|(_, graph)| graph.modules.get(&id)))
            .map(|module| SyntaxOrigin::new(module.file, range));
        let (range, related_locations) = self.diagnostic_location(range);
        self.diagnostics.push(Diagnostic {
            related_locations,
            range,
            severity: DiagnosticSeverity::Error,
            code,
            message,
            source: DiagnosticSource::Semantic,
            semantic_details: declaration_origin.filter(|_| !self.active_calls.is_empty()).map(|origin| serde_json::json!({"declaration_origin":origin, "call_chain":self.active_calls.iter().map(|(_, origin)|origin).collect::<Vec<_>>()})),
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
        mut repair_hints: Vec<RepairHint>,
        reference_queries: Vec<String>,
    ) {
        let (range, related_locations) = self.diagnostic_location(range);
        // Legacy hints have implicit document ownership. Cross-file edits are
        // issued only by the versioned workspace-edit query using typed links.
        if related_locations
            .first()
            .is_some_and(|location| location.origin.file != self.file)
        {
            for hint in &mut repair_hints {
                hint.source_range = None;
            }
        }
        self.diagnostics.push(Diagnostic {
            related_locations,
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

/// Declaration/UI queries use the canonical annotation checker, never a second
/// type-name table. Full declaration checking retains these diagnostics.
pub(crate) fn declared_type(annotation: &webtest_hir::HirType) -> (Type, Vec<Diagnostic>) {
    let schemas = ProviderRegistry::default();
    let mut compiler = Compiler::new(
        annotation.origin.file,
        SourceRevision::of(""),
        &schemas,
        Vec::new(),
    );
    let ty = compiler.lower_type(annotation);
    (ty, compiler.diagnostics)
}

pub(crate) fn type_completions() -> Vec<crate::Completion> {
    type_system::named_types()
        .iter()
        .map(|(name, _)| crate::Completion {
            label: (*name).into(),
            insert_text: (*name).into(),
            kind: crate::CompletionKind::Type,
            ..Default::default()
        })
        .chain(
            ["List<", "Option<", "Response<"]
                .into_iter()
                .map(|name| crate::Completion {
                    label: name.into(),
                    insert_text: name.into(),
                    kind: crate::CompletionKind::Type,
                    ..Default::default()
                }),
        )
        .collect()
}
