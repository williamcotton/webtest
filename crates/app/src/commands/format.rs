use std::{
    io::{self, Write},
    path::PathBuf,
};

use crate::{
    error::AppError,
    project_context::{display_path, project, read_source},
    report::ExitClass,
};

pub(crate) fn run_format(paths: Vec<PathBuf>, check: bool) -> Result<ExitClass, AppError> {
    let project = project(&paths)?;
    let mut class = ExitClass::Success;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for warning in &project.warnings {
        writeln!(output, "warning[config.unknown]: {}", warning.message)
            .map_err(AppError::infrastructure)?;
    }
    for file in &project.files {
        let source = read_source(&file.path)?;
        let formatted = webtest_format::format_file(&webtest_syntax::parse(&source));
        if source == formatted {
            continue;
        }
        if check {
            writeln!(output, "{}: would be reformatted", display_path(file))
                .map_err(AppError::infrastructure)?;
            class = class.combine(ExitClass::TestFailure);
        } else {
            std::fs::write(&file.path, formatted).map_err(|error| {
                AppError::infrastructure(format!(
                    "could not write {}: {error}",
                    file.path.display()
                ))
            })?;
            writeln!(output, "{}: formatted", display_path(file))
                .map_err(AppError::infrastructure)?;
        }
    }
    Ok(class)
}
