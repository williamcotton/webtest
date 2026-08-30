use webtest_browser_manager::BrowserManager;

use crate::{
    chrome::{browser_manager_error, resolve_chrome},
    cli::BrowserCommand,
    error::AppError,
    project_context::project,
    report::ExitClass,
};

pub(crate) fn run_browser(command: BrowserCommand) -> Result<ExitClass, AppError> {
    let manager = BrowserManager::new().map_err(AppError::infrastructure)?;
    match command {
        BrowserCommand::Install { version } => {
            let installed = manager
                .install(version.as_deref())
                .map_err(browser_manager_error)?;
            println!(
                "installed Chrome for Testing {} ({})",
                installed.version,
                installed.platform.name()
            );
            println!("{}", installed.executable.display());
        }
        BrowserCommand::List => {
            for installed in manager.list().map_err(AppError::infrastructure)? {
                println!(
                    "{}\t{}\t{}",
                    installed.version,
                    installed.platform.name(),
                    installed.executable.display()
                );
            }
        }
        BrowserCommand::Path => {
            let project = project(&[])?;
            let resolved = resolve_chrome(&project, None)?;
            println!("{}", resolved.path.display());
            eprintln!("Chrome source: {}", resolved.provenance);
        }
        BrowserCommand::Clean { version } => {
            let removed = manager
                .clean(version.as_deref())
                .map_err(AppError::infrastructure)?;
            println!("removed {removed} managed Chrome installation(s)");
        }
    }
    Ok(ExitClass::Success)
}
