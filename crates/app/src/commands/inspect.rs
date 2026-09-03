use std::{
    io::{self, Write},
    path::PathBuf,
};

use webtest_browser::{BrowserHost, PageInspection};
use webtest_browser_cdp::ChromeHost;

use crate::{
    chrome::resolve_chrome,
    cli::ReferenceReporter,
    error::AppError,
    project_context::project,
    provider_composition::{runtime_application, runtime_provider_registry},
    report::ExitClass,
    runtime_configuration::{browser_context_options, inspection_options, runner_options},
};

pub(crate) async fn run_inspect(
    requested_url: Option<String>,
    chrome_path: Option<PathBuf>,
    headed: bool,
    reporter: ReferenceReporter,
) -> Result<ExitClass, AppError> {
    let project = project(&[])?;
    let project_mode = requested_url
        .as_deref()
        .is_none_or(|url| !is_absolute_http_url(url));
    let requested_url = requested_url
        .as_deref()
        .or(project.config.browser.base_url.as_deref())
        .ok_or_else(|| AppError::usage("inspect requires a URL or configured browser.base_url"))?;
    let url = webtest_runtime::resolve_browser_url(
        project.config.browser.base_url.as_deref(),
        requested_url,
    )
    .map_err(AppError::usage)?;
    let application = if project_mode && project.config.app.is_some() {
        let providers = runtime_provider_registry(&project, &runner_options(&project))?;
        runtime_application(&project, providers.app)
    } else {
        None
    };
    if let Some(application) = &application
        && let Err(error) = application.start(&project).await
    {
        let _ = application.shutdown().await;
        return Err(AppError::infrastructure(error));
    }
    let inspected = inspect_page(&project, &url, chrome_path, headed).await;
    let shutdown = match &application {
        Some(application) => application.shutdown().await,
        None => Ok(()),
    };
    let inspection = match inspected {
        Ok(inspection) => inspection,
        Err(error) => {
            if let Err(shutdown) = shutdown {
                tracing::warn!(%shutdown, "application teardown also failed after inspection failure");
            }
            return Err(error);
        }
    };
    shutdown.map_err(AppError::infrastructure)?;

    let stdout = io::stdout();
    let mut output = stdout.lock();
    write_inspection(&inspection, reporter, &mut output)?;
    Ok(ExitClass::Success)
}

async fn inspect_page(
    project: &webtest_project::Project,
    url: &str,
    chrome_path: Option<PathBuf>,
    headed: bool,
) -> Result<PageInspection, AppError> {
    let resolved = resolve_chrome(project, chrome_path)?;
    let host = ChromeHost::new(Some(resolved.path))
        .with_headed(headed || !project.config.browser.headless)
        .with_timeouts(
            project.config.timeouts.browser_command,
            project.config.timeouts.navigation,
        );
    let mut session = host.start().await.map_err(AppError::infrastructure)?;
    let mut context = match session.new_context(&browser_context_options(project)).await {
        Ok(context) => context,
        Err(error) => {
            let _ = session.close().await;
            return Err(AppError::infrastructure(error));
        }
    };
    let mut page = match context.new_page().await {
        Ok(page) => page,
        Err(error) => {
            let _ = context.close().await;
            let _ = session.close().await;
            return Err(AppError::infrastructure(error));
        }
    };
    let primary = match page.open(url).await {
        Ok(()) => page.inspect(&inspection_options(project)).await,
        Err(error) => Err(error),
    };
    drop(page);
    let context_cleanup = context.close().await;
    let session_cleanup = session.close().await;
    let inspection = primary.map_err(AppError::infrastructure)?;
    context_cleanup.map_err(AppError::infrastructure)?;
    session_cleanup.map_err(AppError::infrastructure)?;

    Ok(inspection)
}

fn is_absolute_http_url(value: &str) -> bool {
    url::Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

fn write_inspection(
    inspection: &PageInspection,
    reporter: ReferenceReporter,
    output: &mut dyn Write,
) -> Result<(), AppError> {
    match reporter {
        ReferenceReporter::Json => {
            serde_json::to_writer_pretty(&mut *output, inspection)
                .map_err(AppError::infrastructure)?;
            writeln!(output).map_err(AppError::infrastructure)?;
        }
        ReferenceReporter::Human => {
            writeln!(
                output,
                "{} — {}",
                inspection.page.url, inspection.page.title
            )
            .map_err(AppError::infrastructure)?;
            for element in &inspection.elements {
                let actions = element
                    .supported_actions
                    .iter()
                    .map(|action| format!("{action:?}").to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ");
                let description = match (&element.role, &element.accessible_name) {
                    (Some(role), Some(name)) => format!("{role} {name:?}"),
                    (Some(role), None) => role.clone(),
                    (None, Some(name)) => format!("text {name:?}"),
                    (None, None) => "element".into(),
                };
                writeln!(
                    output,
                    "  {:<44} {:<24} {}",
                    element.preferred_locator.source, description, actions
                )
                .map_err(AppError::infrastructure)?;
            }
            if inspection.truncation.elements_truncated {
                writeln!(
                    output,
                    "  … {} additional semantic element(s) omitted",
                    inspection.truncation.omitted_elements
                )
                .map_err(AppError::infrastructure)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use webtest_browser::{
        ElementStates, InspectableElement, InspectionTruncation, LocatorCandidate,
        LocatorCandidateKind, PageSummary, SupportedAction,
    };

    use super::*;

    #[test]
    fn absolute_http_urls_select_standalone_inspection() {
        assert!(is_absolute_http_url("http://127.0.0.1:3000/login"));
        assert!(is_absolute_http_url("https://example.test"));
        assert!(!is_absolute_http_url("/login"));
        assert!(!is_absolute_http_url("login"));
        assert!(!is_absolute_http_url("file:///tmp/page.html"));
    }

    #[test]
    fn human_inspection_columns_and_truncation_notice_are_stable() {
        let inspection = PageInspection {
            kind: "page_inspection".into(),
            inspection_schema_version: webtest_browser::INSPECTION_SCHEMA_VERSION,
            snapshot_id: "snapshot".into(),
            browser_version: "test".into(),
            page: PageSummary {
                url: "https://example.test".into(),
                title: "Example".into(),
            },
            elements: vec![InspectableElement {
                role: Some("button".into()),
                accessible_name: Some("Save".into()),
                label: None,
                placeholder: None,
                test_id: None,
                dom_id: None,
                states: ElementStates::default(),
                supported_actions: vec![SupportedAction::Click],
                preferred_locator: LocatorCandidate {
                    source: "role(\"button\", name: \"Save\")".into(),
                    kind: LocatorCandidateKind::Role,
                    reason: "role and accessible name".into(),
                },
                alternate_locators: Vec::new(),
                options: Vec::new(),
            }],
            truncation: InspectionTruncation {
                elements_truncated: true,
                omitted_elements: 2,
                ..InspectionTruncation::default()
            },
        };
        let mut output = Vec::new();
        write_inspection(&inspection, ReferenceReporter::Human, &mut output).expect("render");
        let output = String::from_utf8(output).expect("UTF-8");
        assert_eq!(
            output,
            "https://example.test — Example\n  role(\"button\", name: \"Save\")                 button \"Save\"            click\n  … 2 additional semantic element(s) omitted\n"
        );
    }
}
