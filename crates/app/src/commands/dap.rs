use std::{path::PathBuf, sync::Arc};

use crate::{
    chrome::LazyChromeHost, error::AppError, project_context::project,
    provider_composition::runtime_provider_registry, report::ExitClass,
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
    let providers = runtime_provider_registry(&project, &options)?;
    let serve_result =
        webtest_dap::serve_with_configuration(Arc::new(browser), options, providers.registry).await;
    if let Some(provider) = providers.app {
        provider
            .shutdown()
            .await
            .map_err(AppError::infrastructure)?;
    }
    serve_result.map_err(AppError::infrastructure)?;
    Ok(ExitClass::Success)
}
