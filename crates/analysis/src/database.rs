//! Explicit source/provider inputs and the revision-coherent per-file query cache.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use crate::{
    compiler,
    description::{
        self, DescriptionLimits, DescriptionProject, DescriptionRequest, DescriptionResponse,
    },
    diagnostic::Diagnostic,
    editor_queries,
    facts::{Completion, DocumentationFact, Signature, TypeFact},
    syntax_diagnostics,
};
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
}

#[derive(Clone, Debug)]
struct SourceFile {
    path: String,
    text: Arc<str>,
    revision: SourceRevision,
}

#[derive(Clone, Debug)]
struct CachedQueries {
    revision: SourceRevision,
    parse: Parse,
    hir: Arc<HirFile>,
    diagnostics: Arc<Vec<Diagnostic>>,
    type_facts: Arc<Vec<TypeFact>>,
    plan: Arc<TestPlan>,
}

pub struct AnalysisDatabase {
    files: HashMap<FileId, SourceFile>,
    paths: HashMap<String, FileId>,
    cache: HashMap<FileId, CachedQueries>,
    next_file: u32,
    providers: ProviderRegistry,
}

impl Default for AnalysisDatabase {
    fn default() -> Self {
        Self {
            files: HashMap::new(),
            paths: HashMap::new(),
            cache: HashMap::new(),
            next_file: 0,
            providers: ProviderRegistry::built_in_schemas(),
        }
    }
}

impl AnalysisDatabase {
    pub fn with_provider_registry(providers: ProviderRegistry) -> Self {
        Self {
            providers,
            ..Self::default()
        }
    }

    pub fn open_file(&mut self, path: impl Into<String>, text: impl Into<String>) -> FileId {
        let path = path.into();
        if let Some(file) = self.paths.get(&path).copied() {
            self.set_file_text(file, text);
            return file;
        }
        let file = FileId::new(self.next_file);
        self.next_file += 1;
        let text: Arc<str> = Arc::from(text.into());
        self.files.insert(
            file,
            SourceFile {
                path: path.clone(),
                revision: SourceRevision::of(&text),
                text,
            },
        );
        self.paths.insert(path, file);
        file
    }

    pub fn set_file_text(&mut self, file: FileId, text: impl Into<String>) {
        let text: Arc<str> = Arc::from(text.into());
        if let Some(source) = self.files.get_mut(&file) {
            let revision = SourceRevision::of(&text);
            if source.revision != revision {
                source.text = text;
                source.revision = revision;
                self.cache.remove(&file);
            }
        }
    }

    pub fn file_for_path(&self, path: &str) -> Option<FileId> {
        self.paths.get(path).copied()
    }

    pub fn close_file(&mut self, file: FileId) {
        if let Some(source) = self.files.remove(&file) {
            self.paths.remove(&source.path);
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
        self.ensure_queries(file)?;
        Ok(self.cache[&file].parse.clone())
    }

    pub fn hir_file(&mut self, file: FileId) -> Result<Arc<HirFile>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].hir))
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
        Ok(self
            .type_facts(file)?
            .iter()
            .filter(|fact| fact.range.contains(offset) || fact.range.end() == offset)
            .min_by_key(|fact| fact.range.len())
            .cloned())
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
        let current = self.provider_schema_hashes();
        let incoming = providers
            .schemas()
            .map(|schema| (schema.name.0.clone(), schema.hash()))
            .collect::<BTreeMap<_, _>>();
        self.providers = providers;
        if current != incoming {
            self.cache.clear();
        }
    }

    pub fn completions(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Vec<Completion>, AnalysisError> {
        self.ensure_queries(file)?;
        let cached = &self.cache[&file];
        Ok(editor_queries::completions(
            &cached.parse,
            &cached.type_facts,
            &self.providers,
            offset,
        ))
    }

    pub fn signature_help(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<Signature>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(editor_queries::signature_help(
            &self.cache[&file].parse,
            &self.providers,
            offset,
        ))
    }

    pub fn documentation_at(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<DocumentationFact>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(editor_queries::documentation_at(
            &self.cache[&file].parse,
            &self.providers,
            offset,
        ))
    }

    pub fn describe(
        &self,
        request: DescriptionRequest,
        project: Option<DescriptionProject>,
        limits: DescriptionLimits,
    ) -> DescriptionResponse {
        description::describe(&self.providers, request, project, limits)
    }

    fn ensure_queries(&mut self, file: FileId) -> Result<(), AnalysisError> {
        let source = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?;
        if self
            .cache
            .get(&file)
            .is_some_and(|cached| cached.revision == source.revision)
        {
            return Ok(());
        }
        let text = Arc::clone(&source.text);
        let revision = source.revision;

        let parse = webtest_syntax::parse(&text);
        let diagnostics = syntax_diagnostics::collect(&parse);
        let hir = Arc::new(webtest_hir::lower(file, &parse));
        let compiled = compiler::compile(file, revision, &self.providers, diagnostics, &hir);
        self.cache.insert(
            file,
            CachedQueries {
                revision,
                parse,
                hir,
                diagnostics: Arc::new(compiled.diagnostics),
                type_facts: Arc::new(compiled.type_facts),
                plan: Arc::new(compiled.plan),
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_provider::{
        Capability, OperationName, OperationSchema, ProviderName, ProviderSchema, Type,
    };

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
    fn changed_added_and_removed_provider_schemas_invalidate_all_files() {
        let original = app_registry("echo", Type::String);
        let mut database = AnalysisDatabase::with_provider_registry(original);
        let first = database.open_file("one.webtest", "test \"one\" {}");
        let second = database.open_file("two.webtest", "test \"two\" {}");
        database.diagnostics(first).expect("diagnostics");
        database.diagnostics(second).expect("diagnostics");

        database.set_provider_registry(app_registry("echo", Type::Bool));
        assert!(database.cache.is_empty());
        database.diagnostics(first).expect("diagnostics");
        database.diagnostics(second).expect("diagnostics");

        database.set_provider_registry(ProviderRegistry::built_in_schemas());
        assert!(database.cache.is_empty());
        database.diagnostics(first).expect("diagnostics");
        database.diagnostics(second).expect("diagnostics");

        database.set_provider_registry(app_registry("new_operation", Type::String));
        assert!(database.cache.is_empty());
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
