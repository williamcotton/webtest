//! Debug Adapter Protocol transport over the source-mapped WebTest runtime.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter,
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use webtest_analysis::{AnalysisDatabase, DiagnosticSeverity};
use webtest_browser::BrowserHost;
use webtest_observation::ObservationStore;
use webtest_plan::{
    AssertionOperation, BrowserOperation, PlannedStep, PlannedTest, TestOperation, TestPlan,
};
use webtest_runtime::{RunControl, Runner, RunnerOptions};
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
    fn load(path: PathBuf, source_override: Option<String>) -> Result<Self, String> {
        let path = normalize_path(&path);
        let source = match source_override {
            Some(source) => source,
            None => std::fs::read_to_string(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?,
        };
        let mut database = AnalysisDatabase::default();
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

#[derive(Clone)]
struct PausedFrame {
    test_name: String,
    operation: String,
    source_line: String,
    path: PathBuf,
    location: StepLocation,
    bindings: BTreeMap<String, webtest_provider::Value>,
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
    program: Mutex<Option<LoadedProgram>>,
    breakpoints: Mutex<HashMap<PathBuf, HashSet<u32>>>,
    paused: Mutex<Option<PausedFrame>>,
    pending_pause: Mutex<Option<&'static str>>,
    resume_sender: mpsc::UnboundedSender<ResumeCommand>,
    resume_receiver: AsyncMutex<mpsc::UnboundedReceiver<ResumeCommand>>,
    configured: AtomicBool,
    started: AtomicBool,
    shutting_down: AtomicBool,
}

impl DebugState {
    fn new_with_options(
        writer: ProtocolWriter,
        browser: Arc<dyn BrowserHost>,
        runner_options: RunnerOptions,
    ) -> Arc<Self> {
        let (resume_sender, resume_receiver) = mpsc::unbounded_channel();
        Arc::new(Self {
            writer,
            browser,
            runner_options,
            program: Mutex::new(None),
            breakpoints: Mutex::new(HashMap::new()),
            paused: Mutex::new(None),
            pending_pause: Mutex::new(None),
            resume_sender,
            resume_receiver: AsyncMutex::new(resume_receiver),
            configured: AtomicBool::new(false),
            started: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
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
                        }),
                    )
                    .await?;
            }
            "launch" => self.launch(&request).await?,
            "setBreakpoints" => self.set_breakpoints(&request).await?,
            "setExceptionBreakpoints" => self.writer.response(&request, json!({})).await?,
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
        let loaded = match LoadedProgram::load(PathBuf::from(program), source_override) {
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
            .or_else(|| LoadedProgram::load(path.clone(), None).ok());
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
        let frame = lock(&self.paused).clone();
        let frames = frame.map_or_else(Vec::new, |frame| {
            vec![json!({
                "id": 1,
                "name": frame.operation,
                "source": { "name": source_name(&frame.path), "path": frame.path },
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
        let frame = lock(&self.paused).clone();
        let variables =
            if reference == Some(VARIABLES_REFERENCE) {
                frame.map_or_else(Vec::new, |frame| {
                    vec![
                        dap_variable("test", &format!("{:?}", frame.test_name), "string"),
                        dap_variable("operation", &format!("{:?}", frame.operation), "string"),
                        dap_variable("source", &format!("{:?}", frame.source_line), "string"),
                        dap_variable("line", &frame.location.line.to_string(), "number"),
                    ]
                    .into_iter()
                    .chain(frame.bindings.iter().map(|(name, value)| {
                        dap_variable(name, &dap_value(value), value.type_name())
                    }))
                    .collect()
                })
            } else {
                Vec::new()
            };
        self.writer
            .response(request, json!({ "variables": variables }))
            .await
    }

    async fn resume(&self, request: &Request, command: ResumeCommand) -> Result<(), DapError> {
        let body = if matches!(command, ResumeCommand::Continue) {
            json!({ "allThreadsContinued": true })
        } else {
            json!({})
        };
        self.writer.response(request, body).await?;
        if matches!(command, ResumeCommand::Step) {
            *lock(&self.pending_pause) = Some("step");
        }
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
            .with_options(self.runner_options.clone());
        let result = runner
            .run_with_control(&program.plan, self.browser.as_ref(), Some(self.as_ref()))
            .await;
        let exit_code = match result {
            Ok(result) => {
                for test in &result.tests {
                    let (category, status) = if test.passed {
                        ("stdout", "ok".to_owned())
                    } else {
                        let error = test
                            .failure
                            .as_ref()
                            .map(|failure| failure.error.to_string())
                            .unwrap_or_else(|| "failed".into());
                        ("stderr", format!("FAILED: {error}"))
                    };
                    let _ = self
                        .writer
                        .event(
                            "output",
                            json!({
                                "category": category,
                                "output": format!("test {:?} ... {status}\n", test.name),
                            }),
                        )
                        .await;
                }
                i32::from(result.failed() != 0)
            }
            Err(error) => {
                let _ = self
                    .writer
                    .event(
                        "output",
                        json!({
                            "category": "stderr",
                            "output": format!("browser infrastructure error: {error}\n"),
                        }),
                    )
                    .await;
                1
            }
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
            bindings,
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
}

#[async_trait]
impl RunControl for DebugState {
    async fn before_step(&self, test: &PlannedTest, step: &PlannedStep) {
        self.pause_before_step(test, step, BTreeMap::new()).await;
    }

    async fn before_step_with_bindings(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        bindings: &BTreeMap<String, webtest_provider::Value>,
    ) {
        self.pause_before_step(test, step, bindings.clone()).await;
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
    let mut reader = BufReader::new(tokio::io::stdin());
    while let Some(request) = read_request(&mut reader).await? {
        if state.handle(request).await? {
            break;
        }
    }
    Ok(())
}

async fn read_request(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> Result<Option<Request>, DapError> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        let read = reader.read_line(&mut header).await?;
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
    reader.read_exact(&mut body).await?;
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

fn dap_variable(name: &str, value: &str, kind: &str) -> Value {
    json!({
        "name": name,
        "value": value,
        "type": kind,
        "variablesReference": 0,
    })
}

fn dap_value(value: &webtest_provider::Value) -> String {
    let mut rendered = webtest_provider::value_to_json(value)
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| format!("<{}>", value.type_name()));
    if rendered.len() > 1024 {
        let mut end = 1024;
        while !rendered.is_char_boundary(end) {
            end -= 1;
        }
        rendered.truncate(end);
        rendered.push_str("...");
    }
    rendered
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
    use webtest_browser::{BrowserError, BrowserSession};

    use super::*;

    struct UnusedBrowserHost;

    #[async_trait]
    impl BrowserHost for UnusedBrowserHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Err(BrowserError::Launch("unused in this test".into()))
        }
    }

    #[tokio::test]
    async fn reads_content_length_framed_requests() {
        let body = r#"{"seq":7,"type":"request","command":"threads"}"#;
        let message = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut reader = BufReader::new(message.as_bytes());
        let request = read_request(&mut reader)
            .await
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
    fn locations_use_one_based_utf16_columns() {
        let source = "😀 click id(\"x\")";
        let start = source.find("id").expect("locator");
        let range = TextRange::new(
            webtest_text::TextSize::from(start as u32),
            webtest_text::TextSize::from((start + 7) as u32),
        );
        assert_eq!(StepLocation::new(source, range).column, 10);
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
        lock(&state.breakpoints).insert(program.path.clone(), HashSet::from([4]));
        *lock(&state.program) = Some(program);

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
                .before_step_with_bindings(&test, &step, &bindings)
                .await;
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while lock(&state.paused).is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("breakpoint pause");

        let frame = lock(&state.paused).clone().expect("paused frame");
        assert_eq!(frame.location.line, 4);
        assert_eq!(frame.operation, "click id(\"submit\")");
        assert_eq!(
            frame.bindings["user"].member("token"),
            Some(webtest_provider::Value::String("[redacted]".into()))
        );
        state
            .resume_sender
            .send(ResumeCommand::Continue)
            .expect("continue");
        task.await.expect("control task");
        assert!(lock(&state.paused).is_none());
    }
}
