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
) -> Result<AnalyzedFile, AppError> {
    let source = read_source(&file.path)?;
    let source_revision = revision_hex(SourceRevision::of(&source));
    let (mut database, file_id) = database_for(project, &file.path, &source)?;
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
    let mut report = base_report("check", project);
    for file in &project.files {
        let started = Instant::now();
        let analyzed = analyze_file(project, file)?;
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

pub(crate) fn database_for(
    project: &Project,
    path: &Path,
    source: &str,
) -> Result<(AnalysisDatabase, FileId), AppError> {
    let mut database = analysis_database_for_project(project)?;
    let file = database.open_file(path.display().to_string(), source);
    Ok((database, file))
}
