mod browser;
mod build;
mod check;
mod dap;
mod describe;
mod format;
mod inspect;
mod lsp;
mod test;

use crate::{
    cli::{Cli, Command},
    error::AppError,
    init,
    report::ExitClass,
};

pub(crate) async fn run(cli: Cli) -> Result<ExitClass, AppError> {
    match cli.command {
        Command::Init { path } => init::run(&path),
        Command::Check { paths, reporter } => check::run_check(paths, reporter),
        Command::Fmt { paths, check } => format::run_format(paths, check),
        Command::Build { paths, emit } => build::run_build(paths, emit),
        Command::Test {
            paths,
            chrome_path,
            headed,
            reporter,
        } => test::run_test(paths, chrome_path, headed, reporter).await,
        Command::Inspect {
            url,
            chrome_path,
            headed,
            reporter,
        } => inspect::run_inspect(url, chrome_path, headed, reporter).await,
        Command::Describe {
            query,
            search,
            project,
            reporter,
        } => describe::run_describe(query, search, project, reporter),
        Command::Browser { command } => browser::run_browser(command),
        Command::Lsp { chrome_path } => lsp::run_lsp(chrome_path).await,
        Command::Dap {
            chrome_path,
            project,
            headless,
        } => dap::run_dap(chrome_path, project, headless).await,
    }
}
