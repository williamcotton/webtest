use std::sync::Arc;

use webtest_analysis::AnalysisDatabase;
use webtest_app_bridge::{
    AppAdapter, AppHttpConfig, AppManifest, AppProcessConfig, AppProvider, AppProviderConfig,
    AppTransport, ApplicationLifecycle, HealthCheck, HttpOperation,
};
use webtest_project::Project;
use webtest_provider::ProviderRegistry;
use webtest_runtime::RunnerOptions;

use crate::error::AppError;

pub(crate) struct RuntimeProviders {
    pub(crate) registry: ProviderRegistry,
    pub(crate) app: Option<Arc<AppProvider>>,
}

pub(crate) enum RuntimeApplication {
    Provider(Arc<AppProvider>),
    Process(Arc<ApplicationLifecycle>),
}

impl RuntimeApplication {
    pub(crate) async fn start(
        &self,
        project: &Project,
    ) -> Result<(), webtest_provider::ProviderError> {
        match self {
            Self::Provider(provider) => provider.start(&project.root).await,
            Self::Process(process) => process.start(&project.root).await,
        }
    }

    pub(crate) async fn shutdown(&self) -> Result<(), webtest_provider::ProviderError> {
        match self {
            Self::Provider(provider) => provider.shutdown().await,
            Self::Process(process) => process.shutdown().await,
        }
    }

    pub(crate) fn configuration_key(&self) -> &'static str {
        match self {
            Self::Provider(_) => "server.app",
            Self::Process(_) => "app",
        }
    }
}

pub(crate) fn runtime_application(
    project: &Project,
    provider: Option<Arc<AppProvider>>,
) -> Option<RuntimeApplication> {
    provider.map(RuntimeApplication::Provider).or_else(|| {
        app_process_config(project)
            .map(ApplicationLifecycle::new)
            .map(Arc::new)
            .map(RuntimeApplication::Process)
    })
}

pub(crate) fn analysis_database_for_project(
    project: &Project,
) -> Result<AnalysisDatabase, AppError> {
    let mut providers = ProviderRegistry::built_in_schemas();
    if let Some(manifest) = app_manifest(project)? {
        providers.register_schema(manifest.provider_schema());
    }
    Ok(AnalysisDatabase::with_provider_registry(providers))
}

pub(crate) fn runtime_provider_registry(
    project: &Project,
    options: &RunnerOptions,
) -> Result<RuntimeProviders, AppError> {
    let mut registry = ProviderRegistry::built_in(options.provider_config.clone());
    let Some(manifest) = app_manifest(project)? else {
        return Ok(RuntimeProviders {
            registry,
            app: None,
        });
    };
    let config = app_provider_config(project)?;
    let provider = Arc::new(AppProvider::new(manifest, config).map_err(AppError::usage)?);
    registry.register(provider.clone());
    Ok(RuntimeProviders {
        registry,
        app: Some(provider),
    })
}

fn app_manifest(project: &Project) -> Result<Option<AppManifest>, AppError> {
    let Some(app) = &project.config.server.app else {
        return Ok(None);
    };
    AppManifest::read_normalized(&project.root.join(&app.schema))
        .map(Some)
        .map_err(AppError::usage)
}

fn app_provider_config(project: &Project) -> Result<AppProviderConfig, AppError> {
    let app = project
        .config
        .server
        .app
        .as_ref()
        .ok_or_else(|| AppError::internal("app provider configuration was not resolved"))?;
    let adapter = match app.adapter {
        webtest_project::ServerAppAdapter::Bridge => AppAdapter::Bridge,
        webtest_project::ServerAppAdapter::Command => AppAdapter::Command,
        webtest_project::ServerAppAdapter::Http => AppAdapter::Http,
    };
    let transport = match app.transport {
        webtest_project::ServerAppTransport::Auto => AppTransport::Auto,
        webtest_project::ServerAppTransport::Unix => AppTransport::Unix,
        webtest_project::ServerAppTransport::NamedPipe => AppTransport::NamedPipe,
        webtest_project::ServerAppTransport::Tcp => AppTransport::Tcp,
        webtest_project::ServerAppTransport::Stdio => AppTransport::Stdio,
    };
    let application = app_process_config(project);
    let http = AppHttpConfig {
        base_url: app.http_base_url.clone().unwrap_or_default(),
        operations: app
            .http_operations
            .iter()
            .map(|(name, operation)| {
                (
                    name.clone(),
                    HttpOperation {
                        method: operation.method.clone(),
                        path: operation.path.clone(),
                    },
                )
            })
            .collect(),
    };
    Ok(AppProviderConfig {
        adapter,
        transport,
        command: app.command.clone(),
        application,
        http,
        startup_timeout: app.startup_timeout,
        shutdown_timeout: app.shutdown_timeout,
        max_message_bytes: app.max_message_bytes,
        max_stderr_bytes: app.max_stderr_bytes,
        max_pending_calls: app.max_pending_calls,
        ..AppProviderConfig::default()
    })
}

fn app_process_config(project: &Project) -> Option<AppProcessConfig> {
    project
        .config
        .app
        .as_ref()
        .map(|application| AppProcessConfig {
            command: application.command.clone().unwrap_or_default(),
            args: application.args.clone(),
            working_directory: application.working_directory.clone(),
            environment: application.environment.clone(),
            owned: application.owned,
            health: application.health.as_ref().map(|health| HealthCheck {
                url: health.url.clone(),
                timeout: health.timeout,
            }),
        })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;
    use crate::runtime_configuration::runner_options;

    fn write(path: &Path, contents: &str) {
        fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn browser_only_application_uses_the_standalone_lifecycle() {
        let directory = tempfile::tempdir().expect("project");
        write(
            &directory.path().join("webtest.toml"),
            "[app]\ncommand = \"node\"\nworking_directory = \".\"\n",
        );
        let project =
            webtest_project::discover(&[directory.path().to_path_buf()]).expect("discover project");
        let providers = runtime_provider_registry(&project, &runner_options(&project))
            .expect("runtime providers");
        assert!(providers.app.is_none());
        assert!(matches!(
            runtime_application(&project, providers.app),
            Some(RuntimeApplication::Process(_))
        ));
    }

    #[test]
    fn configured_provider_retains_ownership_of_bridge_application_startup() {
        let directory = tempfile::tempdir().expect("project");
        write(
            &directory.path().join("webtest.toml"),
            "[app]\ncommand = \"node\"\nworking_directory = \".\"\n\n[server.app]\nadapter = \"bridge\"\ntransport = \"tcp\"\nschema = \"app-schema.json\"\n",
        );
        write(
            &directory.path().join("app-schema.json"),
            include_str!("../../../protocol/examples/app-schema.json"),
        );
        let project =
            webtest_project::discover(&[directory.path().to_path_buf()]).expect("discover project");
        let providers = runtime_provider_registry(&project, &runner_options(&project))
            .expect("runtime providers");
        assert!(providers.app.is_some());
        assert!(matches!(
            runtime_application(&project, providers.app),
            Some(RuntimeApplication::Provider(_))
        ));
    }
}
