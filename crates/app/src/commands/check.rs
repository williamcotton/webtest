use std::path::PathBuf;

use crate::{
    cli::CheckReporter,
    error::AppError,
    project_analysis::analyze_project,
    project_context::project,
    report::{ExitClass, write_report},
};

pub(crate) fn run_check(
    paths: Vec<PathBuf>,
    reporter: CheckReporter,
) -> Result<ExitClass, AppError> {
    let project = project(&paths)?;
    let report = analyze_project(&project)?;
    write_report(&report, reporter.into())?;
    Ok(report.exit_class)
}
