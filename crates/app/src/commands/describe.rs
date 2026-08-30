use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use webtest_analysis::{
    DescriptionLimits, DescriptionProject, DescriptionRequest, DescriptionResponse,
};
use webtest_text::SourceRevision;

use crate::{
    cli::ReferenceReporter,
    error::AppError,
    project_context::{normalized_path, read_source, revision_hex},
    provider_composition::analysis_database_for_project,
    report::ExitClass,
    runtime_configuration::resolved_runtime_configuration,
};

pub(crate) fn run_describe(
    query: Option<String>,
    search: Option<String>,
    project_path: Option<PathBuf>,
    reporter: ReferenceReporter,
) -> Result<ExitClass, AppError> {
    let project_input = if let Some(path) = project_path {
        Some(path)
    } else {
        std::env::current_dir()
            .map_err(AppError::usage)?
            .ancestors()
            .find(|directory| directory.join("webtest.toml").is_file())
            .map(Path::to_path_buf)
    };
    let resolved_project =
        project_input.as_ref().and_then(|path| {
            match webtest_project::discover(std::slice::from_ref(path)) {
                Ok(project) => Some(project),
                Err(error) => {
                    eprintln!("warning[description.project]: {error}");
                    None
                }
            }
        });
    let (project_reference, limits) = match resolved_project.as_ref() {
        Some(project) => {
            let configuration = project
                .config_path
                .as_deref()
                .map(read_source)
                .transpose()?
                .unwrap_or_default();
            (
                Some(DescriptionProject {
                    root: normalized_path(&project.root),
                    configuration_revision: revision_hex(SourceRevision::of(&configuration)),
                    resolved_runtime_configuration: Some(resolved_runtime_configuration(project)),
                }),
                DescriptionLimits {
                    max_category_children: project.config.description.max_category_children,
                    max_search_results: project.config.description.max_search_results,
                    max_summary_bytes: project.config.description.max_summary_bytes,
                    max_guidance_entries: project.config.description.max_guidance_entries,
                    max_examples: project.config.description.max_examples,
                    max_example_bytes: project.config.description.max_example_bytes,
                },
            )
        }
        None => (None, DescriptionLimits::default()),
    };
    let request = if let Some(search) = search {
        DescriptionRequest::Search(search)
    } else if let Some(query) = query {
        DescriptionRequest::Query(query)
    } else {
        DescriptionRequest::Index
    };
    let database = resolved_project
        .as_ref()
        .map(analysis_database_for_project)
        .transpose()?
        .unwrap_or_default();
    let response = database.describe(request, project_reference, limits);
    let failed = matches!(response, DescriptionResponse::Diagnostic(_));
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match reporter {
        ReferenceReporter::Json => {
            serde_json::to_writer_pretty(&mut output, &response)
                .map_err(AppError::infrastructure)?;
            writeln!(output).map_err(AppError::infrastructure)?;
        }
        ReferenceReporter::Human => write_description_human(&response, &mut output)?,
    }
    Ok(if failed {
        ExitClass::Usage
    } else {
        ExitClass::Success
    })
}

fn write_description_human(
    response: &DescriptionResponse,
    output: &mut dyn Write,
) -> Result<(), AppError> {
    match response {
        DescriptionResponse::Index(index) => {
            writeln!(output, "WebTest {} reference", index.language_version)
                .map_err(AppError::infrastructure)?;
            for (category, children) in &index.categories {
                writeln!(output, "{category}").map_err(AppError::infrastructure)?;
                for child in children {
                    writeln!(output, "  {child}").map_err(AppError::infrastructure)?;
                }
            }
        }
        DescriptionResponse::Language(language) => {
            writeln!(output, "WebTest {} language", language.language_version)
                .map_err(AppError::infrastructure)?;
            for (rule, syntax) in &language.language.grammar {
                writeln!(output, "  {rule:<20} {syntax}").map_err(AppError::infrastructure)?;
            }
        }
        DescriptionResponse::Grammar(grammar) => {
            for (rule, syntax) in &grammar.grammar {
                writeln!(output, "{rule:<20} {syntax}").map_err(AppError::infrastructure)?;
            }
        }
        DescriptionResponse::Category(category) => {
            writeln!(output, "{} — {}", category.id, category.summary)
                .map_err(AppError::infrastructure)?;
            for child in &category.children {
                writeln!(output, "  {child}").map_err(AppError::infrastructure)?;
            }
        }
        DescriptionResponse::Construct(construct) => {
            writeln!(output, "{}\n  {}", construct.id, construct.syntax)
                .map_err(AppError::infrastructure)?;
            writeln!(output, "\n{}", construct.summary).map_err(AppError::infrastructure)?;
            if !construct.allowed_contexts.is_empty() {
                writeln!(
                    output,
                    "contexts: {}",
                    construct.allowed_contexts.join(", ")
                )
                .map_err(AppError::infrastructure)?;
            }
            if !construct.requires_capabilities.is_empty() {
                writeln!(
                    output,
                    "capabilities: {}",
                    construct
                        .requires_capabilities
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .map_err(AppError::infrastructure)?;
            }
            if !construct.parameters.is_empty() {
                writeln!(output, "parameters:").map_err(AppError::infrastructure)?;
                for parameter in &construct.parameters {
                    let requirement = if parameter.required {
                        "required"
                    } else {
                        "optional"
                    };
                    let secret = if parameter.secret { ", secret" } else { "" };
                    write!(
                        output,
                        "  {}: {} ({requirement}{secret})",
                        parameter.name, parameter.ty
                    )
                    .map_err(AppError::infrastructure)?;
                    if let Some(default) = &parameter.default {
                        write!(output, ", default {default}").map_err(AppError::infrastructure)?;
                    }
                    writeln!(output).map_err(AppError::infrastructure)?;
                    if !parameter.documentation.is_empty() {
                        writeln!(output, "    {}", parameter.documentation)
                            .map_err(AppError::infrastructure)?;
                    }
                }
            }
            if let Some(return_type) = &construct.return_type {
                writeln!(output, "returns: {return_type}").map_err(AppError::infrastructure)?;
            }
            if let Some(retry_safe) = construct.retry_safe {
                writeln!(output, "retry safe: {retry_safe}").map_err(AppError::infrastructure)?;
            }
            if !construct.effects.is_empty() {
                writeln!(output, "effects: {}", construct.effects.join(", "))
                    .map_err(AppError::infrastructure)?;
            }
            if !construct.failure_modes.is_empty() {
                writeln!(
                    output,
                    "failure modes: {}",
                    construct.failure_modes.join(", ")
                )
                .map_err(AppError::infrastructure)?;
            }
            if !construct.constraints.is_empty() {
                writeln!(output, "\nconstraints:").map_err(AppError::infrastructure)?;
                for entry in &construct.constraints {
                    writeln!(output, "  - {}: {}", entry.code, entry.summary)
                        .map_err(AppError::infrastructure)?;
                }
            }
            if !construct.guidance.is_empty() {
                writeln!(output, "\nguidance:").map_err(AppError::infrastructure)?;
                for entry in &construct.guidance {
                    writeln!(output, "  - {}: {}", entry.code, entry.summary)
                        .map_err(AppError::infrastructure)?;
                }
            }
            if !construct.availability.runtime_requires.is_empty()
                || !construct
                    .availability
                    .configuration_prerequisites
                    .is_empty()
            {
                writeln!(output, "\navailability:").map_err(AppError::infrastructure)?;
                if !construct.availability.runtime_requires.is_empty() {
                    writeln!(
                        output,
                        "  runtime: {}",
                        construct.availability.runtime_requires.join(", ")
                    )
                    .map_err(AppError::infrastructure)?;
                }
                for prerequisite in &construct.availability.configuration_prerequisites {
                    writeln!(output, "  configuration: {prerequisite}")
                        .map_err(AppError::infrastructure)?;
                }
            }
            if let Some(configuration) = &construct.resolved_configuration {
                writeln!(output, "\nresolved configuration:").map_err(AppError::infrastructure)?;
                for (name, value) in [
                    (
                        "selected adapter",
                        configuration.selected_adapter.as_deref(),
                    ),
                    (
                        "selected transport",
                        configuration.selected_transport.as_deref(),
                    ),
                    (
                        "resolved command",
                        configuration.resolved_command.as_deref(),
                    ),
                    (
                        "working directory",
                        configuration.working_directory.as_deref(),
                    ),
                    ("schema path", configuration.schema_path.as_deref()),
                    (
                        "browser base URL",
                        configuration.browser_base_url.as_deref(),
                    ),
                    ("server base URL", configuration.server_base_url.as_deref()),
                ] {
                    writeln!(output, "  {name}: {}", value.unwrap_or("<not configured>"))
                        .map_err(AppError::infrastructure)?;
                }
                writeln!(
                    output,
                    "  resolved arguments: [{}]",
                    configuration.resolved_arguments.join(", ")
                )
                .map_err(AppError::infrastructure)?;
            }
            for example in &construct.examples {
                writeln!(
                    output,
                    "\n{}:\n  {}",
                    example.name,
                    example.source.replace('\n', "\n  ")
                )
                .map_err(AppError::infrastructure)?;
                for prerequisite in &example.prerequisites {
                    writeln!(output, "  requires: {prerequisite}")
                        .map_err(AppError::infrastructure)?;
                }
            }
        }
        DescriptionResponse::Search(search) => {
            for result in &search.results {
                writeln!(
                    output,
                    "{:<32} {:<44} {}",
                    result.id, result.syntax, result.summary
                )
                .map_err(AppError::infrastructure)?;
            }
        }
        DescriptionResponse::Diagnostic(diagnostic) => {
            writeln!(output, "error[{}]: {}", diagnostic.code, diagnostic.message)
                .map_err(AppError::infrastructure)?;
            for hint in &diagnostic.repair_hints {
                let replacement = match &hint.replacement {
                    webtest_feedback::RepairReplacement::Locator { source } => source,
                    webtest_feedback::RepairReplacement::Text(value) => value,
                };
                writeln!(output, "  suggestion: {replacement}")
                    .map_err(AppError::infrastructure)?;
            }
            if !diagnostic.reference_queries.is_empty() {
                writeln!(
                    output,
                    "  reference: {}",
                    diagnostic.reference_queries.join(", ")
                )
                .map_err(AppError::infrastructure)?;
            }
        }
    }
    Ok(())
}
