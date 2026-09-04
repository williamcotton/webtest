use std::{path::PathBuf, sync::Arc};

use crate::{
    chrome::LazyChromeHost,
    error::AppError,
    project_context::project,
    provider_composition::{runtime_application, runtime_provider_registry},
    report::ExitClass,
    runtime_configuration::runner_options,
};

pub(crate) async fn run_dap(
    chrome_path: Option<PathBuf>,
    project_path: Option<PathBuf>,
    headless: bool,
) -> Result<ExitClass, AppError> {
    let project = match project_path {
        Some(path) => project(std::slice::from_ref(&path))?,
        None => project(&[])?,
    };
    let browser = LazyChromeHost::new(project.clone(), chrome_path, !headless, None);
    let options = runner_options(&project);
    let runtime_providers = runtime_provider_registry(&project, &options)?;
    let application = runtime_application(&project, runtime_providers.app.clone());
    let providers = runtime_providers.registry;
    if let Some(application) = &application
        && let Err(error) = application.start(&project).await
    {
        let _ = application.shutdown().await;
        return Err(AppError::infrastructure(error));
    }
    let serve_result =
        webtest_dap::serve_with_configuration(Arc::new(browser), options, providers).await;
    let shutdown_result = match &application {
        Some(application) => application.shutdown().await,
        None => Ok(()),
    };
    if let Err(error) = serve_result {
        if let Err(shutdown) = shutdown_result {
            tracing::warn!(%shutdown, "application teardown also failed after DAP failure");
        }
        return Err(AppError::infrastructure(error));
    }
    shutdown_result.map_err(AppError::infrastructure)?;
    Ok(ExitClass::Success)
}
