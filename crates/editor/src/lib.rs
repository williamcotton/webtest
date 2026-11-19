//! Protocol-independent editor services used by both LSP and future WASM hosts.

mod lifecycle;
mod workspace_run;
pub use lifecycle::{RunLease, RunLifecycle};
pub use workspace_run::{PreparedWorkspaceRun, WorkspaceRunSummary, WorkspaceTestResult};

use std::sync::{Arc, Mutex, RwLock};

use thiserror::Error;
use webtest_analysis::{
    AnalysisDatabase, AnalysisError, Completion, Diagnostic, DiagnosticSeverity, DiagnosticSource,
    Signature,
};
use webtest_browser::{BrowserError, BrowserHost, Locator};
use webtest_observation::{ObservationStore, RuntimeFailureCode, RuntimeObservationKind};
use webtest_provider::ProviderRegistry;
use webtest_runtime::{RunResult, Runner, RunnerOptions};
use webtest_text::{FileId, TextRange};

pub use webtest_analysis::{SemanticToken, SemanticTokenKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    pub range: TextRange,
    pub contents: String,
}

#[derive(Debug, Error)]
pub enum EditorError {
    #[error(transparent)]
    Analysis(#[from] AnalysisError),
    #[error(transparent)]
    Browser(#[from] BrowserError),
    #[error("the file has static errors and cannot be executed")]
    StaticErrors,
    #[error("workspace revision changed; refresh test discovery before running")]
    StaleDiscovery,
    #[error("selected variants are missing, exceed the query limit, or have static errors")]
    InvalidVariants,
    #[error("editor execution lifecycle failed: {failure}")]
    ExecutionLifecycle {
        failure: webtest_provider::ProviderError,
        result: Option<Box<RunResult>>,
    },
}

pub struct EditorService {
    database: Mutex<AnalysisDatabase>,
    disk: Mutex<DiskInputs>,
    observations: Arc<ObservationStore>,
    runtime: RwLock<RuntimeConfiguration>,
    configuration: Mutex<()>,
}

#[derive(Default)]
struct DiskInputs {
    sources: std::collections::HashMap<String, Arc<str>>,
    open: std::collections::HashSet<FileId>,
    aliases: Vec<String>,
}

#[derive(Clone)]
struct RuntimeConfiguration {
    lifecycle: Option<Arc<dyn RunLifecycle>>,
    runner_options: RunnerOptions,
    providers: ProviderRegistry,
}

impl Default for EditorService {
    fn default() -> Self {
        Self::with_runner_options(RunnerOptions::default())
    }
}

impl EditorService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_runner_options(runner_options: RunnerOptions) -> Self {
        let providers = ProviderRegistry::built_in(runner_options.provider_config.clone());
        Self::with_provider_registry(runner_options, providers)
    }

    pub fn with_provider_registry(
        runner_options: RunnerOptions,
        providers: ProviderRegistry,
    ) -> Self {
        let database = AnalysisDatabase::with_provider_registry(providers.clone());
        Self::with_analysis_database(runner_options, providers, database)
    }
    pub fn with_analysis_database(
        runner_options: RunnerOptions,
        providers: ProviderRegistry,
        mut database: AnalysisDatabase,
    ) -> Self {
        database.set_provider_registry(providers.clone());
        let disk = DiskInputs {
            sources: database.source_inputs().into_iter().collect(),
            open: Default::default(),
            aliases: database
                .source_aliases()
                .into_iter()
                .map(|(alias, _)| alias)
                .collect(),
        };
        Self {
            disk: Mutex::new(disk),
            database: Mutex::new(database),
            observations: Arc::new(ObservationStore::default()),
            runtime: RwLock::new(RuntimeConfiguration {
                lifecycle: None,
                runner_options,
                providers,
            }),
            configuration: Mutex::new(()),
        }
    }

    pub fn provider_registry(&self) -> ProviderRegistry {
        self.read_runtime().providers.clone()
    }
    pub fn set_run_lifecycle(&self, lifecycle: Option<Arc<dyn RunLifecycle>>) {
        let _configuration = self.lock_configuration();
        self.write_runtime().lifecycle = lifecycle;
    }
    pub fn reconfigure(&self, runner_options: RunnerOptions, providers: ProviderRegistry) {
        let _configuration = self.lock_configuration();
        self.write_database()
            .set_provider_registry(providers.clone());
        *self.write_runtime() = RuntimeConfiguration {
            lifecycle: None,
            runner_options,
            providers,
        };
        self.observations.clear();
    }

    pub fn snapshot(&self) -> EditorSnapshot {
        let _configuration = self.lock_configuration();
        EditorSnapshot {
            analysis: self.read_database().snapshot(),
            observations: self.observations.clone(),
            runtime: self.read_runtime().clone(),
            observation_revision: self.observations.generation(),
        }
    }
    pub fn analysis_snapshot(&self) -> webtest_analysis::AnalysisSnapshot {
        let _configuration = self.lock_configuration();
        self.read_database().snapshot()
    }
    pub fn analysis_revision(&self) -> u64 {
        self.read_database().workspace_revision()
    }
    pub fn observations(&self) -> Arc<ObservationStore> {
        Arc::clone(&self.observations)
    }

    pub fn begin_publication(
        &self,
        file: FileId,
        revision: webtest_text::SourceRevision,
        provider_schema_hashes: &std::collections::BTreeMap<String, String>,
    ) -> Option<u64> {
        let _configuration = self.lock_configuration();
        let database = self.read_database();
        if database.source_revision(file).ok()? != revision
            || database.provider_schema_hashes() != *provider_schema_hashes
        {
            return None;
        }
        Some(self.observations.begin_publication(file))
    }

    pub fn complete_publication(
        &self,
        file: FileId,
        ticket: u64,
        batch: webtest_observation::ObservationPublication,
    ) -> bool {
        let _configuration = self.lock_configuration();
        let database = self.read_database();
        let Ok(source) = database.source(file) else {
            return false;
        };
        let revision = batch.revision;
        let execution = batch.execution_id;
        let Some(mut values) = batch.into_observations(file, &source) else {
            return false;
        };
        for observation in &mut values {
            for related in &mut observation.related_locations {
                let Some(file) = database.file_for_path(&related.path) else {
                    return false;
                };
                let Ok(source) = database.source(file) else {
                    return false;
                };
                if webtest_text::SourceRevision::of(&source) != related.revision
                    || !source.is_char_boundary(usize::from(related.origin.range.start()))
                    || !source.is_char_boundary(usize::from(related.origin.range.end()))
                {
                    return false;
                }
                related.origin.file = file;
            }
        }
        self.observations
            .complete_publication(file, revision, ticket, execution, values)
    }

    pub fn open_document(&self, path: impl Into<String>, text: impl Into<String>) -> FileId {
        let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        let file = self.write_database().open_file(path, text);
        disk.open.insert(file);
        file
    }

    pub fn update_document(&self, file: FileId, text: impl Into<String>) {
        let mut database = self.write_database();
        database.set_file_text(file, text);
        self.observations.clear_for_file(file);
    }

    /// Native hosts may resolve only these canonical analyzer-produced candidates.
    pub fn import_candidates(&self, path: &str) -> Result<Vec<String>, EditorError> {
        let database = self.write_database();
        let Some(file) = database.file_for_path(path) else {
            return Ok(Vec::new());
        };
        Ok(database.module_graph(file)?.required_inputs.clone())
    }
    /// Install host-read disk content without overwriting a synchronized buffer.
    pub fn install_disk_module(
        &self,
        canonical: String,
        text: String,
        alias: String,
    ) -> Result<(), EditorError> {
        let mut disk = self.disk.lock().unwrap_or_else(|error| error.into_inner());
        let mut database = self.write_database();
        let existing = database.file_for_path(&canonical);
        let file = if let Some(file) = existing.filter(|file| disk.open.contains(file)) {
            file
        } else {
            database.open_file(canonical.clone(), text.clone())
        };
        disk.sources.insert(canonical, Arc::from(text));
        database.alias_file(&alias, file)?;
        if !disk.aliases.contains(&alias) {
            disk.aliases.push(alias);
        }
        self.observations.clear();
        Ok(())
    }
    pub fn register_path_alias(&self, alias: String, canonical: String) -> Result<(), EditorError> {
        let mut database = self.write_database();
        let file = database
            .file_for_path(&canonical)
            .unwrap_or_else(|| database.open_file(canonical, ""));
        database.alias_file(&alias, file)?;
        Ok(())
    }
    pub fn set_document_uri(&self, file: FileId, uri: String) -> Result<(), EditorError> {
        Ok(self.write_database().set_document_uri(file, Some(uri))?)
    }
    pub fn set_document_version(
        &self,
        file: FileId,
        version: webtest_text::DocumentVersion,
    ) -> Result<(), EditorError> {
        Ok(self.write_database().set_document_version(file, version)?)
    }
    pub fn update_document_versioned(
        &self,
        file: FileId,
        text: String,
        version: webtest_text::DocumentVersion,
    ) -> Result<(), EditorError> {
        let mut database = self.write_database();
        let _ = database.source(file)?;
        database.set_file_text(file, text);
        database.set_document_version(file, version)?;
        self.observations.clear_for_file(file);
        Ok(())
    }
    pub fn close_document(&self, file: FileId) {
        let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        let mut database = self.write_database();
        disk.open.remove(&file);
        if let Ok(path) = database.path(file).map(str::to_owned) {
            if let Some(text) = disk.sources.get(&path) {
                let _ = database.set_document_uri(file, None);
                database.set_file_text(file, text.to_string());
                let _ = database.clear_document_version(file);
            } else {
                database.close_file(file);
            }
        }
        self.observations.clear_for_file(file);
    }

    pub fn reconfigure_workspace_inputs(
        &self,
        runner_options: RunnerOptions,
        providers: ProviderRegistry,
        configuration: webtest_analysis::WorkspaceConfiguration,
        sources: Vec<(String, Arc<str>)>,
        aliases: Vec<(String, String)>,
    ) -> Result<(), EditorError> {
        self.reconfigure_workspace_inputs_with_lifecycle(
            runner_options,
            providers,
            configuration,
            sources,
            aliases,
            None,
        )
    }
    pub fn reconfigure_workspace_inputs_with_lifecycle(
        &self,
        runner_options: RunnerOptions,
        providers: ProviderRegistry,
        configuration: webtest_analysis::WorkspaceConfiguration,
        sources: Vec<(String, Arc<str>)>,
        aliases: Vec<(String, String)>,
        lifecycle: Option<Arc<dyn RunLifecycle>>,
    ) -> Result<(), EditorError> {
        let _configuration = self.lock_configuration();
        self.replace_disk_inputs(sources, aliases)?;
        let mut database = self.write_database();
        database.configure_workspace(configuration);
        database.set_provider_registry(providers.clone());
        *self.write_runtime() = RuntimeConfiguration {
            lifecycle,
            runner_options,
            providers,
        };
        self.observations.clear();
        Ok(())
    }
    pub fn invalidate_project_inputs(&self) {
        let _configuration = self.lock_configuration();
        let mut database = self.write_database();
        let mut configuration = database.workspace_configuration();
        configuration.invalid_inputs = true;
        database.configure_workspace(configuration);
        self.observations.clear();
    }
    pub fn configure_workspace(&self, configuration: webtest_analysis::WorkspaceConfiguration) {
        let _configuration = self.lock_configuration();
        self.write_database().configure_workspace(configuration);
        self.observations.clear();
    }
    pub fn replace_disk_inputs(
        &self,
        sources: Vec<(String, Arc<str>)>,
        aliases: Vec<(String, String)>,
    ) -> Result<(), EditorError> {
        let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        let mut database = self.write_database();
        let sources = sources
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        for old in disk
            .sources
            .keys()
            .filter(|path| !sources.contains_key(*path))
        {
            if let Some(file) = database.file_for_path(old)
                && !disk.open.contains(&file)
            {
                database.close_file(file);
                self.observations.clear_for_file(file);
            }
        }
        for (path, text) in &sources {
            let existing = database.file_for_path(path);
            if existing.is_some_and(|file| disk.open.contains(&file)) {
                continue;
            }
            let changed = existing.is_none_or(|file| {
                database
                    .source(file)
                    .is_ok_and(|previous| previous != *text)
            });
            if changed {
                let file = database.open_file(path, text.to_string());
                self.observations.clear_for_file(file);
            }
        }
        for old in &disk.aliases {
            if !aliases.iter().any(|(alias, _)| alias == old) {
                database.remove_alias(old);
            }
        }
        for (alias, target) in &aliases {
            if let Some(file) = database.file_for_path(target) {
                database.alias_file(alias, file)?;
            }
        }
        disk.aliases = aliases.into_iter().map(|(alias, _)| alias).collect();
        disk.sources = sources;
        Ok(())
    }
    pub fn update_disk_source(&self, path: String, text: Option<String>) {
        let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        let mut database = self.write_database();
        let file = database.file_for_path(&path);
        match text {
            Some(text) => {
                disk.sources.insert(path.clone(), Arc::from(text.as_str()));
                if file.is_none_or(|file| !disk.open.contains(&file)) {
                    let file = database.open_file(path, text);
                    self.observations.clear_for_file(file);
                }
            }
            None => {
                disk.sources.remove(&path);
                if let Some(file) = file
                    && !disk.open.contains(&file)
                {
                    database.close_file(file);
                    self.observations.clear_for_file(file);
                }
            }
        }
    }
    pub fn file_for_path(&self, path: &str) -> Option<FileId> {
        self.read_database().file_for_path(path)
    }

    pub fn source(&self, file: FileId) -> Result<Arc<str>, EditorError> {
        Ok(self.read_database().source(file)?)
    }

    pub fn diagnostics(&self, file: FileId) -> Result<Vec<Diagnostic>, EditorError> {
        self.snapshot().diagnostics(file)
    }

    pub fn format(&self, file: FileId) -> Result<String, EditorError> {
        self.snapshot().format(file)
    }

    pub fn hover(
        &self,
        file: FileId,
        offset: webtest_text::TextSize,
    ) -> Result<Option<Hover>, EditorError> {
        self.snapshot().hover(file, offset)
    }

    pub fn completions(
        &self,
        file: FileId,
        offset: webtest_text::TextSize,
    ) -> Result<Vec<Completion>, EditorError> {
        Ok(self.analysis_snapshot().completions(file, offset)?)
    }

    pub fn signature_help(
        &self,
        file: FileId,
        offset: webtest_text::TextSize,
    ) -> Result<Option<Signature>, EditorError> {
        Ok(self.analysis_snapshot().signature_help(file, offset)?)
    }

    pub fn semantic_tokens(&self, file: FileId) -> Result<Vec<SemanticToken>, EditorError> {
        self.snapshot().semantic_tokens(file)
    }

    /// Capture runtime configuration and all selected plans before starting any host work.
    pub fn prepare_variants(
        &self,
        expected_revision: u64,
        variants: &std::collections::BTreeSet<webtest_model::TestVariantId>,
    ) -> Result<PreparedWorkspaceRun, EditorError> {
        self.snapshot()
            .prepare_variants(expected_revision, variants)
    }

    pub async fn run_file(
        &self,
        file: FileId,
        browser: &dyn BrowserHost,
    ) -> Result<RunResult, EditorError> {
        let (plan, runner_options, providers, lifecycle) = {
            let _configuration = self.lock_configuration();
            let mut database = self.write_database();
            if database
                .diagnostics(file)?
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            {
                return Err(EditorError::StaticErrors);
            }
            let plan = database.test_plan(file)?;
            drop(database);
            let runtime = self.read_runtime();
            (
                plan,
                runtime.runner_options.clone(),
                runtime.providers.clone(),
                runtime.lifecycle.clone(),
            )
        };
        let runner = Runner::new(Arc::clone(&self.observations))
            .with_provider_registry(providers)
            .with_options(runner_options);
        let lease = match lifecycle {
            Some(lifecycle) => Some(lifecycle.begin().await.map_err(|failure| {
                EditorError::ExecutionLifecycle {
                    failure,
                    result: None,
                }
            })?),
            None => None,
        };
        let result = runner.run(&plan, browser).await;
        if let Some(lease) = lease {
            lease
                .finish()
                .await
                .map_err(|failure| EditorError::ExecutionLifecycle {
                    failure,
                    result: Some(Box::new(result.clone())),
                })?;
        }
        Ok(result)
    }

    fn read_database(&self) -> std::sync::MutexGuard<'_, AnalysisDatabase> {
        self.database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_database(&self) -> std::sync::MutexGuard<'_, AnalysisDatabase> {
        self.database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_runtime(&self) -> std::sync::RwLockReadGuard<'_, RuntimeConfiguration> {
        self.runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_runtime(&self) -> std::sync::RwLockWriteGuard<'_, RuntimeConfiguration> {
        self.runtime
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_configuration(&self) -> std::sync::MutexGuard<'_, ()> {
        self.configuration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn locator_description(locator: &Locator) -> String {
    match locator {
        Locator::Id(value) => format!("id {value:?}"),
        Locator::Text(value) => format!("text {value:?}"),
        _ => locator.to_string(),
    }
}

fn locator_reference_query(locator: &Locator) -> &'static str {
    match locator {
        Locator::Id(_) => "locator.id",
        Locator::Role { .. } => "locator.role",
        Locator::Label(_) => "locator.label",
        Locator::Text(_) => "locator.text",
        Locator::Placeholder(_) => "locator.placeholder",
        Locator::TestId(_) => "locator.test_id",
        Locator::Css(_) => "locator.css",
        Locator::XPath(_) => "locator.xpath",
    }
}

fn friendly_runtime_message(
    code: RuntimeFailureCode,
    locator: Option<&Locator>,
    fallback: &str,
) -> String {
    match (code, locator) {
        (RuntimeFailureCode::LocatorNotFound, Some(locator)) => format!(
            "No element with {} was found during the last test run.",
            locator_description(locator)
        ),
        (RuntimeFailureCode::LocatorNotVisible, Some(locator)) => format!(
            "The element with {} was not visible during the last test run.",
            locator_description(locator)
        ),
        _ => fallback.into(),
    }
}

/// A request-scoped immutable analysis view. Drop it before host I/O.
pub struct EditorSnapshot {
    observation_revision: u64,
    runtime: RuntimeConfiguration,
    analysis: webtest_analysis::AnalysisSnapshot,
    observations: Arc<ObservationStore>,
}
impl std::ops::Deref for EditorSnapshot {
    type Target = webtest_analysis::AnalysisSnapshot;
    fn deref(&self) -> &Self::Target {
        &self.analysis
    }
}
impl std::ops::DerefMut for EditorSnapshot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.analysis
    }
}
impl EditorSnapshot {
    pub fn observation_revision(&self) -> u64 {
        self.observation_revision
    }
    pub fn prepare_variants(
        &mut self,
        expected_revision: u64,
        variants: &std::collections::BTreeSet<webtest_model::TestVariantId>,
    ) -> Result<PreparedWorkspaceRun, EditorError> {
        if self.analysis.workspace_revision() != expected_revision {
            return Err(EditorError::StaleDiscovery);
        }
        if variants.is_empty() || variants.len() > 10_000 {
            return Err(EditorError::InvalidVariants);
        }
        let plans = self
            .analysis
            .plans_for_variants(variants)?
            .ok_or(EditorError::InvalidVariants)?;
        Ok(PreparedWorkspaceRun {
            lifecycle: self.runtime.lifecycle.clone(),
            workspace_revision: expected_revision,
            plans,
            runner: Runner::new(self.observations.clone())
                .with_options(self.runtime.runner_options.clone())
                .with_provider_registry(self.runtime.providers.clone()),
        })
    }
    pub fn diagnostics(&mut self, file: FileId) -> Result<Vec<Diagnostic>, EditorError> {
        let (mut diagnostics, revision) = {
            let database = &mut self.analysis;
            (
                database.diagnostics(file)?.as_ref().clone(),
                database.source_revision(file)?,
            )
        };
        diagnostics.extend(
            self.observations
                .observations_for(file, revision)
                .into_iter()
                .map(|observation| {
                    let related_locations=observation.related_locations;
                    match observation.kind {
                    RuntimeObservationKind::TestTimeout {
                        timeout_ms,
                        active_step,
                    } => Diagnostic {
            related_locations:related_locations.clone(),
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: RuntimeFailureCode::TestTimeout.diagnostic_code(),
                        message: format!("Test timed out after {timeout_ms}ms."),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "timeout_ms": timeout_ms,
                            "active_step_id": active_step.map(|step| step.0),
                        })),
                        repair_hints: Vec::new(),
                        reference_queries: Vec::new(),
                    },
                    RuntimeObservationKind::BrowserFailure {
                        code,
                        message,
                        locator,
                        page_url,
                        candidates,
                        actionability,
                        artifacts,
                        elapsed_ms,
                        repair_hints,
                        ..
                    } => Diagnostic {
            related_locations:related_locations.clone(),
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: code.diagnostic_code(),
                        message: friendly_runtime_message(code, locator.as_ref(), &message),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.as_ref().map(ToString::to_string),
                            "page_url": page_url,
                            "candidates": candidates,
                            "actionability": actionability,
                            "artifacts": artifacts,
                            "elapsed_ms": elapsed_ms,
                        })),
                        repair_hints,
                        reference_queries: locator.as_ref().map_or_else(
                            || {
                                code.default_reference_queries()
                                    .iter()
                                    .map(|query| (*query).to_owned())
                                    .collect()
                            },
                            |locator| vec![locator_reference_query(locator).to_owned()],
                        ),
                    },
                    RuntimeObservationKind::ValueFailure {
                        code,
                        message,
                        path,
                        expected,
                        actual,
                        diff: _,
                    } => {
                        let mut details = message;
                        if let Some(path) = &path {
                            details.push_str(&format!(" Path: {path}."));
                        }
                        if let (Some(expected), Some(actual)) = (&expected, &actual) {
                            details.push_str(&format!(" Expected {expected}; got {actual}."));
                        }
                        Diagnostic {
            related_locations:related_locations.clone(),
                            range: observation.range,
                            severity: DiagnosticSeverity::Error,
                            code: code.diagnostic_code(),
                            message: details,
                            source: DiagnosticSource::Runtime,
                            semantic_details: Some(serde_json::json!({
                                "path": path,
                                "expected": expected,
                                "actual": actual,
                            })),
                            repair_hints: Vec::new(),
                            reference_queries: code
                                .default_reference_queries()
                                .iter()
                                .map(|query| (*query).to_owned())
                                .collect(),
                        }
                    }
                    RuntimeObservationKind::LocatorNotFound { locator, .. } => Diagnostic {
            related_locations:related_locations.clone(),
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: RuntimeFailureCode::LocatorNotFound.diagnostic_code(),
                        message: format!(
                            "No element with {} was found during the last test run.",
                            locator_description(&locator)
                        ),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.to_string(),
                        })),
                        repair_hints: Vec::new(),
                        reference_queries: vec![locator_reference_query(&locator).into()],
                    },
                    RuntimeObservationKind::LocatorAmbiguous {
                        locator, matches, ..
                    } => Diagnostic {
            related_locations:related_locations.clone(),
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: RuntimeFailureCode::LocatorAmbiguous.diagnostic_code(),
                        message: format!(
                            "The locator {locator} matched {matches} elements during the last test run."
                        ),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.to_string(),
                            "matches": matches,
                        })),
                        repair_hints: Vec::new(),
                        reference_queries: vec![locator_reference_query(&locator).into()],
                    },
                    RuntimeObservationKind::LocatorNotVisible { locator, .. } => Diagnostic {
            related_locations:related_locations.clone(),
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: RuntimeFailureCode::LocatorNotVisible.diagnostic_code(),
                        message: format!(
                            "The element with {} was not visible during the last test run.",
                            locator_description(&locator)
                        ),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.to_string(),
                            "visible": false,
                        })),
                        repair_hints: Vec::new(),
                        reference_queries: vec![locator_reference_query(&locator).into()],
                    },
                }}),
        );
        Ok(diagnostics)
    }
    pub fn format(&mut self, file: FileId) -> Result<String, EditorError> {
        let parse = self.analysis.parse(file)?;
        Ok(webtest_format::format_file(&parse))
    }
    pub fn hover(
        &mut self,
        file: FileId,
        offset: webtest_text::TextSize,
    ) -> Result<Option<Hover>, EditorError> {
        let database = &mut self.analysis;
        if let Some(documentation) = database.documentation_at(file, offset)? {
            return Ok(Some(Hover {
                range: documentation.range,
                contents: documentation.contents,
            }));
        }
        Ok(database.type_at(file, offset)?.map(|fact| Hover {
            range: fact.range,
            contents: format!("{} ({})", fact.ty, fact.capability),
        }))
    }
    pub fn semantic_tokens(&mut self, file: FileId) -> Result<Vec<SemanticToken>, EditorError> {
        Ok(self.analysis.semantic_tokens(file)?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use webtest_browser::{BrowserSession, Page};
    use webtest_browser_cdp::ChromeHost;
    use webtest_model::{Capability, Type, Value};
    use webtest_observation::{ExecutionId, RuntimeObservation};

    #[test]
    fn external_publication_is_atomic_revision_safe_and_superseded_by_new_runs() {
        use webtest_observation::{ObservationPublication, PublishedObservation};
        let editor = EditorService::new();
        let source = "test \"é\" { expect 1 == 2 }";
        let file = editor.open_document("external.webtest", source);
        let revision = webtest_text::SourceRevision::of(source);
        let batch = || ObservationPublication {
            execution_id: ExecutionId::from_u128(42),
            revision,
            observations: vec![PublishedObservation {
                related_locations: Vec::new(),
                test_declaration_id: None,
                variant_id: None,
                test_id: webtest_model::TestId(0),
                step_id: None,
                start: 0,
                end: source.len() as u32,
                kind: RuntimeObservationKind::TestTimeout {
                    timeout_ms: 1,
                    active_step: None,
                },
            }],
        };
        let schemas = editor.read_database().provider_schema_hashes();
        assert!(
            editor
                .begin_publication(file, revision, &Default::default())
                .is_none()
        );
        let old = editor.begin_publication(file, revision, &schemas).unwrap();
        let current = editor.begin_publication(file, revision, &schemas).unwrap();
        assert!(!editor.complete_publication(file, old, batch()));
        assert!(editor.complete_publication(file, current, batch()));
        assert_eq!(
            editor.observations().observations_for(file, revision).len(),
            1
        );
        let clear = editor.begin_publication(file, revision, &schemas).unwrap();
        let mut success = batch();
        success.observations.clear();
        assert!(editor.complete_publication(file, clear, success));
        assert!(
            editor
                .observations()
                .observations_for(file, revision)
                .is_empty()
        );
        let edited = editor.begin_publication(file, revision, &schemas).unwrap();
        editor.update_document(file, "test \"changed\" {}");
        assert!(!editor.complete_publication(file, edited, batch()));
        assert!(editor.begin_publication(file, revision, &schemas).is_none());
        editor.update_document(file, source);
        assert!(
            !editor.complete_publication(file, edited, batch()),
            "editing back must not resurrect a ticket"
        );
        let native = editor.begin_publication(file, revision, &schemas).unwrap();
        editor
            .observations()
            .begin_execution(file, ExecutionId::from_u128(99));
        assert!(!editor.complete_publication(file, native, batch()));
        let invalid = editor.begin_publication(file, revision, &schemas).unwrap();
        let mut mixed = batch();
        let mut bad = mixed.observations[0].clone();
        bad.start = source.find('é').unwrap() as u32 + 1;
        mixed.observations.push(bad);
        assert!(!editor.complete_publication(file, invalid, mixed));
        assert!(
            editor
                .observations()
                .observations_for(file, revision)
                .is_empty()
        );
        editor.reconfigure(
            RunnerOptions::default(),
            ProviderRegistry::built_in_schemas(),
        );
        assert!(!editor.complete_publication(file, invalid, batch()));
    }
    use webtest_provider::{
        CallContext, OperationName, OperationSchema, ProviderCall, ProviderName, ProviderResult,
        ProviderSchema, ServerProvider,
    };
    use webtest_runtime::RunOutcome;

    use super::*;

    struct FakeHost(bool);
    struct FakeSession(bool);
    struct FakePage(bool);
    struct DisconnectHost;

    struct CountingProvider {
        calls: AtomicUsize,
    }

    #[test]
    fn typed_runtime_codes_remove_generic_editor_fallbacks() {
        let editor = EditorService::new();
        let source = "test \"typed codes\" {}";
        let file = editor.open_document("file:///typed-codes.webtest", source);
        let revision = webtest_text::SourceRevision::of(source);
        for (index, code) in [
            RuntimeFailureCode::IntegerOverflow,
            RuntimeFailureCode::AppProviderFailure,
        ]
        .into_iter()
        .enumerate()
        {
            editor.observations().record(RuntimeObservation {
                related_locations: Vec::new(),
                test_declaration_id: None,
                variant_id: None,
                execution_id: ExecutionId::from_u128((index as u64 + 1) as u128),
                file,
                source_revision: revision,
                test_id: webtest_model::TestId(0),
                step_id: Some(webtest_model::StepId(index as u32)),
                range: TextRange::default(),
                kind: RuntimeObservationKind::ValueFailure {
                    code,
                    message: "failure".into(),
                    path: None,
                    expected: None,
                    actual: None,
                    diff: None,
                },
            });
        }
        let codes = editor
            .diagnostics(file)
            .expect("diagnostics")
            .into_iter()
            .filter(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            ["runtime.integer_overflow", "runtime.app_provider_failure"]
        );
    }

    #[async_trait]
    impl ServerProvider for CountingProvider {
        fn schema(&self) -> ProviderSchema {
            ProviderSchema {
                name: ProviderName("fake".into()),
                operations: [(
                    "call".into(),
                    OperationSchema {
                        name: OperationName("call".into()),
                        parameters: Vec::new(),
                        result: Type::String,
                        capability: Capability::Server,
                        documentation: "Call the adapter test provider.".into(),
                        retry_safe: false,
                    },
                )]
                .into_iter()
                .collect(),
                schema_identity: Some("counting-provider".into()),
            }
        }

        async fn call(
            &self,
            _call: ProviderCall,
            _context: CallContext,
        ) -> Result<ProviderResult, webtest_provider::ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderResult {
                value: Value::String("called".into()),
            })
        }
    }

    fn app_registry(functions: &[&str]) -> ProviderRegistry {
        let mut providers = ProviderRegistry::built_in_schemas();
        providers.register_schema(ProviderSchema {
            name: ProviderName("app".into()),
            operations: functions
                .iter()
                .map(|name| {
                    (
                        (*name).into(),
                        OperationSchema {
                            name: OperationName((*name).into()),
                            parameters: Vec::new(),
                            result: Type::String,
                            capability: Capability::Server,
                            documentation: format!("Call {name}."),
                            retry_safe: false,
                        },
                    )
                })
                .collect(),
            schema_identity: Some(functions.join(",")),
        });
        providers
    }

    #[test]
    fn provider_reconfiguration_invalidates_cached_analysis() {
        let editor = EditorService::with_provider_registry(
            RunnerOptions::default(),
            app_registry(&["existing"]),
        );
        let source = "test \"schema\" { server { let value = app.new_function() } }";
        let file = editor.open_document("file:///schema.webtest", source);
        assert!(
            editor
                .diagnostics(file)
                .expect("initial diagnostics")
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.unknown_provider_operation")
        );

        editor.reconfigure(
            RunnerOptions::default(),
            app_registry(&["existing", "new_function"]),
        );
        assert!(
            editor
                .diagnostics(file)
                .expect("reconfigured diagnostics")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn run_file_keeps_the_composed_registry_with_reversed_runner_builder_order() {
        let provider = Arc::new(CountingProvider {
            calls: AtomicUsize::new(0),
        });
        let mut providers = ProviderRegistry::built_in_schemas();
        providers.register(provider.clone());
        let editor = EditorService::with_provider_registry(RunnerOptions::default(), providers);
        let source = "test \"custom\" { server { let value = fake.call() } }";
        let file = editor.open_document("file:///custom-provider.webtest", source);

        let result = editor
            .run_file(file, &FakeHost(true))
            .await
            .expect("custom provider run");

        assert_eq!(result.passed(), 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[async_trait]
    impl BrowserHost for FakeHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Ok(Box::new(FakeSession(self.0)))
        }
    }

    #[async_trait]
    impl BrowserHost for DisconnectHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Err(BrowserError::BrowserDisconnected)
        }
    }

    #[async_trait]
    impl BrowserSession for FakeSession {
        async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
            Ok(Box::new(FakePage(self.0)))
        }
    }

    #[async_trait]
    impl Page for FakePage {
        async fn open(&mut self, _url: &str) -> Result<(), BrowserError> {
            Ok(())
        }

        async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError> {
            if self.0 {
                Ok(())
            } else {
                Err(BrowserError::LocatorNotFound {
                    locator: locator.clone(),
                })
            }
        }

        async fn expect_visible(&mut self, locator: &Locator) -> Result<(), BrowserError> {
            if self.0 {
                Ok(())
            } else {
                Err(BrowserError::LocatorNotFound {
                    locator: locator.clone(),
                })
            }
        }

        async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError> {
            if self.0 {
                Ok(())
            } else {
                Err(BrowserError::EvaluationFailed {
                    expression: expression.into(),
                    message: "".into(),
                })
            }
        }
    }

    #[tokio::test]
    async fn runtime_diagnostics_are_exact_revision_safe_and_clear_on_success() {
        let editor = EditorService::new();
        let failing_source =
            "test \"x\" { browser { open \"about:blank\" click id(\"missing\") } }";
        let file = editor.open_document("file:///test.webtest", failing_source);
        editor
            .run_file(file, &FakeHost(false))
            .await
            .expect("failed test is a valid run");
        let diagnostics = editor.diagnostics(file).expect("diagnostics");
        let runtime = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
            .expect("runtime diagnostic");
        let start = u32::from(runtime.range.start()) as usize;
        let end = u32::from(runtime.range.end()) as usize;
        assert_eq!(&failing_source[start..end], "id(\"missing\")");

        let passing_source = failing_source.replace("missing", "submit");
        editor.update_document(file, &passing_source);
        assert!(
            editor
                .diagnostics(file)
                .expect("new diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );

        editor
            .run_file(file, &FakeHost(true))
            .await
            .expect("passing run");
        assert!(
            editor
                .diagnostics(file)
                .expect("passing diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );
    }

    #[test]
    fn per_test_timeout_observation_is_revision_bound_and_not_an_assertion_mismatch() {
        let editor = EditorService::new();
        let source = "test \"slow\" {}";
        let file = editor.open_document("file:///timeout.webtest", source);
        editor
            .observations()
            .record(webtest_observation::RuntimeObservation {
                related_locations: Vec::new(),
                test_declaration_id: None,
                variant_id: None,
                execution_id: webtest_observation::ExecutionId::from_u128(1),
                file,
                source_revision: webtest_text::SourceRevision::of(source),
                test_id: webtest_model::TestId(0),
                step_id: None,
                range: webtest_text::TextRange::new(
                    webtest_text::TextSize::new(0),
                    webtest_text::TextSize::new(source.len() as u32),
                ),
                kind: RuntimeObservationKind::TestTimeout {
                    timeout_ms: 250,
                    active_step: None,
                },
            });

        let diagnostic = editor
            .diagnostics(file)
            .expect("diagnostics")
            .into_iter()
            .find(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
            .expect("timeout diagnostic");
        assert_eq!(diagnostic.code, "runtime.test_timeout");
        assert_eq!(diagnostic.message, "Test timed out after 250ms.");
        assert_eq!(
            diagnostic.range.len(),
            webtest_text::TextSize::new(source.len() as u32)
        );

        editor.update_document(file, format!("{source}\n"));
        assert!(
            editor
                .diagnostics(file)
                .expect("new revision diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );
    }

    #[tokio::test]
    async fn infrastructure_rerun_clears_previous_runtime_diagnostic() {
        let editor = EditorService::new();
        let source = "test \"x\" { browser { click id(\"missing\") } }";
        let file = editor.open_document("file:///disconnect.webtest", source);
        editor
            .run_file(file, &FakeHost(false))
            .await
            .expect("failed assertion run");
        assert!(
            editor
                .diagnostics(file)
                .expect("failure diagnostics")
                .iter()
                .any(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
        );

        let result = editor
            .run_file(file, &DisconnectHost)
            .await
            .expect("forced disconnect is an aborted runtime result");
        assert!(matches!(
            result.outcome,
            RunOutcome::Aborted {
                failure: webtest_runtime::RunError::Browser(BrowserError::BrowserDisconnected),
                ..
            }
        ));
        assert!(
            editor
                .diagnostics(file)
                .expect("disconnect diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );

        editor
            .run_file(file, &FakeHost(true))
            .await
            .expect("successful rerun");
        assert!(
            editor
                .diagnostics(file)
                .expect("successful diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );
    }

    #[tokio::test]
    async fn missing_visible_text_produces_a_precise_runtime_diagnostic() {
        let editor = EditorService::new();
        let source = "test \"x\" { browser { expect text(\"submitted\").visible } }";
        let file = editor.open_document("file:///expect.webtest", source);
        editor
            .run_file(file, &FakeHost(false))
            .await
            .expect("failed expectation is a valid run");

        let runtime = editor
            .diagnostics(file)
            .expect("diagnostics")
            .into_iter()
            .find(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
            .expect("runtime diagnostic");
        let start = u32::from(runtime.range.start()) as usize;
        let end = u32::from(runtime.range.end()) as usize;
        assert_eq!(&source[start..end], "text(\"submitted\")");
        assert_eq!(runtime.code, "runtime.locator_not_found");
        assert_eq!(
            runtime.message,
            "No element with text \"submitted\" was found during the last test run."
        );
    }

    #[test]
    fn modeling_highlighting_uses_shared_cst_and_resolved_names() {
        let editor = EditorService::new();
        let source = include_str!("../../../examples/test-modeling/modeling.webtest");
        let file = editor.open_document("file:///modeling.webtest", source);
        assert!(editor.diagnostics(file).unwrap().is_empty());
        let tokens = editor.semantic_tokens(file).unwrap();
        let rendered: Vec<_> = tokens
            .iter()
            .map(|token| {
                (
                    &source[usize::from(token.range.start())..usize::from(token.range.end())],
                    token.kind,
                )
            })
            .collect();
        for word in [
            "pattern", "subset", "absent", "cases", "case", "as", "check", "matches", "assume",
            "false", "else", "skip",
        ] {
            assert!(
                rendered.contains(&(word, SemanticTokenKind::Keyword)),
                "missing keyword {word}"
            );
        }
        for word in ["regex", "each", "range", "label"] {
            assert!(
                rendered.contains(&(word, SemanticTokenKind::Function)),
                "missing function {word}"
            );
        }
        for word in ["Named", "String"] {
            assert!(
                rendered.contains(&(word, SemanticTokenKind::Type)),
                "missing type {word}"
            );
        }
        assert!(rendered.contains(&("expected", SemanticTokenKind::Parameter)));
        for word in ["name", "password"] {
            assert!(
                rendered.contains(&(word, SemanticTokenKind::Property)),
                "missing property {word}"
            );
        }

        let file = editor.open_document(
            "file:///context.webtest",
            "test \"context\" { let absent = 1 let regex = 2 expect absent == regex }",
        );
        let tokens = editor.semantic_tokens(file).unwrap();
        let source = "test \"context\" { let absent = 1 let regex = 2 expect absent == regex }";
        for token in tokens {
            let text = &source[usize::from(token.range.start())..usize::from(token.range.end())];
            if matches!(text, "absent" | "regex") {
                assert_eq!(token.kind, SemanticTokenKind::Variable);
            }
        }
    }

    #[test]
    fn semantic_tokens_are_views_over_cst_tokens() {
        let editor = EditorService::new();
        let source = "test \"x\" { // note\n parallel { server { let parallel = 7 expect parallel == 7 } } browser { open \"about:blank\" click id(\"submit\") expect text(\"submitted\").visible } }";
        let file = editor.open_document("file:///tokens.webtest", source);
        let tokens = editor.semantic_tokens(file).expect("semantic tokens");
        let rendered: Vec<_> = tokens
            .iter()
            .map(|token| {
                let start = u32::from(token.range.start()) as usize;
                let end = u32::from(token.range.end()) as usize;
                (&source[start..end], token.kind)
            })
            .collect();
        assert!(rendered.contains(&("test", SemanticTokenKind::Keyword)));
        assert!(rendered.contains(&("parallel", SemanticTokenKind::Keyword)));
        assert!(rendered.contains(&("parallel", SemanticTokenKind::Variable)));
        assert!(rendered.contains(&("// note", SemanticTokenKind::Comment)));
        assert!(rendered.contains(&("id", SemanticTokenKind::Function)));
        assert!(rendered.contains(&("expect", SemanticTokenKind::Keyword)));
        assert!(rendered.contains(&("text", SemanticTokenKind::Function)));
        assert!(rendered.contains(&("visible", SemanticTokenKind::Keyword)));
        assert!(rendered.contains(&("\"submit\"", SemanticTokenKind::String)));
    }

    #[test]
    fn race_keywords_contextual_names_and_winner_type_use_shared_semantic_facts() {
        let editor = EditorService::new();
        let source = "test \"x\" { let race = 1 let provide = 2 let selected = race { server { provide race } server { provide provide } } expect selected > 0 }";
        let file = editor.open_document("file:///race.webtest", source);
        assert!(editor.diagnostics(file).unwrap().is_empty());
        let tokens = editor.semantic_tokens(file).unwrap();
        for name in ["race", "provide"] {
            for kind in [SemanticTokenKind::Keyword, SemanticTokenKind::Variable] {
                assert!(tokens.iter().any(|token| {
                    let start = u32::from(token.range.start()) as usize;
                    let end = u32::from(token.range.end()) as usize;
                    &source[start..end] == name && token.kind == kind
                }));
            }
        }
        let hover = editor
            .hover(
                file,
                webtest_text::TextSize::from(source.rfind("selected").unwrap() as u32 + 1),
            )
            .unwrap()
            .unwrap();
        assert!(hover.contents.contains("Int"));
    }

    #[test]
    fn retry_settings_and_contextual_names_use_cst_tokens_and_shared_type_facts() {
        let editor = EditorService::new();
        let source = "test \"é\" { let retry = 7 let backoff = 1 let max = 2 retry 3 backoff 20ms max 1s { expect retry > max } }";
        let file = editor.open_document("file:///retry.webtest", source);
        assert!(editor.diagnostics(file).unwrap().is_empty());
        let tokens = editor.semantic_tokens(file).unwrap();
        for name in ["retry", "backoff", "max"] {
            for kind in [SemanticTokenKind::Keyword, SemanticTokenKind::Variable] {
                assert!(tokens.iter().any(|token| &source
                    [usize::from(token.range.start())..usize::from(token.range.end())]
                    == name
                    && token.kind == kind));
            }
        }
        for (text, ty) in [("3 backoff", "Int"), ("20ms", "Duration")] {
            let hover = editor
                .hover(
                    file,
                    webtest_text::TextSize::from(source.find(text).unwrap() as u32),
                )
                .unwrap()
                .unwrap();
            assert!(hover.contents.contains(ty), "{}", hover.contents);
        }
    }

    #[test]
    fn hover_uses_shared_static_type_facts() {
        let editor = EditorService::new();
        let source = "test \"x\" { server { let response = http.get(\"/user\") expect response.status == 200 } }";
        let file = editor.open_document("file:///hover.webtest", source);
        let offset = source.find("status").expect("status") + 1;
        let hover = editor
            .hover(file, webtest_text::TextSize::from(offset as u32))
            .expect("hover")
            .expect("type fact");
        assert!(hover.contents.contains("StatusCode"), "{}", hover.contents);
    }

    #[tokio::test]
    async fn typed_json_decode_failure_is_published_on_the_constrained_expression() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let (mut stream, _) = listener.accept().await.expect("fixture request");
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await;
            let body = r#"{"id":"wrong","email":"alice@example.test"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("fixture response");
        });

        let source = format!(
            r#"test "decode" {{
    server {{
        let response = http.get("http://{address}/user")
        let user: {{ id: Int, email: String }} = response.json
    }}
}}
"#,
        );
        let editor = EditorService::new();
        let file = editor.open_document("file:///decode.webtest", &source);
        let run = editor
            .run_file(file, &FakeHost(true))
            .await
            .expect("decode failure is a completed test run");
        assert_eq!(run.failed(), 1);
        let diagnostic = editor
            .diagnostics(file)
            .expect("diagnostics")
            .into_iter()
            .find(|diagnostic| diagnostic.code == "runtime.json_decode_failed")
            .expect("runtime decode diagnostic");
        let start = u32::from(diagnostic.range.start()) as usize;
        let end = u32::from(diagnostic.range.end()) as usize;
        assert_eq!(&source[start..end], "response.json");
        assert!(
            diagnostic.message.contains("$.id"),
            "{}",
            diagnostic.message
        );
        assert!(diagnostic.message.contains("Int"), "{}", diagnostic.message);
    }

    #[tokio::test]
    async fn real_browser_vertical_slice_when_chrome_is_available() {
        let browser = ChromeHost::default();
        if browser.locate().is_none() {
            return;
        }
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let address = listener.local_addr().expect("fixture address");
        // Chrome may preconnect without sending a request. Keep each accepted
        // connection independent so that idle sockets cannot block navigation.
        let mut connections = tokio::task::JoinSet::new();
        let serve = async {
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.expect("accept fixture request");
                        connections.spawn(async move {
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};

                            let mut request = [0u8; 2048];
                            let _ = stream.read(&mut request).await;
                            let body = "<!doctype html><html><body><button id=\"submit\" onclick=\"const result=document.createElement('div');result.textContent='submitted';document.body.append(result)\">Submit</button></body></html>";
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => {
                        result.expect("fixture connection task");
                    }
                }
            }
        };
        let exercise = async {
            // Deterministically exercise the preconnection case, keeping the
            // idle socket open throughout both browser runs.
            let _idle = tokio::net::TcpStream::connect(address)
                .await
                .expect("idle connection");
            let editor = EditorService::new();
            let failing = format!(
                "test \"missing\" {{ browser {{ open \"http://{address}\" click id(\"missing\") expect text(\"submitted\").visible }} }}"
            );
            let file = editor.open_document("file:///vertical.webtest", &failing);
            let failed = editor
                .run_file(file, &browser)
                .await
                .expect("run failing test");
            assert_eq!(failed.failed(), 1, "{failed:?}");
            assert!(
                editor
                    .diagnostics(file)
                    .expect("runtime diagnostics")
                    .iter()
                    .any(|diagnostic| diagnostic.code == "runtime.locator_not_found")
            );

            let passing = failing.replace("id(\"missing\")", "id(\"submit\")");
            editor.update_document(file, passing);
            let passed = editor
                .run_file(file, &browser)
                .await
                .expect("run passing test");
            assert_eq!(passed.passed(), 1, "{passed:?}");
            assert!(
                editor
                    .diagnostics(file)
                    .expect("passing diagnostics")
                    .iter()
                    .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
            );
        };
        tokio::select! {
            () = serve => unreachable!("fixture ended before its test"),
            () = exercise => {},
        }
        connections.shutdown().await;
    }
}
