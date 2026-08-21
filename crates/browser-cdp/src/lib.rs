//! Direct Chrome DevTools Protocol implementation of the browser abstraction.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    process::{Child, Command as ProcessCommand},
    sync::{mpsc, oneshot},
    time::{Instant, interval, sleep, timeout},
};
use tokio_tungstenite::tungstenite::Message;
use tracing::instrument;
use webtest_browser::{BrowserError, BrowserHost, BrowserSession, Locator, Page};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const CORRELATION_SWEEP: Duration = Duration::from_millis(10);

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
                    Ok(contents) => break Ok(contents),
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
                        let Some(id) = message.id else { continue };
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
}

struct CdpBrowserSession {
    process: Option<ChromeProcess>,
    connection: CdpConnection,
    navigation_timeout: Duration,
}

#[async_trait]
impl BrowserSession for CdpBrowserSession {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        let target = self
            .connection
            .command(
                "Target.createTarget",
                Some(json!({ "url": "about:blank" })),
                None,
            )
            .await?;
        let target_id = string_field(&target, "targetId", "Target.createTarget")?;
        let attached = self
            .connection
            .command(
                "Target.attachToTarget",
                Some(json!({ "targetId": target_id, "flatten": true })),
                None,
            )
            .await?;
        let session_id = string_field(&attached, "sessionId", "Target.attachToTarget")?;
        self.connection
            .command("Page.enable", None, Some(&session_id))
            .await?;
        self.connection
            .command("Runtime.enable", None, Some(&session_id))
            .await?;
        Ok(Box::new(CdpPage {
            connection: self.connection.clone(),
            session_id,
            navigation_timeout: self.navigation_timeout,
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

struct CdpPage {
    connection: CdpConnection,
    session_id: String,
    navigation_timeout: Duration,
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
        let elements = locator_expression(locator)?;
        let expression = format!(
            "(() => {{ const elements = {elements}; if (elements.length !== 1) return {{ matches: elements.length }}; elements[0].click(); return {{ matches: 1 }}; }})()"
        );
        let result = self.evaluate(expression).await?;
        match result_field(&result, "matches")?.as_u64() {
            Some(1) => Ok(()),
            Some(0) => Err(BrowserError::LocatorNotFound {
                locator: locator.clone(),
            }),
            Some(matches) => Err(BrowserError::LocatorAmbiguous {
                locator: locator.clone(),
                matches: matches as usize,
            }),
            None => Err(invalid_evaluation("`matches` was not an integer")),
        }
    }

    async fn expect_visible(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        let elements = locator_expression(locator)?;
        let expression = format!(
            "(() => {{ const elements = {elements}; if (elements.length !== 1) return {{ matches: elements.length }}; const element = elements[0]; const style = getComputedStyle(element); const bounds = element.getBoundingClientRect(); const visible = style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity) !== 0 && bounds.width > 0 && bounds.height > 0; return {{ matches: 1, visible }}; }})()"
        );
        let result = self.evaluate(expression).await?;
        match result_field(&result, "matches")?.as_u64() {
            Some(0) => Err(BrowserError::LocatorNotFound {
                locator: locator.clone(),
            }),
            Some(1) => match result_field(&result, "visible")?.as_bool() {
                Some(true) => Ok(()),
                Some(false) => Err(BrowserError::LocatorNotVisible {
                    locator: locator.clone(),
                }),
                None => Err(invalid_evaluation("`visible` was not a boolean")),
            },
            Some(matches) => Err(BrowserError::LocatorAmbiguous {
                locator: locator.clone(),
                matches: matches as usize,
            }),
            None => Err(invalid_evaluation("`matches` was not an integer")),
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

impl CdpPage {
    async fn evaluate(&self, expression: String) -> Result<Value, BrowserError> {
        self.connection
            .command(
                "Runtime.evaluate",
                Some(json!({ "expression": expression, "returnByValue": true })),
                Some(&self.session_id),
            )
            .await
    }
}

fn locator_expression(locator: &Locator) -> Result<String, BrowserError> {
    let (property, value) = match locator {
        Locator::Id(value) => ("element.id", value),
        Locator::Text(value) => ("element.textContent.trim()", value),
    };
    let value = serde_json::to_string(value).map_err(|error| BrowserError::Protocol {
        method: "Runtime.evaluate".into(),
        message: error.to_string(),
    })?;
    Ok(format!(
        "Array.from(document.querySelectorAll('body *')).filter((element) => {property} === {value})"
    ))
}

fn result_field<'a>(result: &'a Value, field: &str) -> Result<&'a Value, BrowserError> {
    result
        .pointer(&format!("/result/value/{field}"))
        .ok_or_else(|| invalid_evaluation(&format!("result did not contain `{field}`")))
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
