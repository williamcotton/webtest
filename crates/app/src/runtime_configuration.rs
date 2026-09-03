use webtest_analysis::ResolvedRuntimeConfiguration;
use webtest_browser::{BrowserContextOptions, InspectionOptions, Viewport};
use webtest_project::Project;
use webtest_provider::{
    FsProviderConfig, HttpProviderConfig, NativeProviderConfig, ProcessProviderConfig,
};
use webtest_runtime::{EvidenceOptions, RunnerOptions};

use crate::project_context::normalized_path;

pub(crate) fn runner_options(project: &Project) -> RunnerOptions {
    RunnerOptions {
        base_url: project.config.browser.base_url.clone(),
        action_timeout: project.config.timeouts.action,
        assertion_timeout: project.config.timeouts.assertion,
        navigation_timeout: project.config.timeouts.navigation,
        provider_call_timeout: project.config.timeouts.provider_call,
        test_timeout: project.config.timeouts.test,
        browser_context: browser_context_options(project),
        evidence: EvidenceOptions {
            screenshot_on_failure: project.config.evidence.screenshot
                == webtest_project::EvidenceMode::OnFailure,
            dom_snapshot_on_failure: project.config.evidence.dom_snapshot
                == webtest_project::EvidenceMode::OnFailure,
            max_dom_bytes: project.config.evidence.max_dom_bytes,
            artifact_directory: project.root.join(&project.config.artifacts.directory),
        },
        project_root: project.root.clone(),
        redacted_json_fields: project
            .config
            .redaction
            .json_fields
            .iter()
            .chain(&project.config.redaction.headers)
            .cloned()
            .collect(),
        provider_config: NativeProviderConfig {
            http: HttpProviderConfig {
                base_url: project.config.server.base_url.clone(),
                follow_redirects: project.config.server.http.follow_redirects,
                max_response_bytes: project.config.server.http.max_response_bytes,
            },
            process: ProcessProviderConfig {
                allowed_working_roots: project.config.server.process.allowed_working_roots.clone(),
                max_output_bytes: project.config.server.process.max_output_bytes,
            },
            fs: FsProviderConfig {
                read_roots: project.config.server.fs.read_roots.clone(),
                write_root: project.config.server.fs.write_root.clone(),
            },
        },
        inspection: inspection_options(project),
    }
}

pub(crate) fn browser_context_options(project: &Project) -> BrowserContextOptions {
    BrowserContextOptions {
        viewport: Viewport {
            width: project.config.browser.viewport.width,
            height: project.config.browser.viewport.height,
        },
        test_id_attribute: project.config.browser.test_id_attribute.clone(),
    }
}

pub(crate) fn inspection_options(project: &Project) -> InspectionOptions {
    InspectionOptions {
        max_elements: project.config.inspection.max_elements,
        max_candidates_per_element: project.config.inspection.max_candidates_per_element,
        max_text_bytes: project.config.inspection.max_text_bytes,
        include_hidden: project.config.inspection.include_hidden,
        redacted_query_parameters: project.config.redaction.query_params.clone(),
        redacted_values: Vec::new(),
    }
}

pub(crate) fn resolved_runtime_configuration(project: &Project) -> ResolvedRuntimeConfiguration {
    let provider = project.config.server.app.as_ref();
    let application = project.config.app.as_ref();
    let adapter = provider.map(|app| match app.adapter {
        webtest_project::ServerAppAdapter::Bridge => "bridge",
        webtest_project::ServerAppAdapter::Command => "command",
        webtest_project::ServerAppAdapter::Http => "http",
    });
    let transport = provider
        .filter(|app| app.adapter == webtest_project::ServerAppAdapter::Bridge)
        .map(|app| match app.transport {
            webtest_project::ServerAppTransport::Auto => "auto",
            webtest_project::ServerAppTransport::Unix => "unix",
            webtest_project::ServerAppTransport::NamedPipe => "named_pipe",
            webtest_project::ServerAppTransport::Tcp => "tcp",
            webtest_project::ServerAppTransport::Stdio => "stdio",
        });
    let uses_provider_command = provider.is_some_and(|app| {
        app.adapter == webtest_project::ServerAppAdapter::Command
            || (app.adapter == webtest_project::ServerAppAdapter::Bridge
                && app.transport == webtest_project::ServerAppTransport::Stdio)
    });
    let (command, arguments, working_directory) = if uses_provider_command {
        let command = provider.and_then(|app| app.command.first()).cloned();
        let arguments = provider
            .map(|app| {
                redact_secret_arguments(&app.command[usize::from(!app.command.is_empty())..])
            })
            .unwrap_or_default();
        (command, arguments, Some(normalized_path(&project.root)))
    } else {
        let command = application.and_then(|app| app.command.clone());
        let arguments = application
            .map(|app| redact_secret_arguments(&app.args))
            .unwrap_or_default();
        let working_directory =
            application.map(|app| normalized_path(&project.root.join(&app.working_directory)));
        (command, arguments, working_directory)
    };
    ResolvedRuntimeConfiguration {
        selected_adapter: adapter.map(str::to_owned),
        selected_transport: transport.map(str::to_owned),
        resolved_command: command,
        resolved_arguments: arguments,
        working_directory,
        schema_path: provider.map(|app| normalized_path(&project.root.join(&app.schema))),
        application_owned: application.map(|app| app.owned),
        application_health_configured: application.is_some_and(|app| app.health.is_some()),
        browser_base_url: project.config.browser.base_url.clone(),
        server_base_url: project.config.server.base_url.clone(),
        test_timeout_ms: duration_millis(project.config.timeouts.test),
        provider_call_timeout_ms: duration_millis(project.config.timeouts.provider_call),
    }
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn redact_secret_arguments(arguments: &[String]) -> Vec<String> {
    let mut redact_next = false;
    arguments
        .iter()
        .map(|argument| {
            if redact_next {
                redact_next = false;
                return "<redacted>".into();
            }
            if let Some((key, _)) = argument.split_once('=')
                && secret_argument_name(key)
            {
                return format!("{key}=<redacted>");
            }
            if argument.starts_with('-') && secret_argument_name(argument) {
                redact_next = true;
            }
            argument.clone()
        })
        .collect()
}

fn secret_argument_name(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api-key",
        "apikey",
        "credential",
        "authorization",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_configuration_redacts_secret_like_arguments() {
        assert_eq!(
            redact_secret_arguments(&[
                "serve".into(),
                "--token".into(),
                "secret-value".into(),
                "--api-key=also-secret".into(),
            ]),
            ["serve", "--token", "<redacted>", "--api-key=<redacted>"]
        );
    }

    #[test]
    fn redaction_is_case_insensitive_and_preserves_nonsecrets() {
        assert_eq!(
            redact_secret_arguments(&[
                "--PASSWORD".into(),
                "hunter2".into(),
                "--port=3000".into(),
                "--Authorization=bearer".into(),
            ]),
            [
                "--PASSWORD",
                "<redacted>",
                "--port=3000",
                "--Authorization=<redacted>",
            ]
        );
    }
}
