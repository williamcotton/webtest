use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use webtest_plan::{PLAN_FORMAT_VERSION, PlanEnvelope, PlanSourceFile};
use webtest_text::SourceRevision;

use crate::{
    error::AppError,
    plan_security::reject_literal_secrets,
    project_analysis::{analyze_workspace, workspace_for},
    project_context::{normalized_path, project, revision_hex},
    report::{ExitClass, Reporter, write_report},
};

pub(crate) fn run_build(paths: Vec<PathBuf>, emit: PathBuf) -> Result<ExitClass, AppError> {
    let project = project(&paths)?;
    build_project(&project, &emit)
}

fn build_project(project: &webtest_project::Project, emit: &Path) -> Result<ExitClass, AppError> {
    let mut database = workspace_for(project)?;
    let report = analyze_workspace(project, &mut database)?;
    if report.exit_class != ExitClass::Success {
        write_report(&report, Reporter::Human)?;
        return Ok(report.exit_class);
    }

    let opened = project
        .files
        .iter()
        .map(|file| {
            database
                .file_for_path(&file.path.display().to_string())
                .map(|id| (file, id))
                .ok_or_else(|| AppError::internal("workspace source was not loaded"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut source_files = Vec::new();
    let mut tests = Vec::new();
    let mut fixtures = std::collections::BTreeMap::new();
    let mut capabilities = BTreeSet::new();
    let mut next_test = 0u32;
    let mut next_step = 0u32;
    for (_, file_id) in opened {
        let plan = database.test_plan(file_id).map_err(AppError::internal)?;
        for source in &plan.source_files {
            if !source_files
                .iter()
                .any(|previous: &PlanSourceFile| previous.file == source.file)
            {
                let mut source = source.clone();
                source.path = Path::new(&source.path)
                    .strip_prefix(&project.root)
                    .unwrap_or(Path::new(&source.path))
                    .display()
                    .to_string();
                source_files.push(source);
            }
        }
        for definition in &plan.fixtures {
            fixtures.insert(definition.id, definition.clone());
        }
        capabilities.extend(plan.required_host_capabilities.iter().copied());
        for mut test in plan.tests.clone() {
            test.id = webtest_model::TestId(next_test);
            next_test += 1;
            for step in test.body.steps_mut() {
                step.id = webtest_model::StepId(next_step);
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
    source_files.sort_by(|left, right| left.path.cmp(&right.path));
    let semantic_inputs = serde_json::to_string(&project.config).map_err(AppError::internal)?;
    let mut envelope = PlanEnvelope {
        fixtures: fixtures.into_values().collect(),
        format_version: PLAN_FORMAT_VERSION,
        runtime_semantics_version: webtest_plan::RUNTIME_SEMANTICS_VERSION,
        project_input_fingerprint: SourceRevision::of(&format!(
            "webtest-inputs/v1/{semantic_inputs}"
        )),
        compiler_version: env!("CARGO_PKG_VERSION").into(),
        project_identity: format!(
            "{project_name}@{}",
            revision_hex(SourceRevision::of(&normalized_path(&project.root)))
        ),
        source_files,
        required_host_capabilities: capabilities.into_iter().collect(),
        provider_schema_hashes: database.provider_schema_hashes(),
        tests,
    };
    envelope.remap_source_links();
    envelope
        .validate_capabilities()
        .map_err(AppError::internal)?;
    envelope.validate_tree().map_err(AppError::internal)?;
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
