mod chrome;
mod cli;
mod commands;
mod diagnostic_output;
mod error;
mod init;
mod lsp_projects;
mod plan_security;
mod project_analysis;
mod project_context;
mod provider_composition;
mod report;
mod runtime_configuration;
mod runtime_output;
mod source_output;
mod test_progress;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    match commands::run(cli).await {
        Ok(class) => ExitCode::from(class.code()),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.class.code())
        }
    }
}
