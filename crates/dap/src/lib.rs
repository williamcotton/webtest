//! Debug Adapter Protocol transport over the source-mapped WebTest runtime.

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
use webtest_observation::ObservationStore;
use webtest_plan::{
    AssertionOperation, BrowserOperation, PlannedStep, PlannedTest, TestOperation, TestPlan,
};
use webtest_provider::ProviderRegistry;
use webtest_runtime::{RunControl, RunOutcome, Runner, RunnerOptions, StepError, TestOutcome};
use webtest_text::TextRange;

const THREAD_ID: i64 = 1;
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

#[derive(Clone)]
struct LoadedProgram {
    path: PathBuf,
    source: Arc<str>,
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
        let plan = database
            .test_plan(file)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            path,
            source: Arc::from(source),
            plan,
        })
    }

    fn locations(&self) -> Vec<StepLocation> {
        self.plan
            .tests
            .iter()
            .flat_map(|test| {
                test.steps
                    .iter()
                    .map(|step| StepLocation::new(&self.source, step.origin.range))
            })
            .collect()
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

struct PausedFrame {
    test_name: String,
    operation: String,
    source_line: String,
    path: PathBuf,
    location: StepLocation,
    variables: DapVariableStore,
}

#[derive(Clone, Copy)]
enum ResumeCommand {
    Continue,
    Step,
}

struct DebugState {
    writer: ProtocolWriter,
    browser: Arc<dyn BrowserHost>,
    runner_options: RunnerOptions,
    providers: ProviderRegistry,
    program: Mutex<Option<LoadedProgram>>,
    breakpoints: Mutex<HashMap<PathBuf, HashSet<u32>>>,
    exception_breakpoints: Mutex<HashSet<String>>,
    paused: Mutex<Option<PausedFrame>>,
    pending_pause: Mutex<Option<&'static str>>,
    resume_sender: mpsc::UnboundedSender<ResumeCommand>,
    resume_receiver: AsyncMutex<mpsc::UnboundedReceiver<ResumeCommand>>,
    configured: AtomicBool,
    started: AtomicBool,
    shutting_down: AtomicBool,
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
        let (resume_sender, resume_receiver) = mpsc::unbounded_channel();
        let (completion, _) = watch::channel(false);
        Arc::new(Self {
            writer,
            browser,
            runner_options,
            providers,
            program: Mutex::new(None),
            breakpoints: Mutex::new(HashMap::new()),
            exception_breakpoints: Mutex::new(HashSet::new()),
            paused: Mutex::new(None),
            pending_pause: Mutex::new(None),
            resume_sender,
            resume_receiver: AsyncMutex::new(resume_receiver),
            configured: AtomicBool::new(false),
            started: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
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
                            "supportsPauseRequest": true,
                            "supportsStepInTargetsRequest": false,
                            "supportsEvaluateForHovers": false,
                            "exceptionBreakpointFilters": [
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
                self.writer
                    .response(
                        &request,
                        json!({ "threads": [{ "id": THREAD_ID, "name": "WebTest" }] }),
                    )
                    .await?;
            }
            "stackTrace" => self.stack_trace(&request).await?,
            "scopes" => {
                self.writer
                    .response(
                        &request,
                        json!({
                            "scopes": [{
                                "name": "WebTest",
                                "presentationHint": "locals",
                                "variablesReference": VARIABLES_REFERENCE,
                                "expensive": false,
                            }]
                        }),
                    )
                    .await?;
            }
            "variables" => self.variables(&request).await?,
            "continue" => self.resume(&request, ResumeCommand::Continue).await?,
            "next" | "stepIn" | "stepOut" => self.resume(&request, ResumeCommand::Step).await?,
            "pause" => {
                *lock(&self.pending_pause) = Some("pause");
                self.writer.response(&request, json!({})).await?;
            }
            "disconnect" | "terminate" => {
                self.shutting_down.store(true, Ordering::Release);
                self.writer.response(&request, json!({})).await?;
                let _ = self.resume_sender.send(ResumeCommand::Continue);
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
        let loaded = match LoadedProgram::load_with_registry(
            PathBuf::from(program),
            source_override,
            &self.providers,
        ) {
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
            *lock(&self.pending_pause) = Some("entry");
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
        let program = lock(&self.program)
            .as_ref()
            .filter(|program| program.path == path)
            .cloned()
            .or_else(|| {
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
        let locations = program.locations();
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

    async fn stack_trace(&self, request: &Request) -> Result<(), DapError> {
        let frames = lock(&self.paused).as_ref().map_or_else(Vec::new, |frame| {
            vec![json!({
                "id": 1,
                "name": &frame.operation,
                "source": { "name": source_name(&frame.path), "path": &frame.path },
                "line": frame.location.line,
                "column": frame.location.column,
                "endLine": frame.location.end_line,
                "endColumn": frame.location.end_column,
            })]
        });
        self.writer
            .response(
                request,
                json!({ "stackFrames": frames, "totalFrames": frames.len() }),
            )
            .await
    }

    async fn variables(&self, request: &Request) -> Result<(), DapError> {
        let reference = request
            .arguments
            .get("variablesReference")
            .and_then(Value::as_i64);
        let variables = reference
            .and_then(|reference| {
                lock(&self.paused).as_mut().map(|frame| {
                    let mut variables = if reference == VARIABLES_REFERENCE {
                        vec![
                            dap_leaf_variable("test", &format!("{:?}", frame.test_name), "string"),
                            dap_leaf_variable(
                                "operation",
                                &format!("{:?}", frame.operation),
                                "string",
                            ),
                            dap_leaf_variable(
                                "source",
                                &format!("{:?}", frame.source_line),
                                "string",
                            ),
                            dap_leaf_variable("line", &frame.location.line.to_string(), "number"),
                        ]
                    } else {
                        Vec::new()
                    };
                    variables.extend(frame.variables.variables(reference));
                    variables
                })
            })
            .unwrap_or_default();
        self.writer
            .response(request, json!({ "variables": variables }))
            .await
    }

    async fn resume(&self, request: &Request, command: ResumeCommand) -> Result<(), DapError> {
        *lock(&self.pending_pause) = match command {
            ResumeCommand::Continue => None,
            ResumeCommand::Step => Some("step"),
        };
        let body = if matches!(command, ResumeCommand::Continue) {
            json!({ "allThreadsContinued": true })
        } else {
            json!({})
        };
        self.writer.response(request, body).await?;
        let _ = self.resume_sender.send(command);
        self.writer
            .event(
                "continued",
                json!({ "threadId": THREAD_ID, "allThreadsContinued": true }),
            )
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
        tokio::spawn(async move {
            state.run_program().await;
        });
    }

    async fn run_program(self: Arc<Self>) {
        let program = lock(&self.program).clone();
        let Some(program) = program else {
            return;
        };
        let _ = self
            .writer
            .event(
                "thread",
                json!({ "reason": "started", "threadId": THREAD_ID }),
            )
            .await;
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
                TestOutcome::TimedOut { timeout } => (
                    "stderr",
                    format!("TIMED OUT after {}ms", timeout.as_millis()),
                    Value::Null,
                ),
                TestOutcome::Cancelled { reason } => {
                    ("stderr", format!("CANCELLED: {reason:?}"), Value::Null)
                }
                TestOutcome::Skipped { reason, .. } => {
                    ("stderr", format!("SKIPPED: {reason:?}"), Value::Null)
                }
                TestOutcome::Aborted { failure } => (
                    "stderr",
                    format!(
                        "ABORTED ({} error): {failure}",
                        failure_class_name(failure.failure_class())
                    ),
                    run_failure_output_data(failure),
                ),
            };
            let _ = self
                .writer
                .event(
                    "output",
                    json!({
                        "category": category,
                        "output": format!("test {:?} ... {status}\n", test.name),
                        "data": data,
                    }),
                )
                .await;
        }
        if result.aborted() == 0
            && let RunOutcome::Aborted { failure } = &result.outcome
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
                        "data": run_failure_output_data(failure),
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
            RunOutcome::Aborted { failure } => failure_exit_code(failure.failure_class()),
        };
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let _ = self
            .writer
            .event(
                "thread",
                json!({ "reason": "exited", "threadId": THREAD_ID }),
            )
            .await;
        let _ = self
            .writer
            .event("exited", json!({ "exitCode": exit_code }))
            .await;
        let _ = self.writer.event("terminated", json!({})).await;
        self.completion.send_replace(true);
    }
}

fn failure_output_data(failure: &webtest_runtime::StepFailure) -> Value {
    json!({
        "diagnostic_schema_version": webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        "repair_hint_schema_version": webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        "code": failure.error.code(),
        "failure_class": failure.error.failure_class(),
        "repair_hints": failure.repair_hints,
        "page": failure.inspection.as_ref().map(|inspection| &inspection.page),
        "secondary": failure.secondary_failures,
    })
}

fn run_failure_output_data(failure: &webtest_runtime::RunError) -> Value {
    json!({
        "code": failure.code(),
        "failure_class": failure.failure_class(),
    })
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

impl DebugState {
    async fn pause_before_step(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        bindings: BTreeMap<String, webtest_provider::Value>,
    ) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let program = lock(&self.program).clone();
        let Some(program) = program else {
            return;
        };
        let location = StepLocation::new(&program.source, step.origin.range);
        let requested_reason = lock(&self.pending_pause).take();
        let has_breakpoint = lock(&self.breakpoints)
            .get(&program.path)
            .is_some_and(|lines| lines.contains(&location.line));
        let reason = requested_reason.or(has_breakpoint.then_some("breakpoint"));
        let Some(reason) = reason else {
            return;
        };
        let source_line = program
            .source
            .lines()
            .nth(location.line.saturating_sub(1) as usize)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let bindings = bindings
            .into_iter()
            .map(|(name, value)| {
                (
                    name,
                    value.redacted(&self.runner_options.redacted_json_fields),
                )
            })
            .collect();
        *lock(&self.paused) = Some(PausedFrame {
            test_name: test.name.clone(),
            operation: operation_name(&step.operation),
            source_line,
            path: program.path,
            location,
            variables: DapVariableStore::new(bindings),
        });
        let _ = self
            .writer
            .event(
                "stopped",
                json!({
                    "reason": reason,
                    "description": format!("Paused before {}", operation_name(&step.operation)),
                    "threadId": THREAD_ID,
                    "allThreadsStopped": true,
                }),
            )
            .await;
        let _ = self.resume_receiver.lock().await.recv().await;
        *lock(&self.paused) = None;
    }

    async fn pause_after_app_failure(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        error: &StepError,
        bindings: BTreeMap<String, webtest_provider::Value>,
    ) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let TestOperation::ServerProviderCall(call) = &step.operation else {
            return;
        };
        if call.provider != "app" {
            return;
        }
        let StepError::Provider(provider_error) = error else {
            return;
        };
        let filter = if provider_error.is_infrastructure() {
            "appInfrastructure"
        } else {
            "appProviderFailure"
        };
        if !lock(&self.exception_breakpoints).contains(filter) {
            return;
        }
        let Some(program) = lock(&self.program).clone() else {
            return;
        };
        let location = StepLocation::new(&program.source, step.origin.range);
        let source_line = program
            .source
            .lines()
            .nth(location.line.saturating_sub(1) as usize)
            .unwrap_or_default()
            .trim()
            .to_owned();
        *lock(&self.paused) = Some(PausedFrame {
            test_name: test.name.clone(),
            operation: format!(
                "{} failed: {provider_error}",
                operation_name(&step.operation)
            ),
            source_line,
            path: program.path,
            location,
            variables: DapVariableStore::new(
                bindings
                    .into_iter()
                    .map(|(name, value)| {
                        (
                            name,
                            value.redacted(&self.runner_options.redacted_json_fields),
                        )
                    })
                    .collect(),
            ),
        });
        let _ = self
            .writer
            .event(
                "stopped",
                json!({
                    "reason": "exception",
                    "description": provider_error.to_string(),
                    "text": provider_error.code(),
                    "threadId": THREAD_ID,
                    "allThreadsStopped": true,
                }),
            )
            .await;
        let _ = self.resume_receiver.lock().await.recv().await;
        *lock(&self.paused) = None;
    }
}

#[async_trait]
impl RunControl for DebugState {
    fn is_cancelled(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    async fn before_step(&self, test: &PlannedTest, step: &PlannedStep) {
        self.pause_before_step(test, step, BTreeMap::new()).await;
    }

    fn should_capture_bindings(&self, _test: &PlannedTest, step: &PlannedStep) -> bool {
        if lock(&self.pending_pause).is_some() {
            return true;
        }
        let (path, line) = {
            let program = lock(&self.program);
            let Some(program) = program.as_ref() else {
                return false;
            };
            (
                program.path.clone(),
                StepLocation::new(&program.source, step.origin.range).line,
            )
        };
        lock(&self.breakpoints)
            .get(&path)
            .is_some_and(|lines| lines.contains(&line))
    }

    async fn before_step_with_bindings(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        bindings: BTreeMap<String, webtest_provider::Value>,
    ) {
        self.pause_before_step(test, step, bindings).await;
    }

    async fn after_step_failure(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        error: &StepError,
        bindings: &BTreeMap<String, webtest_provider::Value>,
    ) {
        self.pause_after_app_failure(test, step, error, bindings.clone())
            .await;
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

async fn serve_state(state: Arc<DebugState>) -> Result<(), DapError> {
    let mut requests = request_channel();
    let mut completion = state.completion.subscribe();
    loop {
        if *completion.borrow() {
            return Ok(());
        }
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else {
                    return Ok(());
                };
                if state.handle(request?).await? {
                    return Ok(());
                }
            }
            changed = completion.changed() => {
                let _ = changed;
                return Ok(());
            }
        }
    }
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
    targets: HashMap<i64, Arc<webtest_provider::Value>>,
    root: Vec<(String, Arc<webtest_provider::Value>)>,
    next_reference: i64,
}

impl DapVariableStore {
    fn new(bindings: BTreeMap<String, webtest_provider::Value>) -> Self {
        Self {
            variables: HashMap::new(),
            targets: HashMap::new(),
            root: bindings
                .into_iter()
                .map(|(name, value)| (name, Arc::new(value)))
                .collect(),
            next_reference: VARIABLES_REFERENCE + 1,
        }
    }

    fn variables(&mut self, reference: i64) -> Vec<Value> {
        if let Some(variables) = self.variables.get(&reference) {
            return variables.clone();
        }
        let values = if reference == VARIABLES_REFERENCE {
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

    fn provider_variable(&mut self, name: &str, value: Arc<webtest_provider::Value>) -> Value {
        let child_shape = dap_child_shape(&value);
        let reference = if child_shape.is_some() {
            let reference = self.next_reference;
            self.next_reference += 1;
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

fn dap_child_shape(value: &webtest_provider::Value) -> Option<DapChildShape> {
    use webtest_provider::Value as ProviderValue;

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

fn dap_leaf_variable(name: &str, value: &str, kind: &str) -> Value {
    dap_variable(name, value, kind, 0)
}

fn dap_variable(name: &str, value: &str, kind: &str, variables_reference: i64) -> Value {
    json!({
        "name": name,
        "value": value,
        "type": kind,
        "variablesReference": variables_reference,
    })
}

fn dap_children(value: &webtest_provider::Value) -> Vec<(String, Arc<webtest_provider::Value>)> {
    use webtest_provider::Value as ProviderValue;

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

fn dap_value(value: &webtest_provider::Value) -> String {
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
        values: impl IntoIterator<Item = (&'a str, &'a webtest_provider::Value)>,
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

    fn value(&mut self, value: &webtest_provider::Value) {
        use webtest_provider::Value as ProviderValue;

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

fn debug_bytes_value(bytes: &[u8]) -> webtest_provider::Value {
    let sample = &bytes[..bytes.len().min(DAP_VALUE_PREVIEW_BYTES)];
    match std::str::from_utf8(sample) {
        Ok(text) => webtest_provider::Value::String(text.to_owned()),
        Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => {
            let text = std::str::from_utf8(&sample[..error.valid_up_to()])
                .unwrap_or_default()
                .to_owned();
            webtest_provider::Value::String(text)
        }
        Err(_) => webtest_provider::Value::String(format!("<{} bytes>", bytes.len())),
    }
}

fn operation_name(operation: &TestOperation) -> String {
    match operation {
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
        webtest_plan::PlanExpr::Literal(webtest_provider::Value::String(value)) => {
            format!("{value:?}")
        }
        webtest_plan::PlanExpr::Binding(binding) => format!("binding_{}", binding.0),
        _ => "<expression>".into(),
    }
}

fn locator_name(locator: &webtest_plan::Locator) -> String {
    match locator {
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
        BrowserError, BrowserSession, InspectionTruncation, Locator, PageEvidence, PageInspection,
        PageSummary, RepairHint,
    };

    use super::*;

    struct UnusedBrowserHost;

    #[async_trait]
    impl BrowserHost for UnusedBrowserHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Err(BrowserError::Launch("unused in this test".into()))
        }
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
            operation_name(&program.plan.tests[0].steps[1].operation),
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
            .steps
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
        let step = program.plan.tests[0].steps[0].clone();
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
        let value = webtest_provider::Value::Response(webtest_provider::ResponseValue {
            status: 201,
            headers: [("content-type".into(), "application/json".into())]
                .into_iter()
                .collect(),
            body: br#"{"id":7,"email":"alice@example.test"}"#.to_vec(),
            json: Some(Box::new(webtest_provider::Value::Record(
                [
                    (
                        "email".into(),
                        webtest_provider::Value::String("alice@example.test".into()),
                    ),
                    ("id".into(), webtest_provider::Value::Int(7)),
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

    #[tokio::test]
    async fn breakpoint_pauses_before_the_step_until_continue() {
        let source = "test \"x\" {\n    browser {\n        open \"about:blank\"\n        click id(\"submit\")\n    }\n}\n";
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("debug.webtest");
        let program = LoadedProgram::load(path, Some(source.into())).expect("program");
        let test = program.plan.tests[0].clone();
        let step = test.steps[1].clone();
        let state = DebugState::new_with_options(
            ProtocolWriter::new(tokio::io::sink()),
            Arc::new(UnusedBrowserHost),
            RunnerOptions::default(),
        );
        *lock(&state.program) = Some(program.clone());
        assert!(
            !state.should_capture_bindings(&test, &step),
            "uninterrupted steps must not build debugger variable snapshots"
        );
        lock(&state.breakpoints).insert(program.path.clone(), HashSet::from([4]));
        *lock(&state.program) = Some(program);
        assert!(state.should_capture_bindings(&test, &step));

        let bindings = BTreeMap::from([(
            "user".into(),
            webtest_provider::Value::Record(
                [
                    (
                        "email".into(),
                        webtest_provider::Value::String("alice@example.test".into()),
                    ),
                    (
                        "token".into(),
                        webtest_provider::Value::String("private".into()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        )]);
        let paused_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            paused_state
                .before_step_with_bindings(&test, &step, bindings)
                .await;
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while lock(&state.paused).is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("breakpoint pause");

        {
            let mut paused = lock(&state.paused);
            let frame = paused.as_mut().expect("paused frame");
            assert_eq!(frame.location.line, 4);
            assert_eq!(frame.operation, "click id(\"submit\")");
            let variables = frame.variables.variables(VARIABLES_REFERENCE);
            let user = variables
                .iter()
                .find(|variable| variable["name"] == "user")
                .expect("user variable");
            assert!(user["value"].as_str().is_some_and(|value| {
                value.contains("[redacted]") && !value.contains("private")
            }));
        }
        *lock(&state.pending_pause) = Some("step");
        state
            .resume(
                &Request {
                    seq: 1,
                    message_type: "request".into(),
                    command: "continue".into(),
                    arguments: json!({ "threadId": THREAD_ID }),
                },
                ResumeCommand::Continue,
            )
            .await
            .expect("continue");
        task.await.expect("control task");
        assert!(lock(&state.paused).is_none());
        assert!(
            lock(&state.pending_pause).is_none(),
            "continue must cancel queued step mode"
        );
    }

    #[tokio::test]
    async fn disconnect_releases_a_paused_step_as_a_cancelled_runtime_outcome() {
        let source = "test \"x\" {\n    server {\n        let value = 1\n        expect value == 1\n    }\n}\n";
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("cancel.webtest");
        let program = LoadedProgram::load(path, Some(source.into())).expect("program");
        let state = DebugState::new_with_options(
            ProtocolWriter::new(tokio::io::sink()),
            Arc::new(UnusedBrowserHost),
            RunnerOptions::default(),
        );
        lock(&state.breakpoints).insert(program.path.clone(), HashSet::from([3]));
        *lock(&state.program) = Some(program.clone());

        let run_state = Arc::clone(&state);
        let plan = program.plan.clone();
        let task = tokio::spawn(async move {
            Runner::new(Arc::new(ObservationStore::default()))
                .run_with_control(&plan, &UnusedBrowserHost, Some(run_state.as_ref()))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while lock(&state.paused).is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("breakpoint pause");

        let disconnected = state
            .handle(Request {
                seq: 2,
                message_type: "request".into(),
                command: "disconnect".into(),
                arguments: json!({ "terminateDebuggee": true }),
            })
            .await
            .expect("disconnect response");
        assert!(disconnected);

        let result = task.await.expect("runtime task");
        assert!(matches!(
            result.outcome,
            RunOutcome::Cancelled {
                reason: webtest_runtime::CancellationReason::Requested
            }
        ));
        assert!(matches!(
            result.tests[0].outcome,
            TestOutcome::Cancelled {
                reason: webtest_runtime::CancellationReason::Requested
            }
        ));
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(
                    event,
                    webtest_observation::ExecutionEvent::TestFinished { .. }
                ))
                .count(),
            1
        );
        assert!(matches!(
            result.events.last(),
            Some(webtest_observation::ExecutionEvent::RunFinished {
                outcome: webtest_observation::RunOutcomeKind::Cancelled,
                ..
            })
        ));
    }
}
