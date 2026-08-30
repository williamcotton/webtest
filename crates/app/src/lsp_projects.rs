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
    error::AppError, project_context::project, provider_composition::runtime_provider_registry,
    runtime_configuration::runner_options,
};

#[derive(Clone)]
struct LspProjectEditor {
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
        self.editor_for_project(&project(&[input])?)
    }

    pub(crate) fn editor_for_project(
        &self,
        project: &Project,
    ) -> Result<Arc<EditorService>, AppError> {
        if let Some(project) = self.lock().get(&project.root) {
            return Ok(Arc::clone(&project.editor));
        }

        let options = runner_options(project);
        let runtime_providers = match runtime_provider_registry(project, &options) {
            Ok(providers) => providers,
            Err(error) => {
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
        let candidate = LspProjectEditor {
            editor: Arc::new(EditorService::with_provider_registry(
                options.clone(),
                runtime_providers.registry,
            )),
            app_provider: runtime_providers.app,
            schema_path: configured_app_schema_path(project),
            runner_options: options,
        };
        let mut projects = self.lock();
        let project = projects.entry(project.root.clone()).or_insert(candidate);
        Ok(Arc::clone(&project.editor))
    }

    pub(crate) fn reload_for_changed_path(&self, path: &Path) -> Result<bool, AppError> {
        let path = canonical_event_path(path);
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
                Ok::<_, AppError>((project, options, providers))
            })();
            let (project, options, providers) = match refreshed {
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
                    .reconfigure(options.clone(), providers.registry);
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
}
