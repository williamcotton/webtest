//! Debug Adapter Protocol transport over the source-mapped WebTest runtime.

mod concurrent;
use concurrent::{DebugTasks, ResumeCommand};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::BufRead,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use webtest_analysis::{AnalysisDatabase, DiagnosticSeverity};
use webtest_browser::BrowserHost;
use webtest_observation::{ObservationStore, RuntimeFailureCode};
use webtest_plan::{
    AssertionOperation, BrowserOperation, PlannedStep, PlannedTest, TestOperation, TestPlan,
};
use webtest_provider::ProviderRegistry;
use webtest_runtime::{RunControl, RunOutcome, Runner, RunnerOptions, StepError, TestOutcome};
use webtest_text::TextRange;

#[cfg(test)]
const VARIABLES_REFERENCE: i64 = 1;

#[derive(Debug, Error)]
pub enum DapError {
    #[error("DAP transport error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid DAP message: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Deserialize)]
struct Request {
    seq: i64,
    #[serde(rename = "type")]
    message_type: String,
    command: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Clone)]
struct ProtocolWriter {
    output: Arc<AsyncMutex<Box<dyn AsyncWrite + Unpin + Send>>>,
    next_seq: Arc<AtomicI64>,
}

impl ProtocolWriter {
    fn new(output: impl AsyncWrite + Unpin + Send + 'static) -> Self {
        Self {
            output: Arc::new(AsyncMutex::new(Box::new(BufWriter::new(output)))),
            next_seq: Arc::new(AtomicI64::new(1)),
        }
    }

    async fn response(&self, request: &Request, body: Value) -> Result<(), DapError> {
        self.send(json!({
            "type": "response",
            "request_seq": request.seq,
            "success": true,
            "command": request.command,
            "body": body,
        }))
        .await
    }

    async fn failure(&self, request: &Request, message: &str) -> Result<(), DapError> {
        self.send(json!({
            "type": "response",
            "request_seq": request.seq,
            "success": false,
            "command": request.command,
            "message": message,
            "body": {
                "error": {
                    "id": 1,
                    "format": message,
                    "showUser": true,
                }
            }
        }))
        .await
    }

    async fn event(&self, event: &str, body: Value) -> Result<(), DapError> {
        self.send(json!({
            "type": "event",
            "event": event,
            "body": body,
        }))
        .await
    }

    async fn send(&self, mut message: Value) -> Result<(), DapError> {
        if let Some(object) = message.as_object_mut() {
            object.insert(
                "seq".into(),
                self.next_seq.fetch_add(1, Ordering::Relaxed).into(),
            );
        }
        let encoded = serde_json::to_vec(&message)?;
        let mut output = self.output.lock().await;
        output
            .write_all(format!("Content-Length: {}\r\n\r\n", encoded.len()).as_bytes())
            .await?;
        output.write_all(&encoded).await?;
        output.flush().await?;
        Ok(())
    }
}

/// Native composition supplies explicit workspace inputs, including unsaved buffers.
/// The adapter compiles them through the shared analysis database.
pub trait WorkspaceLoader: Send + Sync {
    fn load(
        &self,
        program: &Path,
        sources: &BTreeMap<PathBuf, String>,
    ) -> Result<AnalysisDatabase, String>;
}
#[derive(Clone)]
struct CapturedSource {
    path: PathBuf,
    text: Arc<str>,
    revision: webtest_text::SourceRevision,
}

#[derive(Clone)]
struct LoadedProgram {
    path: PathBuf,
    source: Arc<str>,
    sources: HashMap<webtest_text::FileId, CapturedSource>,
    plan: Arc<TestPlan>,
}

impl LoadedProgram {
    #[cfg(test)]
    fn load(path: PathBuf, source_override: Option<String>) -> Result<Self, String> {
        Self::load_with_registry(path, source_override, &ProviderRegistry::built_in_schemas())
    }

    fn load_with_registry(
        path: PathBuf,
        source_override: Option<String>,
        providers: &ProviderRegistry,
    ) -> Result<Self, String> {
        let path = normalize_path(&path);
        let source = match source_override {
            Some(source) => source,
            None => std::fs::read_to_string(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?,
        };
        let mut database = AnalysisDatabase::with_provider_registry(providers.clone());
        let file = database.open_file(path.display().to_string(), &source);
        Self::from_database(path, database, file, &[])
    }

    fn from_database(
        path: PathBuf,
        mut database: AnalysisDatabase,
        file: webtest_text::FileId,
        variants: &[webtest_model::TestVariantId],
    ) -> Result<Self, String> {
        let diagnostics = database
            .diagnostics(file)
            .map_err(|error| error.to_string())?;
        if let Some(diagnostic) = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        {
            return Err(format!(
                "{} has static error[{}]: {}",
                path.display(),
                diagnostic.code,
                diagnostic.message
            ));
        }
        let mut plan = database
            .test_plan(file)
            .map_err(|error| error.to_string())?;
        if !variants.is_empty() {
            if variants
                .iter()
                .any(|id| !plan.tests.iter().any(|test| test.variant_id == *id))
            {
                return Err(
                    "requested test variant does not belong to the launched program".into(),
                );
            }
            plan = Arc::new(plan.select_variants(&variants.iter().copied().collect()));
        }
        let mut sources = HashMap::new();
        for (captured_file, captured_revision) in plan.sources() {
            let text = database
                .source(captured_file)
                .map_err(|error| error.to_string())?;
            if webtest_text::SourceRevision::of(&text) != captured_revision {
                return Err("workspace source changed while preparing debug launch".into());
            }
            sources.insert(
                captured_file,
                CapturedSource {
                    path: normalize_path(Path::new(
                        database
                            .path(captured_file)
                            .map_err(|error| error.to_string())?,
                    )),
                    text,
                    revision: captured_revision,
                },
            );
        }
        let source = database.source(file).map_err(|error| error.to_string())?;
        Ok(Self {
            path,
            source,
            sources,
            plan,
        })
    }

    fn source_for(&self, file: webtest_text::FileId) -> Option<&CapturedSource> {
        self.sources.get(&file)
    }
    fn source_at_path(&self, path: &Path) -> Option<&CapturedSource> {
        self.sources.values().find(|source| source.path == path)
    }
    fn locations_for(&self, path: &Path) -> Vec<StepLocation> {
        self.plan
            .tests
            .iter()
            .flat_map(|test| test.steps())
            .chain(self.plan.fixtures.iter().flat_map(|fixture| {
                fixture
                    .setup
                    .steps()
                    .into_iter()
                    .chain(fixture.teardown.steps())
            }))
            .filter_map(|step| {
                let source = self.source_for(step.origin.file)?;
                (source.path == path).then(|| StepLocation::new(&source.text, step.origin.range))
            })
            .collect()
    }

    #[cfg(test)]
    fn locations(&self) -> Vec<StepLocation> {
        self.locations_for(&self.path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StepLocation {
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
}

impl StepLocation {
    fn new(source: &str, range: TextRange) -> Self {
        let (line, column) = offset_to_line_column(source, u32::from(range.start()) as usize);
        let (end_line, end_column) = offset_to_line_column(source, u32::from(range.end()) as usize);
        Self {
            line,
            column,
            end_line,
            end_column,
        }
    }
}

struct DebugState {
    writer: ProtocolWriter,
    browser: Arc<dyn BrowserHost>,
    runner_options: RunnerOptions,
    providers: ProviderRegistry,
    workspace_loader: Option<Arc<dyn WorkspaceLoader>>,
    program: Mutex<Option<LoadedProgram>>,
    breakpoints: Mutex<HashMap<PathBuf, HashSet<u32>>>,
    breakpoint_revisions: Mutex<HashMap<PathBuf, webtest_text::SourceRevision>>,
    exception_breakpoints: Mutex<HashSet<String>>,
    tasks: Mutex<DebugTasks>,
    wake: watch::Sender<u64>,
    handles: Arc<AtomicI64>,
    run_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    configured: AtomicBool,
    started: AtomicBool,
    shutting_down: AtomicBool,
    cancellation_reason: Mutex<webtest_host::CancellationReason>,
    completion: watch::Sender<bool>,
}

impl DebugState {
    fn new_with_options(
        writer: ProtocolWriter,
        browser: Arc<dyn BrowserHost>,
        runner_options: RunnerOptions,
    ) -> Arc<Self> {
        let providers = ProviderRegistry::built_in(runner_options.provider_config.clone());
        Self::new_with_configuration(writer, browser, runner_options, providers)
    }

    fn new_with_configuration(
        writer: ProtocolWriter,
        browser: Arc<dyn BrowserHost>,
        runner_options: RunnerOptions,
        providers: ProviderRegistry,
    ) -> Arc<Self> {
        Self::new_with_workspace(writer, browser, runner_options, providers, None)
    }
    fn new_with_workspace(
        writer: ProtocolWriter,
        browser: Arc<dyn BrowserHost>,
        runner_options: RunnerOptions,
        providers: ProviderRegistry,
        workspace_loader: Option<Arc<dyn WorkspaceLoader>>,
    ) -> Arc<Self> {
        let (wake, _) = watch::channel(0);
        let (completion, _) = watch::channel(false);
        Arc::new(Self {
            writer,
            browser,
            runner_options,
            providers,
            workspace_loader,
            program: Mutex::new(None),
            breakpoints: Mutex::new(HashMap::new()),
            breakpoint_revisions: Mutex::new(HashMap::new()),
            exception_breakpoints: Mutex::new(HashSet::new()),
            tasks: Mutex::new(DebugTasks::default()),
            wake,
            handles: Arc::new(AtomicI64::new(1)),
            run_task: Mutex::new(None),
            configured: AtomicBool::new(false),
            started: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            cancellation_reason: Mutex::new(webtest_host::CancellationReason::DebugDisconnect),
            completion,
        })
    }

    async fn handle(self: &Arc<Self>, request: Request) -> Result<bool, DapError> {
        if request.message_type != "request" {
            return Ok(false);
        }
        match request.command.as_str() {
            "initialize" => {
                self.writer
                    .response(
                        &request,
                        json!({
                            "supportsConfigurationDoneRequest": true,
                            "supportsTerminateRequest": true,
                            "supportsRestartRequest": true,
                            "supportsSingleThreadExecutionRequests": false,
                            "supportsPauseRequest": true,
                            "supportsStepInTargetsRequest": false,
                            "supportsEvaluateForHovers": false,
                            "exceptionBreakpointFilters": [
                                {"filter":"assertion", "label":"Assertion failures", "default":false},
                                {"filter":"provider", "label":"Provider failures", "default":false},
                                {"filter":"infrastructure", "label":"Infrastructure failures", "default":false},
                                {"filter":"internal", "label":"Internal failures", "default":false},
                                {
                                    "filter": "appProviderFailure",
                                    "label": "Application provider failures",
                                    "description": "Pause when app.* returns an application error",
                                    "default": false
                                },
                                {
                                    "filter": "appInfrastructure",
                                    "label": "Application bridge infrastructure failures",
                                    "description": "Pause on app bridge lifecycle, transport, protocol, or schema failures",
                                    "default": false
                                }
                            ],
                        }),
                    )
                    .await?;
            }
            "launch" => self.launch(&request).await?,
            "setBreakpoints" => self.set_breakpoints(&request).await?,
            "setExceptionBreakpoints" => {
                *lock(&self.exception_breakpoints) = request
                    .arguments
                    .get("filters")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                self.writer.response(&request, json!({})).await?;
            }
            "configurationDone" => {
                self.configured.store(true, Ordering::Release);
                self.writer.response(&request, json!({})).await?;
                self.start_if_ready();
            }
            "threads" => {
                let threads = lock(&self.tasks).threads();
                self.writer
                    .response(&request, json!({"threads":threads}))
                    .await?;
            }
            "stackTrace" => self.stack_trace(&request).await?,
            "scopes" => self.scopes(&request).await?,
            "variables" => self.variables(&request).await?,
            "continue" => self.resume(&request, ResumeCommand::Continue).await?,
            "next" => self.resume(&request, ResumeCommand::Over).await?,
            "stepIn" => self.resume(&request, ResumeCommand::In).await?,
            "stepOut" => self.resume(&request, ResumeCommand::Out).await?,
            "restart" => self.restart(&request).await?,
            "pause" => {
                lock(&self.tasks).pending_pause = Some("pause");
                self.writer.response(&request, json!({})).await?;
            }
            "disconnect" | "terminate" => {
                self.cancel_execution();
                self.writer.response(&request, json!({})).await?;
                return Ok(true);
            }
            _ => {
                self.writer
                    .failure(
                        &request,
                        &format!("WebTest does not support `{}`", request.command),
                    )
                    .await?;
            }
        }
        Ok(false)
    }

    async fn launch(self: &Arc<Self>, request: &Request) -> Result<(), DapError> {
        let Some(program) = request.arguments.get("program").and_then(Value::as_str) else {
            self.writer
                .failure(request, "A WebTest debug launch requires `program`")
                .await?;
            return Ok(());
        };
        let source_override = request
            .arguments
            .get("sourceText")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let path = normalize_path(Path::new(program));
        let variants = request
            .arguments
            .get("variants")
            .cloned()
            .map(serde_json::from_value::<Vec<webtest_model::TestVariantId>>)
            .transpose();
        let sources = request
            .arguments
            .get("sources")
            .cloned()
            .map(serde_json::from_value::<BTreeMap<PathBuf, String>>)
            .transpose();
        let loaded = (|| {
            let variants = variants
                .map_err(|error| format!("invalid variant selection: {error}"))?
                .unwrap_or_default();
            let mut sources = sources
                .map_err(|error| format!("invalid source overrides: {error}"))?
                .unwrap_or_default()
                .into_iter()
                .map(|(path, text)| (normalize_path(&path), text))
                .collect::<BTreeMap<_, _>>();
            if let Some(source) = source_override {
                sources.insert(path.clone(), source);
            }
            if let Some(loader) = &self.workspace_loader {
                let database = loader.load(&path, &sources)?;
                let file = database
                    .file_for_path(&path.display().to_string())
                    .ok_or("debug program was not loaded into the workspace")?;
                LoadedProgram::from_database(path, database, file, &variants)
            } else {
                let mut program = LoadedProgram::load_with_registry(
                    path.clone(),
                    sources.remove(&path),
                    &self.providers,
                )?;
                if !variants.is_empty() {
                    if variants
                        .iter()
                        .any(|id| !program.plan.tests.iter().any(|test| test.variant_id == *id))
                    {
                        return Err(
                            "requested test variant does not belong to the launched program".into(),
                        );
                    }
                    program.plan = Arc::new(
                        program
                            .plan
                            .select_variants(&variants.into_iter().collect()),
                    );
                }
                Ok(program)
            }
        })();
        let loaded = match loaded {
            Ok(program) => program,
            Err(error) => {
                self.writer.failure(request, &error).await?;
                return Ok(());
            }
        };
        let stop_on_entry = request
            .arguments
            .get("stopOnEntry")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        *lock(&self.program) = Some(loaded);
        if stop_on_entry {
            lock(&self.tasks).pending_pause = Some("entry");
        }
        self.writer.response(request, json!({})).await?;
        self.writer.event("initialized", json!({})).await?;
        self.start_if_ready();
        Ok(())
    }

    async fn set_breakpoints(&self, request: &Request) -> Result<(), DapError> {
        let source_path = request
            .arguments
            .pointer("/source/path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let requested: Vec<SourceBreakpoint> = request
            .arguments
            .get("breakpoints")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let Some(path) = source_path else {
            self.writer
                .response(
                    request,
                    json!({ "breakpoints": unverified_breakpoints(&requested, "The source has no file path") }),
                )
                .await?;
            return Ok(());
        };
        let path = normalize_path(&path);
        let program = lock(&self.program).as_ref().cloned().or_else(|| {
            LoadedProgram::load_with_registry(path.clone(), None, &self.providers).ok()
        });
        let Some(program) = program else {
            self.writer
                .response(
                    request,
                    json!({ "breakpoints": unverified_breakpoints(&requested, "Could not load this WebTest source") }),
                )
                .await?;
            return Ok(());
        };
        if request.arguments["sourceModified"].as_bool() == Some(true)
            || request
                .arguments
                .pointer("/source/adapterData/sourceRevision")
                .is_some_and(|revision| {
                    serde_json::from_value::<webtest_text::SourceRevision>(revision.clone()).ok()
                        != program.source_at_path(&path).map(|source| source.revision)
                })
        {
            lock(&self.breakpoints).remove(&path);
            lock(&self.breakpoint_revisions).remove(&path);
            return self.writer.response(request, json!({"breakpoints":unverified_breakpoints(
                &requested, "Source revision differs from the launched execution; restart with the current source.") })).await;
        }
        let Some(source) = program.source_at_path(&path) else {
            return self.writer.response(request, json!({"breakpoints":unverified_breakpoints(&requested, "Source is not part of this launch")})).await;
        };
        lock(&self.breakpoint_revisions).insert(path.clone(), source.revision);
        let locations = program.locations_for(&path);
        let mut verified_lines = HashSet::new();
        let breakpoints: Vec<_> = requested
            .iter()
            .enumerate()
            .map(|(index, breakpoint)| {
                if let Some(location) = locations
                    .iter()
                    .find(|location| location.line == breakpoint.line)
                {
                    verified_lines.insert(location.line);
                    json!({
                        "id": index + 1,
                        "verified": true,
                        "line": location.line,
                        "column": location.column,
                        "endLine": location.end_line,
                        "endColumn": location.end_column,
                        "source": { "name": source_name(&path), "path": path },
                    })
                } else {
                    json!({
                        "id": index + 1,
                        "verified": false,
                        "line": breakpoint.line,
                        "message": "No executable WebTest step is on this line.",
                    })
                }
            })
            .collect();
        lock(&self.breakpoints).insert(path, verified_lines);
        self.writer
            .response(request, json!({ "breakpoints": breakpoints }))
            .await
    }

    fn start_if_ready(self: &Arc<Self>) {
        if !self.configured.load(Ordering::Acquire) || lock(&self.program).is_none() {
            return;
        }
        if self
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let state = Arc::clone(self);
        *lock(&self.run_task) = Some(tokio::spawn(async move {
            state.run_program().await;
        }));
    }

    async fn run_program(self: Arc<Self>) {
        let program = lock(&self.program).clone();
        let Some(program) = program else {
            return;
        };
        let _ = self
            .writer
            .event(
                "output",
                json!({
                    "category": "console",
                    "output": format!("Debugging {} in Chrome\n", program.path.display()),
                }),
            )
            .await;

        let runner = Runner::new(Arc::new(ObservationStore::default()))
            .with_options(self.runner_options.clone())
            .with_provider_registry(self.providers.clone());
        let result = runner
            .run_with_control(&program.plan, self.browser.as_ref(), Some(self.as_ref()))
            .await;
        for test in &result.tests {
            let (category, status, data) = match &test.outcome {
                TestOutcome::Verdict { kind, message } => (
                    "console",
                    format!("{}: {}", kind.as_str(), message),
                    json!({"verdict":kind,"message":message}),
                ),
                TestOutcome::Passed => ("stdout", "ok".to_owned(), Value::Null),
                TestOutcome::Failed(failure) => {
                    let hints = (!failure.repair_hints.is_empty())
                        .then(|| serde_json::to_string(&failure.repair_hints).ok())
                        .flatten()
                        .map(|hints| format!("\nsemantic repair candidates: {hints}"))
                        .unwrap_or_default();
                    (
                        "stderr",
                        format!("FAILED: {}{hints}", failure.error),
                        failure_output_data(failure),
                    )
                }
                TestOutcome::TimedOut {
                    timeout,
                    active_step,
                } => (
                    "stderr",
                    format!("TIMED OUT after {}ms", timeout.as_millis()),
                    json!({
                        "code": "runtime.test_timeout",
                        "timeout_ms": timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                        "active_step_id": active_step.map(|step| step.0),
                    }),
                ),
                TestOutcome::Cancelled { reason } => {
                    ("stderr", format!("CANCELLED: {reason:?}"), Value::Null)
                }
                TestOutcome::Skipped { reason, .. } => {
                    ("stderr", format!("SKIPPED: {reason:?}"), Value::Null)
                }
                TestOutcome::Aborted {
                    failure,
                    prior_outcome,
                } => (
                    "stderr",
                    format!(
                        "ABORTED ({} error): {failure}",
                        failure_class_name(failure.failure_class())
                    ),
                    aborted_failure_output_data(failure, prior_outcome.as_deref()),
                ),
            };
            let _ = self
                .writer
                .event(
                    "output",
                    json!({
                        "category": category,
                        "output": format!("test {:?} ... {status}\n", test.name),
                        "data": if test.branches.is_empty() { data } else { json!({ "summary": data, "branches": branch_output_data(&test.branches) }) },
                    }),
                )
                .await;
        }
        if let RunOutcome::Aborted {
            failure,
            prior_outcome,
        } = &result.outcome
            && (result.aborted() == 0 || failure.journal_overflow().is_some())
        {
            let _ = self
                .writer
                .event(
                    "output",
                    json!({
                        "category": "stderr",
                        "output": format!(
                            "run ... ABORTED ({} error): {failure}\n",
                            failure_class_name(failure.failure_class())
                        ),
                        "data": aborted_run_failure_output_data(failure, *prior_outcome),
                    }),
                )
                .await;
        }
        let exit_code = match &result.outcome {
            RunOutcome::Completed => i32::from(
                result.failed() != 0
                    || result.timed_out() != 0
                    || result.cancelled() != 0
                    || result.aborted() != 0,
            ),
            RunOutcome::Cancelled { .. } => 1,
            RunOutcome::Aborted { failure, .. } => failure_exit_code(failure.failure_class()),
        };
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        self.flush_thread_events().await;
        let _ = self
            .writer
            .event("exited", json!({ "exitCode": exit_code }))
            .await;
        let _ = self.writer.event("terminated", json!({})).await;
        self.completion.send_replace(true);
    }
}

fn branch_output_data(branches: &[webtest_runtime::BranchResult]) -> Value {
    Value::Array(branches.iter().map(|branch| {
        let (outcome, data) = match &branch.outcome {
            TestOutcome::Verdict{kind,message}=>(kind.as_str(),json!({"verdict":kind,"message":message})),
            TestOutcome::Passed => ("passed", Value::Null),
            TestOutcome::Failed(failure) => ("failed", failure_output_data(failure)),
            TestOutcome::TimedOut { timeout, active_step } => ("timed_out", json!({"timeout_ms": timeout.as_millis(), "active_step_id": active_step.map(|id| id.0)})),
            TestOutcome::Cancelled { reason } => ("cancelled", json!({"reason": reason})),
            TestOutcome::Skipped { reason, failure_class } => ("skipped", json!({"reason": reason, "failure_class": failure_class})),
            TestOutcome::Aborted { failure, prior_outcome } => ("aborted", aborted_failure_output_data(failure, prior_outcome.as_deref())),
        };
        json!({ "scope": branch.scope, "race_winner": branch.race_winner, "outcome": outcome, "data": data, "duration_ms": branch.duration.as_millis(), "branches": branch_output_data(&branch.branches) })
    }).collect())
}

fn failure_output_data(failure: &webtest_runtime::StepFailure) -> Value {
    json!({
        "diagnostic_schema_version": webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        "repair_hint_schema_version": webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        "code": failure.error.code().short_code(),
        "failure_class": failure.error.failure_class(),
        "repair_hints": failure.repair_hints,
        "page": failure.inspection.as_ref().map(|inspection| &inspection.page),
        "secondary": failure.secondary_failures,
    })
}

fn run_failure_output_data(failure: &webtest_runtime::RunError) -> Value {
    match failure {
        webtest_runtime::RunError::JournalOverflow(overflow) => serde_json::json!({
            "code": failure.code().short_code(),
            "failure_class": failure.failure_class(),
            "overflow": overflow,
        }),
        webtest_runtime::RunError::Cleanup(cleanup) => {
            let cause = match &cleanup.cause {
                webtest_runtime::CleanupCause::Execution {
                    failure,
                    failure_class,
                } => json!({"kind":"execution", "failure":failure, "failure_class":failure_class}),
                webtest_runtime::CleanupCause::Runtime {
                    failure,
                    failure_class,
                } => json!({"kind":"runtime", "failure":failure, "failure_class":failure_class}),
                webtest_runtime::CleanupCause::Provider(error) => {
                    json!({"kind": "provider", "error": error})
                }
                webtest_runtime::CleanupCause::Browser(error) => json!({
                    "kind": "browser",
                    "code": RuntimeFailureCode::from(error).short_code(),
                    "message": error.to_string(),
                }),
                webtest_runtime::CleanupCause::Io(error) => json!({
                    "kind": "io",
                    "error_kind": error.kind,
                    "raw_os_error": error.raw_os_error,
                    "message": error.message,
                }),
                webtest_runtime::CleanupCause::TimedOut { timeout_ms } => {
                    json!({"kind": "timed_out", "timeout_ms": timeout_ms})
                }
                webtest_runtime::CleanupCause::Internal { message } => json!({
                    "kind": "internal",
                    "message": message,
                }),
            };
            json!({
                "code": cleanup.code().short_code(),
                "failure_class": cleanup.failure_class(),
                "resource": cleanup.resource,
                "cause": cause,
            })
        }
        webtest_runtime::RunError::Multiple { primary, secondary } => json!({
            "code": failure.code().short_code(),
            "failure_class": failure.failure_class(),
            "primary": run_failure_output_data(primary),
            "secondary": secondary.iter().map(run_failure_output_data).collect::<Vec<_>>(),
        }),
        webtest_runtime::RunError::FixtureBudget(budget) => {
            json!({"code": failure.code().short_code(), "failure_class":failure.failure_class(), "budget":budget})
        }
        webtest_runtime::RunError::ExecutionIdentity(_)
        | webtest_runtime::RunError::Browser(_)
        | webtest_runtime::RunError::Provider(_)
        | webtest_runtime::RunError::Internal(_) => json!({
            "code": failure.code().short_code(),
            "failure_class": failure.failure_class(),
        }),
    }
}

fn aborted_failure_output_data(
    failure: &webtest_runtime::RunError,
    prior: Option<&webtest_runtime::PriorTestOutcome>,
) -> Value {
    let mut data = run_failure_output_data(failure);
    let prior = match prior {
        Some(webtest_runtime::PriorTestOutcome::Verdict { kind, message }) => {
            json!({"kind":"verdict","verdict":kind,"message":message})
        }
        Some(webtest_runtime::PriorTestOutcome::Failed(failure)) => json!({
            "kind": "failed",
            "failure": failure_output_data(failure),
        }),
        Some(webtest_runtime::PriorTestOutcome::TimedOut {
            timeout,
            active_step,
        }) => json!({
            "kind": "timed_out",
            "timeout_ms": timeout.as_millis().min(u128::from(u64::MAX)) as u64,
            "active_step_id": active_step.map(|step| step.0),
        }),
        Some(webtest_runtime::PriorTestOutcome::Cancelled { reason }) => json!({
            "kind": "cancelled",
            "reason": reason,
        }),
        None => return data,
    };
    if let Some(data) = data.as_object_mut() {
        data.insert("prior_outcome".into(), prior);
    }
    data
}

fn aborted_run_failure_output_data(
    failure: &webtest_runtime::RunError,
    prior: Option<webtest_runtime::PriorRunOutcome>,
) -> Value {
    let mut data = run_failure_output_data(failure);
    let Some(webtest_runtime::PriorRunOutcome::Cancelled { reason }) = prior else {
        return data;
    };
    if let Some(data) = data.as_object_mut() {
        data.insert(
            "prior_outcome".into(),
            json!({
                "kind": "cancelled",
                "reason": reason,
            }),
        );
    }
    data
}

const fn failure_exit_code(class: webtest_feedback::FailureClass) -> i32 {
    match class {
        webtest_feedback::FailureClass::Test => 1,
        webtest_feedback::FailureClass::Infrastructure => 3,
        webtest_feedback::FailureClass::Internal => 4,
    }
}

const fn failure_class_name(class: webtest_feedback::FailureClass) -> &'static str {
    match class {
        webtest_feedback::FailureClass::Test => "test",
        webtest_feedback::FailureClass::Infrastructure => "infrastructure",
        webtest_feedback::FailureClass::Internal => "internal",
    }
}

pub async fn serve(browser: Arc<dyn BrowserHost>) -> Result<(), DapError> {
    serve_with_options(browser, RunnerOptions::default()).await
}

pub async fn serve_with_options(
    browser: Arc<dyn BrowserHost>,
    options: RunnerOptions,
) -> Result<(), DapError> {
    let writer = ProtocolWriter::new(tokio::io::stdout());
    let state = DebugState::new_with_options(writer, browser, options);
    serve_state(state).await
}

pub async fn serve_with_configuration(
    browser: Arc<dyn BrowserHost>,
    options: RunnerOptions,
    providers: ProviderRegistry,
) -> Result<(), DapError> {
    let writer = ProtocolWriter::new(tokio::io::stdout());
    let state = DebugState::new_with_configuration(writer, browser, options, providers);
    serve_state(state).await
}

pub async fn serve_with_workspace(
    browser: Arc<dyn BrowserHost>,
    options: RunnerOptions,
    providers: ProviderRegistry,
    loader: Arc<dyn WorkspaceLoader>,
) -> Result<(), DapError> {
    let state = DebugState::new_with_workspace(
        ProtocolWriter::new(tokio::io::stdout()),
        browser,
        options,
        providers,
        Some(loader),
    );
    serve_state(state).await
}

async fn serve_state(state: Arc<DebugState>) -> Result<(), DapError> {
    let mut requests = request_channel();
    let mut completion = state.completion.subscribe();
    let result = async {
        loop {
            if *completion.borrow() {
                return Ok(());
            }
            tokio::select! {
                request = requests.recv() => {
                    let Some(request) = request else { return Ok(()); };
                    if state.handle(request?).await? { return Ok(()); }
                }
                changed = completion.changed() => {
                    if changed.is_err() || *completion.borrow() { return Ok(()); }
                }
            }
        }
    }
    .await;
    state.cancel_execution();
    state.join_execution().await;
    result
}

fn request_channel() -> mpsc::UnboundedReceiver<Result<Request, DapError>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let input = std::io::stdin();
        let mut reader = std::io::BufReader::new(input.lock());
        loop {
            match read_request(&mut reader) {
                Ok(Some(request)) => {
                    if sender.send(Ok(request)).is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    return;
                }
            }
        }
    });
    receiver
}

fn read_request(reader: &mut impl BufRead) -> Result<Option<Request>, DapError> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        let read = reader.read_line(&mut header)?;
        if read == 0 {
            return Ok(None);
        }
        let header = header.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(content_length) = content_length else {
        return Err(DapError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "DAP message did not include Content-Length",
        )));
    };
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

#[derive(Clone, Debug, Deserialize)]
struct SourceBreakpoint {
    line: u32,
}

fn unverified_breakpoints(requested: &[SourceBreakpoint], message: &str) -> Vec<Value> {
    requested
        .iter()
        .enumerate()
        .map(|(index, breakpoint)| {
            json!({
                "id": index + 1,
                "verified": false,
                "line": breakpoint.line,
                "message": message,
            })
        })
        .collect()
}

struct DapVariableStore {
    variables: HashMap<i64, Vec<Value>>,
    targets: HashMap<i64, Arc<webtest_model::Value>>,
    root: Vec<(String, Arc<webtest_model::Value>)>,
    root_reference: i64,
    handles: Arc<AtomicI64>,
}

impl DapVariableStore {
    #[cfg(test)]
    fn new(bindings: BTreeMap<String, webtest_model::Value>) -> Self {
        Self::with_handles(bindings, Arc::new(AtomicI64::new(1)))
    }
    fn with_handles(
        bindings: BTreeMap<String, webtest_model::Value>,
        handles: Arc<AtomicI64>,
    ) -> Self {
        Self {
            variables: HashMap::new(),
            targets: HashMap::new(),
            root: bindings
                .into_iter()
                .map(|(name, value)| (name, Arc::new(value)))
                .collect(),
            root_reference: handles.fetch_add(1, Ordering::Relaxed),
            handles,
        }
    }

    fn variables(&mut self, reference: i64) -> Vec<Value> {
        if let Some(variables) = self.variables.get(&reference) {
            return variables.clone();
        }
        let values = if reference == self.root_reference {
            self.root.clone()
        } else {
            let Some(value) = self.targets.get(&reference).cloned() else {
                return Vec::new();
            };
            dap_children(&value)
        };
        let variables = values
            .into_iter()
            .map(|(name, value)| self.provider_variable(&name, value))
            .collect::<Vec<_>>();
        self.variables.insert(reference, variables.clone());
        variables
    }

    fn provider_variable(&mut self, name: &str, value: Arc<webtest_model::Value>) -> Value {
        let child_shape = dap_child_shape(&value);
        let reference = if child_shape.is_some() {
            let reference = self.handles.fetch_add(1, Ordering::Relaxed);
            self.targets.insert(reference, Arc::clone(&value));
            reference
        } else {
            0
        };
        let mut variable = dap_variable(name, &dap_value(&value), value.type_name(), reference);
        if let Some((count_name, child_count)) = child_shape.and_then(|shape| shape.count) {
            variable[count_name] = child_count.into();
        }
        variable
    }
}

#[derive(Clone, Copy)]
struct DapChildShape {
    count: Option<(&'static str, usize)>,
}

fn dap_child_shape(value: &webtest_model::Value) -> Option<DapChildShape> {
    use webtest_model::Value as ProviderValue;

    match value {
        ProviderValue::List(values) if !values.is_empty() => Some(DapChildShape {
            count: Some(("indexedVariables", values.len())),
        }),
        ProviderValue::Record(values) if !values.is_empty() => Some(DapChildShape {
            count: Some(("namedVariables", values.len())),
        }),
        ProviderValue::Headers(values) if !values.is_empty() => Some(DapChildShape {
            count: Some(("namedVariables", values.len())),
        }),
        ProviderValue::Response(_) => Some(DapChildShape { count: None }),
        ProviderValue::ProcessResult(_) => Some(DapChildShape {
            count: Some(("namedVariables", 5)),
        }),
        _ => None,
    }
}

fn dap_variable(name: &str, value: &str, kind: &str, variables_reference: i64) -> Value {
    json!({
        "name": name,
        "value": value,
        "type": kind,
        "variablesReference": variables_reference,
    })
}

fn dap_children(value: &webtest_model::Value) -> Vec<(String, Arc<webtest_model::Value>)> {
    use webtest_model::Value as ProviderValue;

    match value {
        ProviderValue::List(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| (format!("[{index}]"), Arc::new(value.clone())))
            .collect(),
        ProviderValue::Record(values) => values
            .iter()
            .map(|(name, value)| (name.clone(), Arc::new(value.clone())))
            .collect(),
        ProviderValue::Headers(values) => values
            .iter()
            .map(|(name, value)| (name.clone(), Arc::new(ProviderValue::String(value.clone()))))
            .collect(),
        ProviderValue::Response(response) => {
            let mut values = vec![
                (
                    "status".into(),
                    Arc::new(ProviderValue::Int(i64::from(response.status))),
                ),
                (
                    "headers".into(),
                    Arc::new(ProviderValue::Headers(response.headers.clone())),
                ),
                ("body".into(), Arc::new(debug_bytes_value(&response.body))),
            ];
            if let Ok(text) = std::str::from_utf8(&response.body) {
                values.push((
                    "text".into(),
                    Arc::new(ProviderValue::String(bounded_debug_text(text))),
                ));
            }
            values.push((
                "json".into(),
                Arc::new(
                    response
                        .json
                        .as_deref()
                        .cloned()
                        .unwrap_or(ProviderValue::Null),
                ),
            ));
            values
        }
        ProviderValue::ProcessResult(result) => vec![
            ("exit_code".into(), ProviderValue::Int(result.exit_code)),
            (
                "stdout".into(),
                ProviderValue::String(bounded_debug_text(&result.stdout)),
            ),
            (
                "stderr".into(),
                ProviderValue::String(bounded_debug_text(&result.stderr)),
            ),
            (
                "stdout_bytes".into(),
                debug_bytes_value(&result.stdout_bytes),
            ),
            (
                "stderr_bytes".into(),
                debug_bytes_value(&result.stderr_bytes),
            ),
        ]
        .into_iter()
        .map(|(name, value)| (name, Arc::new(value)))
        .collect(),
        ProviderValue::Null
        | ProviderValue::Bool(_)
        | ProviderValue::Int(_)
        | ProviderValue::Float(_)
        | ProviderValue::String(_)
        | ProviderValue::DurationMillis(_)
        | ProviderValue::Bytes(_)
        | ProviderValue::FilePath(_)
        | ProviderValue::TempDirectory(_) => Vec::new(),
    }
}

fn dap_value(value: &webtest_model::Value) -> String {
    let mut preview = DapValuePreview::new();
    preview.value(value);
    preview.finish()
}

const DAP_VALUE_PREVIEW_BYTES: usize = 1024;

struct DapValuePreview {
    rendered: String,
    truncated: bool,
}

impl DapValuePreview {
    fn new() -> Self {
        Self {
            rendered: String::with_capacity(DAP_VALUE_PREVIEW_BYTES),
            truncated: false,
        }
    }

    fn finish(mut self) -> String {
        if self.truncated {
            let limit = DAP_VALUE_PREVIEW_BYTES.saturating_sub(3);
            while self.rendered.len() > limit {
                self.rendered.pop();
            }
            self.rendered.push_str("...");
        }
        self.rendered
    }

    fn push(&mut self, value: &str) {
        let remaining = DAP_VALUE_PREVIEW_BYTES.saturating_sub(self.rendered.len());
        if value.len() <= remaining {
            self.rendered.push_str(value);
            return;
        }
        let mut end = remaining;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        self.rendered.push_str(&value[..end]);
        self.truncated = true;
    }

    fn quoted(&mut self, value: &str) {
        let bounded = bounded_debug_text(value);
        let rendered = serde_json::to_string(&bounded).unwrap_or_else(|_| "\"<string>\"".into());
        self.push(&rendered);
        if bounded.len() < value.len() {
            self.truncated = true;
        }
    }

    fn fields<'a>(
        &mut self,
        values: impl IntoIterator<Item = (&'a str, &'a webtest_model::Value)>,
    ) {
        self.push("{");
        for (index, (name, value)) in values.into_iter().enumerate() {
            if self.truncated {
                break;
            }
            if index > 0 {
                self.push(",");
            }
            self.quoted(name);
            self.push(":");
            self.value(value);
        }
        self.push("}");
    }

    fn string_fields<'a>(&mut self, values: impl IntoIterator<Item = (&'a str, &'a str)>) {
        self.push("{");
        for (index, (name, value)) in values.into_iter().enumerate() {
            if self.truncated {
                break;
            }
            if index > 0 {
                self.push(",");
            }
            self.quoted(name);
            self.push(":");
            self.quoted(value);
        }
        self.push("}");
    }

    fn value(&mut self, value: &webtest_model::Value) {
        use webtest_model::Value as ProviderValue;

        match value {
            ProviderValue::Null => self.push("null"),
            ProviderValue::Bool(value) => self.push(if *value { "true" } else { "false" }),
            ProviderValue::Int(value) => self.push(&value.to_string()),
            ProviderValue::Float(value) => self.push(&value.to_string()),
            ProviderValue::String(value) => self.quoted(value),
            ProviderValue::DurationMillis(value) => self.push(&value.to_string()),
            ProviderValue::List(values) => {
                self.push("[");
                for (index, value) in values.iter().enumerate() {
                    if self.truncated {
                        break;
                    }
                    if index > 0 {
                        self.push(",");
                    }
                    self.value(value);
                }
                self.push("]");
            }
            ProviderValue::Record(values) => {
                self.fields(values.iter().map(|(name, value)| (name.as_str(), value)));
            }
            ProviderValue::Headers(values) => {
                self.string_fields(
                    values
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str())),
                );
            }
            ProviderValue::Bytes(bytes) => self.value(&debug_bytes_value(bytes)),
            ProviderValue::Response(response) => {
                self.push("{");
                self.quoted("status");
                self.push(":");
                self.push(&response.status.to_string());
                self.push(",");
                self.quoted("headers");
                self.push(":");
                self.string_fields(
                    response
                        .headers
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str())),
                );
                self.push(",");
                self.quoted("body");
                self.push(":");
                self.value(&debug_bytes_value(&response.body));
                self.push(",");
                self.quoted("json");
                self.push(":");
                self.value(response.json.as_deref().unwrap_or(&ProviderValue::Null));
                self.push("}");
            }
            ProviderValue::ProcessResult(result) => {
                let exit_code = ProviderValue::Int(result.exit_code);
                let stdout = ProviderValue::String(bounded_debug_text(&result.stdout));
                let stderr = ProviderValue::String(bounded_debug_text(&result.stderr));
                self.fields([
                    ("exit_code", &exit_code),
                    ("stdout", &stdout),
                    ("stderr", &stderr),
                ]);
            }
            ProviderValue::FilePath(path) | ProviderValue::TempDirectory(path) => {
                self.quoted(&path.display().to_string());
            }
        }
    }
}

fn bounded_debug_text(value: &str) -> String {
    let mut end = value.len().min(DAP_VALUE_PREVIEW_BYTES);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn debug_bytes_value(bytes: &[u8]) -> webtest_model::Value {
    let sample = &bytes[..bytes.len().min(DAP_VALUE_PREVIEW_BYTES)];
    match std::str::from_utf8(sample) {
        Ok(text) => webtest_model::Value::String(text.to_owned()),
        Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => {
            let text = std::str::from_utf8(&sample[..error.valid_up_to()])
                .unwrap_or_default()
                .to_owned();
            webtest_model::Value::String(text)
        }
        Err(_) => webtest_model::Value::String(format!("<{} bytes>", bytes.len())),
    }
}

fn operation_name(operation: &TestOperation) -> String {
    match operation {
        TestOperation::Reactive(webtest_plan::semantics::ReactiveOperation::Checkpoint {
            name,
            ..
        }) => format!("checkpoint {name}"),
        TestOperation::Reactive(webtest_plan::semantics::ReactiveOperation::Failure {
            reason,
            ..
        }) => format!("event {reason:?}"),
        TestOperation::Verdict { verdict, .. } => format!("verdict {}", verdict.as_str()),
        TestOperation::AcquireFixture(call) => format!("use fixture {}", call.fixture),
        TestOperation::Return(_) => "return helper value".into(),
        TestOperation::Provide(_) => "provide".into(),
        TestOperation::EvaluatePure(operation) => operation.result_binding.map_or_else(
            || "evaluate expression".into(),
            |binding| format!("let binding_{}", binding.0),
        ),
        TestOperation::ServerProviderCall(call) => {
            format!("{}.{}", call.provider, call.operation)
        }
        TestOperation::Browser(BrowserOperation::Navigate { url }) => {
            format!("open {}", expression_name(url))
        }
        TestOperation::Browser(BrowserOperation::Evaluate { .. }) => "evaluate <script>".into(),
        TestOperation::Browser(BrowserOperation::Click { locator }) => {
            format!("click {}", locator_name(locator))
        }
        TestOperation::Browser(BrowserOperation::Fill { locator, .. }) => {
            format!("fill {} with <redacted>", locator_name(locator))
        }
        TestOperation::Browser(BrowserOperation::Type { locator, .. }) => {
            format!("type {} with <text>", locator_name(locator))
        }
        TestOperation::Browser(BrowserOperation::Press { locator, key }) => {
            format!(
                "press {} key {}",
                locator_name(locator),
                expression_name(key)
            )
        }
        TestOperation::Browser(BrowserOperation::Check {
            locator,
            checked: true,
        }) => format!("check {}", locator_name(locator)),
        TestOperation::Browser(BrowserOperation::Check {
            locator,
            checked: false,
        }) => format!("uncheck {}", locator_name(locator)),
        TestOperation::Browser(BrowserOperation::Select { locator, option }) => {
            format!(
                "select {} option {}",
                locator_name(locator),
                expression_name(option)
            )
        }
        TestOperation::Browser(BrowserOperation::Hover { locator }) => {
            format!("hover {}", locator_name(locator))
        }
        TestOperation::Browser(BrowserOperation::WaitForLocator { locator, state, .. }) => {
            format!("wait {}.{state}", locator_name(locator))
        }
        TestOperation::Browser(BrowserOperation::WaitForUrl { url, .. }) => {
            format!("wait url({url:?})")
        }
        TestOperation::Assertion(AssertionOperation::Locator { locator, state, .. }) => {
            format!("expect {}.{state}", locator_name(locator))
        }
        TestOperation::Assertion(AssertionOperation::Url { url, .. }) => {
            format!("expect url({})", expression_name(url))
        }
        TestOperation::Assertion(AssertionOperation::Value { matcher, .. }) => {
            format!("expect {matcher:?}")
        }
    }
}

fn expression_name(expression: &webtest_plan::PlanExpr) -> String {
    match expression {
        webtest_plan::PlanExpr::Literal(webtest_model::Value::String(value)) => {
            format!("{value:?}")
        }
        webtest_plan::PlanExpr::Binding(binding) => format!("binding_{}", binding.0),
        _ => "<expression>".into(),
    }
}

fn locator_name(locator: &webtest_plan::Locator) -> String {
    match locator {
        webtest_plan::Locator::Dynamic { template, .. } => {
            format!("{} <value>", locator_name(template))
        }
        webtest_plan::Locator::Id(value) => format!("id({value:?})"),
        webtest_plan::Locator::Role {
            role,
            name: Some(name),
        } => format!("role({role:?}, name: {name:?})"),
        webtest_plan::Locator::Role { role, name: None } => format!("role({role:?})"),
        webtest_plan::Locator::Label(value) => format!("label({value:?})"),
        webtest_plan::Locator::Text(value) => format!("text({value:?})"),
        webtest_plan::Locator::Placeholder(value) => format!("placeholder({value:?})"),
        webtest_plan::Locator::TestId(value) => format!("test_id({value:?})"),
        webtest_plan::Locator::Css(value) => format!("css({value:?})"),
        webtest_plan::Locator::XPath(value) => format!("xpath({value:?})"),
    }
}

fn source_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("WebTest")
        .to_owned()
}

fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_owned(), |cwd| cwd.join(path))
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}

fn offset_to_line_column(source: &str, requested_offset: usize) -> (u32, u32) {
    let mut offset = requested_offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let mut line = 1u32;
    let mut column = 1u32;
    for character in source[..offset].chars() {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += character.len_utf16() as u32;
        }
    }
    (line, column)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use webtest_browser::{
        BrowserError, InspectionTruncation, Locator, PageEvidence, PageInspection, PageSummary,
        RepairHint,
    };

    use super::*;

    #[test]
    fn journal_overflow_output_preserves_the_exact_missing_interval() {
        let first = webtest_observation::EventIdentity {
            execution_id: webtest_observation::ExecutionId::from_u128(42),
            event_sequence: webtest_observation::EventSequence(9),
        };
        let error = webtest_runtime::RunError::JournalOverflow(webtest_runtime::JournalOverflow {
            capacity: 10,
            first_rejected: first,
            last_rejected: webtest_observation::EventIdentity {
                event_sequence: webtest_observation::EventSequence(12),
                ..first
            },
            rejected_events: 4,
        });
        let data = run_failure_output_data(&error);
        assert_eq!(data["code"], "journal_capacity_exceeded");
        assert_eq!(data["failure_class"], "infrastructure");
        assert_eq!(data["overflow"]["capacity"], 10);
        assert_eq!(
            data["overflow"]["first_rejected"]["execution_id"],
            "0000000000000000000000000000002a"
        );
        assert_eq!(data["overflow"]["first_rejected"]["event_sequence"], 9);
        assert_eq!(data["overflow"]["last_rejected"]["event_sequence"], 12);
        assert_eq!(data["overflow"]["rejected_events"], 4);
    }

    #[test]
    fn debug_exit_and_output_distinguish_internal_from_infrastructure_failures() {
        let infrastructure = webtest_runtime::RunError::Browser(BrowserError::BrowserDisconnected);
        let internal = webtest_runtime::RunError::Internal("violated invariant".into());

        assert_eq!(failure_exit_code(infrastructure.failure_class()), 3);
        assert_eq!(failure_exit_code(internal.failure_class()), 4);
        assert_eq!(
            run_failure_output_data(&infrastructure)["failure_class"],
            "infrastructure"
        );
        assert_eq!(
            run_failure_output_data(&internal)["failure_class"],
            "internal"
        );
        assert_eq!(failure_class_name(internal.failure_class()), "internal");
    }

    #[test]
    fn cleanup_output_preserves_typed_resource_and_cause() {
        let cleanup = webtest_runtime::RunError::Cleanup(webtest_runtime::CleanupFailure {
            resource: webtest_runtime::CleanupResource::BrowserSession,
            cause: webtest_runtime::CleanupCause::Browser(BrowserError::BrowserDisconnected),
        });
        let data = run_failure_output_data(&cleanup);

        assert_eq!(data["code"], "cleanup_browser_session_failed");
        assert_eq!(data["failure_class"], "infrastructure");
        assert_eq!(data["resource"]["kind"], "browser_session");
        assert_eq!(data["cause"]["kind"], "browser");
        assert_eq!(data["cause"]["code"], "browser_disconnected");
        let after_cancellation = aborted_run_failure_output_data(
            &cleanup,
            Some(webtest_runtime::PriorRunOutcome::Cancelled {
                reason: webtest_runtime::CancellationReason::UserCancelled,
            }),
        );
        assert_eq!(after_cancellation["prior_outcome"]["kind"], "cancelled");
        assert_eq!(
            after_cancellation["prior_outcome"]["reason"],
            "user_cancelled"
        );
    }

    #[test]
    fn reads_content_length_framed_requests() {
        let body = r#"{"seq":7,"type":"request","command":"threads"}"#;
        let message = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut reader = std::io::BufReader::new(message.as_bytes());
        let request = read_request(&mut reader)
            .expect("valid frame")
            .expect("request");
        assert_eq!(request.seq, 7);
        assert_eq!(request.command, "threads");
    }

    #[test]
    fn executable_locations_and_operation_names_come_from_the_plan() {
        let source = "test \"x\" {\n    browser {\n        open \"about:blank\"\n        click id(\"submit\")\n    }\n}\n";
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("debug.webtest");
        let program = LoadedProgram::load(path, Some(source.into())).expect("program");
        let locations = program.locations();
        assert_eq!(locations[0].line, 3);
        assert_eq!(locations[1].line, 4);
        assert_eq!(
            operation_name(&program.plan.tests[0].steps()[1].operation),
            "click id(\"submit\")"
        );
    }

    #[test]
    fn every_browser_operation_is_a_breakpoint_target_and_secrets_are_hidden() {
        let source = r#"test "x" {
    browser {
        open "http://example.test"
        fill label("Email") with "alice@example.com"
        type label("Bio") with "hello"
        press placeholder("Search") key "Enter"
        check test_id("mail")
        uncheck id("sms")
        select label("Timezone") option "UTC"
        hover text("Account")
        click role("button", name: "Save")
        wait id("ready").visible within 1s
        wait url("http://example.test/done")
        expect id("ready").enabled
        expect url("http://example.test/done")
    }
}
"#;
        let directory = tempfile::tempdir().expect("temp directory");
        let program =
            LoadedProgram::load(directory.path().join("all.webtest"), Some(source.into()))
                .expect("program");
        assert_eq!(program.locations().len(), 13);
        let names = program.plan.tests[0]
            .steps()
            .iter()
            .map(|step| operation_name(&step.operation))
            .collect::<Vec<_>>();
        assert!(
            names
                .iter()
                .any(|name| name == "fill label(\"Email\") with <redacted>")
        );
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("wait id(\"ready\")"))
        );
        assert!(names.iter().any(|name| name.starts_with("expect url(")));
        assert!(names.iter().all(|name| !name.contains("alice@example.com")));
    }

    #[test]
    fn failure_output_exposes_semantic_hints_without_raw_evidence_secrets() {
        let source = r#"test "x" { browser { click role("button", name: "Log in") } }"#;
        let directory = tempfile::tempdir().expect("temp directory");
        let program = LoadedProgram::load(
            directory.path().join("failure.webtest"),
            Some(source.into()),
        )
        .expect("program");
        let step = program.plan.tests[0].steps()[0].clone();
        let locator = Locator::Role {
            role: "button".into(),
            name: Some("Log in".into()),
        };
        let failure = webtest_runtime::StepFailure {
            step,
            error: webtest_runtime::StepError::Browser(BrowserError::LocatorNotFound { locator }),
            evidence: PageEvidence {
                dom_snapshot: Some("password=must-not-leak".into()),
                ..PageEvidence::default()
            },
            artifacts: Vec::new(),
            inspection: Some(PageInspection {
                kind: "inspection".into(),
                inspection_schema_version: 1,
                snapshot_id: "snapshot-1".into(),
                browser_version: "Chrome/1".into(),
                page: PageSummary {
                    url: "http://example.test/login?token=%5Bredacted%5D".into(),
                    title: "Sign in".into(),
                },
                elements: Vec::new(),
                truncation: InspectionTruncation::default(),
            }),
            repair_hints: vec![RepairHint::locator(
                "role(\"button\", name: \"Sign in\")",
                "same role",
            )],
            secondary_failures: Vec::new(),
        };
        let details = failure_output_data(&failure);
        let serialized = serde_json::to_string(&details).expect("details JSON");
        assert_eq!(details["code"], "locator_not_found");
        assert_eq!(details["repair_hints"][0]["kind"], "locator_candidate");
        assert!(!serialized.contains("must-not-leak"));
        assert!(!serialized.contains("dom_snapshot"));
    }

    #[test]
    fn locations_use_one_based_utf16_columns() {
        let source = "😀 click id(\"x\")";
        let start = source.find("id").expect("locator");
        let range = TextRange::new(
            webtest_text::TextSize::from(start as u32),
            webtest_text::TextSize::from((start + 7) as u32),
        );
        assert_eq!(StepLocation::new(source, range).column, 10);
    }

    #[test]
    fn response_bindings_have_a_useful_debugger_preview() {
        let value = webtest_model::Value::Response(webtest_model::ResponseValue {
            status: 201,
            headers: [("content-type".into(), "application/json".into())]
                .into_iter()
                .collect(),
            body: br#"{"id":7,"email":"alice@example.test"}"#.to_vec(),
            json: Some(Box::new(webtest_model::Value::Record(
                [
                    (
                        "email".into(),
                        webtest_model::Value::String("alice@example.test".into()),
                    ),
                    ("id".into(), webtest_model::Value::Int(7)),
                ]
                .into_iter()
                .collect(),
            ))),
        });

        let rendered = dap_value(&value);
        assert!(rendered.contains("\"status\":201"));
        assert!(rendered.contains("alice@example.test"));
        assert_ne!(rendered, "<response>");

        let mut store = DapVariableStore::new(BTreeMap::from([("response".into(), value)]));
        let root = store.variables(VARIABLES_REFERENCE);
        let variable = root
            .iter()
            .find(|variable| variable["name"] == "response")
            .expect("response variable");
        let response_reference = variable["variablesReference"]
            .as_i64()
            .expect("response reference");
        assert!(response_reference > VARIABLES_REFERENCE);
        assert_eq!(store.variables.len(), 1, "only the root is materialized");
        let response_children = store.variables(response_reference);
        assert!(
            response_children
                .iter()
                .any(|child| child["name"] == "status" && child["value"] == "201")
        );
        let json = response_children
            .iter()
            .find(|child| child["name"] == "json")
            .expect("response JSON child");
        let json_reference = json["variablesReference"].as_i64().expect("JSON reference");
        assert!(
            !store.variables.contains_key(&json_reference),
            "nested children stay lazy until the debugger expands them"
        );
        let json_children = store.variables(json_reference);
        assert!(json_children.iter().any(|child| {
            child["name"] == "email" && child["value"] == "\"alice@example.test\""
        }));
    }
}
