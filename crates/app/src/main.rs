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
use webtest_plan::{
    AssertionOperation, BrowserOperation, PLAN_FORMAT_VERSION, PlanEnvelope, PlanExpr,
    PlanSourceFile, TestOperation, TestPlan,
};
use webtest_project::{DiscoveredFile, Project};
use webtest_provider::{
    Capability, FsProviderConfig, HttpProviderConfig, NativeProviderConfig, ProcessProviderConfig,
};
use webtest_runtime::{EvidenceOptions, RunError, Runner, RunnerOptions, StepError};
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
    /// Analyze WebTest files and emit a versioned execution plan.
    Build {
        paths: Vec<PathBuf>,
        /// Destination for the serialized plan envelope.
        #[arg(long)]
        emit: PathBuf,
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
        Command::Build { paths, emit } => build_project(&project(&paths)?, &emit),
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

fn build_project(project: &Project, emit: &Path) -> Result<ExitClass, AppError> {
    let report = check_project(project)?;
    if report.exit_class != ExitClass::Success {
        write_report(&report, Reporter::Human)?;
        return Ok(report.exit_class);
    }

    let mut database = AnalysisDatabase::default();
    let mut opened = Vec::new();
    for file in &project.files {
        let source = read_source(&file.path)?;
        let file_id = database.open_file(file.path.display().to_string(), source);
        opened.push((file, file_id));
    }
    let mut source_files = Vec::new();
    let mut tests = Vec::new();
    let mut capabilities = std::collections::BTreeSet::new();
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

fn reject_literal_secrets(envelope: &PlanEnvelope, project: &Project) -> Result<(), AppError> {
    for test in &envelope.tests {
        let mut bindings = std::collections::HashMap::new();
        for step in &test.steps {
            if let TestOperation::EvaluatePure(operation) = &step.operation
                && let Some(binding) = operation.result_binding
            {
                bindings.insert(binding, &operation.expression);
            }
            let TestOperation::ServerProviderCall(call) = &step.operation else {
                continue;
            };
            for argument in &call.redacted_arguments {
                if call
                    .arguments
                    .get(argument)
                    .is_some_and(|value| has_literal_value(value, &bindings))
                {
                    return Err(secret_plan_error(&call.provider, &call.operation, argument));
                }
            }
            if call.provider == "http" {
                if call.arguments.get("json").is_some_and(|value| {
                    has_sensitive_record_literal(
                        value,
                        &project.config.redaction.json_fields,
                        &bindings,
                    )
                }) {
                    return Err(secret_plan_error(&call.provider, &call.operation, "json"));
                }
                if call.arguments.get("headers").is_some_and(|value| {
                    has_sensitive_record_literal(
                        value,
                        &project.config.redaction.headers,
                        &bindings,
                    )
                }) {
                    return Err(secret_plan_error(
                        &call.provider,
                        &call.operation,
                        "headers",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn secret_plan_error(provider: &str, operation: &str, argument: &str) -> AppError {
    AppError::usage(format!(
        "cannot emit a plan containing a literal secret in `{provider}.{operation}` argument `{argument}`; use a late-bound secret source"
    ))
}

fn has_literal_value(
    expression: &PlanExpr,
    bindings: &std::collections::HashMap<webtest_hir::BindingId, &PlanExpr>,
) -> bool {
    has_literal_value_inner(expression, bindings, &mut std::collections::HashSet::new())
}

fn has_literal_value_inner(
    expression: &PlanExpr,
    bindings: &std::collections::HashMap<webtest_hir::BindingId, &PlanExpr>,
    visiting: &mut std::collections::HashSet<webtest_hir::BindingId>,
) -> bool {
    match expression {
        PlanExpr::Literal(_) => true,
        PlanExpr::Binding(binding) => {
            visiting.insert(*binding)
                && bindings
                    .get(binding)
                    .is_some_and(|value| has_literal_value_inner(value, bindings, visiting))
        }
        PlanExpr::List(values) => values
            .iter()
            .any(|value| has_literal_value_inner(value, bindings, visiting)),
        PlanExpr::Record(values) => values
            .values()
            .any(|value| has_literal_value_inner(value, bindings, visiting)),
        PlanExpr::Member { receiver, .. }
        | PlanExpr::Unary {
            operand: receiver, ..
        }
        | PlanExpr::Decode {
            value: receiver, ..
        } => has_literal_value_inner(receiver, bindings, visiting),
        PlanExpr::Binary { left, right, .. } => {
            has_literal_value_inner(left, bindings, visiting)
                || has_literal_value_inner(right, bindings, visiting)
        }
        PlanExpr::Type(_) => false,
    }
}

fn has_sensitive_record_literal(
    expression: &PlanExpr,
    sensitive_fields: &[String],
    bindings: &std::collections::HashMap<webtest_hir::BindingId, &PlanExpr>,
) -> bool {
    has_sensitive_record_literal_inner(
        expression,
        sensitive_fields,
        bindings,
        &mut std::collections::HashSet::new(),
    )
}

fn has_sensitive_record_literal_inner(
    expression: &PlanExpr,
    sensitive_fields: &[String],
    bindings: &std::collections::HashMap<webtest_hir::BindingId, &PlanExpr>,
    visiting: &mut std::collections::HashSet<webtest_hir::BindingId>,
) -> bool {
    match expression {
        PlanExpr::Binding(binding) => {
            visiting.insert(*binding)
                && bindings.get(binding).is_some_and(|value| {
                    has_sensitive_record_literal_inner(value, sensitive_fields, bindings, visiting)
                })
        }
        PlanExpr::Record(values) => values.iter().any(|(name, value)| {
            (sensitive_fields
                .iter()
                .any(|field| field.eq_ignore_ascii_case(name))
                && has_literal_value(value, bindings))
                || has_sensitive_record_literal_inner(value, sensitive_fields, bindings, visiting)
        }),
        PlanExpr::List(values) => values.iter().any(|value| {
            has_sensitive_record_literal_inner(value, sensitive_fields, bindings, visiting)
        }),
        PlanExpr::Member { receiver, .. }
        | PlanExpr::Unary {
            operand: receiver, ..
        }
        | PlanExpr::Decode {
            value: receiver, ..
        } => has_sensitive_record_literal_inner(receiver, sensitive_fields, bindings, visiting),
        PlanExpr::Binary { left, right, .. } => {
            has_sensitive_record_literal_inner(left, sensitive_fields, bindings, visiting)
                || has_sensitive_record_literal_inner(right, sensitive_fields, bindings, visiting)
        }
        PlanExpr::Literal(_) | PlanExpr::Type(_) => false,
    }
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
        let needs_browser = plan
            .required_host_capabilities
            .contains(&Capability::Browser);
        if needs_browser && browser.is_none() {
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
                        diff: None,
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
        let inactive_browser = ChromeHost::new(None);
        let browser = browser.as_ref().unwrap_or(&inactive_browser);
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
                    diff: None,
                    artifacts: Vec::new(),
                });
                report.exit_class = report.exit_class.combine(ExitClass::Infrastructure);
                file_report.exit_class = ExitClass::Infrastructure;
            }
            Ok(Err(error)) => {
                file_report.infrastructure_error = Some(FailureReport {
                    code: run_error_code(&error),
                    message: run_error_message(&error),
                    span: None,
                    diff: None,
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
                            code: step_error_code(&failure.error),
                            message: failure.error.to_string(),
                            span: Some(source_span(&source, failure.step.origin.range)),
                            diff: match &failure.error {
                                StepError::Assertion(error) => Some(error.diff.clone()),
                                _ => None,
                            },
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
        project_root: project.root.clone(),
        redacted_json_fields: project
            .config
            .redaction
            .json_fields
            .iter()
            .chain(&project.config.redaction.headers)
            .cloned()
            .collect(),
        provider_config: NativeProviderConfig {
            http: HttpProviderConfig {
                base_url: project.config.server.base_url.clone(),
                follow_redirects: project.config.server.http.follow_redirects,
                max_response_bytes: project.config.server.http.max_response_bytes,
            },
            process: ProcessProviderConfig {
                allowed_working_roots: project.config.server.process.allowed_working_roots.clone(),
                max_output_bytes: project.config.server.process.max_output_bytes,
            },
            fs: FsProviderConfig {
                read_roots: project.config.server.fs.read_roots.clone(),
                write_root: project.config.server.fs.write_root.clone(),
            },
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
                    (literal_string(url), None)
                }
                TestOperation::Browser(BrowserOperation::WaitForUrl { url, timeout })
                | TestOperation::Assertion(AssertionOperation::Url { url, timeout }) => {
                    (literal_string(url), *timeout)
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

fn literal_string(expression: &webtest_plan::PlanExpr) -> Option<&str> {
    match expression {
        webtest_plan::PlanExpr::Literal(webtest_provider::Value::String(value)) => Some(value),
        _ => None,
    }
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

fn run_error_code(error: &RunError) -> String {
    match error {
        RunError::Browser(error) => runtime_code(error).into(),
        RunError::Provider(error) => format!("runtime.{}", error.code()),
        RunError::Internal(_) => "runtime.internal_error".into(),
    }
}

fn run_error_message(error: &RunError) -> String {
    match error {
        RunError::Browser(error) => infrastructure_message(error),
        _ => error.to_string(),
    }
}

fn step_error_code(error: &StepError) -> String {
    match error {
        StepError::Browser(error) => runtime_code(error).into(),
        _ => format!("runtime.{}", error.code()),
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
            ExecutionEvent::ProviderCallStarted {
                execution_id,
                test_id,
                step_id,
                provider,
                operation,
            } => {
                let mut event = event_report(
                    path,
                    "provider_call_started",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                event.name = Some(format!("{provider}.{operation}"));
                event
            }
            ExecutionEvent::ProviderCallFinished {
                execution_id,
                test_id,
                step_id,
                provider,
                operation,
                elapsed_ms,
            } => {
                let mut event = event_report(
                    path,
                    "provider_call_finished",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                event.name = Some(format!("{provider}.{operation}"));
                event.message = Some(format!("completed in {elapsed_ms}ms"));
                event
            }
            ExecutionEvent::ProviderCallFailed {
                execution_id,
                test_id,
                step_id,
                provider,
                operation,
                code,
                message,
                elapsed_ms,
            } => {
                let mut event = event_report(
                    path,
                    "provider_call_failed",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                event.name = Some(format!("{provider}.{operation}"));
                event.code = Some(format!("runtime.{code}"));
                event.message = Some(format!("{message} (failed after {elapsed_ms}ms)"));
                event
            }
            ExecutionEvent::StepFailed {
                execution_id,
                test_id,
                step_id,
                failure,
            } => {
                let mut event = event_report(
                    path,
                    "step_failed",
                    Some(execution_id.0),
                    Some(test_id.0),
                    Some(step_id.0),
                );
                let (code, message) = runtime_failure_report(failure);
                event.code = Some(code);
                event.message = Some(message);
                if let RuntimeFailure::Assertion { diff, .. } = failure {
                    event.diff = Some(diff.clone());
                }
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

fn runtime_failure_report(failure: &RuntimeFailure) -> (String, String) {
    match failure {
        RuntimeFailure::Browser(error) => (runtime_code(error).into(), runtime_message(error)),
        RuntimeFailure::Provider(error) => (format!("runtime.{}", error.code()), error.to_string()),
        RuntimeFailure::Assertion { message, .. } => {
            ("runtime.assertion_failed".into(), message.clone())
        }
        RuntimeFailure::Decode { message } => {
            ("runtime.json_decode_failed".into(), message.clone())
        }
        RuntimeFailure::Evaluation { code, message } => {
            (format!("runtime.{code}"), message.clone())
        }
        RuntimeFailure::Internal { message } => ("runtime.internal_error".into(), message.clone()),
    }
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
        diff: None,
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
