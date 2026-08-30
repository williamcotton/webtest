use std::sync::Arc;

use webtest_analysis::AnalysisDatabase;
use webtest_app_bridge::{
    AppAdapter, AppHttpConfig, AppManifest, AppProcessConfig, AppProvider, AppProviderConfig,
    AppTransport, HealthCheck, HttpOperation,
};
use webtest_project::Project;
use webtest_provider::ProviderRegistry;
use webtest_runtime::RunnerOptions;

use crate::error::AppError;

pub(crate) struct RuntimeProviders {
    pub(crate) registry: ProviderRegistry,
    pub(crate) app: Option<Arc<AppProvider>>,
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
    let application = project
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
        });
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
