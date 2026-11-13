//! Explicit source/provider inputs and the revision-coherent per-file query cache.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use crate::{
    compiler::CompileResult,
    description::{
        self, DescriptionLimits, DescriptionProject, DescriptionRequest, DescriptionResponse,
    },
    diagnostic::Diagnostic,
    editor_queries,
    facts::{Completion, DocumentationFact, Signature, TypeFact},
    incremental::{self, Engine, SourceInput},
};
use salsa::{Database as _, Setter};
use thiserror::Error;
use webtest_hir::HirFile;
use webtest_plan::TestPlan;
use webtest_provider::ProviderRegistry;
use webtest_syntax::Parse;
use webtest_text::{FileId, SourceRevision, TextSize};

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("unknown source file {0:?}")]
    UnknownFile(FileId),
    #[error("analysis request was cancelled by a workspace update")]
    Cancelled,
    #[error("invalid UTF-8 position {offset} in source file {file:?}")]
    InvalidPosition { file: FileId, offset: u32 },
}

#[derive(Clone, Debug)]
struct SourceFile {
    input: SourceInput,
    path: String,
    text: Arc<str>,
    revision: SourceRevision,
    version: Option<webtest_text::DocumentVersion>,
    uri: Option<String>,
}

#[derive(Clone, Debug)]
struct CachedQueries {
    result: Arc<CompileResult>,
    diagnostics: Arc<Vec<Diagnostic>>,
    type_facts: Arc<Vec<TypeFact>>,
    plan: Arc<TestPlan>,
}

#[derive(Clone)]
pub struct AnalysisDatabase {
    engine: Engine,
    revision: u64,
    files: HashMap<FileId, SourceFile>,
    paths: HashMap<String, FileId>,
    cache: HashMap<FileId, CachedQueries>,
    next_file: u32,
    providers: ProviderRegistry,
}

impl Default for AnalysisDatabase {
    fn default() -> Self {
        let providers = ProviderRegistry::built_in_schemas();
        let mut engine = Engine::default();
        engine.set_providers(&providers);
        Self {
            engine,
            revision: 0,
            files: HashMap::new(),
            paths: HashMap::new(),
            cache: HashMap::new(),
            next_file: 0,
            providers,
        }
    }
}

impl AnalysisDatabase {
    pub fn with_provider_registry(providers: ProviderRegistry) -> Self {
        let mut database = Self::default();
        database.set_provider_registry(providers);
        database
    }

    pub fn open_file(&mut self, path: impl Into<String>, text: impl Into<String>) -> FileId {
        let path = path.into();
        let path = crate::workspace::normalize_path(&path).unwrap_or(path);
        if let Some(file) = self.paths.get(&path).copied() {
            self.set_file_text(file, text);
            return file;
        }
        let file = FileId::new(self.next_file);
        self.next_file += 1;
        let text: Arc<str> = Arc::from(text.into());
        let input = SourceInput::new(&self.engine, file, None, path.clone(), text.clone());
        self.revision += 1;
        self.files.insert(
            file,
            SourceFile {
                input,
                path: path.clone(),
                version: None,
                uri: None,
                revision: SourceRevision::of(&text),
                text,
            },
        );
        self.engine.bind_file(&path, Some(input));
        self.paths.insert(path, file);
        file
    }

    pub fn set_file_text(&mut self, file: FileId, text: impl Into<String>) {
        let text: Arc<str> = Arc::from(text.into());
        if let Some(source) = self.files.get_mut(&file) {
            let revision = SourceRevision::of(&text);
            if source.revision != revision {
                self.revision += 1;
                source.input.set_text(&mut self.engine).to(text.clone());
                source.text = text;
                source.revision = revision;
                self.cache.remove(&file);
            }
        }
    }

    pub fn file_for_path(&self, path: &str) -> Option<FileId> {
        self.paths
            .get(&crate::workspace::normalize_path(path).unwrap_or_else(|| path.into()))
            .copied()
    }

    pub fn close_file(&mut self, file: FileId) {
        if let Some(source) = self.files.get(&file).cloned() {
            self.revision += 1;
            self.engine.unbind_file(source.input);
            self.files.remove(&file);
            self.paths.retain(|_, candidate| *candidate != file);
        }
        self.cache.remove(&file);
    }

    pub fn source(&self, file: FileId) -> Result<Arc<str>, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| Arc::clone(&source.text))
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn path(&self, file: FileId) -> Result<&str, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| source.path.as_str())
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn source_revision(&self, file: FileId) -> Result<SourceRevision, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| source.revision)
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn parse(&mut self, file: FileId) -> Result<Parse, AnalysisError> {
        let input = self.source_input(file)?;
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            (*incremental::parsed(&self.engine, input)).clone()
        }))
        .map_err(|_| AnalysisError::Cancelled)
    }

    pub fn hir_file(&mut self, file: FileId) -> Result<Arc<HirFile>, AnalysisError> {
        let input = self.source_input(file)?;
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            incremental::lowered(&self.engine, input)
        }))
        .map_err(|_| AnalysisError::Cancelled)
    }

    pub fn diagnostics(&mut self, file: FileId) -> Result<Arc<Vec<Diagnostic>>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].diagnostics))
    }

    pub fn type_facts(&mut self, file: FileId) -> Result<Arc<Vec<TypeFact>>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].type_facts))
    }

    pub fn type_at(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<TypeFact>, AnalysisError> {
        self.validate_offset(file, offset)?;
        Ok(self
            .type_facts(file)?
            .iter()
            .filter(|fact| fact.range.contains(offset) || fact.range.end() == offset)
            .min_by_key(|fact| fact.range.len())
            .cloned())
    }

    pub fn fixture_definitions(
        &mut self,
        file: FileId,
    ) -> Result<Vec<webtest_plan::FixtureDefinition>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(self.cache[&file].result.fixtures.clone())
    }
    pub fn test_plan(&mut self, file: FileId) -> Result<Arc<TestPlan>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].plan))
    }

    pub fn provider_schema_hashes(&self) -> BTreeMap<String, String> {
        self.providers
            .schemas()
            .map(|schema| (schema.name.0.clone(), schema.hash()))
            .collect()
    }

    pub fn set_provider_registry(&mut self, providers: ProviderRegistry) {
        if !self.providers.schemas().eq(providers.schemas()) {
            self.revision += 1;
        }
        self.engine.set_providers(&providers);
        self.providers = providers;
    }

    pub fn document_uri(&self, file: FileId) -> Option<&str> {
        self.files
            .get(&file)
            .and_then(|source| source.uri.as_deref())
    }
    pub fn set_document_uri(
        &mut self,
        file: FileId,
        uri: Option<String>,
    ) -> Result<(), AnalysisError> {
        let source = self
            .files
            .get_mut(&file)
            .ok_or(AnalysisError::UnknownFile(file))?;
        if source.uri != uri {
            self.revision += 1;
            source.uri = uri;
        }
        Ok(())
    }
    pub fn source_aliases(&self) -> Vec<(String, String)> {
        let mut aliases = self
            .paths
            .iter()
            .filter_map(|(alias, file)| {
                self.files
                    .get(file)
                    .filter(|source| source.path != *alias)
                    .map(|source| (alias.clone(), source.path.clone()))
            })
            .collect::<Vec<_>>();
        aliases.sort();
        aliases
    }
    pub fn remove_alias(&mut self, path: &str) {
        if let Some(file) = self.paths.get(path).copied()
            && self
                .files
                .get(&file)
                .is_some_and(|source| source.path != path)
        {
            self.revision += 1;
            self.paths.remove(path);
            self.engine.bind_file(path, None);
        }
    }
    pub fn source_inputs(&self) -> Vec<(String, Arc<str>)> {
        let mut inputs = self
            .files
            .values()
            .map(|source| (source.path.clone(), source.text.clone()))
            .collect::<Vec<_>>();
        inputs.sort_by(|left, right| left.0.cmp(&right.0));
        inputs
    }
    pub fn clear_document_version(&mut self, file: FileId) -> Result<(), AnalysisError> {
        let source = self
            .files
            .get_mut(&file)
            .ok_or(AnalysisError::UnknownFile(file))?;
        if source.version.is_some() {
            self.revision += 1;
            source.version = None;
            source.input.set_version(&mut self.engine).to(None);
        }
        Ok(())
    }
    pub fn workspace_revision(&self) -> u64 {
        self.revision
    }
    pub fn workspace_id(&self) -> webtest_model::WorkspaceId {
        self.engine.workspace_id()
    }
    pub fn project_description(&self, limit: usize) -> crate::ProjectSemanticDescription {
        crate::description::workspace::describe(&self.engine, self.revision, limit)
    }
    pub fn discover_tests(&self, limit: usize) -> crate::TestDiscovery {
        crate::discovery::discover(&self.engine, self.revision, limit)
    }
    pub fn document_symbols(&self, file: FileId) -> Result<Vec<crate::Symbol>, AnalysisError> {
        let input = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?
            .input;
        Ok(crate::navigation::document_symbols(&self.engine, input))
    }
    pub fn plans_for_variants(
        &mut self,
        variants: &std::collections::BTreeSet<webtest_model::TestVariantId>,
    ) -> Result<Option<Vec<Arc<TestPlan>>>, AnalysisError> {
        let tree = self.discover_tests(crate::navigation::MAX_QUERY_ITEMS);
        if tree.truncated {
            return Ok(None);
        }
        let mut found = std::collections::BTreeSet::new();
        let mut plans = Vec::new();
        for module in tree.modules {
            let selected = module
                .tests
                .iter()
                .flat_map(|test| &test.variants)
                .filter(|variant| variants.contains(&variant.id))
                .collect::<Vec<_>>();
            if selected.is_empty() {
                continue;
            }
            if selected.iter().any(|variant| !variant.runnable) {
                return Ok(None);
            }
            found.extend(selected.iter().map(|variant| variant.id));
            plans.push(Arc::new(
                self.test_plan(module.file)?.select_variants(variants),
            ));
        }
        Ok((found == *variants).then_some(plans))
    }
    pub fn semantic_tokens(
        &self,
        file: FileId,
    ) -> Result<Vec<crate::SemanticToken>, AnalysisError> {
        Ok(crate::semantic_tokens::tokens(
            &self.engine,
            self.source_input(file)?,
        ))
    }
    pub fn inlay_hints(
        &self,
        file: FileId,
    ) -> Result<crate::QueryResult<crate::InlayHint>, AnalysisError> {
        Ok(crate::navigation::inlay_hints(
            &self.engine,
            self.source_input(file)?,
        ))
    }
    pub fn code_actions(
        &self,
        file: FileId,
        range: webtest_text::TextRange,
    ) -> Result<crate::QueryResult<crate::CodeAction>, AnalysisError> {
        self.validate_offset(file, range.start())?;
        self.validate_offset(file, range.end())?;
        Ok(crate::navigation::code_actions(
            &self.engine,
            self.source_input(file)?,
            range,
        ))
    }
    pub fn workspace_symbols(
        &self,
        query: &str,
        limit: usize,
    ) -> crate::QueryResult<crate::Symbol> {
        crate::navigation::workspace_symbols(&self.engine, query, limit)
    }
    pub fn definition(
        &self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<crate::LocationLink>, AnalysisError> {
        self.validate_offset(file, offset)?;
        let input = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?
            .input;
        Ok(crate::navigation::definition(&self.engine, input, offset))
    }
    pub fn references(
        &self,
        file: FileId,
        offset: TextSize,
        include_declaration: bool,
        limit: usize,
    ) -> Result<crate::QueryResult<crate::Reference>, AnalysisError> {
        self.validate_offset(file, offset)?;
        let input = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?
            .input;
        Ok(crate::navigation::references(
            &self.engine,
            input,
            offset,
            include_declaration,
            limit,
        ))
    }
    pub fn rename(
        &self,
        file: FileId,
        offset: TextSize,
        name: &str,
    ) -> Result<crate::WorkspaceEdit, crate::RenameError> {
        self.validate_offset(file, offset)?;
        let input = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?
            .input;
        let mut edit = crate::navigation::rename(&self.engine, input, offset, name)?;
        for document in &mut edit.documents {
            document.version = self
                .files
                .get(&document.file)
                .and_then(|source| source.version);
        }
        Ok(edit)
    }
    pub fn folding_ranges(&self, file: FileId) -> Result<Vec<crate::FoldingRange>, AnalysisError> {
        let input = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?
            .input;
        Ok(crate::navigation::folding(&self.engine, input))
    }
    pub fn selection_range(
        &self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<crate::SelectionRange>, AnalysisError> {
        self.validate_offset(file, offset)?;
        let input = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?
            .input;
        Ok(crate::navigation::selection(&self.engine, input, offset))
    }
    pub fn set_document_version(
        &mut self,
        file: FileId,
        version: webtest_text::DocumentVersion,
    ) -> Result<(), AnalysisError> {
        let source = self
            .files
            .get_mut(&file)
            .ok_or(AnalysisError::UnknownFile(file))?;
        if source.version != Some(version) {
            self.revision += 1;
            source.version = Some(version);
            source.input.set_version(&mut self.engine).to(Some(version));
        }
        Ok(())
    }
    pub fn declaration_signature(
        &self,
        file: FileId,
        id: webtest_model::SymbolId,
    ) -> Result<Option<Arc<crate::DeclarationSignature>>, AnalysisError> {
        let input = self.source_input(file)?;
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            crate::callables::signature(&self.engine, input, id)
        }))
        .map_err(|_| AnalysisError::Cancelled)
    }
    pub fn workspace_configuration(&self) -> crate::WorkspaceConfiguration {
        self.engine.configuration()
    }
    pub fn configure_workspace(&mut self, configuration: crate::WorkspaceConfiguration) {
        if self.engine.configuration() != configuration {
            self.revision += 1;
            self.engine.configure(configuration);
        }
    }
    pub fn module_graph(&self, file: FileId) -> Result<Arc<crate::ModuleGraph>, AnalysisError> {
        let input = self.source_input(file)?;
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            crate::workspace::module_graph(&self.engine, input)
        }))
        .map_err(|_| AnalysisError::Cancelled)
    }
    pub fn alias_file(&mut self, alias: &str, file: FileId) -> Result<(), AnalysisError> {
        let input = self.source_input(file)?;
        if self.file_for_path(alias) == Some(file) {
            return Ok(());
        }
        self.revision += 1;
        self.engine.bind_file(alias, Some(input));
        self.paths.insert(
            crate::workspace::normalize_path(alias).unwrap_or_else(|| alias.into()),
            file,
        );
        Ok(())
    }

    pub fn query_counts(&self) -> crate::QueryCounts {
        self.engine.counts()
    }

    /// A request-scoped read snapshot. A pending write cancels tracked work and
    /// waits for this handle to be dropped; never retain it across host I/O.
    pub fn snapshot(&self) -> AnalysisSnapshot {
        AnalysisSnapshot {
            database: self.clone(),
        }
    }

    fn validate_offset(&self, file: FileId, offset: TextSize) -> Result<(), AnalysisError> {
        let source = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?;
        if !source.text.is_char_boundary(usize::from(offset)) {
            return Err(AnalysisError::InvalidPosition {
                file,
                offset: offset.into(),
            });
        }
        Ok(())
    }
    fn source_input(&self, file: FileId) -> Result<SourceInput, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| source.input)
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn completions(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Vec<Completion>, AnalysisError> {
        self.validate_offset(file, offset)?;
        let parse = self.parse(file)?;
        self.ensure_queries(file)?;
        let cached = &self.cache[&file];
        let mut items =
            editor_queries::completions(&parse, &cached.type_facts, &self.providers, offset);
        if items.is_empty() {
            items = crate::workspace_editor::completions(
                &self.engine,
                self.source_input(file)?,
                &parse,
                offset,
            );
        }
        crate::workspace_editor::decorate(&parse, offset, &mut items);
        Ok(items)
    }

    pub fn signature_help(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<Signature>, AnalysisError> {
        self.validate_offset(file, offset)?;
        let parse = self.parse(file)?;
        let input = self.source_input(file)?;
        Ok(
            editor_queries::signature_help(&parse, &self.providers, offset).or_else(|| {
                crate::workspace_editor::signature(&self.engine, input, &parse, offset)
            }),
        )
    }

    pub fn documentation_at(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<DocumentationFact>, AnalysisError> {
        self.validate_offset(file, offset)?;
        let parse = self.parse(file)?;
        let input = self.source_input(file)?;
        Ok(
            crate::workspace_editor::documentation(&self.engine, input, offset)
                .or_else(|| editor_queries::documentation_at(&parse, &self.providers, offset)),
        )
    }

    pub fn describe(
        &self,
        request: DescriptionRequest,
        project: Option<DescriptionProject>,
        limits: DescriptionLimits,
    ) -> DescriptionResponse {
        if matches!(&request, DescriptionRequest::Query(query) if matches!(query.trim(), "project" | "project.declarations"))
        {
            return DescriptionResponse::Project(
                self.project_description(limits.max_category_children),
            );
        }
        description::describe(&self.providers, request, project, limits)
    }

    fn ensure_queries(&mut self, file: FileId) -> Result<(), AnalysisError> {
        let input = self.source_input(file)?;
        let compiled = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            incremental::compiled(&self.engine, input)
        }))
        .map_err(|_| AnalysisError::Cancelled)?;
        if self
            .cache
            .get(&file)
            .is_some_and(|cached| Arc::ptr_eq(&cached.result, &compiled))
        {
            return Ok(());
        }
        self.cache.insert(
            file,
            CachedQueries {
                result: compiled.clone(),
                diagnostics: Arc::new(compiled.diagnostics.clone()),
                type_facts: Arc::new(compiled.type_facts.clone()),
                plan: Arc::new(compiled.plan.clone()),
            },
        );
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AnalysisCancellation {
    inner: salsa::CancellationToken,
}
impl AnalysisCancellation {
    pub fn cancel(&self) {
        self.inner.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

pub struct AnalysisSnapshot {
    database: AnalysisDatabase,
}
impl AnalysisSnapshot {
    pub fn workspace_revision(&self) -> u64 {
        self.database.workspace_revision()
    }
    fn query<R>(&self, query: impl FnOnce(&AnalysisDatabase) -> R) -> Result<R, AnalysisError> {
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            self.database.engine.unwind_if_revision_cancelled();
            query(&self.database)
        }))
        .map_err(|_| AnalysisError::Cancelled)
    }
    pub fn project_description(
        &self,
        limit: usize,
    ) -> Result<crate::ProjectSemanticDescription, AnalysisError> {
        self.query(|db| db.project_description(limit))
    }
    pub fn discover_tests(&self, limit: usize) -> Result<crate::TestDiscovery, AnalysisError> {
        self.query(|db| db.discover_tests(limit))
    }
    pub fn document_symbols(&self, file: FileId) -> Result<Vec<crate::Symbol>, AnalysisError> {
        self.query(|db| db.document_symbols(file))?
    }
    pub fn plans_for_variants(
        &mut self,
        variants: &std::collections::BTreeSet<webtest_model::TestVariantId>,
    ) -> Result<Option<Vec<Arc<TestPlan>>>, AnalysisError> {
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            self.database.engine.unwind_if_revision_cancelled();
            self.database.plans_for_variants(variants)
        }))
        .map_err(|_| AnalysisError::Cancelled)?
    }
    pub fn semantic_tokens(
        &self,
        file: FileId,
    ) -> Result<Vec<crate::SemanticToken>, AnalysisError> {
        self.query(|db| db.semantic_tokens(file))?
    }
    pub fn inlay_hints(
        &self,
        file: FileId,
    ) -> Result<crate::QueryResult<crate::InlayHint>, AnalysisError> {
        self.query(|db| db.inlay_hints(file))?
    }
    pub fn code_actions(
        &self,
        file: FileId,
        range: webtest_text::TextRange,
    ) -> Result<crate::QueryResult<crate::CodeAction>, AnalysisError> {
        self.query(|db| db.code_actions(file, range))?
    }
    pub fn workspace_symbols(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<crate::QueryResult<crate::Symbol>, AnalysisError> {
        self.query(|db| db.workspace_symbols(query, limit))
    }
    pub fn definition(
        &self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<crate::LocationLink>, AnalysisError> {
        self.query(|db| db.definition(file, offset))?
    }
    pub fn references(
        &self,
        file: FileId,
        offset: TextSize,
        include_declaration: bool,
        limit: usize,
    ) -> Result<crate::QueryResult<crate::Reference>, AnalysisError> {
        self.query(|db| db.references(file, offset, include_declaration, limit))?
    }
    pub fn rename(
        &self,
        file: FileId,
        offset: TextSize,
        name: &str,
    ) -> Result<crate::WorkspaceEdit, crate::RenameError> {
        self.query(|db| db.rename(file, offset, name))
            .map_err(crate::RenameError::Analysis)?
    }
    pub fn folding_ranges(&self, file: FileId) -> Result<Vec<crate::FoldingRange>, AnalysisError> {
        self.query(|db| db.folding_ranges(file))?
    }
    pub fn selection_range(
        &self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<crate::SelectionRange>, AnalysisError> {
        self.query(|db| db.selection_range(file, offset))?
    }
    pub fn signature_help(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<Signature>, AnalysisError> {
        self.database.signature_help(file, offset)
    }
    pub fn type_at(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<TypeFact>, AnalysisError> {
        self.database.type_at(file, offset)
    }
    pub fn documentation_at(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<DocumentationFact>, AnalysisError> {
        self.database.documentation_at(file, offset)
    }
    pub fn document_uri(&self, file: FileId) -> Option<&str> {
        self.database.document_uri(file)
    }
    pub fn path(&self, file: FileId) -> Result<&str, AnalysisError> {
        self.database.path(file)
    }
    pub fn parse(&mut self, file: FileId) -> Result<Parse, AnalysisError> {
        self.database.parse(file)
    }

    pub fn module_graph(&self, file: FileId) -> Result<Arc<crate::ModuleGraph>, AnalysisError> {
        self.database.module_graph(file)
    }
    pub fn cancellation(&self) -> AnalysisCancellation {
        AnalysisCancellation {
            inner: self.database.engine.cancellation_token(),
        }
    }
    pub fn file_for_path(&self, path: &str) -> Option<FileId> {
        self.database.file_for_path(path)
    }
    pub fn source(&self, file: FileId) -> Result<Arc<str>, AnalysisError> {
        self.database.source(file)
    }
    pub fn source_revision(&self, file: FileId) -> Result<SourceRevision, AnalysisError> {
        self.database.source_revision(file)
    }
    pub fn diagnostics(&mut self, file: FileId) -> Result<Arc<Vec<Diagnostic>>, AnalysisError> {
        self.database.diagnostics(file)
    }
    pub fn fixture_definitions(
        &mut self,
        file: FileId,
    ) -> Result<Vec<webtest_plan::FixtureDefinition>, AnalysisError> {
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            self.database.fixture_definitions(file)
        }))
        .map_err(|_| AnalysisError::Cancelled)?
    }
    pub fn test_plan(&mut self, file: FileId) -> Result<Arc<TestPlan>, AnalysisError> {
        self.database.test_plan(file)
    }
    pub fn completions(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Vec<Completion>, AnalysisError> {
        self.database.completions(file, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_model::{Capability, Type};
    use webtest_provider::{OperationName, OperationSchema, ProviderName, ProviderSchema};

    fn app_registry(operation: &str, result: Type) -> ProviderRegistry {
        let mut registry = ProviderRegistry::built_in_schemas();
        registry.register_schema(ProviderSchema {
            name: ProviderName("app".into()),
            operations: [(
                operation.into(),
                OperationSchema {
                    name: OperationName(operation.into()),
                    parameters: Vec::new(),
                    result,
                    capability: Capability::Server,
                    documentation: format!("Call {operation}."),
                    retry_safe: false,
                },
            )]
            .into(),
            schema_identity: None,
        });
        registry
    }

    #[test]
    fn file_lifecycle_preserves_identity_revision_and_unknown_file_contracts() {
        let mut database = AnalysisDatabase::default();
        let first = database.open_file("one.webtest", "test \"one\" {}");
        let second = database.open_file("two.webtest", "test \"two\" {}");
        assert_eq!(first, FileId::new(0));
        assert_eq!(second, FileId::new(1));
        assert_eq!(database.file_for_path("one.webtest"), Some(first));
        assert_eq!(database.path(first).expect("path"), "one.webtest");
        assert_eq!(&*database.source(first).expect("source"), "test \"one\" {}");

        let revision = database.source_revision(first).expect("revision");
        let reopened = database.open_file("one.webtest", "test \"one\" {}");
        assert_eq!(reopened, first);
        assert_eq!(database.source_revision(first).expect("revision"), revision);

        database.open_file("one.webtest", "test \"changed\" {}");
        assert_ne!(database.source_revision(first).expect("revision"), revision);
        database.close_file(first);
        assert_eq!(database.file_for_path("one.webtest"), None);
        assert!(
            matches!(database.source(first), Err(AnalysisError::UnknownFile(id)) if id == first)
        );
        assert!(
            matches!(database.parse(first), Err(AnalysisError::UnknownFile(id)) if id == first)
        );

        database.set_file_text(FileId::new(999), "ignored");
        let third = database.open_file("three.webtest", "test \"three\" {}");
        assert_eq!(third, FileId::new(2));
    }

    #[test]
    fn source_changes_invalidate_only_the_changed_file_and_queries_stay_revision_coherent() {
        let mut database = AnalysisDatabase::default();
        let first = database.open_file("one.webtest", "test \"one\" { let value = 1 }");
        let second = database.open_file("two.webtest", "test \"two\" { let value = 2 }");
        let first_diagnostics = database.diagnostics(first).expect("diagnostics");
        let second_diagnostics = database.diagnostics(second).expect("diagnostics");
        let first_plan = database.test_plan(first).expect("plan");

        database.set_file_text(first, "test \"one\" { let value = true }");
        assert!(!database.cache.contains_key(&first));
        assert!(database.cache.contains_key(&second));
        assert!(Arc::ptr_eq(
            &second_diagnostics,
            &database.diagnostics(second).expect("diagnostics")
        ));

        let revision = database.source_revision(first).expect("revision");
        let parse = database.parse(first).expect("parse");
        let hir = database.hir_file(first).expect("hir");
        let diagnostics = database.diagnostics(first).expect("diagnostics");
        let facts = database.type_facts(first).expect("facts");
        let plan = database.test_plan(first).expect("plan");
        assert_eq!(
            parse.syntax().text().to_string(),
            "test \"one\" { let value = true }"
        );
        assert_eq!(hir.tests.len(), 1);
        assert!(diagnostics.is_empty());
        assert!(!facts.is_empty());
        assert_eq!(plan.source_revision, revision);
        assert!(!Arc::ptr_eq(&first_diagnostics, &diagnostics));
        assert!(!Arc::ptr_eq(&first_plan, &plan));
    }

    #[test]
    fn unchanged_source_and_equivalent_provider_schemas_retain_cached_results() {
        let registry = app_registry("echo", Type::String);
        let mut database = AnalysisDatabase::with_provider_registry(registry.clone());
        let file = database.open_file(
            "app.webtest",
            "test \"app\" { server { let value = app.echo() } }",
        );
        let diagnostics = database.diagnostics(file).expect("diagnostics");
        let plan = database.test_plan(file).expect("plan");
        database.set_file_text(file, "test \"app\" { server { let value = app.echo() } }");
        database.set_provider_registry(registry);
        assert!(Arc::ptr_eq(
            &diagnostics,
            &database.diagnostics(file).expect("diagnostics")
        ));
        assert!(Arc::ptr_eq(&plan, &database.test_plan(file).expect("plan")));
    }

    #[test]
    fn provider_changes_recompute_only_semantic_consumers_and_reuse_parsing_and_lowering() {
        let mut database =
            AnalysisDatabase::with_provider_registry(app_registry("echo", Type::String));
        let dependent = database.open_file(
            "app.webtest",
            "test \"app\" { server { let value = app.echo() } }",
        );
        let unrelated = database.open_file("pure.webtest", "test \"pure\" { expect 1 == 1 }");
        database.diagnostics(dependent).unwrap();
        let unchanged = database.test_plan(unrelated).unwrap();
        let before = database.query_counts();
        database.set_provider_registry(app_registry("echo", Type::Bool));
        database.diagnostics(dependent).unwrap();
        assert!(Arc::ptr_eq(
            &unchanged,
            &database.test_plan(unrelated).unwrap()
        ));
        let after = database.query_counts();
        assert_eq!(after.parse, before.parse);
        assert_eq!(after.hir, before.hir);
        assert_eq!(after.compile, before.compile + 1);
        database.set_provider_registry(ProviderRegistry::built_in_schemas());
        assert!(
            database
                .diagnostics(dependent)
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.reserved_provider")
        );
        assert!(Arc::ptr_eq(
            &unchanged,
            &database.test_plan(unrelated).unwrap()
        ));
        database.set_provider_registry(app_registry("echo", Type::String));
        assert!(database.diagnostics(dependent).unwrap().is_empty());
        assert_eq!(database.query_counts().compile, before.compile + 3);
    }

    #[test]
    fn parsing_does_not_eagerly_run_lowering_or_semantics() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("one.webtest", "test \"one\" { expect 1 == 1 }");
        database.parse(file).unwrap();
        assert_eq!(
            database.query_counts(),
            crate::QueryCounts {
                parse: 1,
                hir: 0,
                compile: 0
            }
        );
        database.hir_file(file).unwrap();
        assert_eq!(
            database.query_counts(),
            crate::QueryCounts {
                parse: 1,
                hir: 1,
                compile: 0
            }
        );
        database.diagnostics(file).unwrap();
        assert_eq!(
            database.query_counts(),
            crate::QueryCounts {
                parse: 1,
                hir: 1,
                compile: 1
            }
        );
    }

    #[test]
    fn snapshot_cancellation_is_request_local_and_completed_results_stay_immutable() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("one.webtest", "test \"one\" {}");
        let old = database.test_plan(file).unwrap();
        let mut snapshot = database.snapshot();
        snapshot.cancellation().cancel();
        assert!(matches!(
            snapshot.diagnostics(file),
            Err(AnalysisError::Cancelled)
        ));
        drop(snapshot);
        database.set_file_text(file, "test \"two\" {}");
        assert_eq!(old.tests[0].name, "one");
        assert_eq!(database.test_plan(file).unwrap().tests[0].name, "two");
    }

    #[test]
    fn a_pending_write_cancels_parallel_read_snapshots_before_publishing_new_inputs() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("one.webtest", "test \"one\" {}");
        let old = database.test_plan(file).unwrap();
        let mut snapshot = database.snapshot();
        let writer = std::thread::spawn(move || {
            database.set_file_text(file, "test \"two\" {}");
            database
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if matches!(snapshot.diagnostics(file), Err(AnalysisError::Cancelled)) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "writer did not cancel its stale reader"
            );
            std::thread::yield_now();
        }
        assert_eq!(&*snapshot.source(file).unwrap(), "test \"one\" {}");
        drop(snapshot);
        let mut database = writer.join().unwrap();
        assert_eq!(old.tests[0].name, "one");
        assert_eq!(database.test_plan(file).unwrap().tests[0].name, "two");
    }

    #[test]
    fn repeated_edits_drop_previous_compiler_memos_instead_of_retaining_history() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("one.webtest", "test \"0\" {}");
        let mut old = Vec::new();
        for edit in 0..1000 {
            database.set_file_text(file, format!("test \"{edit}\" {{}}"));
            database.diagnostics(file).unwrap();
            old.push(Arc::downgrade(&database.cache[&file].result));
            let retained = old.iter().filter(|memo| memo.upgrade().is_some()).count();
            assert!(
                retained <= 2,
                "retained {retained} historical compiler results after {edit} edits"
            );
        }
        assert_eq!(database.cache.len(), 1);
    }

    #[test]
    fn type_at_prefers_the_smallest_fact_and_includes_end_offsets() {
        let source = "test \"x\" { let value = { id: 1 } let selected = value.id }";
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("type.webtest", source);
        let member_offset = TextSize::from(
            u32::try_from(source.find("value.id").expect("member") + "value.id".len())
                .expect("offset"),
        );
        let fact = database
            .type_at(file, member_offset)
            .expect("type query")
            .expect("fact");
        assert_eq!(fact.ty, Type::Int);
    }
}
