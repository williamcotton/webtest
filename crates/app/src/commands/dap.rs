use std::{path::PathBuf, sync::Arc};

use crate::{
    chrome::LazyChromeHost,
    error::AppError,
    project_context::project,
    provider_composition::{runtime_application, runtime_provider_registry},
    report::ExitClass,
    runtime_configuration::runner_options,
};

struct ProjectWorkspaceLoader {
    project: webtest_project::Project,
    providers: webtest_provider::ProviderRegistry,
}
impl webtest_dap::WorkspaceLoader for ProjectWorkspaceLoader {
    fn load(
        &self,
        program: &std::path::Path,
        sources: &std::collections::BTreeMap<PathBuf, String>,
    ) -> Result<webtest_analysis::AnalysisDatabase, String> {
        let roots = webtest_project::workspace_source_roots(&self.project)
            .map_err(|error| error.to_string())?;
        let allowed = |path: &std::path::Path| {
            !path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }) && path
                .extension()
                .is_some_and(|extension| extension == "webtest")
                && roots.iter().any(|root| path.starts_with(root))
        };
        if !allowed(program) || sources.keys().any(|path| !allowed(path)) {
            return Err("debug source is outside configured workspace source roots".into());
        }
        if sources.len() > 16_384
            || sources.values().map(String::len).sum::<usize>() > 64 * 1024 * 1024
        {
            return Err("debug source overrides exceed the workspace input budget".into());
        }
        let mut database = crate::project_analysis::workspace_with_providers(
            &self.project,
            self.providers.clone(),
        )
        .map_err(|error| error.to_string())?;
        for (path, source) in sources {
            database.open_file(path.display().to_string(), source);
        }
        let file = if let Some(file) = database.file_for_path(&program.display().to_string()) {
            file
        } else {
            database.open_file(
                program.display().to_string(),
                std::fs::read_to_string(program).map_err(|error| error.to_string())?,
            )
        };
        crate::project_analysis::populate_imports(&self.project, &mut database, file)
            .map_err(|error| error.to_string())?;
        Ok(database)
    }
}

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
    let loader = Arc::new(ProjectWorkspaceLoader {
        project: project.clone(),
        providers: providers.clone(),
    });
    let serve_result =
        webtest_dap::serve_with_workspace(Arc::new(browser), options, providers, loader).await;
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
