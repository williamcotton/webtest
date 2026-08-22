mod report;

use std::{
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Instant,
};

use clap::{Parser, Subcommand, ValueEnum};
use report::{
    CommandReport, DiagnosticReport, EventReport, ExitClass, FailureReport, FileReport, Reporter,
    SourceSpanReport, TestReport, WarningReport,
};
use webtest_analysis::{AnalysisDatabase, Diagnostic, DiagnosticSeverity};
use webtest_browser::{BrowserContextOptions, BrowserError, Locator, Viewport};
use webtest_browser_cdp::{ChromeHost, find_system_chrome};
use webtest_browser_manager::{BrowserManager, BrowserManagerError};
use webtest_observation::{ExecutionEvent, ObservationStore, RuntimeFailure};
use webtest_plan::{AssertionOperation, BrowserOperation, TestOperation, TestPlan};
use webtest_project::{DiscoveredFile, Project};
use webtest_runtime::{EvidenceOptions, Runner, RunnerOptions};
use webtest_text::{SourceRevision, TextRange};

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
    /// Parse and statically check WebTest files.
    Check {
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = CheckReporter::Human)]
        reporter: CheckReporter,
    },
    /// Rewrite WebTest files using the canonical formatter.
    Fmt {
        paths: Vec<PathBuf>,
        /// Report files that differ without rewriting them.
        #[arg(long)]
        check: bool,
    },
    /// Execute WebTest files in Chrome.
    Test {
        paths: Vec<PathBuf>,
        #[arg(long)]
        chrome_path: Option<PathBuf>,
        /// Show the Chrome window while tests run.
        #[arg(long)]
        headed: bool,
        #[arg(long, value_enum, default_value_t = TestReporter::Human)]
        reporter: TestReporter,
    },
    /// Install and inspect managed Chrome for Testing versions.
    Browser {
        #[command(subcommand)]
        command: BrowserCommand,
    },
    /// Run the language server over stdio.
    Lsp {
        #[arg(long)]
        chrome_path: Option<PathBuf>,
    },
    /// Run the debug adapter protocol server over stdio.
    Dap {
        #[arg(long)]
        chrome_path: Option<PathBuf>,
        /// Hide Chrome while debugging. Debug sessions are headed by default.
        #[arg(long)]
        headless: bool,
    },
}

#[derive(Subcommand)]
enum BrowserCommand {
    /// Download and atomically install the tested Chrome for Testing version.
    Install {
        #[arg(long)]
        version: Option<String>,
    },
    /// List valid managed installations.
    List,
    /// Print the Chrome executable selected by normal resolution precedence.
    Path,
    /// Remove validated WebTest-owned managed installations.
    Clean {
        #[arg(long)]
        version: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CheckReporter {
    Human,
    Concise,
    Json,
}

impl From<CheckReporter> for Reporter {
    fn from(value: CheckReporter) -> Self {
        match value {
            CheckReporter::Human => Self::Human,
            CheckReporter::Concise => Self::Concise,
            CheckReporter::Json => Self::Json,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TestReporter {
    Human,
    Concise,
    Json,
    Junit,
    Events,
}

impl From<TestReporter> for Reporter {
    fn from(value: TestReporter) -> Self {
        match value {
            TestReporter::Human => Self::Human,
            TestReporter::Concise => Self::Concise,
            TestReporter::Json => Self::Json,
            TestReporter::Junit => Self::Junit,
            TestReporter::Events => Self::Events,
        }
    }
}

#[derive(Debug)]
struct AppError {
    class: ExitClass,
    message: String,
}

impl AppError {
    fn usage(error: impl fmt::Display) -> Self {
        Self {
            class: ExitClass::Usage,
            message: error.to_string(),
        }
    }

    fn infrastructure(error: impl fmt::Display) -> Self {
        Self {
            class: ExitClass::Infrastructure,
            message: error.to_string(),
        }
    }

    fn internal(error: impl fmt::Display) -> Self {
        Self {
            class: ExitClass::Internal,
            message: error.to_string(),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
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

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    match run(cli).await {
        Ok(class) => ExitCode::from(class.code()),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.class.code())
        }
    }
}

async fn run(cli: Cli) -> Result<ExitClass, AppError> {
    match cli.command {
        Command::Check { paths, reporter } => {
            let project = project(&paths)?;
            let report = check_project(&project)?;
            write_report(&report, reporter.into())?;
            Ok(report.exit_class)
        }
        Command::Fmt { paths, check } => format_project(&project(&paths)?, check),
        Command::Test {
            paths,
            chrome_path,
            headed,
            reporter,
        } => {
            let project = project(&paths)?;
            let report = test_project(&project, chrome_path, headed).await?;
            write_report(&report, reporter.into())?;
            Ok(report.exit_class)
        }
        Command::Browser { command } => browser_command(command),
        Command::Lsp { chrome_path } => {
            let project = project(&[])?;
            let executable = resolve_chrome(&project, chrome_path)
                .ok()
                .map(|resolved| resolved.path);
            let browser = ChromeHost::new(executable).with_timeouts(
                project.config.timeouts.browser_command,
                project.config.timeouts.navigation,
            );
            let editor = Arc::new(webtest_editor::EditorService::with_runner_options(
                runner_options(&project),
            ));
            webtest_lsp::serve_with_editor(Arc::new(browser), editor).await;
            Ok(ExitClass::Success)
        }
        Command::Dap {
            chrome_path,
            headless,
        } => {
            let project = project(&[])?;
            let executable = resolve_chrome(&project, chrome_path)
                .ok()
                .map(|resolved| resolved.path);
            let browser = ChromeHost::new(executable)
                .with_headed(!headless)
                .with_timeouts(
                    project.config.timeouts.browser_command,
                    project.config.timeouts.navigation,
                );
            webtest_dap::serve_with_options(Arc::new(browser), runner_options(&project))
                .await
                .map_err(AppError::infrastructure)?;
            Ok(ExitClass::Success)
        }
    }
}

fn project(paths: &[PathBuf]) -> Result<Project, AppError> {
    webtest_project::discover(paths).map_err(AppError::usage)
}

fn check_project(project: &Project) -> Result<CommandReport, AppError> {
    let mut report = base_report("check", project);
    for file in &project.files {
        let started = Instant::now();
        let source = read_source(&file.path)?;
        let (mut database, file_id) = database_for(&file.path, &source);
        let diagnostics = database.diagnostics(file_id).map_err(AppError::internal)?;
        let mut diagnostics = diagnostics
            .iter()
            .map(|diagnostic| diagnostic_report(&source, diagnostic))
            .collect::<Vec<_>>();
        let plan = database.test_plan(file_id).map_err(AppError::internal)?;
        diagnostics.extend(config_diagnostics(project, &source, &plan));
        if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error")
        {
            report.exit_class = report.exit_class.combine(ExitClass::TestFailure);
        }
        let file_exit_class = if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error")
        {
            ExitClass::TestFailure
        } else {
            ExitClass::Success
        };
        report.files.push(FileReport {
            path: display_path(file),
            exit_class: file_exit_class,
            source_revision: revision_hex(SourceRevision::of(&source)),
            duration_nanos: nanos(started.elapsed()),
            diagnostics,
            tests: Vec::new(),
            infrastructure_error: None,
            events: Vec::new(),
        });
    }
    report.finish();
    Ok(report)
}

fn format_project(project: &Project, check: bool) -> Result<ExitClass, AppError> {
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

async fn test_project(
    project: &Project,
    chrome_path: Option<PathBuf>,
    headed: bool,
) -> Result<CommandReport, AppError> {
    let mut report = base_report("test", project);
    let show_browser = headed || !project.config.browser.headless;
    let mut browser = None;

    for file in &project.files {
        let started = Instant::now();
        let source = read_source(&file.path)?;
        let revision = SourceRevision::of(&source);
        let (mut database, file_id) = database_for(&file.path, &source);
        let diagnostics = database.diagnostics(file_id).map_err(AppError::internal)?;
        let mut diagnostic_reports = diagnostics
            .iter()
            .map(|diagnostic| diagnostic_report(&source, diagnostic))
            .collect::<Vec<_>>();
        let plan = database.test_plan(file_id).map_err(AppError::internal)?;
        diagnostic_reports.extend(config_diagnostics(project, &source, &plan));
        if diagnostic_reports
            .iter()
            .any(|diagnostic| diagnostic.severity == "error")
        {
            report.exit_class = report.exit_class.combine(ExitClass::TestFailure);
            report.files.push(FileReport {
                path: display_path(file),
                exit_class: ExitClass::TestFailure,
                source_revision: revision_hex(revision),
                duration_nanos: nanos(started.elapsed()),
                diagnostics: diagnostic_reports,
                tests: Vec::new(),
                infrastructure_error: None,
                events: Vec::new(),
            });
            continue;
        }

        let observations = Arc::new(ObservationStore::default());
        let runner = Runner::new(observations).with_options(runner_options(project));
        let mut file_report = FileReport {
            path: display_path(file),
            exit_class: ExitClass::Success,
            source_revision: revision_hex(revision),
            duration_nanos: 0,
            diagnostics: diagnostic_reports,
            tests: Vec::new(),
            infrastructure_error: None,
            events: Vec::new(),
        };
        if browser.is_none() {
            match resolve_chrome(project, chrome_path.clone()) {
                Ok(resolved) => {
                    browser = Some(
                        ChromeHost::new(Some(resolved.path))
                            .with_headed(show_browser)
                            .with_timeouts(
                                project.config.timeouts.browser_command,
                                project.config.timeouts.navigation,
                            ),
                    );
                }
                Err(error) => {
                    file_report.infrastructure_error = Some(FailureReport {
                        code: "runtime.browser_launch".into(),
                        message: error.message,
                        span: None,
                        artifacts: Vec::new(),
                    });
                    file_report.duration_nanos = nanos(started.elapsed());
                    report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                    file_report.exit_class = ExitClass::Infrastructure;
                    report.files.push(file_report);
                    continue;
                }
            }
        }
        let Some(browser) = browser.as_ref() else {
            return Err(AppError::internal(
                "browser resolution completed without a browser host",
            ));
        };
        let run =
            tokio::time::timeout(project.config.timeouts.test, runner.run(&plan, browser)).await;
        match run {
            Err(_) => {
                file_report.infrastructure_error = Some(FailureReport {
                    code: "runtime.test_timeout".into(),
                    message: format!(
                        "test file exceeded its {}ms timeout",
                        project.config.timeouts.test.as_millis()
                    ),
                    span: None,
                    artifacts: Vec::new(),
                });
                report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                file_report.exit_class = ExitClass::Infrastructure;
            }
            Ok(Err(error)) => {
                file_report.infrastructure_error = Some(FailureReport {
                    code: runtime_code(&error).into(),
                    message: infrastructure_message(&error),
                    span: None,
                    artifacts: Vec::new(),
                });
                report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                file_report.exit_class = ExitClass::Infrastructure;
            }
            Ok(Ok(result)) => {
                file_report.duration_nanos = nanos(result.duration);
                file_report.events = event_reports(&file_report.path, &result.events);
                file_report.tests = result
                    .tests
                    .into_iter()
                    .map(|test| {
                        let failure = test.failure.map(|failure| FailureReport {
                            code: runtime_code(&failure.error).into(),
                            message: runtime_message(&failure.error),
                            span: Some(source_span(&source, failure.step.origin.range)),
                            artifacts: failure
                                .artifacts
                                .into_iter()
                                .map(|artifact| normalized_path(&artifact.path))
                                .collect(),
                        });
                        TestReport {
                            name: test.name,
                            exit_class: if test.passed {
                                ExitClass::Success
                            } else {
                                ExitClass::TestFailure
                            },
                            passed: test.passed,
                            duration_nanos: nanos(test.duration),
                            failure,
                        }
                    })
                    .collect();
                if file_report.tests.iter().any(|test| !test.passed) {
                    report.exit_class = report.exit_class.combine(ExitClass::TestFailure);
                    file_report.exit_class = ExitClass::TestFailure;
                }
            }
        }
        if file_report.duration_nanos == 0 {
            file_report.duration_nanos = nanos(started.elapsed());
        }
        report.files.push(file_report);
    }
    report.finish();
    Ok(report)
}

fn base_report(command: &str, project: &Project) -> CommandReport {
    let mut report = CommandReport::new(command, normalized_path(&project.root));
    report.warnings = project
        .warnings
        .iter()
        .map(|warning| WarningReport {
            code: "config.unknown".into(),
            key: warning.key.clone(),
            message: warning.message.clone(),
        })
        .collect();
    report
}

fn runner_options(project: &Project) -> RunnerOptions {
    RunnerOptions {
        base_url: project.config.browser.base_url.clone(),
        action_timeout: project.config.timeouts.action,
        assertion_timeout: project.config.timeouts.assertion,
        test_timeout: project.config.timeouts.test,
        browser_context: BrowserContextOptions {
            viewport: Viewport {
                width: project.config.browser.viewport.width,
                height: project.config.browser.viewport.height,
            },
            test_id_attribute: project.config.browser.test_id_attribute.clone(),
        },
        evidence: EvidenceOptions {
            screenshot_on_failure: project.config.evidence.screenshot
                == webtest_project::EvidenceMode::OnFailure,
            dom_snapshot_on_failure: project.config.evidence.dom_snapshot
                == webtest_project::EvidenceMode::OnFailure,
            max_dom_bytes: project.config.evidence.max_dom_bytes,
            artifact_directory: project.root.join(&project.config.artifacts.directory),
        },
    }
}

#[derive(Clone, Copy, Debug)]
enum ChromeProvenance {
    Cli,
    Environment,
    Configuration,
    Managed,
    System,
}

impl fmt::Display for ChromeProvenance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cli => "command line",
            Self::Environment => "WEBTEST_CHROME_PATH",
            Self::Configuration => "webtest.toml",
            Self::Managed => "managed Chrome for Testing",
            Self::System => "system discovery",
        })
    }
}

#[derive(Debug)]
struct ResolvedChrome {
    path: PathBuf,
    provenance: ChromeProvenance,
}

fn resolve_chrome(
    project: &Project,
    explicit: Option<PathBuf>,
) -> Result<ResolvedChrome, AppError> {
    if let Some(path) = explicit {
        return resolved_existing(path, ChromeProvenance::Cli);
    }
    if let Some(path) = std::env::var_os("WEBTEST_CHROME_PATH") {
        return resolved_existing(PathBuf::from(path), ChromeProvenance::Environment);
    }
    if let Some(path) = &project.config.browser.path {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            project.root.join(path)
        };
        return resolved_existing(path, ChromeProvenance::Configuration);
    }
    if project.config.browser.channel == webtest_project::BrowserChannel::Managed
        && let Some(installed) = BrowserManager::new()
            .map_err(AppError::infrastructure)?
            .current()
            .map_err(AppError::infrastructure)?
    {
        return Ok(ResolvedChrome {
            path: installed.executable,
            provenance: ChromeProvenance::Managed,
        });
    }
    if let Some(path) = find_system_chrome() {
        return Ok(ResolvedChrome {
            path,
            provenance: ChromeProvenance::System,
        });
    }
    Err(AppError::infrastructure(
        "Chrome was not found; run `webtest browser install`, set WEBTEST_CHROME_PATH, or configure browser.path",
    ))
}

fn resolved_existing(
    path: PathBuf,
    provenance: ChromeProvenance,
) -> Result<ResolvedChrome, AppError> {
    if !path.is_file() {
        return Err(AppError::infrastructure(format!(
            "Chrome selected from {provenance} does not exist at {}",
            path.display()
        )));
    }
    let path = std::fs::canonicalize(&path).map_err(|error| {
        AppError::infrastructure(format!(
            "could not resolve Chrome selected from {provenance} at {}: {error}",
            path.display()
        ))
    })?;
    Ok(ResolvedChrome { path, provenance })
}

fn browser_command(command: BrowserCommand) -> Result<ExitClass, AppError> {
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

fn browser_manager_error(error: BrowserManagerError) -> AppError {
    match error {
        BrowserManagerError::UnsupportedVersion { .. } => AppError::usage(error),
        _ => AppError::infrastructure(error),
    }
}

fn write_report(report: &CommandReport, reporter: Reporter) -> Result<(), AppError> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    report
        .write(reporter, &mut output)
        .map_err(AppError::infrastructure)
}

fn database_for(path: &Path, source: &str) -> (AnalysisDatabase, webtest_text::FileId) {
    let mut database = AnalysisDatabase::default();
    let file = database.open_file(path.display().to_string(), source);
    (database, file)
}

fn read_source(path: &Path) -> Result<String, AppError> {
    std::fs::read_to_string(path)
        .map_err(|error| AppError::usage(format!("could not read {}: {error}", path.display())))
}

fn diagnostic_report(source: &str, diagnostic: &Diagnostic) -> DiagnosticReport {
    DiagnosticReport {
        severity: match diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Information => "information",
            DiagnosticSeverity::Hint => "hint",
        }
        .into(),
        code: diagnostic.code.into(),
        message: diagnostic.message.clone(),
        span: source_span(source, diagnostic.range),
    }
}

fn config_diagnostics(project: &Project, source: &str, plan: &TestPlan) -> Vec<DiagnosticReport> {
    let mut diagnostics = Vec::new();
    for test in &plan.tests {
        for step in &test.steps {
            let (url, timeout) = match &step.operation {
                TestOperation::Browser(BrowserOperation::Navigate { url }) => {
                    (Some(url.as_str()), None)
                }
                TestOperation::Browser(BrowserOperation::WaitForUrl { url, timeout })
                | TestOperation::Assertion(AssertionOperation::Url { url, timeout }) => {
                    (Some(url.as_str()), *timeout)
                }
                TestOperation::Browser(BrowserOperation::WaitForLocator { timeout, .. })
                | TestOperation::Assertion(AssertionOperation::Locator { timeout, .. }) => {
                    (None, *timeout)
                }
                _ => (None, None),
            };
            if let Some(url) = url
                && !is_absolute_config_url(url)
                && project.config.browser.base_url.is_none()
            {
                diagnostics.push(DiagnosticReport {
                    severity: "error".into(),
                    code: "config.missing_base_url".into(),
                    message: format!("relative URL {url:?} requires browser.base_url"),
                    span: source_span(source, step.origin.range),
                });
            }
            if timeout.is_some_and(|timeout| timeout > project.config.timeouts.test) {
                diagnostics.push(DiagnosticReport {
                    severity: "error".into(),
                    code: "config.timeout_exceeds_test".into(),
                    message: "step timeout must not exceed timeouts.test".into(),
                    span: source_span(source, step.origin.range),
                });
            }
        }
    }
    diagnostics
}

fn is_absolute_config_url(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && !rest.is_empty()
            && scheme.chars().next().is_some_and(char::is_alphabetic)
            && scheme.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            })
    })
}

fn source_span(source: &str, range: TextRange) -> SourceSpanReport {
    let (line, column, line_text, width) = line_details(source, range);
    SourceSpanReport {
        line: line + 1,
        column: column + 1,
        source_line: line_text.into(),
        underline_start: column,
        underline_width: width.max(1),
    }
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
        BrowserError::LocatorInvalid { .. } => "runtime.locator_invalid",
        BrowserError::ElementDetached { .. } => "runtime.element_detached",
        BrowserError::LocatorNotVisible { .. } => "runtime.locator_not_visible",
        BrowserError::ElementUnstable { .. } => "runtime.element_unstable",
        BrowserError::ElementDisabled { .. } => "runtime.element_disabled",
        BrowserError::ElementObscured { .. } => "runtime.element_obscured",
        BrowserError::ElementNotEditable { .. } => "runtime.element_not_editable",
        BrowserError::OptionNotFound { .. } => "runtime.option_not_found",
        BrowserError::OptionAmbiguous { .. } => "runtime.option_ambiguous",
        BrowserError::InvalidKey { .. } => "runtime.invalid_key",
        BrowserError::ActionTimeout { .. } => "runtime.action_timeout",
        BrowserError::AssertionFailed { .. } => "runtime.assertion_failed",
        BrowserError::UrlMismatch { .. } => "runtime.url_mismatch",
        BrowserError::NavigationFailed { .. } => "runtime.navigation_failed",
        BrowserError::NavigationTimeout { .. } => "runtime.navigation_timeout",
        BrowserError::CommandTimeout { .. } => "runtime.browser_command_timeout",
        BrowserError::BrowserDisconnected => "runtime.browser_disconnected",
        BrowserError::BrowserCrashed { .. } => "runtime.browser_crashed",
        BrowserError::MalformedProtocol { .. } => "runtime.browser_malformed_protocol",
        BrowserError::Protocol { .. } => "runtime.browser_protocol",
        BrowserError::Launch(_) => "runtime.browser_launch",
        BrowserError::EvaluationFailed { .. } => "runtime.evaluation_failed",
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

fn infrastructure_message(error: &BrowserError) -> String {
    match error {
        BrowserError::Launch(message) if message.contains("Chrome was not found") => format!(
            "{message}. Install the managed browser with `webtest browser install` or configure an explicit path"
        ),
        _ => error.to_string(),
    }
}

fn locator_description(locator: &Locator) -> String {
    locator.to_string()
}

fn event_reports(path: &str, events: &[ExecutionEvent]) -> Vec<EventReport> {
    events
        .iter()
        .map(|event| match event {
            ExecutionEvent::RunStarted { execution_id } => {
                event_report(path, "run_started", Some(execution_id.0), None, None)
            }
            ExecutionEvent::TestStarted {
                execution_id,
                test_id,
                name,
            } => {
                let mut event = event_report(
                    path,
                    "test_started",
                    Some(execution_id.0),
                    Some(test_id.0),
                    None,
                );
                event.name = Some(name.clone());
                event
            }
            ExecutionEvent::StepStarted {
                execution_id,
                test_id,
                step_id,
            } => event_report(
                path,
                "step_started",
                Some(execution_id.0),
                Some(test_id.0),
                Some(step_id.0),
            ),
            ExecutionEvent::StepPassed {
                execution_id,
                test_id,
                step_id,
            } => event_report(
                path,
                "step_passed",
                Some(execution_id.0),
                Some(test_id.0),
                Some(step_id.0),
            ),
            ExecutionEvent::StepFailed {
                execution_id,
                test_id,
                step_id,
                failure,
            } => {
                let RuntimeFailure::Browser(error) = failure;
                let mut event = event_report(
                    path,
                    "step_failed",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                event.code = Some(runtime_code(error).into());
                event.message = Some(runtime_message(error));
                event
            }
            ExecutionEvent::TestFinished {
                execution_id,
                test_id,
                passed,
            } => {
                let mut event = event_report(
                    path,
                    "test_finished",
                    Some(execution_id.0),
                    Some(test_id.0),
                    None,
                );
                event.passed = Some(*passed);
                event.exit_class = Some(if *passed {
                    ExitClass::Success
                } else {
                    ExitClass::TestFailure
                });
                event
            }
            ExecutionEvent::RunFinished { execution_id } => {
                event_report(path, "run_finished", Some(execution_id.0), None, None)
            }
        })
        .collect()
}

fn event_report(
    path: &str,
    kind: &str,
    execution_id: Option<u64>,
    test_id: Option<u32>,
    step_id: Option<u32>,
) -> EventReport {
    EventReport {
        schema_version: 1,
        kind: kind.into(),
        file: path.into(),
        execution_id,
        test_id,
        step_id,
        name: None,
        passed: None,
        exit_class: None,
        code: None,
        message: None,
    }
}

fn display_path(file: &DiscoveredFile) -> String {
    normalized_path(&file.display_path)
}

fn normalized_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        path.into_owned()
    } else {
        path.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

fn revision_hex(revision: SourceRevision) -> String {
    revision
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn nanos(duration: std::time::Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
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
    fn commands_accept_multiple_paths_and_reporters() {
        let cli = Cli::try_parse_from([
            "webtest",
            "test",
            "a.webtest",
            "tests",
            "--headed",
            "--reporter",
            "junit",
        ])
        .expect("parse test command");
        assert!(matches!(
            cli.command,
            Command::Test {
                paths,
                headed: true,
                reporter: TestReporter::Junit,
                ..
            } if paths.len() == 2
        ));
    }

    #[test]
    fn dap_is_headed_by_default_and_accepts_headless_mode() {
        let headed = Cli::try_parse_from(["webtest", "dap"]).expect("parse headed DAP command");
        assert!(matches!(
            headed.command,
            Command::Dap {
                headless: false,
                ..
            }
        ));

        let headless =
            Cli::try_parse_from(["webtest", "dap", "--headless"]).expect("parse headless DAP");
        assert!(matches!(
            headless.command,
            Command::Dap { headless: true, .. }
        ));
    }
}
