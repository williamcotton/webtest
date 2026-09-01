use std::{path::PathBuf, sync::Arc};

use crate::{
    chrome::LazyChromeHost, error::AppError, lsp_projects::LspProjectEditors,
    project_context::project, report::ExitClass,
};

pub(crate) async fn run_lsp(chrome_path: Option<PathBuf>) -> Result<ExitClass, AppError> {
    let project = project(&[])?;
    let browser = LazyChromeHost::new(project.clone(), chrome_path, false, None);
    let project_editors = Arc::new(LspProjectEditors::default());
    let editor = project_editors.editor_for_project(&project)?;
    let document_projects = Arc::clone(&project_editors);
    let changed_projects = Arc::clone(&project_editors);
    webtest_lsp::serve_with_document_editors_and_project_changes(
        Arc::new(browser),
        editor,
        Arc::new(move |path| {
            document_projects
                .editor_for_path(path)
                .map_err(|error| error.to_string())
        }),
        Arc::new(move |path| {
            changed_projects
                .reload_for_changed_path(path)
                .map_err(|error| error.to_string())
        }),
    )
    .await;
    project_editors.shutdown().await?;
    Ok(ExitClass::Success)
}
