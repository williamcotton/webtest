use std::{path::Path, sync::Arc, time::Instant};

use webtest_analysis::AnalysisDatabase;
use webtest_plan::TestPlan;
use webtest_project::{DiscoveredFile, Project};
use webtest_text::{FileId, SourceRevision};

use crate::{
    diagnostic_output::{config_diagnostics, diagnostic_report},
    error::AppError,
    project_context::{display_path, nanos, read_source, revision_hex},
    provider_composition::analysis_database_for_project,
    report::{CommandReport, DiagnosticReport, ExitClass, FileReport, base_report},
};

pub(crate) struct AnalyzedFile {
    pub(crate) source: String,
    pub(crate) source_revision: String,
    pub(crate) plan: Arc<TestPlan>,
    pub(crate) diagnostics: Vec<DiagnosticReport>,
}

pub(crate) fn analyze_file(
    project: &Project,
    file: &DiscoveredFile,
    database: &mut AnalysisDatabase,
) -> Result<AnalyzedFile, AppError> {
    let file_id = database
        .file_for_path(&file.path.display().to_string())
        .ok_or_else(|| AppError::internal("workspace source was not loaded"))?;
    let source = database
        .source(file_id)
        .map_err(AppError::internal)?
        .to_string();
    let source_revision = revision_hex(SourceRevision::of(&source));
    let diagnostics = database.diagnostics(file_id).map_err(AppError::internal)?;
    let mut diagnostics = diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic_report(&display_path(file), &source_revision, &source, diagnostic)
        })
        .collect::<Vec<_>>();
    let plan = database.test_plan(file_id).map_err(AppError::internal)?;
    diagnostics.extend(config_diagnostics(
        project,
        &display_path(file),
        &source_revision,
        &source,
        &plan,
    ));
    Ok(AnalyzedFile {
        source,
        source_revision,
        plan,
        diagnostics,
    })
}

pub(crate) fn analyze_project(project: &Project) -> Result<CommandReport, AppError> {
    let mut database = workspace_for(project)?;
    analyze_workspace(project, &mut database)
}

pub(crate) fn analyze_workspace(
    project: &Project,
    database: &mut AnalysisDatabase,
) -> Result<CommandReport, AppError> {
    let mut report = base_report("check", project);
    for file in &project.files {
        let started = Instant::now();
        let analyzed = analyze_file(project, file, database)?;
        let has_errors = analyzed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error");
        if has_errors {
            report.exit_class = report.exit_class.combine(ExitClass::TestFailure);
        }
        report.files.push(FileReport {
            path: display_path(file),
            exit_class: if has_errors {
                ExitClass::TestFailure
            } else {
                ExitClass::Success
            },
            source_revision: analyzed.source_revision,
            duration_nanos: nanos(started.elapsed()),
            diagnostics: analyzed.diagnostics,
            tests: Vec::new(),
            outcome: None,
            reason: None,
            execution_error: None,
            events: Vec::new(),
        });
    }
    report.finish();
    Ok(report)
}

pub(crate) fn workspace_for(project: &Project) -> Result<AnalysisDatabase, AppError> {
    load_workspace(project, analysis_database_for_project(project)?)
}
pub(crate) fn workspace_with_providers(
    project: &Project,
    providers: webtest_provider::ProviderRegistry,
) -> Result<AnalysisDatabase, AppError> {
    load_workspace(
        project,
        crate::provider_composition::analysis_database_with_providers(project, providers)?,
    )
}
fn load_workspace(
    project: &Project,
    mut database: AnalysisDatabase,
) -> Result<AnalysisDatabase, AppError> {
    let mut roots = Vec::new();
    for file in webtest_project::workspace_sources(project)
        .map_err(AppError::usage)?
        .iter()
        .chain(&project.files)
    {
        roots.push(database.open_file(file.path.display().to_string(), read_source(&file.path)?));
    }
    for root in roots {
        populate_imports(project, &mut database, root)?;
    }
    Ok(database)
}

/// Native I/O feeds only candidates produced by canonical module resolution.
/// Canonical aliases share one source input; symlinks cannot escape source roots.
pub(crate) fn populate_imports(
    project: &Project,
    database: &mut AnalysisDatabase,
    root: FileId,
) -> Result<(), AppError> {
    let roots = webtest_project::workspace_source_roots(project).map_err(AppError::usage)?;
    let mut attempted = std::collections::BTreeSet::new();
    loop {
        let graph = database.module_graph(root).map_err(AppError::internal)?;
        let mut changed = false;
        for candidate in &graph.required_inputs {
            if !attempted.insert(candidate.clone()) || database.file_for_path(candidate).is_some() {
                continue;
            }
            let path = Path::new(candidate);
            let canonical = match std::fs::canonicalize(path) {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(AppError::usage(format!(
                        "cannot resolve module {}: {error}",
                        path.display()
                    )));
                }
            };
            if !roots.iter().any(|root| canonical.starts_with(root)) {
                return Err(AppError::usage(format!(
                    "module {} resolves outside configured source roots",
                    path.display()
                )));
            }
            let file = if let Some(file) = database.file_for_path(&canonical.display().to_string())
            {
                file
            } else {
                if attempted.len() > 16_384 {
                    return Err(AppError::usage("module input limit exceeded"));
                }
                database.open_file(canonical.display().to_string(), read_source(&canonical)?)
            };
            database
                .alias_file(candidate, file)
                .map_err(AppError::internal)?;
            changed = true;
        }
        if !changed {
            return Ok(());
        }
    }
}

#[derive(Clone)]
pub(crate) struct CapturedSource {
    pub path: String,
    pub text: Arc<str>,
    pub revision: SourceRevision,
}
pub(crate) type CapturedSources = std::collections::HashMap<FileId, CapturedSource>;
pub(crate) fn capture_sources(
    database: &AnalysisDatabase,
    project: &Project,
) -> Result<CapturedSources, AppError> {
    database
        .source_inputs()
        .into_iter()
        .map(|(path, text)| {
            let file = database
                .file_for_path(&path)
                .ok_or_else(|| AppError::internal("captured source has no file identity"))?;
            let path = Path::new(&path)
                .strip_prefix(&project.root)
                .unwrap_or(Path::new(&path))
                .to_string_lossy()
                .replace('\\', "/");
            Ok((
                file,
                CapturedSource {
                    path,
                    revision: SourceRevision::of(&text),
                    text,
                },
            ))
        })
        .collect()
}
