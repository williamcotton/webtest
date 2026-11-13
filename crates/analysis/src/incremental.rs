//! Dependency tracking over explicit inputs. Native/portable adapters own input I/O.
use crate::workspace::WorkspaceConfiguration;
use crate::{
    compiler::{self, CompileResult, ProviderSchemas},
    syntax_diagnostics,
};
use salsa::Setter;
use std::sync::OnceLock;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
};
use webtest_hir::HirFile;
use webtest_model::WorkspaceId;
use webtest_provider::{ProviderRegistry, ProviderSchema};
use webtest_syntax::Parse;
use webtest_text::{FileId, SourceRevision};

#[salsa::input(debug)]
pub(crate) struct SourceInput {
    pub file: FileId,
    pub version: Option<webtest_text::DocumentVersion>,
    #[returns(ref)]
    pub path: String,
    #[returns(ref)]
    pub text: Arc<str>,
}

#[salsa::input(debug)]
pub(crate) struct SchemaInput {
    #[returns(ref)]
    pub schema: Option<ProviderSchema>,
    #[returns(ref)]
    names: Vec<String>,
}

#[salsa::input(debug)]
pub(crate) struct FileSlot {
    pub target: Option<SourceInput>,
}
#[salsa::input(debug)]
pub(crate) struct CaseSlot {
    #[returns(ref)]
    pub entries: Vec<(String, SourceInput)>,
}
#[salsa::input(debug)]
pub(crate) struct WorkspaceInput {
    pub identity: WorkspaceId,
    #[returns(ref)]
    pub configuration: WorkspaceConfiguration,
    #[returns(ref)]
    pub files: Vec<(String, SourceInput)>,
}

#[derive(Default)]
pub(crate) struct Counters {
    parse: AtomicU64,
    hir: AtomicU64,
    compile: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueryCounts {
    pub parse: u64,
    pub hir: u64,
    pub compile: u64,
}

#[salsa::db]
pub(crate) trait Db: salsa::Database {
    fn schema_slot(&self, name: &str) -> SchemaInput;
    fn counters(&self) -> &Counters;
    fn file_slot(&self, path: &str) -> FileSlot;
    fn case_slot(&self, folded: &str) -> CaseSlot;
    fn workspace_input(&self) -> WorkspaceInput;
}

#[salsa::db]
#[derive(Clone, Default)]
pub(crate) struct Engine {
    storage: salsa::Storage<Self>,
    schemas: Arc<Mutex<BTreeMap<String, SchemaInput>>>,
    counts: Arc<Counters>,
    files: Arc<Mutex<BTreeMap<String, FileSlot>>>,
    cases: Arc<Mutex<BTreeMap<String, CaseSlot>>>,
    workspace: Arc<OnceLock<WorkspaceInput>>,
}

#[salsa::db]
impl salsa::Database for Engine {}
#[salsa::db]
impl Db for Engine {
    fn schema_slot(&self, name: &str) -> SchemaInput {
        let mut slots = self.schemas.lock().unwrap_or_else(|e| e.into_inner());
        *slots
            .entry(name.into())
            .or_insert_with(|| SchemaInput::new(self, None, Vec::new()))
    }
    fn counters(&self) -> &Counters {
        &self.counts
    }
    fn file_slot(&self, path: &str) -> FileSlot {
        let path = crate::workspace::normalize_path(path).unwrap_or_else(|| path.into());
        *self
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(path)
            .or_insert_with(|| FileSlot::new(self, None))
    }
    fn case_slot(&self, folded: &str) -> CaseSlot {
        *self
            .cases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(folded.into())
            .or_insert_with(|| CaseSlot::new(self, Vec::new()))
    }
    fn workspace_input(&self) -> WorkspaceInput {
        *self.workspace.get_or_init(|| {
            WorkspaceInput::new(
                self,
                WorkspaceId(*SourceRevision::of("webtest-workspace/v1/").as_bytes()),
                WorkspaceConfiguration::default(),
                Vec::new(),
            )
        })
    }
}

pub(crate) fn provider_names(db: &dyn Db) -> Vec<String> {
    db.schema_slot("").names(db).clone()
}

impl Engine {
    pub fn unbind_file(&mut self, input: SourceInput) {
        let slots: Vec<_> = self
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(path, slot)| (path.clone(), *slot))
            .collect();
        for (path, slot) in slots {
            if slot.target(self) == Some(input) {
                self.bind_file(&path, None);
            }
        }
    }
    pub fn bind_file(&mut self, path: &str, target: Option<SourceInput>) {
        let path = crate::workspace::normalize_path(path).unwrap_or_else(|| path.into());
        let slot = self.file_slot(&path);
        if slot.target(self) != target {
            slot.set_target(self).to(target);
        }
        let case = self.case_slot(&path.to_lowercase());
        let mut entries = case.entries(self).clone();
        entries.retain(|(candidate, _)| candidate != &path);
        if let Some(target) = target {
            entries.push((path.clone(), target));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        if case.entries(self) != &entries {
            case.set_entries(self).to(entries);
        }
        let workspace = self.workspace_input();
        let mut files = workspace.files(self).clone();
        files.retain(|(candidate, _)| candidate != &path);
        if let Some(target) = target {
            files.push((path, target));
        }
        files.sort_by(|left, right| left.0.cmp(&right.0));
        if workspace.files(self) != &files {
            workspace.set_files(self).to(files);
        }
    }
    pub fn configure(&mut self, configuration: WorkspaceConfiguration) {
        let workspace = self.workspace_input();
        let identity = WorkspaceId(
            *SourceRevision::of(&format!("webtest-workspace/v1/{}", configuration.root)).as_bytes(),
        );
        if workspace.identity(self) != identity {
            workspace.set_identity(self).to(identity);
        }
        if workspace.configuration(self) != &configuration {
            workspace.set_configuration(self).to(configuration);
        }
    }
    pub fn set_providers(&mut self, registry: &ProviderRegistry) {
        // Never hold the slot lock during a Salsa write: a cancelled request may
        // need the map while unwinding its snapshot.
        let slots: Vec<_> = self
            .schemas
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(name, input)| (name.clone(), *input))
            .collect();
        for (name, input) in slots {
            let schema = registry.schema(&name).cloned();
            if input.schema(self) != &schema {
                input.set_schema(self).to(schema);
            }
        }
        for schema in registry.schemas() {
            let slot = self.schema_slot(&schema.name.0);
            if slot.schema(self).as_ref() != Some(schema) {
                slot.set_schema(self).to(Some(schema.clone()));
            }
        }
        let names = registry
            .schemas()
            .map(|schema| schema.name.0.clone())
            .collect::<Vec<_>>();
        let catalog = self.schema_slot("");
        if catalog.names(self) != &names {
            catalog.set_names(self).to(names);
        }
    }
    pub fn configuration(&self) -> WorkspaceConfiguration {
        self.workspace_input().configuration(self).clone()
    }
    pub fn workspace_id(&self) -> WorkspaceId {
        self.workspace_input().identity(self)
    }
    pub fn counts(&self) -> QueryCounts {
        QueryCounts {
            parse: self.counts.parse.load(Ordering::Relaxed),
            hir: self.counts.hir.load(Ordering::Relaxed),
            compile: self.counts.compile.load(Ordering::Relaxed),
        }
    }
}

struct Schemas<'a>(&'a dyn Db);
impl ProviderSchemas for Schemas<'_> {
    fn schema(&self, name: &str) -> Option<&ProviderSchema> {
        self.0.schema_slot(name).schema(self.0).as_ref()
    }
    fn names(&self) -> Vec<String> {
        self.0.schema_slot("").names(self.0).clone()
    }
}

#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn parsed(db: &dyn Db, file: SourceInput) -> Arc<Parse> {
    db.counters().parse.fetch_add(1, Ordering::Relaxed);
    Arc::new(webtest_syntax::parse(file.text(db)))
}

#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn lowered(db: &dyn Db, file: SourceInput) -> Arc<HirFile> {
    db.counters().hir.fetch_add(1, Ordering::Relaxed);
    let identity = format!(
        "workspace:{}/{}",
        db.workspace_input().identity(db),
        file.path(db)
    );
    Arc::new(webtest_hir::lower_with_source_identity(
        file.file(db),
        &parsed(db, file),
        &identity,
    ))
}

fn fixture_cycle(
    _: &dyn Db,
    _: salsa::Id,
    _: SourceInput,
    _: webtest_model::FixtureId,
) -> Arc<compiler::FixtureCompileResult> {
    Arc::new(compiler::FixtureCompileResult::default())
}
#[salsa::tracked(no_eq, lru = 2048, cycle_result = fixture_cycle)]
pub(crate) fn fixture_template(
    db: &dyn Db,
    file: SourceInput,
    id: webtest_model::FixtureId,
) -> Arc<compiler::FixtureCompileResult> {
    let graph = crate::workspace::module_graph(db, file);
    let hir = lowered(db, file);
    Arc::new(compiler::compile_fixture_template(
        file.file(db),
        SourceRevision::of(file.text(db)),
        &Schemas(db),
        &hir,
        id,
        (db, graph.as_ref()),
    ))
}

#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn compiled(db: &dyn Db, file: SourceInput) -> Arc<CompileResult> {
    db.counters().compile.fetch_add(1, Ordering::Relaxed);
    let parse = parsed(db, file);
    let hir = lowered(db, file);
    let graph = crate::workspace::module_graph(db, file);
    let mut diagnostics = syntax_diagnostics::collect(&parse);
    if db.workspace_input().configuration(db).invalid_inputs {
        diagnostics.push(crate::Diagnostic { range: webtest_text::TextRange::empty(0.into()), severity: crate::DiagnosticSeverity::Error, code: "workspace.invalid_inputs", message: "Project configuration or provider inputs are invalid; fix them before running tests.".into(), source: crate::DiagnosticSource::Semantic, related_locations: Vec::new(), semantic_details: None, repair_hints: Vec::new(), reference_queries: Vec::new() });
    }
    diagnostics.extend(graph.diagnostics.iter().map(|diagnostic| crate::Diagnostic {
        related_locations:std::iter::once(diagnostic.origin).chain(diagnostic.related.iter().copied()).filter_map(|origin|graph.source_location(origin)).collect(),
        range: graph.actionable_origin(diagnostic.origin).range,
        code: diagnostic.code, message: diagnostic.message.clone(),
        severity: crate::DiagnosticSeverity::Error,
        source: if diagnostic.code.starts_with("syntax.") { crate::DiagnosticSource::Syntax } else { crate::DiagnosticSource::Semantic },
        semantic_details: Some(serde_json::json!({ "declaration_origin": diagnostic.origin, "related_origins": diagnostic.related, "candidate_module_paths": diagnostic.candidates })),
        repair_hints: Vec::new(), reference_queries: vec!["language.import".into()],
    }));
    for module in graph.modules.values() {
        for diagnostic in crate::callables::declaration_diagnostics(db, module.input).iter() {
            diagnostics.push(crate::Diagnostic {
                related_locations: std::iter::once(diagnostic.origin).chain(diagnostic.related.iter().copied()).filter_map(|origin| graph.source_location(origin)).collect(),
                range: graph.actionable_origin(diagnostic.origin).range, code: diagnostic.code, message: diagnostic.message.clone(), severity: crate::DiagnosticSeverity::Error, source: crate::DiagnosticSource::Semantic,
                semantic_details: Some(serde_json::json!({"declaration_origin":diagnostic.origin,"related_origins":diagnostic.related})), repair_hints: Vec::new(), reference_queries: Vec::new(),
            });
        }
    }
    Arc::new(compiler::compile_in_workspace(
        file.file(db),
        SourceRevision::of(file.text(db)),
        &Schemas(db),
        diagnostics,
        &hir,
        file.path(db),
        (db, &graph),
    ))
}

/// Cross-module fixture calls project an already checked module. Same-module
/// forward declarations retain the independent template query to avoid a cycle
/// between compiling a module and resolving one of its own declarations.
#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn fixture_definition(
    db: &dyn Db,
    file: SourceInput,
    id: webtest_model::FixtureId,
) -> Arc<compiler::FixtureCompileResult> {
    let compiled = compiled(db, file);
    let definition = compiled
        .fixtures
        .iter()
        .find(|definition| definition.id == id)
        .cloned();
    let mut pending = definition.iter().collect::<Vec<_>>();
    let mut seen = std::collections::BTreeSet::from([id]);
    let mut dependencies = Vec::new();
    while let Some(definition) = pending.pop() {
        for step in definition
            .setup
            .steps()
            .into_iter()
            .chain(definition.teardown.steps())
        {
            if let webtest_plan::TestOperation::AcquireFixture(call) = &step.operation
                && seen.insert(call.fixture)
                && let Some(dependency) = compiled
                    .plan
                    .fixtures
                    .iter()
                    .find(|definition| definition.id == call.fixture)
            {
                dependencies.push(dependency.clone());
                pending.push(dependency);
            }
        }
    }
    let diagnostics = compiled
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            definition
                .as_ref()
                .is_some_and(|definition| definition.origin.range.contains_range(diagnostic.range))
        })
        .cloned()
        .collect();
    Arc::new(compiler::FixtureCompileResult {
        definition,
        dependencies,
        diagnostics,
    })
}
