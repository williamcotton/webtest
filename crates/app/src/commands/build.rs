use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use webtest_plan::{PLAN_FORMAT_VERSION, PlanEnvelope, PlanSourceFile};
use webtest_text::SourceRevision;

use crate::{
    error::AppError,
    plan_security::reject_literal_secrets,
    project_analysis::analyze_project,
    project_context::{display_path, normalized_path, project, read_source, revision_hex},
    provider_composition::analysis_database_for_project,
    report::{ExitClass, Reporter, write_report},
};

pub(crate) fn run_build(paths: Vec<PathBuf>, emit: PathBuf) -> Result<ExitClass, AppError> {
    let project = project(&paths)?;
    build_project(&project, &emit)
}

fn build_project(project: &webtest_project::Project, emit: &Path) -> Result<ExitClass, AppError> {
    let report = analyze_project(project)?;
    if report.exit_class != ExitClass::Success {
        write_report(&report, Reporter::Human)?;
        return Ok(report.exit_class);
    }

    let mut database = analysis_database_for_project(project)?;
    let mut opened = Vec::new();
    for file in &project.files {
        let source = read_source(&file.path)?;
        let file_id = database.open_file(file.path.display().to_string(), source);
        opened.push((file, file_id));
    }
    let mut source_files = Vec::new();
    let mut tests = Vec::new();
    let mut capabilities = BTreeSet::new();
    let mut next_test = 0u32;
    let mut next_step = 0u32;
    for (file, file_id) in opened {
        let plan = database.test_plan(file_id).map_err(AppError::internal)?;
        source_files.push(PlanSourceFile {
            file: file_id,
            path: display_path(file),
            revision: plan.source_revision,
        });
        capabilities.extend(plan.required_host_capabilities.iter().copied());
        for mut test in plan.tests.clone() {
            test.id = webtest_hir::TestId(next_test);
            next_test += 1;
            for step in &mut test.steps {
                step.id = webtest_hir::StepId(next_step);
                next_step += 1;
            }
            tests.push(test);
        }
    }
    let project_name = project
        .config
        .project
        .name
        .clone()
        .unwrap_or_else(|| normalized_path(&project.root));
    let config_source = project
        .config_path
        .as_deref()
        .map(read_source)
        .transpose()?
        .unwrap_or_default();
    let envelope = PlanEnvelope {
        format_version: PLAN_FORMAT_VERSION,
        compiler_version: env!("CARGO_PKG_VERSION").into(),
        project_identity: format!(
            "{project_name}@{}",
            revision_hex(SourceRevision::of(&config_source))
        ),
        source_files,
        required_host_capabilities: capabilities.into_iter().collect(),
        provider_schema_hashes: database.provider_schema_hashes(),
        tests,
    };
    reject_literal_secrets(&envelope, project)?;
    let encoded = serde_json::to_vec_pretty(&envelope).map_err(AppError::internal)?;
    if let Some(parent) = emit.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(AppError::infrastructure)?;
    }
    std::fs::write(emit, encoded).map_err(AppError::infrastructure)?;
    println!("emitted {}", emit.display());
    Ok(ExitClass::Success)
}
