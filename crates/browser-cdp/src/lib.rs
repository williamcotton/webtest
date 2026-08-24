//! Direct Chrome DevTools Protocol implementation of the browser abstraction.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    process::{Child, Command as ProcessCommand},
    sync::{Mutex, mpsc, oneshot},
    time::{Instant, interval, sleep, timeout},
};
use tokio_tungstenite::tungstenite::Message;
use tracing::instrument;
use url::Url;
use webtest_browser::{
    Action, BrowserContext, BrowserContextOptions, BrowserError, BrowserHost, BrowserSession,
    CandidateEvidence, ElementStates, EvidenceRequest, INSPECTION_SCHEMA_VERSION,
    InspectableElement, InspectionOptions, InspectionTruncation, Locator, LocatorCandidate,
    LocatorCandidateKind, LocatorState, Page, PageEvidence, PageInspection, PageSummary,
    SupportedAction,
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const CORRELATION_SWEEP: Duration = Duration::from_millis(10);
static NEXT_SNAPSHOT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct ChromeHost {
    executable: Option<PathBuf>,
    headless: bool,
    command_timeout: Duration,
    navigation_timeout: Duration,
}

impl ChromeHost {
    pub fn new(executable: Option<PathBuf>) -> Self {
        Self {
            executable,
            headless: true,
            command_timeout: COMMAND_TIMEOUT,
            navigation_timeout: LOAD_TIMEOUT,
        }
    }

    pub fn with_headed(mut self, headed: bool) -> Self {
        self.headless = !headed;
        self
    }

    pub fn with_timeouts(mut self, command: Duration, navigation: Duration) -> Self {
        self.command_timeout = command;
        self.navigation_timeout = navigation;
        self
    }

    pub fn locate(&self) -> Option<PathBuf> {
        self.executable
            .clone()
            .or_else(|| std::env::var_os("WEBTEST_CHROME_PATH").map(PathBuf::from))
            .or_else(find_system_chrome)
    }
}

#[async_trait]
impl BrowserHost for ChromeHost {
    #[instrument(skip_all)]
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
        let executable = self.locate().ok_or_else(|| {
            BrowserError::Launch(
                "Chrome was not found; set WEBTEST_CHROME_PATH or pass --chrome-path".into(),
            )
        })?;
        let (process, websocket_url) = ChromeProcess::launch(&executable, self.headless).await?;
        let connection = CdpConnection::connect(&websocket_url, self.command_timeout).await?;
        Ok(Box::new(CdpBrowserSession {
            process: Some(process),
            connection,
            navigation_timeout: self.navigation_timeout,
        }))
    }
}

impl Default for ChromeHost {
    fn default() -> Self {
        Self::new(None)
    }
}

struct ChromeProcess {
    child: Child,
    profile: Option<TempDir>,
}

impl ChromeProcess {
    async fn launch(executable: &Path, headless: bool) -> Result<(Self, String), BrowserError> {
        let profile =
            tempfile::tempdir().map_err(|error| BrowserError::Launch(error.to_string()))?;
        let mut command = ProcessCommand::new(executable);
        if headless {
            command.arg("--headless=new");
        }
        let mut child = command
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| BrowserError::Launch(error.to_string()))?;

        let port_file = profile.path().join("DevToolsActivePort");
        let contents = timeout(STARTUP_TIMEOUT, async {
            loop {
                match tokio::fs::read_to_string(&port_file).await {
                    Ok(contents) if contents.lines().take(2).count() == 2 => break Ok(contents),
                    Ok(_) => {
                        if let Some(status) = child
                            .try_wait()
                            .map_err(|error| BrowserError::Launch(error.to_string()))?
                        {
                            break Err(BrowserError::BrowserCrashed {
                                status: format!("exited before CDP became available ({status})"),
                            });
                        }
                        sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if let Some(status) = child
                            .try_wait()
                            .map_err(|error| BrowserError::Launch(error.to_string()))?
                        {
                            break Err(BrowserError::BrowserCrashed {
                                status: format!("exited before CDP became available ({status})"),
                            });
                        }
                        sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) => break Err(BrowserError::Launch(error.to_string())),
                }
            }
        })
        .await
        .map_err(|_| BrowserError::Launch("timed out waiting for Chrome to start".into()))??;

        let mut lines = contents.lines();
        let port = lines.next().ok_or_else(|| {
            BrowserError::Launch("DevToolsActivePort did not contain a port".into())
        })?;
        let path = lines.next().ok_or_else(|| {
            BrowserError::Launch("DevToolsActivePort did not contain a WebSocket path".into())
        })?;
        let websocket_url = format!("ws://127.0.0.1:{port}{path}");
        Ok((
            Self {
                child,
                profile: Some(profile),
            },
            websocket_url,
        ))
    }

    async fn shutdown(&mut self) -> Result<(), BrowserError> {
        let process_result = match timeout(SHUTDOWN_GRACE, self.child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(BrowserError::Launch(format!(
                "could not reap Chrome: {error}"
            ))),
            Err(_) => {
                self.child.start_kill().map_err(|error| {
                    BrowserError::Launch(format!("could not terminate Chrome: {error}"))
                })?;
                timeout(SHUTDOWN_GRACE, self.child.wait())
                    .await
                    .map_err(|_| BrowserError::Launch("timed out while reaping Chrome".into()))?
                    .map_err(|error| {
                        BrowserError::Launch(format!("could not reap Chrome: {error}"))
                    })?;
                Ok(())
            }
        };
        let profile_result = self.cleanup_profile().await;
        process_result.and(profile_result)
    }

    async fn cleanup_profile(&mut self) -> Result<(), BrowserError> {
        let Some(profile) = self.profile.take() else {
            return Ok(());
        };
        let path = profile.keep();
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match tokio::fs::remove_dir_all(&path).await {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) if Instant::now() >= deadline => {
                    return Err(BrowserError::Launch(format!(
                        "could not remove temporary Chrome profile {}: {error}",
                        path.display()
                    )));
                }
                Err(_) => sleep(Duration::from_millis(25)).await,
            }
        }
    }
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let Some(profile) = self.profile.take() else {
            return;
        };
        let path = profile.keep();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let deadline = Instant::now() + SHUTDOWN_GRACE;
                loop {
                    match tokio::fs::remove_dir_all(&path).await {
                        Ok(()) => break,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                        Err(_) if Instant::now() >= deadline => break,
                        Err(_) => sleep(Duration::from_millis(25)).await,
                    }
                }
            });
        } else {
            let _ = fs_err_remove_dir_all(&path);
        }
    }
}

fn fs_err_remove_dir_all(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[derive(Clone)]
struct CdpConnection {
    sender: mpsc::Sender<OutgoingCommand>,
    command_timeout: Duration,
    in_flight: Arc<AtomicUsize>,
    console_errors: Arc<Mutex<Vec<String>>>,
}

struct OutgoingCommand {
    method: String,
    params: Option<Value>,
    session_id: Option<String>,
    response: oneshot::Sender<Result<Value, BrowserError>>,
    deadline: Instant,
}

struct PendingCommand {
    method: String,
    session_id: Option<String>,
    response: oneshot::Sender<Result<Value, BrowserError>>,
    deadline: Instant,
}

#[derive(Serialize)]
struct Command<'a> {
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a Value>,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct IncomingMessage {
    id: Option<u64>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    result: Option<Value>,
    error: Option<CdpError>,
    method: Option<String>,
    params: Option<Value>,
}

#[derive(Deserialize)]
struct CdpError {
    code: i64,
    message: String,
}

impl CdpConnection {
    async fn connect(url: &str, command_timeout: Duration) -> Result<Self, BrowserError> {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|error| {
                BrowserError::Launch(format!("could not connect to Chrome: {error}"))
            })?;
        let (mut writer, mut reader) = socket.split();
        let (sender, mut receiver) = mpsc::channel::<OutgoingCommand>(32);
        let in_flight = Arc::new(AtomicUsize::new(0));
        let actor_in_flight = Arc::clone(&in_flight);
        let console_errors = Arc::new(Mutex::new(Vec::new()));
        let actor_console_errors = Arc::clone(&console_errors);

        tokio::spawn(async move {
            let mut next_id = 1u64;
            let mut pending = HashMap::<u64, PendingCommand>::new();
            let mut sweep = interval(CORRELATION_SWEEP);
            let terminal = loop {
                tokio::select! {
                    outgoing = receiver.recv() => {
                        let Some(outgoing) = outgoing else {
                            break BrowserError::BrowserDisconnected;
                        };
                        let id = next_id;
                        next_id = next_id.wrapping_add(1).max(1);
                        let command = Command {
                            id,
                            method: &outgoing.method,
                            params: outgoing.params.as_ref(),
                            session_id: outgoing.session_id.as_deref(),
                        };
                        let encoded = match serde_json::to_string(&command) {
                            Ok(encoded) => encoded,
                            Err(error) => {
                                let _ = outgoing.response.send(Err(BrowserError::Protocol {
                                    method: outgoing.method,
                                    message: error.to_string(),
                                }));
                                continue;
                            }
                        };
                        let method = outgoing.method.clone();
                        let session_id = outgoing.session_id.clone();
                        pending.insert(id, PendingCommand {
                            method,
                            session_id,
                            response: outgoing.response,
                            deadline: outgoing.deadline,
                        });
                        actor_in_flight.fetch_add(1, Ordering::Relaxed);
                        let remaining = outgoing.deadline.saturating_duration_since(Instant::now());
                        match timeout(remaining, writer.send(Message::Text(encoded.into()))).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => break BrowserError::BrowserDisconnected,
                            Err(_) => {
                                if let Some(command) = pending.remove(&id) {
                                    actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                                    let _ = command.response.send(Err(BrowserError::CommandTimeout {
                                        method: command.method,
                                        timeout_ms: duration_millis(command_timeout),
                                    }));
                                }
                            }
                        }
                    }
                    incoming = reader.next() => {
                        let message = match incoming {
                            Some(Ok(message)) => message,
                            Some(Err(_)) | None => break BrowserError::BrowserDisconnected,
                        };
                        let text = match message {
                            Message::Text(text) => text,
                            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                            Message::Close(_) => break BrowserError::BrowserDisconnected,
                            Message::Binary(_) => break BrowserError::MalformedProtocol {
                                message: "Chrome sent a binary CDP response".into(),
                            },
                        };
                        let message = match serde_json::from_str::<IncomingMessage>(&text) {
                            Ok(message) => message,
                            Err(error) => break BrowserError::MalformedProtocol {
                                message: error.to_string(),
                            },
                        };
                        let Some(id) = message.id else {
                            if let Some(entry) = console_error(&message) {
                                let mut errors = actor_console_errors.lock().await;
                                if errors.len() == 20 { errors.remove(0); }
                                errors.push(entry);
                            }
                            continue
                        };
                        let Some(pending_command) = pending.remove(&id) else {
                            tracing::warn!(id, "Chrome returned a response for an unknown CDP command");
                            continue;
                        };
                        actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                        let result = if message.session_id != pending_command.session_id {
                            Err(BrowserError::MalformedProtocol {
                                message: format!(
                                    "response {id} used session {:?}, expected {:?}",
                                    message.session_id, pending_command.session_id
                                ),
                            })
                        } else if let Some(error) = message.error {
                            Err(BrowserError::Protocol {
                                method: pending_command.method,
                                message: format!("{} ({})", error.message, error.code),
                            })
                        } else if let Some(result) = message.result {
                            Ok(result)
                        } else {
                            Err(BrowserError::MalformedProtocol {
                                message: format!("response {id} contained neither result nor error"),
                            })
                        };
                        let _ = pending_command.response.send(result);
                    }
                    _ = sweep.tick() => {
                        let now = Instant::now();
                        let expired = pending
                            .iter()
                            .filter_map(|(id, command)| {
                                (command.deadline <= now || command.response.is_closed()).then_some(*id)
                            })
                            .collect::<Vec<_>>();
                        for id in expired {
                            let Some(command) = pending.remove(&id) else { continue };
                            actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                            if !command.response.is_closed() {
                                let _ = command.response.send(Err(BrowserError::CommandTimeout {
                                    method: command.method,
                                    timeout_ms: duration_millis(command_timeout),
                                }));
                            }
                        }
                    }
                }
            };
            for (_, command) in pending {
                actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                let _ = command.response.send(Err(terminal.clone()));
            }
        });

        Ok(Self {
            sender,
            command_timeout,
            in_flight,
            console_errors,
        })
    }

    #[instrument(skip_all, fields(method))]
    async fn command(
        &self,
        method: &str,
        params: Option<Value>,
        session_id: Option<&str>,
    ) -> Result<Value, BrowserError> {
        let (response, receive) = oneshot::channel();
        self.sender
            .send(OutgoingCommand {
                method: method.to_owned(),
                params,
                session_id: session_id.map(str::to_owned),
                response,
                deadline: Instant::now() + self.command_timeout,
            })
            .await
            .map_err(|_| BrowserError::BrowserDisconnected)?;
        let result = receive
            .await
            .map_err(|_| BrowserError::BrowserDisconnected)?;
        tracing::trace!(
            pending_commands = self.in_flight.load(Ordering::Relaxed),
            "completed CDP command"
        );
        result
    }

    #[cfg(test)]
    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    async fn console_errors(&self) -> Vec<String> {
        self.console_errors.lock().await.clone()
    }
}

fn console_error(message: &IncomingMessage) -> Option<String> {
    match message.method.as_deref()? {
        "Runtime.exceptionThrown" => message
            .params
            .as_ref()?
            .pointer("/exceptionDetails/text")
            .and_then(Value::as_str)
            .map(bounded_text),
        "Runtime.consoleAPICalled"
            if message.params.as_ref()?.get("type").and_then(Value::as_str) == Some("error") =>
        {
            Some("console.error".into())
        }
        _ => None,
    }
}

fn bounded_text(value: &str) -> String {
    value.chars().take(256).collect()
}

struct CdpBrowserSession {
    process: Option<ChromeProcess>,
    connection: CdpConnection,
    navigation_timeout: Duration,
}

#[async_trait]
impl BrowserSession for CdpBrowserSession {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        let (page, _) = create_page(
            &self.connection,
            self.navigation_timeout,
            None,
            &BrowserContextOptions::default(),
        )
        .await?;
        Ok(Box::new(page))
    }

    async fn new_context(
        &mut self,
        options: &BrowserContextOptions,
    ) -> Result<Box<dyn BrowserContext>, BrowserError> {
        let created = self
            .connection
            .command("Target.createBrowserContext", Some(json!({})), None)
            .await?;
        let context_id = string_field(&created, "browserContextId", "Target.createBrowserContext")?;
        Ok(Box::new(CdpBrowserContext {
            connection: self.connection.clone(),
            context_id: Some(context_id),
            options: options.clone(),
            navigation_timeout: self.navigation_timeout,
            target_ids: Vec::new(),
        }))
    }

    async fn close(&mut self) -> Result<(), BrowserError> {
        let graceful = self.connection.command("Browser.close", None, None).await;
        let Some(mut process) = self.process.take() else {
            return graceful.map(|_| ());
        };
        let shutdown = process.shutdown().await;
        match (graceful, shutdown) {
            (_, Err(error)) => Err(error),
            (Ok(_), Ok(())) | (Err(BrowserError::BrowserDisconnected), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
        }
    }
}

struct CdpBrowserContext {
    connection: CdpConnection,
    context_id: Option<String>,
    options: BrowserContextOptions,
    navigation_timeout: Duration,
    target_ids: Vec<String>,
}

#[async_trait]
impl BrowserContext for CdpBrowserContext {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        let context_id = self
            .context_id
            .as_deref()
            .ok_or_else(|| BrowserError::Protocol {
                method: "BrowserContext.new_page".into(),
                message: "browser context is already closed".into(),
            })?;
        let (page, target_id) = create_page(
            &self.connection,
            self.navigation_timeout,
            Some(context_id),
            &self.options,
        )
        .await?;
        self.target_ids.push(target_id);
        Ok(Box::new(page))
    }

    async fn close(&mut self) -> Result<(), BrowserError> {
        let Some(context_id) = self.context_id.take() else {
            return Ok(());
        };
        self.target_ids.clear();
        self.connection
            .command(
                "Target.disposeBrowserContext",
                Some(json!({ "browserContextId": context_id })),
                None,
            )
            .await
            .map(|_| ())
    }
}

async fn create_page(
    connection: &CdpConnection,
    navigation_timeout: Duration,
    context_id: Option<&str>,
    options: &BrowserContextOptions,
) -> Result<(CdpPage, String), BrowserError> {
    let mut params = json!({ "url": "about:blank" });
    if let Some(context_id) = context_id {
        params["browserContextId"] = Value::String(context_id.into());
    }
    let target = connection
        .command("Target.createTarget", Some(params), None)
        .await?;
    let target_id = string_field(&target, "targetId", "Target.createTarget")?;
    let attached = connection
        .command(
            "Target.attachToTarget",
            Some(json!({ "targetId": target_id, "flatten": true })),
            None,
        )
        .await?;
    let session_id = string_field(&attached, "sessionId", "Target.attachToTarget")?;
    connection
        .command("Page.enable", None, Some(&session_id))
        .await?;
    connection
        .command("Runtime.enable", None, Some(&session_id))
        .await?;
    connection
        .command(
            "Emulation.setDeviceMetricsOverride",
            Some(json!({
                "width": options.viewport.width,
                "height": options.viewport.height,
                "deviceScaleFactor": 1,
                "mobile": false
            })),
            Some(&session_id),
        )
        .await?;
    Ok((
        CdpPage {
            connection: connection.clone(),
            session_id,
            navigation_timeout,
            test_id_attribute: options.test_id_attribute.clone(),
        },
        target_id,
    ))
}

struct CdpPage {
    connection: CdpConnection,
    session_id: String,
    navigation_timeout: Duration,
    test_id_attribute: String,
}

#[async_trait]
impl Page for CdpPage {
    async fn open(&mut self, url: &str) -> Result<(), BrowserError> {
        let navigation = self
            .connection
            .command(
                "Page.navigate",
                Some(json!({ "url": url })),
                Some(&self.session_id),
            )
            .await?;
        if let Some(reason) = navigation.get("errorText").and_then(Value::as_str) {
            return Err(BrowserError::NavigationFailed {
                url: url.to_owned(),
                reason: reason.to_owned(),
            });
        }

        let deadline = Instant::now() + self.navigation_timeout;
        loop {
            let ready = self
                .connection
                .command(
                    "Runtime.evaluate",
                    Some(json!({
                        "expression": "document.readyState",
                        "returnByValue": true
                    })),
                    Some(&self.session_id),
                )
                .await?;
            let state = ready.pointer("/result/value").and_then(Value::as_str);
            if matches!(state, Some("interactive" | "complete")) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(BrowserError::NavigationTimeout {
                    url: url.to_owned(),
                    timeout_ms: duration_millis(self.navigation_timeout),
                });
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        self.perform(
            &Action::Click {
                locator: locator.clone(),
            },
            Duration::from_secs(5),
        )
        .await
    }

    async fn expect_visible(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        self.wait_for_locator(locator, LocatorState::Visible, Duration::from_secs(5))
            .await
    }

    async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError> {
        let evaluation = self.evaluate_expression(expression.to_owned()).await?;
        if let Some(message) = evaluation.get("errorText").and_then(Value::as_str) {
            return Err(BrowserError::EvaluationFailed {
                expression: expression.to_owned(),
                message: message.to_owned(),
            });
        }
        if let Some(message) = evaluation
            .pointer("/exceptionDetails/exception/description")
            .and_then(Value::as_str)
            .or_else(|| {
                evaluation
                    .pointer("/exceptionDetails/text")
                    .and_then(Value::as_str)
            })
        {
            return Err(BrowserError::EvaluationFailed {
                expression: expression.to_owned(),
                message: message.to_owned(),
            });
        }
        Ok(())
    }

    async fn perform(&mut self, action: &Action, timeout: Duration) -> Result<(), BrowserError> {
        let snapshot = self.wait_for_actionability(action, timeout).await?;
        match action {
            Action::Click { .. } => self.physical_click(&snapshot).await,
            Action::Hover { .. } => self.mouse_move(&snapshot).await,
            Action::Fill { value, .. } => {
                self.physical_click(&snapshot).await?;
                self.select_all().await?;
                self.key_event("Backspace", "Backspace", 0, None).await?;
                self.insert_text(value).await
            }
            Action::Type { value, .. } => {
                self.physical_click(&snapshot).await?;
                self.insert_text(value).await
            }
            Action::Press { key, .. } => {
                self.physical_click(&snapshot).await?;
                let key =
                    parse_key(key).ok_or_else(|| BrowserError::InvalidKey { key: key.clone() })?;
                self.key_event(&key.key, &key.code, key.modifiers, key.text.as_deref())
                    .await
            }
            Action::Check { locator, checked } => {
                if snapshot.checked == Some(*checked) {
                    Ok(())
                } else {
                    self.physical_click(&snapshot).await?;
                    let after = self.resolve(locator).await?;
                    if after.checked == Some(*checked) {
                        Ok(())
                    } else {
                        Err(BrowserError::AssertionFailed {
                            locator: locator.clone(),
                            expected: if *checked {
                                LocatorState::Checked
                            } else {
                                LocatorState::Unchecked
                            },
                            actual: format!("checked={:?}", after.checked),
                        })
                    }
                }
            }
            Action::Select { locator, option } => self.select_option(locator, option).await,
        }
    }

    async fn wait_for_locator(
        &mut self,
        locator: &Locator,
        state: LocatorState,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        let deadline = Instant::now() + timeout;
        let mut backoff = Duration::from_millis(20);
        loop {
            match self.resolve(locator).await {
                Err(BrowserError::LocatorInvalid { locator, message }) => {
                    return Err(BrowserError::LocatorInvalid { locator, message });
                }
                Err(error) => return Err(error),
                Ok(snapshot) => {
                    if state_satisfied(&snapshot, state) {
                        return Ok(());
                    }
                    let final_actual = snapshot.state_summary();
                    if snapshot.matches > 1 {
                        if Instant::now() >= deadline {
                            return Err(BrowserError::LocatorAmbiguous {
                                locator: locator.clone(),
                                matches: snapshot.matches,
                            });
                        }
                    } else if Instant::now() >= deadline {
                        if snapshot.matches == 0
                            && !matches!(state, LocatorState::Hidden | LocatorState::Detached)
                        {
                            return Err(BrowserError::LocatorNotFound {
                                locator: locator.clone(),
                            });
                        }
                        if state == LocatorState::Visible
                            && snapshot.matches == 1
                            && !snapshot.visible
                        {
                            return Err(BrowserError::LocatorNotVisible {
                                locator: locator.clone(),
                            });
                        }
                        return Err(BrowserError::AssertionFailed {
                            locator: locator.clone(),
                            expected: state,
                            actual: final_actual,
                        });
                    }
                }
            }
            sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_millis(100));
        }
    }

    async fn wait_for_url(
        &mut self,
        expected: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        let deadline = Instant::now() + timeout;
        loop {
            let actual = self.current_url().await?;
            if actual == expected {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(BrowserError::UrlMismatch {
                    expected: expected.into(),
                    actual,
                });
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    async fn current_url(&mut self) -> Result<String, BrowserError> {
        let result = self.evaluate_expression("location.href".into()).await?;
        evaluation_value(&result)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| invalid_evaluation("current URL was not a string"))
    }

    async fn capture_evidence(&mut self, request: &EvidenceRequest) -> PageEvidence {
        let mut evidence = PageEvidence::default();
        if request.include_screenshot {
            match self
                .connection
                .command(
                    "Page.captureScreenshot",
                    Some(json!({ "format": "png", "fromSurface": true })),
                    Some(&self.session_id),
                )
                .await
            {
                Ok(value) => {
                    match value.get("data").and_then(Value::as_str).and_then(|data| {
                        base64::engine::general_purpose::STANDARD.decode(data).ok()
                    }) {
                        Some(png) => evidence.screenshot_png = Some(png),
                        None => evidence
                            .capture_failures
                            .push("screenshot response was invalid".into()),
                    }
                }
                Err(error) => evidence
                    .capture_failures
                    .push(format!("screenshot: {error}")),
            }
        }
        let page_state = self
            .evaluate_expression("({url: location.href, title: document.title})".into())
            .await;
        match page_state.and_then(|result| {
            evaluation_value(&result)
                .cloned()
                .ok_or_else(|| invalid_evaluation("page state missing"))
        }) {
            Ok(value) => {
                evidence.current_url = value.get("url").and_then(Value::as_str).map(str::to_owned);
                evidence.title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Err(error) => evidence
                .capture_failures
                .push(format!("page state: {error}")),
        }
        if let Some(locator) = &request.locator {
            match self.resolve(locator).await {
                Ok(snapshot) => {
                    evidence.actionability = snapshot.actionability_facts();
                    evidence.candidates = snapshot.candidates;
                }
                Err(error) => evidence
                    .capture_failures
                    .push(format!("locator evidence: {error}")),
            }
        }
        if request.include_dom {
            let expression = "(() => { const root = document.documentElement.cloneNode(true); root.querySelectorAll('input,textarea').forEach(e => { e.removeAttribute('value'); if (e.tagName === 'TEXTAREA') e.textContent = ''; }); return '<!doctype html>' + root.outerHTML; })()";
            match self.evaluate_expression(expression.into()).await {
                Ok(result) => match evaluation_value(&result).and_then(Value::as_str) {
                    Some(dom) => {
                        evidence.dom_snapshot = Some(truncate_utf8(dom, request.max_dom_bytes))
                    }
                    None => evidence
                        .capture_failures
                        .push("DOM snapshot was not a string".into()),
                },
                Err(error) => evidence
                    .capture_failures
                    .push(format!("DOM snapshot: {error}")),
            }
        }
        evidence.console_errors = self.connection.console_errors().await;
        redact_evidence(
            &mut evidence,
            &request.redactions,
            &request.redacted_query_parameters,
        );
        evidence
    }

    async fn inspect(
        &mut self,
        options: &InspectionOptions,
    ) -> Result<PageInspection, BrowserError> {
        let options = options.bounded();
        let expression = inspection_expression(
            &self.test_id_attribute,
            options.include_hidden,
            options.max_elements,
        )?;
        let result = self.evaluate_expression(expression).await?;
        let value = evaluation_value(&result)
            .ok_or_else(|| invalid_evaluation("inspection result was missing"))?;
        let raw: RawInspection = serde_json::from_value(value.clone())
            .map_err(|error| invalid_evaluation(&error.to_string()))?;
        let returned_elements = raw.elements.len();
        let version = self
            .connection
            .command("Browser.getVersion", None, None)
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("product")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "unknown".into());
        let mut text_truncated = false;
        let mut candidates_truncated = false;
        let mut options_truncated = false;
        let mut elements = Vec::new();
        for raw_element in raw.elements {
            let built = self
                .inspectable_element(
                    raw_element,
                    &options,
                    &mut text_truncated,
                    &mut candidates_truncated,
                    &mut options_truncated,
                )
                .await?;
            if let Some(element) = built {
                elements.push(element);
            }
        }
        let omitted_elements = raw.total.saturating_sub(returned_elements);
        Ok(PageInspection {
            kind: "inspection".into(),
            inspection_schema_version: INSPECTION_SCHEMA_VERSION,
            snapshot_id: format!(
                "snapshot-{}",
                NEXT_SNAPSHOT_ID.fetch_add(1, Ordering::Relaxed)
            ),
            browser_version: truncate_utf8(&version, 128),
            page: PageSummary {
                url: redact_url_query(&raw.url, &options.redacted_query_parameters),
                title: redact_and_truncate(
                    &raw.title,
                    &options.redacted_values,
                    options.max_text_bytes,
                ),
            },
            elements,
            truncation: InspectionTruncation {
                elements_truncated: omitted_elements > 0,
                omitted_elements,
                candidates_truncated,
                text_truncated,
                options_truncated,
            },
        })
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

impl CdpPage {
    async fn evaluate_expression(&self, expression: String) -> Result<Value, BrowserError> {
        self.connection
            .command(
                "Runtime.evaluate",
                Some(json!({ "expression": expression, "returnByValue": true })),
                Some(&self.session_id),
            )
            .await
    }

    async fn resolve(&self, locator: &Locator) -> Result<ResolveSnapshot, BrowserError> {
        let expression = resolver_expression(locator, &self.test_id_attribute)?;
        let result = self.evaluate_expression(expression).await?;
        let value = evaluation_value(&result)
            .ok_or_else(|| invalid_evaluation("locator result was missing"))?;
        let snapshot: ResolveSnapshot = serde_json::from_value(value.clone())
            .map_err(|error| invalid_evaluation(&error.to_string()))?;
        if let Some(message) = snapshot.invalid.clone() {
            return Err(BrowserError::LocatorInvalid {
                locator: locator.clone(),
                message,
            });
        }
        Ok(snapshot)
    }

    async fn inspectable_element(
        &self,
        raw: RawInspectableElement,
        options: &InspectionOptions,
        text_truncated: &mut bool,
        candidates_truncated: &mut bool,
        options_truncated: &mut bool,
    ) -> Result<Option<InspectableElement>, BrowserError> {
        let (role, role_source) = bounded_field(raw.role, options, text_truncated);
        let (name, name_source) = bounded_field(raw.name, options, text_truncated);
        let (label, label_source) = bounded_field(raw.label, options, text_truncated);
        let (placeholder, placeholder_source) =
            bounded_field(raw.placeholder, options, text_truncated);
        let (test_id, test_id_source) = bounded_field(raw.test_id, options, text_truncated);
        let (dom_id, dom_id_source) = bounded_field(raw.dom_id, options, text_truncated);
        let (_text, text_source) = bounded_field(raw.text, options, text_truncated);

        let mut locators = Vec::new();
        if let Some(value) = label_source {
            locators.push((
                Locator::Label(value),
                LocatorCandidateKind::Label,
                "unique associated label",
            ));
        }
        if let Some(role_value) = role_source {
            locators.push((
                Locator::Role {
                    role: role_value,
                    name: name_source,
                },
                LocatorCandidateKind::Role,
                "unique accessible role and name",
            ));
        }
        if let Some(value) = test_id_source {
            locators.push((
                Locator::TestId(value),
                LocatorCandidateKind::TestId,
                "unique configured test ID",
            ));
        }
        if let Some(value) = dom_id_source {
            locators.push((
                Locator::Id(value),
                LocatorCandidateKind::Id,
                "unique DOM ID",
            ));
        }
        if let Some(value) = placeholder_source {
            locators.push((
                Locator::Placeholder(value),
                LocatorCandidateKind::Placeholder,
                "unique placeholder",
            ));
        }
        if let Some(value) = text_source {
            locators.push((
                Locator::Text(value),
                LocatorCandidateKind::Text,
                "unique exact user-facing text",
            ));
        }

        let mut validated = Vec::new();
        for (locator, kind, reason) in locators {
            let snapshot = self.resolve(&locator).await?;
            if snapshot.matches == 1 && snapshot.document_index == Some(raw.document_index) {
                validated.push(LocatorCandidate {
                    source: locator.to_string(),
                    kind,
                    reason: reason.into(),
                });
            }
        }
        validated.dedup_by(|left, right| left.source == right.source);
        if validated.is_empty() {
            return Ok(None);
        }
        if validated.len() > options.max_candidates_per_element {
            validated.truncate(options.max_candidates_per_element);
            *candidates_truncated = true;
        }
        let preferred_locator = validated.remove(0);
        let interactive = raw.interactive || role.is_some();
        let mut supported_actions = Vec::new();
        if raw.editable && raw.visible && !raw.disabled {
            supported_actions.extend([
                SupportedAction::Fill,
                SupportedAction::Type,
                SupportedAction::Press,
            ]);
        }
        if raw.clickable && raw.visible && !raw.disabled && !raw.obscured {
            supported_actions.push(SupportedAction::Click);
        }
        if raw.checkable && raw.visible && !raw.disabled {
            supported_actions.extend([SupportedAction::Check, SupportedAction::Uncheck]);
        }
        if raw.selectable && raw.visible && !raw.disabled {
            supported_actions.push(SupportedAction::Select);
        }
        if raw.hoverable && raw.visible && !raw.obscured {
            supported_actions.push(SupportedAction::Hover);
        }
        if raw.options.len() > 50 {
            *options_truncated = true;
        }
        Ok(Some(InspectableElement {
            role,
            accessible_name: name,
            label,
            placeholder,
            test_id,
            dom_id,
            states: ElementStates {
                visible: raw.visible,
                enabled: interactive.then_some(!raw.disabled),
                editable: raw.editable_applicable.then_some(raw.editable),
                checked: raw.checked,
                selected: raw.selected,
                receives_pointer_input: raw.hoverable.then_some(raw.visible && !raw.obscured),
            },
            supported_actions,
            preferred_locator,
            alternate_locators: validated,
            options: raw
                .options
                .into_iter()
                .take(50)
                .map(|option| {
                    redact_and_truncate(&option, &options.redacted_values, options.max_text_bytes)
                })
                .collect(),
        }))
    }

    async fn wait_for_actionability(
        &self,
        action: &Action,
        timeout: Duration,
    ) -> Result<ResolveSnapshot, BrowserError> {
        let locator = action.locator();
        let deadline = Instant::now() + timeout;
        let mut backoff = Duration::from_millis(20);
        let mut observed_failure = None;
        let mut failures_changed = false;
        loop {
            let first = self.resolve(locator).await?;
            let last_error = match first.matches {
                0 => BrowserError::LocatorNotFound {
                    locator: locator.clone(),
                },
                count if count > 1 => BrowserError::LocatorAmbiguous {
                    locator: locator.clone(),
                    matches: count,
                },
                _ if !first.visible => BrowserError::LocatorNotVisible {
                    locator: locator.clone(),
                },
                _ if first.disabled => BrowserError::ElementDisabled {
                    locator: locator.clone(),
                },
                _ if matches!(action, Action::Fill { .. } | Action::Type { .. })
                    && !first.editable =>
                {
                    BrowserError::ElementNotEditable {
                        locator: locator.clone(),
                    }
                }
                _ if matches!(action, Action::Select { .. })
                    && first.tag.as_deref() != Some("select") =>
                {
                    BrowserError::ElementNotEditable {
                        locator: locator.clone(),
                    }
                }
                _ if matches!(action, Action::Check { .. }) && first.checked.is_none() => {
                    BrowserError::ElementNotEditable {
                        locator: locator.clone(),
                    }
                }
                _ if matches!(
                    action,
                    Action::Click { .. } | Action::Hover { .. } | Action::Check { .. }
                ) && first.obscured =>
                {
                    BrowserError::ElementObscured {
                        locator: locator.clone(),
                    }
                }
                _ => {
                    sleep(Duration::from_millis(50)).await;
                    let second = self.resolve(locator).await?;
                    if second.matches == 0 {
                        BrowserError::ElementDetached {
                            locator: locator.clone(),
                        }
                    } else if !rect_stable(first.rect.as_ref(), second.rect.as_ref()) {
                        BrowserError::ElementUnstable {
                            locator: locator.clone(),
                        }
                    } else if second.obscured
                        && matches!(
                            action,
                            Action::Click { .. } | Action::Hover { .. } | Action::Check { .. }
                        )
                    {
                        BrowserError::ElementObscured {
                            locator: locator.clone(),
                        }
                    } else {
                        return Ok(second);
                    }
                }
            };
            if let Some(previous) = observed_failure {
                failures_changed |= previous != last_error.code();
            }
            observed_failure = Some(last_error.code());
            if Instant::now() >= deadline {
                return if failures_changed {
                    Err(BrowserError::ActionTimeout {
                        locator: locator.clone(),
                        timeout_ms: duration_millis(timeout),
                    })
                } else {
                    Err(last_error)
                };
            }
            sleep(backoff.min(deadline.saturating_duration_since(Instant::now()))).await;
            backoff = (backoff * 2).min(Duration::from_millis(100));
        }
    }

    async fn mouse_move(&self, snapshot: &ResolveSnapshot) -> Result<(), BrowserError> {
        let (x, y) = snapshot
            .center()
            .ok_or_else(|| invalid_evaluation("element had no interaction point"))?;
        self.connection
            .command(
                "Input.dispatchMouseEvent",
                Some(json!({
                    "type": "mouseMoved", "x": x, "y": y
                })),
                Some(&self.session_id),
            )
            .await
            .map(|_| ())
    }

    async fn physical_click(&self, snapshot: &ResolveSnapshot) -> Result<(), BrowserError> {
        let (x, y) = snapshot
            .center()
            .ok_or_else(|| invalid_evaluation("element had no interaction point"))?;
        self.mouse_move(snapshot).await?;
        self.connection
            .command(
                "Input.dispatchMouseEvent",
                Some(json!({
                    "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1
                })),
                Some(&self.session_id),
            )
            .await?;
        self.connection
            .command(
                "Input.dispatchMouseEvent",
                Some(json!({
                    "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1
                })),
                Some(&self.session_id),
            )
            .await
            .map(|_| ())
    }

    async fn insert_text(&self, value: &str) -> Result<(), BrowserError> {
        self.connection
            .command(
                "Input.insertText",
                Some(json!({ "text": value })),
                Some(&self.session_id),
            )
            .await
            .map(|_| ())
    }

    async fn select_all(&self) -> Result<(), BrowserError> {
        let modifiers = if cfg!(target_os = "macos") { 4 } else { 2 };
        self.connection
            .command(
                "Input.dispatchKeyEvent",
                Some(json!({
                    "type": "rawKeyDown", "key": "a", "code": "KeyA", "modifiers": modifiers,
                    "commands": ["selectAll"]
                })),
                Some(&self.session_id),
            )
            .await?;
        self.connection
            .command(
                "Input.dispatchKeyEvent",
                Some(json!({
                    "type": "keyUp", "key": "a", "code": "KeyA", "modifiers": modifiers
                })),
                Some(&self.session_id),
            )
            .await
            .map(|_| ())
    }

    async fn key_event(
        &self,
        key: &str,
        code: &str,
        modifiers: i32,
        text: Option<&str>,
    ) -> Result<(), BrowserError> {
        let mut down =
            json!({ "type": "keyDown", "key": key, "code": code, "modifiers": modifiers });
        if let Some(text) = text {
            down["text"] = Value::String(text.into());
        }
        self.connection
            .command("Input.dispatchKeyEvent", Some(down), Some(&self.session_id))
            .await?;
        self.connection
            .command(
                "Input.dispatchKeyEvent",
                Some(json!({
                    "type": "keyUp", "key": key, "code": code, "modifiers": modifiers
                })),
                Some(&self.session_id),
            )
            .await
            .map(|_| ())
    }

    async fn select_option(&self, locator: &Locator, option: &str) -> Result<(), BrowserError> {
        let elements = locator_array_expression(locator, &self.test_id_attribute)?;
        let option_json = serde_json::to_string(option)
            .map_err(|error| invalid_evaluation(&error.to_string()))?;
        let expression = format!(
            r#"(() => {{
            const norm = value => String(value || '').replace(/\s+/g, ' ').trim();
            const implicitRole = element => {{
                const tag = element.tagName.toLowerCase();
                if (tag === 'button') return 'button';
                if (tag === 'textarea') return 'textbox';
                if (tag === 'select') return 'combobox';
                if (tag === 'input') {{
                    if (element.type === 'checkbox') return 'checkbox';
                    if (element.type === 'radio') return 'radio';
                    return 'textbox';
                }}
                return element.getAttribute('role');
            }};
            const accessibleName = element => {{
                const labelledby = element.getAttribute('aria-labelledby');
                if (labelledby) return norm(labelledby.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' '));
                if (element.hasAttribute('aria-label')) return norm(element.getAttribute('aria-label'));
                if (element.labels?.length) return norm(Array.from(element.labels).map(label => {{
                    const copy = label.cloneNode(true);
                    copy.querySelectorAll('input,textarea,select,button').forEach(control => control.remove());
                    return copy.innerText || copy.textContent;
                }}).join(' '));
                return norm(element.innerText || element.textContent || element.title);
            }};
            try {{ const elements = {elements}; if (elements.length !== 1) return {{matches: elements.length}};
            const select = elements[0]; const wanted = {option_json};
            const options = Array.from(select.options || []).filter(o => o.value === wanted || norm(o.text) === wanted);
            if (options.length !== 1) return {{matches: 1, options: options.length}};
            select.value = options[0].value; select.dispatchEvent(new Event('input', {{bubbles:true}}));
            select.dispatchEvent(new Event('change', {{bubbles:true}})); return {{matches:1, options:1}};
            }} catch (error) {{ return {{invalid:String(error)}}; }} }})()"#
        );
        let result = self.evaluate_expression(expression).await?;
        let value =
            evaluation_value(&result).ok_or_else(|| invalid_evaluation("select result missing"))?;
        if let Some(message) = value.get("invalid").and_then(Value::as_str) {
            return Err(BrowserError::LocatorInvalid {
                locator: locator.clone(),
                message: bounded_text(message),
            });
        }
        match value.get("matches").and_then(Value::as_u64) {
            Some(0) => Err(BrowserError::LocatorNotFound {
                locator: locator.clone(),
            }),
            Some(1) if value.get("options").and_then(Value::as_u64) == Some(1) => Ok(()),
            Some(1) if value.get("options").and_then(Value::as_u64).unwrap_or(0) > 1 => {
                Err(BrowserError::OptionAmbiguous {
                    locator: locator.clone(),
                    option: option.into(),
                    matches: value.get("options").and_then(Value::as_u64).unwrap_or(0) as usize,
                })
            }
            Some(1) => Err(BrowserError::OptionNotFound {
                locator: locator.clone(),
                option: option.into(),
            }),
            Some(count) => Err(BrowserError::LocatorAmbiguous {
                locator: locator.clone(),
                matches: count as usize,
            }),
            None => Err(invalid_evaluation(
                "select result did not contain match count",
            )),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ResolveSnapshot {
    matches: usize,
    invalid: Option<String>,
    visible: bool,
    disabled: bool,
    editable: bool,
    checked: Option<bool>,
    obscured: bool,
    tag: Option<String>,
    rect: Option<ElementRect>,
    candidates: Vec<CandidateEvidence>,
    #[serde(default, rename = "documentIndex")]
    document_index: Option<usize>,
}

impl ResolveSnapshot {
    fn center(&self) -> Option<(f64, f64)> {
        self.rect
            .as_ref()
            .map(|rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0))
    }
    fn state_summary(&self) -> String {
        if self.matches == 0 {
            return "detached".into();
        }
        if self.matches > 1 {
            return format!("{} matches", self.matches);
        }
        format!(
            "visible={}, enabled={}, checked={:?}",
            self.visible, !self.disabled, self.checked
        )
    }
    fn actionability_facts(&self) -> Vec<String> {
        vec![
            format!("attached={}", self.matches == 1),
            format!("visible={}", self.visible),
            format!("enabled={}", !self.disabled),
            format!("editable={}", self.editable),
            format!("obscured={}", self.obscured),
        ]
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ElementRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn rect_stable(first: Option<&ElementRect>, second: Option<&ElementRect>) -> bool {
    let (Some(first), Some(second)) = (first, second) else {
        return false;
    };
    (first.x - second.x).abs() < 0.25
        && (first.y - second.y).abs() < 0.25
        && (first.width - second.width).abs() < 0.25
        && (first.height - second.height).abs() < 0.25
}

fn state_satisfied(snapshot: &ResolveSnapshot, state: LocatorState) -> bool {
    match state {
        LocatorState::Hidden => {
            snapshot.matches == 0 || (snapshot.matches == 1 && !snapshot.visible)
        }
        LocatorState::Detached => snapshot.matches == 0,
        LocatorState::Attached => snapshot.matches == 1,
        LocatorState::Visible => snapshot.matches == 1 && snapshot.visible,
        LocatorState::Enabled => snapshot.matches == 1 && !snapshot.disabled,
        LocatorState::Disabled => snapshot.matches == 1 && snapshot.disabled,
        LocatorState::Checked => snapshot.matches == 1 && snapshot.checked == Some(true),
        LocatorState::Unchecked => snapshot.matches == 1 && snapshot.checked == Some(false),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInspection {
    url: String,
    title: String,
    total: usize,
    elements: Vec<RawInspectableElement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInspectableElement {
    document_index: usize,
    role: Option<String>,
    name: Option<String>,
    label: Option<String>,
    placeholder: Option<String>,
    test_id: Option<String>,
    dom_id: Option<String>,
    text: Option<String>,
    visible: bool,
    disabled: bool,
    editable: bool,
    editable_applicable: bool,
    checked: Option<bool>,
    selected: Option<bool>,
    obscured: bool,
    interactive: bool,
    clickable: bool,
    checkable: bool,
    selectable: bool,
    hoverable: bool,
    #[serde(default)]
    options: Vec<String>,
}

fn bounded_field(
    value: Option<String>,
    options: &InspectionOptions,
    truncated: &mut bool,
) -> (Option<String>, Option<String>) {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return (None, None);
    };
    if options
        .redacted_values
        .iter()
        .any(|secret| !secret.is_empty() && value.contains(secret))
    {
        return (
            Some(redact_and_truncate(
                &value,
                &options.redacted_values,
                options.max_text_bytes,
            )),
            None,
        );
    }
    if value.len() <= options.max_text_bytes {
        return (Some(value.clone()), Some(value));
    }
    *truncated = true;
    (Some(truncate_utf8(&value, options.max_text_bytes)), None)
}

fn redact_and_truncate(value: &str, secrets: &[String], max_bytes: usize) -> String {
    let redacted = secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(value.to_owned(), |value, secret| {
            value.replace(secret, "[redacted]")
        });
    truncate_utf8(&redacted, max_bytes)
}

fn redact_url_query(value: &str, sensitive: &[String]) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return truncate_utf8(value, MAX_INSPECTION_URL_BYTES);
    };
    let pairs = url
        .query_pairs()
        .map(|(name, value)| {
            let replacement = if sensitive
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&name))
            {
                "[redacted]".to_owned()
            } else {
                value.into_owned()
            };
            (name.into_owned(), replacement)
        })
        .collect::<Vec<_>>();
    if !pairs.is_empty() {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    truncate_utf8(url.as_str(), MAX_INSPECTION_URL_BYTES)
}

const MAX_INSPECTION_URL_BYTES: usize = 4_096;

fn inspection_expression(
    test_id_attribute: &str,
    include_hidden: bool,
    max_elements: usize,
) -> Result<String, BrowserError> {
    let test_id_attribute = serde_json::to_string(test_id_attribute)
        .map_err(|error| invalid_evaluation(&error.to_string()))?;
    Ok(format!(
        r#"(() => {{
        const norm = value => String(value || '').replace(/\s+/g, ' ').trim();
        const bounded = value => {{ const text = norm(value); return text ? text.slice(0, 4097) : null; }};
        const implicitRole = element => {{
            const tag = element.tagName.toLowerCase();
            if (tag === 'button') return 'button';
            if (tag === 'a' && element.hasAttribute('href')) return 'link';
            if (tag === 'textarea') return 'textbox';
            if (tag === 'select') return 'combobox';
            if (tag === 'img') return 'img';
            if (tag === 'input') {{
                const type = (element.type || 'text').toLowerCase();
                if (['button','submit','reset'].includes(type)) return 'button';
                if (type === 'checkbox') return 'checkbox';
                if (type === 'radio') return 'radio';
                if (['text','email','password','search','tel','url'].includes(type)) return 'textbox';
            }}
            return null;
        }};
        const labelText = element => element.labels?.length ? norm(Array.from(element.labels).map(label => {{
            const copy = label.cloneNode(true);
            copy.querySelectorAll('input,textarea,select,button').forEach(control => control.remove());
            return copy.innerText || copy.textContent;
        }}).join(' ')) : '';
        const accessibleName = element => {{
            const labelledby = element.getAttribute('aria-labelledby');
            if (labelledby) return norm(labelledby.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' '));
            if (element.hasAttribute('aria-label')) return norm(element.getAttribute('aria-label'));
            const label = labelText(element); if (label) return label;
            if (element.tagName === 'IMG') return norm(element.alt);
            if (element.tagName === 'INPUT' && ['button','submit','reset'].includes(element.type)) return norm(element.value);
            return norm(element.innerText || element.textContent || element.title);
        }};
        const all = Array.from(document.querySelectorAll('body *'));
        const inspected = all.map((element, documentIndex) => {{
            if (['SCRIPT','STYLE','NOSCRIPT','TEMPLATE'].includes(element.tagName)) return null;
            const rect = element.getBoundingClientRect(), style = getComputedStyle(element);
            const visible = style.display !== 'none' && style.visibility !== 'hidden'
                && Number(style.opacity) !== 0 && rect.width > 0 && rect.height > 0;
            if (!{include_hidden} && !visible) return null;
            const tag = element.tagName.toLowerCase();
            const role = element.getAttribute('role') || implicitRole(element);
            const name = accessibleName(element);
            const label = labelText(element);
            const placeholder = element.getAttribute('placeholder') || '';
            const testId = element.getAttribute({test_id_attribute}) || '';
            const text = element.tagName === 'INPUT' && element.type === 'password'
                ? '' : norm(element.innerText || '');
            const leafText = text && !Array.from(element.children).some(child => norm(child.innerText) === text);
            const interactive = element.matches('button,a[href],input,textarea,select,[contenteditable=true],[role]');
            if (!(interactive || role || name || label || testId || leafText)) return null;
            const disabled = element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true';
            const editableApplicable = element.tagName === 'TEXTAREA' || element.isContentEditable
                || (element.tagName === 'INPUT' && !['button','submit','reset','checkbox','radio','file','hidden'].includes(element.type));
            const editable = editableApplicable && !disabled && !element.readOnly;
            const checkable = element.matches('input[type=checkbox],input[type=radio],[role=checkbox],[role=radio],[role=switch]');
            const checked = checkable ? (('checked' in element) ? Boolean(element.checked)
                : element.getAttribute('aria-checked') === 'true') : null;
            const selected = element.matches('option,[role=option]')
                ? (element.selected ?? element.getAttribute('aria-selected') === 'true') : null;
            const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
            const hit = visible ? document.elementFromPoint(x, y) : null;
            const obscured = visible && !(hit === element || element.contains(hit));
            const clickable = element.matches('button,a[href],input[type=button],input[type=submit],input[type=reset],[role=button],[role=link]');
            const selectable = element.tagName === 'SELECT';
            const hoverable = clickable || checkable;
            return {{
                documentIndex, role: bounded(role), name: bounded(name), label: bounded(label),
                placeholder: bounded(placeholder), testId: bounded(testId), domId: bounded(element.id),
                text: leafText ? bounded(text) : null, visible, disabled, editable, editableApplicable,
                checked, selected, obscured, interactive, clickable, checkable, selectable, hoverable,
                options: selectable ? Array.from(element.options).slice(0, 51).map(option => bounded(option.label || option.value)).filter(Boolean) : []
            }};
        }}).filter(Boolean);
        return {{url: location.href, title: document.title, total: inspected.length,
            elements: inspected.slice(0, {max_elements})}};
    }})()"#
    ))
}

fn resolver_expression(locator: &Locator, test_id_attribute: &str) -> Result<String, BrowserError> {
    let elements = locator_array_expression(locator, test_id_attribute)?;
    Ok(format!(
        r#"(() => {{
        const norm = value => String(value || '').replace(/\s+/g, ' ').trim();
        const implicitRole = element => {{
            const tag = element.tagName.toLowerCase();
            if (tag === 'button') return 'button';
            if (tag === 'a' && element.hasAttribute('href')) return 'link';
            if (tag === 'textarea') return 'textbox';
            if (tag === 'select') return 'combobox';
            if (tag === 'input') {{
                const type = (element.type || 'text').toLowerCase();
                if (['button','submit','reset'].includes(type)) return 'button';
                if (type === 'checkbox') return 'checkbox';
                if (type === 'radio') return 'radio';
                if (['text','email','password','search','tel','url'].includes(type)) return 'textbox';
            }}
            return null;
        }};
        const accessibleName = element => {{
            const labelledby = element.getAttribute('aria-labelledby');
            if (labelledby) return norm(labelledby.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' '));
            if (element.hasAttribute('aria-label')) return norm(element.getAttribute('aria-label'));
            if (element.labels?.length) return norm(Array.from(element.labels).map(label => {{
                const copy = label.cloneNode(true);
                copy.querySelectorAll('input,textarea,select,button').forEach(control => control.remove());
                return copy.innerText || copy.textContent;
            }}).join(' '));
            if (element.tagName === 'IMG') return norm(element.alt);
            if (element.tagName === 'INPUT' && ['button','submit','reset'].includes(element.type)) return norm(element.value);
            return norm(element.innerText || element.textContent || element.title);
        }};
        try {{
            const elements = {elements};
            const candidates = elements.slice(0, 5).map(element => {{
                const password = element.tagName === 'INPUT' && element.type === 'password';
                return {{
                    tag: element.tagName.toLowerCase(), id: element.id || null,
                    role: element.getAttribute('role') || implicitRole(element),
                    name: accessibleName(element).slice(0, 120) || null,
                    text: password ? null : norm(element.innerText || '').slice(0, 120) || null
                }};
            }});
            if (elements.length !== 1) return {{matches: elements.length, candidates}};
            const element = elements[0];
            element.scrollIntoView({{block:'center', inline:'center', behavior:'instant'}});
            const rect = element.getBoundingClientRect();
            const style = getComputedStyle(element);
            const visible = style.display !== 'none' && style.visibility !== 'hidden'
                && Number(style.opacity) !== 0 && rect.width > 0 && rect.height > 0;
            const disabled = element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true';
            const editable = !disabled && !element.readOnly && (
                element.tagName === 'TEXTAREA' || element.isContentEditable
                || (element.tagName === 'INPUT' && !['button','submit','reset','checkbox','radio','file','hidden'].includes(element.type))
            );
            const checkable = element.matches('input[type=checkbox],input[type=radio],[role=checkbox],[role=radio],[role=switch]');
            const checked = checkable ? (('checked' in element) ? Boolean(element.checked)
                : element.getAttribute('aria-checked') === 'true') : null;
            const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
            const hit = visible ? document.elementFromPoint(x, y) : null;
            const obscured = visible && !(hit === element || element.contains(hit));
            const documentIndex = Array.from(document.querySelectorAll('body *')).indexOf(element);
            return {{matches:1, candidates, visible, disabled, editable, checked, obscured, documentIndex,
                tag: element.tagName.toLowerCase(), rect: {{x:rect.x,y:rect.y,width:rect.width,height:rect.height}}}};
        }} catch (error) {{ return {{matches:0, invalid:String(error)}}; }}
    }})()"#
    ))
}

fn locator_array_expression(
    locator: &Locator,
    test_id_attribute: &str,
) -> Result<String, BrowserError> {
    let json = |value: &str| {
        serde_json::to_string(value).map_err(|error| invalid_evaluation(&error.to_string()))
    };
    let expression = match locator {
        Locator::Id(value) => format!(
            "(() => {{ const e = document.getElementById({}); return e ? [e] : []; }})()",
            json(value)?
        ),
        Locator::Role { role, name } => {
            let role = json(role)?;
            let name = name
                .as_deref()
                .map(json)
                .transpose()?
                .unwrap_or_else(|| "null".into());
            format!(
                "Array.from(document.querySelectorAll('body *')).filter(element => (element.getAttribute('role') || implicitRole(element)) === {role} && ({name} === null || accessibleName(element) === {name}))"
            )
        }
        Locator::Label(value) => {
            let value = json(value)?;
            format!(
                "Array.from(document.querySelectorAll('input,textarea,select,button,[contenteditable=true]')).filter(element => accessibleName(element) === {value})"
            )
        }
        Locator::Text(value) => {
            let value = json(value)?;
            format!(
                "(() => {{ const all = Array.from(document.querySelectorAll('body *')).filter(element => !['SCRIPT','STYLE','NOSCRIPT'].includes(element.tagName) && norm(element.innerText) === {value}); const actionable = all.filter(element => element.matches('button,a[href],input,textarea,select,[role],[contenteditable=true]')); const pool = actionable.length ? actionable : all; return pool.filter(element => !pool.some(other => other !== element && element.contains(other))); }})()"
            )
        }
        Locator::Placeholder(value) => format!(
            "Array.from(document.querySelectorAll('input[placeholder],textarea[placeholder]')).filter(element => element.getAttribute('placeholder') === {})",
            json(value)?
        ),
        Locator::TestId(value) => format!(
            "Array.from(document.querySelectorAll('[{}]')).filter(element => element.getAttribute({}) === {})",
            css_attribute(test_id_attribute)?,
            json(test_id_attribute)?,
            json(value)?
        ),
        Locator::Css(value) => format!("Array.from(document.querySelectorAll({}))", json(value)?),
        Locator::XPath(value) => format!(
            "(() => {{ const result = document.evaluate({}, document, null, XPathResult.ORDERED_NODE_ITERATOR_TYPE, null); const values=[]; let item; while ((item=result.iterateNext())) {{ if (item.nodeType === Node.ELEMENT_NODE) values.push(item); }} return values; }})()",
            json(value)?
        ),
    };
    Ok(expression)
}

fn css_attribute(value: &str) -> Result<String, BrowserError> {
    if !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
    {
        Ok(value.into())
    } else {
        Err(BrowserError::Protocol {
            method: "Runtime.evaluate".into(),
            message: "test-ID attribute is not a valid attribute name".into(),
        })
    }
}

fn evaluation_value(result: &Value) -> Option<&Value> {
    result.pointer("/result/value")
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.into();
    }
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

fn redact_evidence(
    evidence: &mut PageEvidence,
    redactions: &[String],
    query_parameters: &[String],
) {
    let redact = |value: &mut String| {
        for secret in redactions.iter().filter(|secret| !secret.is_empty()) {
            *value = value.replace(secret, "<redacted>");
        }
    };
    if let Some(value) = &mut evidence.current_url {
        *value = redact_url_query(value, query_parameters);
        redact(value)
    }
    if let Some(value) = &mut evidence.title {
        redact(value)
    }
    if let Some(value) = &mut evidence.dom_snapshot {
        redact(value)
    }
    for value in &mut evidence.console_errors {
        redact(value)
    }
    for candidate in &mut evidence.candidates {
        if let Some(value) = &mut candidate.name {
            redact(value)
        }
        if let Some(value) = &mut candidate.text {
            redact(value)
        }
    }
}

struct KeySpec {
    key: String,
    code: String,
    modifiers: i32,
    text: Option<String>,
}

fn parse_key(value: &str) -> Option<KeySpec> {
    let mut modifiers = 0;
    let mut main = None;
    for part in value.split('+') {
        match part {
            "Alt" => modifiers |= 1,
            "Control" | "Ctrl" => modifiers |= 2,
            "Meta" | "Command" => modifiers |= 4,
            "Shift" => modifiers |= 8,
            _ if main.is_none() && !part.is_empty() => main = Some(part),
            _ => return None,
        }
    }
    let main = main?;
    let (key, code, text) = match main {
        "Enter" => ("Enter".into(), "Enter".into(), None),
        "Tab" => ("Tab".into(), "Tab".into(), None),
        "Escape" | "Esc" => ("Escape".into(), "Escape".into(), None),
        "Backspace" => ("Backspace".into(), "Backspace".into(), None),
        "Delete" => ("Delete".into(), "Delete".into(), None),
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp"
        | "PageDown" => (main.into(), main.into(), None),
        "Space" => (" ".into(), "Space".into(), Some(" ".into())),
        value if value.chars().count() == 1 => {
            let character = value.chars().next()?;
            let code = if character.is_ascii_alphabetic() {
                format!("Key{}", character.to_ascii_uppercase())
            } else if character.is_ascii_digit() {
                format!("Digit{character}")
            } else {
                "Unidentified".into()
            };
            (value.into(), code, Some(value.into()))
        }
        _ => return None,
    };
    Some(KeySpec {
        key,
        code,
        modifiers,
        text,
    })
}

fn invalid_evaluation(message: &str) -> BrowserError {
    BrowserError::Protocol {
        method: "Runtime.evaluate".into(),
        message: message.into(),
    }
}

fn string_field(value: &Value, field: &str, method: &str) -> Result<String, BrowserError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| BrowserError::Protocol {
            method: method.into(),
            message: format!("response did not contain `{field}`"),
        })
}

pub fn find_system_chrome() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    const CANDIDATES: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    #[cfg(target_os = "linux")]
    const CANDIDATES: &[&str] = &[
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ];
    #[cfg(target_os = "windows")]
    const CANDIDATES: &[&str] = &[];
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    const CANDIDATES: &[&str] = &[];

    CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::{WebSocketStream, accept_async};

    use super::*;

    async fn fake_cdp(
        command_timeout: Duration,
    ) -> Option<(CdpConnection, WebSocketStream<tokio::net::TcpStream>)> {
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return None;
        };
        let address = listener.local_addr().expect("fake CDP address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept CDP client");
            accept_async(stream).await.expect("accept websocket")
        });
        let connection = CdpConnection::connect(&format!("ws://{address}"), command_timeout)
            .await
            .expect("connect fake CDP");
        let socket = server.await.expect("fake server task");
        Some((connection, socket))
    }

    fn command_id(message: Message) -> u64 {
        let Message::Text(text) = message else {
            panic!("expected text command");
        };
        serde_json::from_str::<Value>(&text)
            .expect("command JSON")
            .get("id")
            .and_then(Value::as_u64)
            .expect("command ID")
    }

    #[test]
    fn chrome_is_headless_by_default_and_can_be_headed() {
        assert!(ChromeHost::default().headless);
        assert!(!ChromeHost::default().with_headed(true).headless);
        assert!(ChromeHost::default().with_headed(false).headless);
    }

    #[test]
    fn evidence_is_bounded_and_secret_values_are_redacted() {
        let mut evidence = PageEvidence {
            current_url: Some("http://example.test/?token=secret".into()),
            dom_snapshot: Some("<body>secret</body>".into()),
            candidates: vec![CandidateEvidence {
                tag: "div".into(),
                text: Some("secret".into()),
                ..CandidateEvidence::default()
            }],
            ..PageEvidence::default()
        };
        redact_evidence(&mut evidence, &["secret".into()], &["token".into()]);
        assert!(!format!("{evidence:?}").contains("secret"));
        assert_eq!(truncate_utf8("ééé", 3), "é");
        let redacted_url = redact_url_query(
            "http://example.test/login?token=must-not-leak&view=full&CODE=private",
            &["token".into(), "code".into()],
        );
        assert!(!redacted_url.contains("must-not-leak"));
        assert!(!redacted_url.contains("private"));
        assert!(redacted_url.contains("view=full"));

        let mut truncated = false;
        let options = InspectionOptions {
            redacted_values: vec!["password-value".into()],
            ..InspectionOptions::default()
        };
        let (display, locator_source) = bounded_field(
            Some("prefix password-value suffix".into()),
            &options,
            &mut truncated,
        );
        assert_eq!(display.as_deref(), Some("prefix [redacted] suffix"));
        assert!(locator_source.is_none());
    }

    #[tokio::test]
    async fn command_timeout_removes_correlation_entry() {
        let Some((connection, mut server)) = fake_cdp(Duration::from_millis(30)).await else {
            return;
        };
        let server_task = tokio::spawn(async move {
            let _ = server.next().await.expect("command").expect("websocket");
            sleep(Duration::from_millis(100)).await;
        });

        let error = connection
            .command("Never.responds", None, None)
            .await
            .expect_err("timeout");
        assert!(matches!(error, BrowserError::CommandTimeout { .. }));
        assert_eq!(connection.in_flight(), 0);
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn out_of_order_responses_remain_correlated() {
        let Some((connection, mut server)) = fake_cdp(Duration::from_secs(1)).await else {
            return;
        };
        let server_task = tokio::spawn(async move {
            let first = server.next().await.expect("first").expect("first frame");
            let second = server.next().await.expect("second").expect("second frame");
            let first_id = command_id(first);
            let second_id = command_id(second);
            server
                .send(Message::Text(
                    json!({"id": second_id, "result": {"value": "second"}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("second response");
            server
                .send(Message::Text(
                    json!({"id": first_id, "result": {"value": "first"}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("first response");
        });

        let (first, second) = tokio::join!(
            connection.command("First", None, None),
            connection.command("Second", None, None)
        );
        assert_eq!(first.expect("first result")["value"], "first");
        assert_eq!(second.expect("second result")["value"], "second");
        assert_eq!(connection.in_flight(), 0);
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn disconnect_fails_pending_command_promptly() {
        let Some((connection, mut server)) = fake_cdp(Duration::from_secs(5)).await else {
            return;
        };
        let server_task = tokio::spawn(async move {
            let _ = server.next().await.expect("command").expect("frame");
            server.close(None).await.expect("close fake CDP");
        });
        let started = Instant::now();
        let error = connection
            .command("Pending", None, None)
            .await
            .expect_err("disconnect");
        assert_eq!(error, BrowserError::BrowserDisconnected);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(connection.in_flight(), 0);
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn malformed_response_is_structured_and_fails_pending_calls() {
        let Some((connection, mut server)) = fake_cdp(Duration::from_secs(1)).await else {
            return;
        };
        let server_task = tokio::spawn(async move {
            let _ = server.next().await.expect("command").expect("frame");
            server
                .send(Message::Text("{".into()))
                .await
                .expect("malformed response");
        });
        let error = connection
            .command("Malformed", None, None)
            .await
            .expect_err("malformed protocol");
        assert!(matches!(error, BrowserError::MalformedProtocol { .. }));
        assert_eq!(connection.in_flight(), 0);
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn event_pressure_and_unknown_ids_do_not_block_responses() {
        let Some((connection, mut server)) = fake_cdp(Duration::from_secs(2)).await else {
            return;
        };
        let server_task = tokio::spawn(async move {
            let command = server.next().await.expect("command").expect("frame");
            let id = command_id(command);
            server
                .send(Message::Text(
                    json!({"id": id + 1000, "result": {}}).to_string().into(),
                ))
                .await
                .expect("unknown response");
            for sequence in 0..256 {
                server
                    .send(Message::Text(
                        json!({"method": "Page.event", "params": {"sequence": sequence}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .expect("event");
            }
            server
                .send(Message::Text(
                    json!({"id": id, "result": {"ok": true}}).to_string().into(),
                ))
                .await
                .expect("response");
        });
        let result = connection
            .command("After.events", None, None)
            .await
            .expect("response after events");
        assert_eq!(result["ok"], true);
        assert_eq!(connection.in_flight(), 0);
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn real_chrome_clicks_and_checks_visible_text_when_available() {
        let host = ChromeHost::default();
        if host.locate().is_none() {
            return;
        }
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            // Some build sandboxes prohibit loopback listeners. The same path is
            // exercised when the browser integration test runs outside them.
            return;
        };
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept fixture request");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            let body = "<!doctype html><html><body><button id=\"submit\" onclick=\"const result=document.createElement('div');result.textContent='submitted';document.body.append(result)\">Submit</button><div style=\"display:none\">hidden</div></body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("serve fixture");
        });
        let mut browser = host.start().await.expect("start Chrome");
        let mut page = browser.new_page().await.expect("create page");
        page.open(&format!("http://{address}"))
            .await
            .expect("navigate");
        page.click(&Locator::Id("submit".into()))
            .await
            .expect("click existing");
        page.expect_visible(&Locator::Text("submitted".into()))
            .await
            .expect("submitted text is visible");
        let hidden = page.expect_visible(&Locator::Text("hidden".into())).await;
        assert!(matches!(
            hidden,
            Err(BrowserError::LocatorNotVisible { .. })
        ));
        let missing = page.click(&Locator::Id("missing".into())).await;
        assert!(matches!(missing, Err(BrowserError::LocatorNotFound { .. })));
        drop(page);
        browser.close().await.expect("close and reap Chrome");
    }

    #[tokio::test]
    async fn real_chrome_runs_semantic_form_flow_with_physical_input_when_available() {
        let host = ChromeHost::default();
        if host.locate().is_none() {
            return;
        }
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let address = listener.local_addr().expect("fixture address");
        let fixture = r#"<!doctype html><html><head><meta charset="utf-8"><title>Sign in</title></head><body>
            <form id="signin">
              <label>Email <input type="email" value="old@example.com"></label>
              <label>Password <input type="password"></label>
              <label>Biography <textarea></textarea></label>
              <label>Search <input type="search" placeholder="Search products"></label>
              <label>Timezone <select><option value="America/Chicago">America/Chicago</option></select></label>
              <label>Email notifications <input type="checkbox"></label>
              <label>SMS notifications <input type="checkbox" checked></label>
              <button type="button">Account</button>
              <button type="submit">Sign in</button>
              <button type="button" disabled>Unavailable</button>
              <label>City 🏙 <input type="text" placeholder="Montréal"></label>
              <button type="button" style="display:none">Hidden action</button>
            </form>
            <script>
              document.querySelector('button[type=submit]').addEventListener('click', event => {
                event.preventDefault();
                const values = Array.from(document.getElementById('signin').elements);
                history.pushState({}, '', '/dashboard');
                const result = document.createElement('div');
                const email = document.querySelector('input[type=email]').value;
                const password = document.querySelector('input[type=password]').value;
                result.textContent = email === 'alice@example.com' && password === 'secret'
                  ? 'Welcome, Alice' : `invalid:${email}:${password}`;
                document.body.append(result);
              });
              document.querySelector('input[type=search]').addEventListener('keydown', event => {
                if (event.key === 'Enter') {
                  const result = document.createElement('div'); result.textContent = 'Key pressed'; document.body.append(result);
                }
              });
            </script>
        </body></html>"#;
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept form request");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{fixture}",
                fixture.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("serve form");
        });

        let mut browser = host.start().await.expect("start Chrome");
        let mut context = browser
            .new_context(&BrowserContextOptions::default())
            .await
            .expect("context");
        let mut page = context.new_page().await.expect("page");
        page.open(&format!("http://{address}/login"))
            .await
            .expect("open form");
        let inspection = page
            .inspect(&InspectionOptions::default())
            .await
            .expect("inspect form");
        let email = inspection
            .elements
            .iter()
            .find(|element| element.label.as_deref() == Some("Email"))
            .unwrap_or_else(|| panic!("email inspection: {inspection:#?}"));
        assert_eq!(email.preferred_locator.source, "label(\"Email\")");
        assert_eq!(
            email.supported_actions,
            vec![
                SupportedAction::Fill,
                SupportedAction::Type,
                SupportedAction::Press
            ]
        );
        let sign_in = inspection
            .elements
            .iter()
            .find(|element| element.accessible_name.as_deref() == Some("Sign in"))
            .expect("sign-in inspection");
        assert_eq!(
            sign_in.preferred_locator.source,
            "role(\"button\", name: \"Sign in\")"
        );
        let repair_hints = webtest_browser::locator_repair_hints(
            &Locator::Role {
                role: "button".into(),
                name: Some("Log in".into()),
            },
            &inspection,
            webtest_browser::MAX_CANDIDATES,
        );
        assert_eq!(
            repair_hints[0].replacement,
            webtest_browser::RepairReplacement::locator("role(\"button\", name: \"Sign in\")")
        );
        let unavailable = inspection
            .elements
            .iter()
            .find(|element| element.accessible_name.as_deref() == Some("Unavailable"))
            .expect("disabled inspection");
        assert_eq!(unavailable.states.enabled, Some(false));
        assert!(
            !unavailable
                .supported_actions
                .contains(&SupportedAction::Click)
        );
        assert!(
            inspection
                .elements
                .iter()
                .all(|element| { element.accessible_name.as_deref() != Some("Hidden action") })
        );
        assert!(inspection.elements.iter().all(|element| {
            std::iter::once(&element.preferred_locator)
                .chain(&element.alternate_locators)
                .all(|candidate| {
                    !matches!(candidate.kind, LocatorCandidateKind::Text)
                        || !candidate.source.contains("secret")
                })
        }));
        assert!(
            inspection
                .elements
                .iter()
                .flat_map(|element| {
                    std::iter::once(&element.preferred_locator).chain(&element.alternate_locators)
                })
                .all(|candidate| {
                    !candidate.source.starts_with("css(") && !candidate.source.starts_with("xpath(")
                })
        );
        assert!(
            inspection
                .elements
                .iter()
                .any(|element| element.label.as_deref() == Some("City 🏙"))
        );
        page.perform(
            &Action::Fill {
                locator: Locator::Label("Email".into()),
                value: "alice@example.com".into(),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("fill email");
        page.perform(
            &Action::Fill {
                locator: Locator::Label("Password".into()),
                value: "secret".into(),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("fill password");
        page.perform(
            &Action::Type {
                locator: Locator::Label("Biography".into()),
                value: "hello".into(),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("type biography");
        page.perform(
            &Action::Press {
                locator: Locator::Placeholder("Search products".into()),
                key: "Enter".into(),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("press Enter");
        page.wait_for_locator(
            &Locator::Text("Key pressed".into()),
            LocatorState::Visible,
            Duration::from_secs(2),
        )
        .await
        .expect("key event was dispatched");
        page.perform(
            &Action::Select {
                locator: Locator::Label("Timezone".into()),
                option: "America/Chicago".into(),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("select timezone");
        page.perform(
            &Action::Check {
                locator: Locator::Label("Email notifications".into()),
                checked: true,
            },
            Duration::from_secs(2),
        )
        .await
        .expect("check notifications");
        page.perform(
            &Action::Check {
                locator: Locator::Label("SMS notifications".into()),
                checked: false,
            },
            Duration::from_secs(2),
        )
        .await
        .expect("uncheck notifications");
        page.perform(
            &Action::Hover {
                locator: Locator::Text("Account".into()),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("hover account");
        page.perform(
            &Action::Click {
                locator: Locator::Role {
                    role: "button".into(),
                    name: Some("Sign in".into()),
                },
            },
            Duration::from_secs(2),
        )
        .await
        .expect("physical sign-in click");
        if let Err(error) = page
            .wait_for_locator(
                &Locator::Text("Welcome, Alice".into()),
                LocatorState::Visible,
                Duration::from_secs(2),
            )
            .await
        {
            let evidence = page
                .capture_evidence(&EvidenceRequest {
                    locator: None,
                    include_screenshot: false,
                    include_dom: true,
                    max_dom_bytes: 4096,
                    redactions: vec!["secret".into()],
                    redacted_query_parameters: Vec::new(),
                })
                .await;
            panic!(
                "welcome assertion: {error}; DOM: {:?}; console: {:?}",
                evidence.dom_snapshot, evidence.console_errors
            );
        }
        page.wait_for_url(
            &format!("http://{address}/dashboard"),
            Duration::from_secs(2),
        )
        .await
        .expect("dashboard URL");
        page.wait_for_locator(
            &Locator::Label("Email notifications".into()),
            LocatorState::Checked,
            Duration::from_secs(2),
        )
        .await
        .expect("checked assertion");
        page.wait_for_locator(
            &Locator::Label("SMS notifications".into()),
            LocatorState::Unchecked,
            Duration::from_secs(2),
        )
        .await
        .expect("unchecked assertion");
        let evidence = page
            .capture_evidence(&EvidenceRequest {
                locator: Some(Locator::Role {
                    role: "button".into(),
                    name: Some("Sign in".into()),
                }),
                include_screenshot: true,
                include_dom: true,
                max_dom_bytes: 512,
                redactions: vec!["secret".into()],
                redacted_query_parameters: Vec::new(),
            })
            .await;
        assert!(
            evidence
                .screenshot_png
                .as_deref()
                .is_some_and(|png| png.starts_with(&[137, 80, 78, 71]))
        );
        assert!(
            evidence
                .dom_snapshot
                .as_ref()
                .is_some_and(|dom| dom.len() <= 512)
        );
        drop(page);
        context.close().await.expect("close context");
        browser.close().await.expect("close browser");
    }

    #[tokio::test]
    async fn isolated_contexts_do_not_share_cookies_or_storage_when_available() {
        let host = ChromeHost::default();
        if host.locate().is_none() {
            return;
        }
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept isolation request");
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).await.expect("read request");
                let request = String::from_utf8_lossy(&request[..read]);
                let check = request.starts_with("GET /check ");
                let body = if check {
                    "<!doctype html><body><div id='result'></div><script>document.getElementById('result').textContent=(document.cookie.includes('shared=')||localStorage.getItem('shared'))?'leaked':'clean'</script></body>"
                } else {
                    "<!doctype html><body>set<script>document.cookie='shared=yes';localStorage.setItem('shared','yes')</script></body>"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("serve isolation fixture");
            }
        });
        let mut browser = host.start().await.expect("start Chrome once");
        let mut first = browser
            .new_context(&BrowserContextOptions::default())
            .await
            .expect("first context");
        let mut first_page = first.new_page().await.expect("first page");
        first_page
            .open(&format!("http://{address}/set"))
            .await
            .expect("set storage");
        drop(first_page);
        first.close().await.expect("close first context");

        let mut second = browser
            .new_context(&BrowserContextOptions::default())
            .await
            .expect("second context");
        let mut second_page = second.new_page().await.expect("second page");
        second_page
            .open(&format!("http://{address}/check"))
            .await
            .expect("check storage");
        second_page
            .wait_for_locator(
                &Locator::Text("clean".into()),
                LocatorState::Visible,
                Duration::from_secs(2),
            )
            .await
            .expect("storage is isolated");
        drop(second_page);
        second.close().await.expect("close second context");
        browser.close().await.expect("close shared Chrome process");
    }

    #[tokio::test]
    async fn actionability_failures_are_distinct_and_candidate_evidence_is_bounded() {
        let host = ChromeHost::default();
        if host.locate().is_none() {
            return;
        }

        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };

        let address = listener.local_addr().expect("fixture address");

        let body = r#"<!doctype html><style>
            #covered { position:absolute;left:20px;top:20px;width:100px;height:30px }
            #overlay { position:absolute;left:20px;top:20px;width:100px;height:30px;z-index:2 }
            #unstable { position:absolute;top:80px;animation:move .08s infinite alternate linear }
            @keyframes move { from { left:10px } to { left:200px } }
        </style><body>
            <button id="disabled" disabled>disabled</button>
            <button id="covered">covered</button><div id="overlay">overlay</div>
            <button id="unstable">unstable</button>
    
            <button class="duplicate">same</button><button class="duplicate">same</button>
            <button class="duplicate">same</button><button class="duplicate">same</button>
            <button class="duplicate">same</button><button class="duplicate">same</button>
    
            <button style="display:none" id="hidden">hidden</button>
    
            <input placeholder="Search products" data-testid="search-box">
    
            <div id="space"> Hello
                 World </div>
    
            <div id="shadow-host"></div>
            <iframe srcdoc="<button>Inside frame</button>"></iframe>
    
            <script>
                document
                    .getElementById('shadow-host')
                    .attachShadow({mode:'open'})
                    .innerHTML='<button>Inside shadow</button>';
            </script>
    
            <div id="transient-host"></div>
    
            <script>
                window.startTransient = () => {
                    const host = document.getElementById('transient-host');
    
                    // Start with no matches.
                    host.innerHTML = '';
    
                    // Then become ambiguous for long enough that the actionability
                    // loop should reliably observe the second failure state.
                    setTimeout(() => {
                        host.innerHTML =
                            '<button class="transient">one</button>' +
                            '<button class="transient">two</button>';
                    }, 80);
    
                    // Return to no matches. The action never becomes actionable,
                    // but multiple distinct failure reasons are observed.
                    setTimeout(() => {
                        host.innerHTML = '';
                    }, 180);
                };
            </script>
        </body>"#;

        tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("accept actionability request");

            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;

            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/html\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );

            stream
                .write_all(response.as_bytes())
                .await
                .expect("serve actionability fixture");
        });

        let mut browser = host.start().await.expect("start Chrome");
        let mut page = browser.new_page().await.expect("page");

        page.open(&format!("http://{address}"))
            .await
            .expect("open fixture");

        let click = |locator| Action::Click { locator };

        assert!(matches!(
            page.perform(
                &click(Locator::Id("missing".into())),
                Duration::from_millis(100)
            )
            .await,
            Err(BrowserError::LocatorNotFound { .. })
        ));

        assert!(matches!(
            page.perform(
                &click(Locator::Css(".duplicate".into())),
                Duration::from_millis(100)
            )
            .await,
            Err(BrowserError::LocatorAmbiguous { .. })
        ));

        assert!(matches!(
            page.perform(
                &click(Locator::Id("disabled".into())),
                Duration::from_millis(100)
            )
            .await,
            Err(BrowserError::ElementDisabled { .. })
        ));

        assert!(matches!(
            page.perform(
                &click(Locator::Id("covered".into())),
                Duration::from_millis(100)
            )
            .await,
            Err(BrowserError::ElementObscured { .. })
        ));

        assert!(matches!(
            page.perform(
                &click(Locator::Id("hidden".into())),
                Duration::from_millis(100)
            )
            .await,
            Err(BrowserError::LocatorNotVisible { .. })
        ));

        assert!(matches!(
            page.perform(
                &click(Locator::Id("unstable".into())),
                Duration::from_millis(140)
            )
            .await,
            Err(BrowserError::ElementUnstable { .. })
        ));

        assert!(matches!(
            page.perform(&click(Locator::Css("[".into())), Duration::from_millis(100))
                .await,
            Err(BrowserError::LocatorInvalid { .. })
        ));

        page.evaluate("window.startTransient()")
            .await
            .expect("start transient fixture");

        assert!(matches!(
            page.perform(
                &click(Locator::Css(".transient".into())),
                Duration::from_millis(300)
            )
            .await,
            Err(BrowserError::ActionTimeout { .. })
        ));

        page.wait_for_locator(
            &Locator::Placeholder("Search products".into()),
            LocatorState::Visible,
            Duration::from_secs(1),
        )
        .await
        .expect("placeholder locator");

        page.wait_for_locator(
            &Locator::TestId("search-box".into()),
            LocatorState::Visible,
            Duration::from_secs(1),
        )
        .await
        .expect("test-ID locator");

        page.wait_for_locator(
            &Locator::Text("Hello World".into()),
            LocatorState::Visible,
            Duration::from_secs(1),
        )
        .await
        .expect("rendered whitespace normalization");

        page.wait_for_locator(
            &Locator::XPath("//*[@id='space']".into()),
            LocatorState::Visible,
            Duration::from_secs(1),
        )
        .await
        .expect("XPath locator");

        assert!(matches!(
            page.wait_for_locator(
                &Locator::Text("Inside shadow".into()),
                LocatorState::Visible,
                Duration::from_millis(100)
            )
            .await,
            Err(BrowserError::LocatorNotFound { .. })
        ));

        assert!(matches!(
            page.wait_for_locator(
                &Locator::Text("Inside frame".into()),
                LocatorState::Visible,
                Duration::from_millis(100)
            )
            .await,
            Err(BrowserError::LocatorNotFound { .. })
        ));

        let evidence = page
            .capture_evidence(&EvidenceRequest {
                locator: Some(Locator::Css(".duplicate".into())),
                include_screenshot: false,
                include_dom: false,
                max_dom_bytes: 0,
                redactions: Vec::new(),
                redacted_query_parameters: Vec::new(),
            })
            .await;

        assert_eq!(evidence.candidates.len(), 5);

        browser.close().await.expect("close browser");
    }

    #[tokio::test]
    async fn forced_real_chrome_disconnect_fails_pending_call_and_reaps_process() {
        let host = ChromeHost::default();
        let Some(executable) = host.locate() else {
            return;
        };
        let Ok((mut process, websocket_url)) = ChromeProcess::launch(&executable, true).await
        else {
            return;
        };
        let profile = process
            .profile
            .as_ref()
            .expect("Chrome profile")
            .path()
            .to_path_buf();
        let connection = CdpConnection::connect(&websocket_url, Duration::from_secs(5))
            .await
            .expect("connect to Chrome");
        let target = connection
            .command(
                "Target.createTarget",
                Some(json!({"url":"about:blank"})),
                None,
            )
            .await
            .expect("create target");
        let target_id =
            string_field(&target, "targetId", "Target.createTarget").expect("target ID");
        let attached = connection
            .command(
                "Target.attachToTarget",
                Some(json!({"targetId":target_id,"flatten":true})),
                None,
            )
            .await
            .expect("attach target");
        let session_id =
            string_field(&attached, "sessionId", "Target.attachToTarget").expect("session ID");
        let pending_connection = connection.clone();
        let pending = tokio::spawn(async move {
            pending_connection
                .command(
                    "Runtime.evaluate",
                    Some(json!({
                        "expression":"new Promise(() => {})",
                        "awaitPromise":true,
                        "returnByValue":true
                    })),
                    Some(&session_id),
                )
                .await
        });
        timeout(Duration::from_secs(1), async {
            while connection.in_flight() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending CDP command");

        process.child.start_kill().expect("force Chrome exit");
        let error = timeout(Duration::from_secs(2), pending)
            .await
            .expect("disconnect deadline")
            .expect("pending task")
            .expect_err("forced disconnect");
        assert_eq!(error, BrowserError::BrowserDisconnected);
        assert_eq!(connection.in_flight(), 0);
        process.shutdown().await.expect("reap killed Chrome");
        drop(process);
        assert!(!profile.exists(), "temporary Chrome profile was removed");
    }
}
