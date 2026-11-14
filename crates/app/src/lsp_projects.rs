use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use webtest_app_bridge::AppProvider;
use webtest_editor::EditorService;
use webtest_project::Project;
use webtest_provider::ProviderRegistry;
use webtest_runtime::RunnerOptions;

use crate::{
    error::AppError,
    project_context::project,
    provider_composition::{RuntimeApplication, runtime_application, runtime_provider_registry},
    runtime_configuration::runner_options,
};

#[derive(Clone)]
struct LspProjectEditor {
    run_gate: Arc<tokio::sync::Mutex<()>>,
    editor: Arc<EditorService>,
    app_provider: Option<Arc<AppProvider>>,
    schema_path: Option<PathBuf>,
    runner_options: RunnerOptions,
}

#[derive(Default)]
pub(crate) struct LspProjectEditors {
    projects: Mutex<HashMap<PathBuf, LspProjectEditor>>,
}

impl LspProjectEditors {
    pub(crate) fn editor_for_path(&self, path: &Path) -> Result<Arc<EditorService>, AppError> {
        let input = if path.exists() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let editor = self.editor_for_project(&project(&[input])?)?;
        let canonical = canonical_event_path(path);
        editor
            .register_path_alias(path.display().to_string(), canonical.display().to_string())
            .map_err(AppError::internal)?;
        Ok(editor)
    }

    pub(crate) fn editor_for_project(
        &self,
        project: &Project,
    ) -> Result<Arc<EditorService>, AppError> {
        if let Some(project) = self.lock().get(&project.root) {
            return Ok(Arc::clone(&project.editor));
        }

        let options = runner_options(project);
        let mut invalid_inputs = false;
        let runtime_providers = match runtime_provider_registry(project, &options) {
            Ok(providers) => providers,
            Err(error) => {
                invalid_inputs = true;
                tracing::error!(
                    project_root = %project.root.display(),
                    %error,
                    "could not load WebTest project provider; continuing with built-in providers"
                );
                crate::provider_composition::RuntimeProviders {
                    registry: ProviderRegistry::built_in(options.provider_config.clone()),
                    app: None,
                }
            }
        };
        let mut database = crate::project_analysis::workspace_with_providers(
            project,
            runtime_providers.registry.clone(),
        )?;
        if invalid_inputs {
            let mut configuration = database.workspace_configuration();
            configuration.invalid_inputs = true;
            database.configure_workspace(configuration);
        }
        let candidate = LspProjectEditor {
            run_gate: Arc::new(tokio::sync::Mutex::new(())),
            editor: Arc::new(EditorService::with_analysis_database(
                options.clone(),
                runtime_providers.registry,
                database,
            )),
            app_provider: runtime_providers.app,
            schema_path: configured_app_schema_path(project),
            runner_options: options,
        };
        candidate.editor.set_run_lifecycle(editor_lifecycle(
            project,
            candidate.app_provider.clone(),
            candidate.run_gate.clone(),
        ));
        let mut projects = self.lock();
        let project = projects.entry(project.root.clone()).or_insert(candidate);
        Ok(Arc::clone(&project.editor))
    }

    pub(crate) fn resolve_imports(&self, path: &Path) -> Result<bool, AppError> {
        let selected = self
            .lock()
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(root, cached)| (root.clone(), cached.editor.clone()));
        let Some((root, editor)) = selected else {
            return Ok(false);
        };
        let path = path.display().to_string();
        if editor
            .import_candidates(&path)
            .map_err(AppError::internal)?
            .is_empty()
        {
            return Ok(false);
        }
        let project = project(std::slice::from_ref(&root))?;
        let roots = webtest_project::workspace_source_roots(&project).map_err(AppError::usage)?;
        let mut attempted = std::collections::BTreeSet::new();
        let mut changed = false;
        loop {
            let mut installed = false;
            for candidate in editor
                .import_candidates(&path)
                .map_err(AppError::internal)?
            {
                if !attempted.insert(candidate.clone()) {
                    continue;
                }
                let canonical = match std::fs::canonicalize(&candidate) {
                    Ok(path) => path,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(AppError::usage(error)),
                };
                if !roots.iter().any(|root| canonical.starts_with(root)) {
                    return Err(AppError::usage(
                        "import resolves outside configured source roots",
                    ));
                }
                let source = crate::project_context::read_source(&canonical)?;
                editor
                    .install_disk_module(canonical.display().to_string(), source, candidate)
                    .map_err(AppError::internal)?;
                installed = true;
                changed = true;
            }
            if !installed {
                return Ok(changed);
            }
        }
    }
    pub(crate) fn reload_for_changed_path(&self, path: &Path) -> Result<bool, AppError> {
        let path = canonical_event_path(path);
        if path
            .extension()
            .is_some_and(|extension| extension == "webtest")
        {
            let editors = self
                .lock()
                .iter()
                .filter(|(root, _)| path.starts_with(root))
                .map(|(root, project)| (root.clone(), project.editor.clone()))
                .collect::<Vec<_>>();
            for (root, editor) in &editors {
                let project = project(std::slice::from_ref(root))?;
                let inputs = crate::project_analysis::workspace_with_providers(
                    &project,
                    editor.provider_registry(),
                )?;
                editor
                    .replace_disk_inputs(inputs.source_inputs(), inputs.source_aliases())
                    .map_err(AppError::internal)?;
            }
            return Ok(!editors.is_empty());
        }
        let roots = self
            .lock()
            .iter()
            .filter(|(root, project)| {
                root.join("webtest.toml") == path
                    || project.schema_path.as_deref() == Some(path.as_path())
            })
            .map(|(root, _)| root.clone())
            .collect::<Vec<_>>();
        if roots.is_empty() {
            return Ok(false);
        }

        for root in roots {
            let refreshed = (|| {
                let project = project(std::slice::from_ref(&root))?;
                let options = runner_options(&project);
                let providers = runtime_provider_registry(&project, &options)?;
                let inputs = crate::project_analysis::workspace_with_providers(
                    &project,
                    providers.registry.clone(),
                )?;
                Ok::<_, AppError>((project, options, providers, inputs))
            })();
            let (project, options, providers, inputs) = match refreshed {
                Ok(refreshed) => refreshed,
                Err(error) => {
                    self.invalidate_cached_project(&root);
                    return Err(error);
                }
            };
            let previous_provider = {
                let mut projects = self.lock();
                let cached = projects.get_mut(&root).ok_or_else(|| {
                    AppError::internal(format!(
                        "cached LSP project `{}` disappeared during reload",
                        root.display()
                    ))
                })?;
                cached
                    .editor
                    .reconfigure_workspace_inputs_with_lifecycle(
                        options.clone(),
                        providers.registry,
                        inputs.workspace_configuration(),
                        inputs.source_inputs(),
                        inputs.source_aliases(),
                        editor_lifecycle(&project, providers.app.clone(), cached.run_gate.clone()),
                    )
                    .map_err(AppError::internal)?;
                cached.schema_path = configured_app_schema_path(&project);
                cached.runner_options = options;
                std::mem::replace(&mut cached.app_provider, providers.app)
            };
            shutdown_replaced_provider(previous_provider);
        }
        Ok(true)
    }

    fn invalidate_cached_project(&self, root: &Path) {
        let previous_provider = {
            let mut projects = self.lock();
            let Some(cached) = projects.get_mut(root) else {
                return;
            };
            let providers =
                ProviderRegistry::built_in(cached.runner_options.provider_config.clone());
            cached
                .editor
                .reconfigure(cached.runner_options.clone(), providers);
            cached.editor.invalidate_project_inputs();
            cached.editor.set_run_lifecycle(None);
            cached.app_provider.take()
        };
        shutdown_replaced_provider(previous_provider);
    }

    pub(crate) async fn shutdown(&self) -> Result<(), AppError> {
        let providers = self
            .lock()
            .values()
            .filter_map(|project| project.app_provider.clone())
            .collect::<Vec<_>>();
        let mut first_error = None;
        for provider in providers {
            if let Err(error) = provider.shutdown().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(AppError::infrastructure(error)),
            None => Ok(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, LspProjectEditor>> {
        self.projects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn configured_app_schema_path(project: &Project) -> Option<PathBuf> {
    project
        .config
        .server
        .app
        .as_ref()
        .map(|app| project.root.join(&app.schema))
}

fn canonical_event_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        path.parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .and_then(|parent| path.file_name().map(|name| parent.join(name)))
            .unwrap_or_else(|| path.to_path_buf())
    })
}

struct EditorApplicationLifecycle {
    project: Project,
    application: RuntimeApplication,
    gate: Arc<tokio::sync::Mutex<()>>,
}
struct EditorApplicationLease {
    application: RuntimeApplication,
    _permit: tokio::sync::OwnedMutexGuard<()>,
}
#[async_trait::async_trait]
impl webtest_editor::RunLifecycle for EditorApplicationLifecycle {
    async fn begin(
        &self,
    ) -> Result<Box<dyn webtest_editor::RunLease>, webtest_provider::ProviderError> {
        let permit = self.gate.clone().lock_owned().await;
        if let Err(primary) = self.application.start(&self.project).await {
            if let Err(cleanup) = self.application.shutdown().await {
                tracing::warn!(%cleanup, "editor application startup cleanup failed");
                return Err(match self.application {
                    RuntimeApplication::Provider(_) => {
                        webtest_provider::ProviderError::BridgeCleanup {
                            primary: Box::new(primary),
                        }
                    }
                    RuntimeApplication::Process(_) => {
                        webtest_provider::ProviderError::ProcessCleanup {
                            primary: Some(Box::new(primary)),
                        }
                    }
                });
            }
            return Err(primary);
        }
        Ok(Box::new(EditorApplicationLease {
            application: self.application.clone(),
            _permit: permit,
        }))
    }
}
#[async_trait::async_trait]
impl webtest_editor::RunLease for EditorApplicationLease {
    async fn finish(self: Box<Self>) -> Result<(), webtest_provider::ProviderError> {
        self.application.shutdown().await
    }
}
fn editor_lifecycle(
    project: &Project,
    provider: Option<Arc<AppProvider>>,
    gate: Arc<tokio::sync::Mutex<()>>,
) -> Option<Arc<dyn webtest_editor::RunLifecycle>> {
    runtime_application(project, provider).map(|application| {
        Arc::new(EditorApplicationLifecycle {
            project: project.clone(),
            application,
            gate,
        }) as Arc<dyn webtest_editor::RunLifecycle>
    })
}

fn shutdown_replaced_provider(provider: Option<Arc<AppProvider>>) {
    if let Some(provider) = provider {
        tokio::spawn(async move {
            if let Err(error) = provider.shutdown().await {
                tracing::warn!(%error, "could not shut down replaced LSP application provider");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoBrowser;
    #[async_trait::async_trait]
    impl webtest_browser::BrowserHost for NoBrowser {
        async fn start(
            &self,
        ) -> Result<Box<dyn webtest_browser::BrowserSession>, webtest_browser::BrowserError>
        {
            panic!("pure lifecycle test must not start Chrome");
        }
    }

    #[tokio::test]
    async fn editor_runs_join_application_shutdown_before_reusing_the_port() {
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reservation.local_addr().unwrap();
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("app.py"),
            r#"import http.server, os, sys
server = http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), http.server.BaseHTTPRequestHandler)
with open('starts', 'a') as output: output.write(str(os.getpid()) + '\n')
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b'ok')
    def log_message(self, *args): pass
server.RequestHandlerClass = Handler
server.serve_forever()
"#,
        )
        .unwrap();
        std::fs::write(directory.path().join("webtest.toml"),format!("[project]\ntest_roots=[\"test.webtest\"]\n[app]\ncommand=\"python3\"\nargs=[\"app.py\",\"{}\"]\nworking_directory=\".\"\n[app.health]\nurl=\"http://{address}/health\"\ntimeout=\"5s\"\n",address.port())).unwrap();
        let path = directory.path().join("test.webtest");
        std::fs::write(&path, "test \"lifecycle\" {expect true}").unwrap();
        drop(reservation);
        let editors = LspProjectEditors::default();
        let editor = editors.editor_for_path(&path).unwrap();
        let canonical = std::fs::canonicalize(&path).unwrap();
        let file = editor.open_document(
            canonical.display().to_string(),
            "test \"lifecycle\" {expect true}",
        );
        assert_eq!(editor.run_file(file, &NoBrowser).await.unwrap().passed(), 1);
        drop(
            std::net::TcpListener::bind(address)
                .expect("successful file run releases its app port"),
        );
        editor.update_document(file, "test \"lifecycle\" {expect false}");
        assert_eq!(editor.run_file(file, &NoBrowser).await.unwrap().failed(), 1);
        drop(std::net::TcpListener::bind(address).expect("failed file run releases its app port"));
        let discovery = editor.snapshot().discover_tests(100).unwrap();
        let selected = discovery
            .modules
            .iter()
            .flat_map(|m| &m.tests)
            .flat_map(|t| &t.variants)
            .map(|v| v.id)
            .collect();
        let first = editor
            .prepare_variants(discovery.workspace_revision, &selected)
            .unwrap();
        let second = editor
            .prepare_variants(discovery.workspace_revision, &selected)
            .unwrap();
        let (first, second) = tokio::join!(
            first.execute(&NoBrowser, None),
            second.execute(&NoBrowser, None)
        );
        for run in [first, second] {
            assert!(
                run.failures.is_empty(),
                "lifecycle failures: {:?}",
                run.failures
            );
            assert_eq!(run.tests.len(), 1);
        }
        drop(
            std::net::TcpListener::bind(address)
                .expect("selected variant runs release their app port"),
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("starts"))
                .unwrap()
                .lines()
                .count(),
            4
        );
        editors.shutdown().await.unwrap();
    }

    #[test]
    fn canonicalizes_existing_parent_for_half_written_inputs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("missing-schema.json");
        assert_eq!(
            canonical_event_path(&path),
            std::fs::canonicalize(directory.path())
                .expect("canonical directory")
                .join("missing-schema.json")
        );
    }

    #[test]
    fn poisoned_cache_lock_remains_usable() {
        let editors = Arc::new(LspProjectEditors::default());
        let poisoned = Arc::clone(&editors);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.projects.lock().expect("lock");
            panic!("poison the lock");
        })
        .join();
        assert!(editors.lock().is_empty());
    }

    #[tokio::test]
    async fn reload_reuses_the_editor_and_invalid_manifest_evicts_stale_schema() {
        let directory = tempfile::tempdir().expect("temporary project");
        crate::init::initialize(directory.path()).expect("initialize project");
        let project = project(&[directory.path().to_path_buf()]).expect("discover project");
        let editors = LspProjectEditors::default();
        let editor = editors
            .editor_for_project(&project)
            .expect("project editor");
        let file = editor.open_document(
            project
                .root
                .join("tests/example.webtest")
                .display()
                .to_string(),
            crate::init::EXAMPLE_TEST,
        );
        assert!(editor.diagnostics(file).expect("diagnostics").is_empty());

        assert!(
            editors
                .reload_for_changed_path(&project.root.join("webtest.toml"))
                .expect("valid reload")
        );
        let reloaded = editors
            .editor_for_project(&project)
            .expect("cached project editor");
        assert!(Arc::ptr_eq(&editor, &reloaded));

        let schema_path = project.root.join(".webtest/app-schema.json");
        std::fs::write(&schema_path, "{").expect("invalid manifest");
        assert!(editors.reload_for_changed_path(&schema_path).is_err());
        let diagnostics = editor
            .diagnostics(file)
            .expect("diagnostics after eviction");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("app")),
            "{diagnostics:#?}"
        );
        editors.shutdown().await.expect("provider shutdown");
    }
    #[cfg(unix)]
    #[test]
    fn synchronized_imports_resolve_directory_aliases_without_overwriting_disk_baselines() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("tests")).unwrap();
        std::fs::create_dir(directory.path().join("support")).unwrap();
        std::fs::write(
            directory.path().join("webtest.toml"),
            "[project]\ntest_roots = ['tests']\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("support/helpers.webtest"),
            "export fn normalize(value: String) -> String = lower(value)",
        )
        .unwrap();
        let disk = "test \"disk\" {}";
        let path = directory.path().join("tests/run.webtest");
        std::fs::write(&path, disk).unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("support"),
            directory.path().join("alias"),
        )
        .unwrap();
        let project = project(std::slice::from_ref(&path)).unwrap();
        let editors = LspProjectEditors::default();
        let editor = editors.editor_for_project(&project).unwrap();
        let path = std::fs::canonicalize(path).unwrap();
        let source = "import { normalize } from \"../alias/helpers\" test \"unsaved\" { let value = normalize(\"É\") expect value == \"é\" }";
        let file = editor.open_document(path.display().to_string(), source);
        assert!(
            editor
                .diagnostics(file)
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.missing_module")
        );
        assert!(editors.resolve_imports(&path).unwrap());
        assert!(
            editor.diagnostics(file).unwrap().is_empty(),
            "{:?}",
            editor.diagnostics(file)
        );
        assert!(!editors.resolve_imports(&path).unwrap());
        editor.close_document(file);
        assert_eq!(editor.source(file).unwrap().as_ref(), disk);
    }
}
