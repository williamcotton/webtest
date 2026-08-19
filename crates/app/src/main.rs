use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use webtest_analysis::{AnalysisDatabase, Diagnostic, DiagnosticSeverity};
use webtest_browser::{BrowserError, Locator};
use webtest_browser_cdp::ChromeHost;
use webtest_observation::ObservationStore;
use webtest_runtime::Runner;
use webtest_text::TextRange;

#[derive(Parser)]
#[command(
    name = "webtest",
    version,
    about = "Statically analyzable web application tests"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse and statically check a WebTest file.
    Check { file: PathBuf },
    /// Rewrite a WebTest file using the canonical formatter.
    Fmt { file: PathBuf },
    /// Execute a WebTest file in Chrome.
    Test {
        file: PathBuf,
        #[arg(long, env = "WEBTEST_CHROME_PATH")]
        chrome_path: Option<PathBuf>,
        /// Show the Chrome window while the test runs.
        #[arg(long)]
        headed: bool,
    },
    /// Run the language server over stdio.
    Lsp {
        #[arg(long, env = "WEBTEST_CHROME_PATH")]
        chrome_path: Option<PathBuf>,
    },
}

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

    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Check { file } => check(&file),
        Command::Fmt { file } => format(&file),
        Command::Test {
            file,
            chrome_path,
            headed,
        } => test(&file, chrome_path, headed).await,
        Command::Lsp { chrome_path } => {
            webtest_lsp::serve(Arc::new(ChromeHost::new(chrome_path))).await;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn check(path: &Path) -> Result<ExitCode> {
    let source = read(path)?;
    let (mut database, file) = database_for(path, &source);
    let diagnostics = database.diagnostics(file)?;
    if diagnostics.is_empty() {
        println!("ok");
        return Ok(ExitCode::SUCCESS);
    }
    for diagnostic in diagnostics.iter() {
        print_diagnostic(path, &source, diagnostic);
    }
    Ok(if has_errors(&diagnostics) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn format(path: &Path) -> Result<ExitCode> {
    let source = read(path)?;
    let formatted = webtest_format::format_file(&webtest_syntax::parse(&source));
    std::fs::write(path, formatted)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(ExitCode::SUCCESS)
}

async fn test(path: &Path, chrome_path: Option<PathBuf>, headed: bool) -> Result<ExitCode> {
    let source = read(path)?;
    let (mut database, file) = database_for(path, &source);
    let diagnostics = database.diagnostics(file)?;
    if has_errors(&diagnostics) {
        for diagnostic in diagnostics.iter() {
            print_diagnostic(path, &source, diagnostic);
        }
        return Ok(ExitCode::FAILURE);
    }

    let plan = database.test_plan(file)?;
    println!(
        "running {} test{}",
        plan.tests.len(),
        if plan.tests.len() == 1 { "" } else { "s" }
    );
    let observations = Arc::new(ObservationStore::default());
    let runner = Runner::new(observations);
    let browser = ChromeHost::new(chrome_path).with_headed(headed);
    let result = match runner.run(&plan, &browser).await {
        Ok(result) => result,
        Err(error) => {
            eprintln!("browser infrastructure error: {error}");
            return Ok(ExitCode::FAILURE);
        }
    };

    for test in &result.tests {
        if test.passed {
            println!("test {:?} ... ok", test.name);
        } else {
            println!("test {:?} ... FAILED", test.name);
        }
    }

    for test in &result.tests {
        let Some(failure) = &test.failure else {
            continue;
        };
        println!();
        print_source_range(path, &source, failure.step.origin.range);
        println!();
        println!("runtime error[{}]:", runtime_code(&failure.error));
        println!("    {}", runtime_message(&failure.error));
    }

    println!();
    println!("{} passed; {} failed", result.passed(), result.failed());
    Ok(if result.failed() == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn database_for(path: &Path, source: &str) -> (AnalysisDatabase, webtest_text::FileId) {
    let mut database = AnalysisDatabase::default();
    let file = database.open_file(path.display().to_string(), source);
    (database, file)
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))
}

fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
}

fn print_diagnostic(path: &Path, source: &str, diagnostic: &Diagnostic) {
    let (line, column, _, _) = line_details(source, diagnostic.range);
    eprintln!(
        "{}:{}:{}: error[{}]",
        path.display(),
        line + 1,
        column + 1,
        diagnostic.code
    );
    eprintln!("    {}", diagnostic.message);
}

fn print_source_range(path: &Path, source: &str, range: TextRange) {
    let (line, column, line_text, underline_width) = line_details(source, range);
    println!("{}:{}:{}", path.display(), line + 1, column + 1);
    println!("    {line_text}");
    println!(
        "    {}{}",
        " ".repeat(column),
        "^".repeat(underline_width.max(1))
    );
}

fn line_details(source: &str, range: TextRange) -> (usize, usize, &str, usize) {
    let requested_start = u32::from(range.start()) as usize;
    let requested_end = u32::from(range.end()) as usize;
    let start = floor_char_boundary(source, requested_start.min(source.len()));
    let end = floor_char_boundary(source, requested_end.min(source.len()));
    let line_start = source[..start].rfind('\n').map_or(0, |offset| offset + 1);
    let line_end = source[start..]
        .find('\n')
        .map_or(source.len(), |offset| start + offset);
    let line = source[..line_start]
        .chars()
        .filter(|character| *character == '\n')
        .count();
    let column = source[line_start..start].chars().count();
    let underline_end = end.min(line_end);
    let width = source[start..underline_end].chars().count();
    (line, column, &source[line_start..line_end], width)
}

fn floor_char_boundary(source: &str, mut offset: usize) -> usize {
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn runtime_code(error: &BrowserError) -> &'static str {
    match error {
        BrowserError::LocatorNotFound { .. } => "runtime.locator_not_found",
        BrowserError::LocatorAmbiguous { .. } => "runtime.locator_ambiguous",
        BrowserError::LocatorNotVisible { .. } => "runtime.locator_not_visible",
        BrowserError::NavigationFailed { .. } => "runtime.navigation_failed",
        BrowserError::BrowserDisconnected => "runtime.browser_disconnected",
        BrowserError::Protocol { .. } => "runtime.browser_protocol",
        BrowserError::Launch(_) => "runtime.browser_launch",
    }
}

fn runtime_message(error: &BrowserError) -> String {
    match error {
        BrowserError::LocatorNotFound { locator } => {
            format!(
                "No element with {} was found.",
                locator_description(locator)
            )
        }
        BrowserError::LocatorNotVisible { locator } => format!(
            "The element with {} was not visible.",
            locator_description(locator)
        ),
        _ => error.to_string(),
    }
}

fn locator_description(locator: &Locator) -> String {
    match locator {
        Locator::Id(value) => format!("id {value:?}"),
        Locator::Text(value) => format!("text {value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_locations_handle_unicode() {
        let source = "😀\nclick id(\"x\")";
        let start = source.find("id").expect("locator");
        let range = TextRange::new(
            webtest_text::TextSize::from(start as u32),
            webtest_text::TextSize::from((start + 7) as u32),
        );
        let (line, column, text, width) = line_details(source, range);
        assert_eq!((line, column, text, width), (1, 6, "click id(\"x\")", 7));
    }

    #[test]
    fn test_command_accepts_headed_mode() {
        let cli = Cli::try_parse_from(["webtest", "test", "example.webtest", "--headed"])
            .expect("parse headed test command");
        assert!(matches!(cli.command, Command::Test { headed: true, .. }));
    }
}
