//! Direct Chrome DevTools Protocol implementation of the browser abstraction.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
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
    time::{Instant, sleep, timeout},
};
use tokio_tungstenite::tungstenite::Message;
use tracing::instrument;
use webtest_browser::{BrowserError, BrowserHost, BrowserSession, Locator, Page};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default)]
pub struct ChromeHost {
    executable: Option<PathBuf>,
}

impl ChromeHost {
    pub fn new(executable: Option<PathBuf>) -> Self {
        Self { executable }
    }

    pub fn locate(&self) -> Option<PathBuf> {
        self.executable
            .clone()
            .or_else(|| std::env::var_os("WEBTEST_CHROME_PATH").map(PathBuf::from))
            .or_else(find_installed_chrome)
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
        let (process, websocket_url) = ChromeProcess::launch(&executable).await?;
        let connection = CdpConnection::connect(&websocket_url).await?;
        Ok(Box::new(CdpBrowserSession {
            _process: process,
            connection,
        }))
    }
}

struct ChromeProcess {
    child: Child,
    _profile: TempDir,
}

impl ChromeProcess {
    async fn launch(executable: &Path) -> Result<(Self, String), BrowserError> {
        let profile =
            tempfile::tempdir().map_err(|error| BrowserError::Launch(error.to_string()))?;
        let mut child = ProcessCommand::new(executable)
            .arg("--headless=new")
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
                            break Err(BrowserError::Launch(format!(
                                "Chrome exited before CDP became available ({status})"
                            )));
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
                _profile: profile,
            },
            websocket_url,
        ))
    }
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[derive(Clone)]
struct CdpConnection {
    sender: mpsc::Sender<OutgoingCommand>,
}

struct OutgoingCommand {
    method: String,
    params: Option<Value>,
    session_id: Option<String>,
    response: oneshot::Sender<Result<Value, BrowserError>>,
}

struct PendingCommand {
    method: String,
    response: oneshot::Sender<Result<Value, BrowserError>>,
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
    result: Option<Value>,
    error: Option<CdpError>,
}

#[derive(Deserialize)]
struct CdpError {
    code: i64,
    message: String,
}

impl CdpConnection {
    async fn connect(url: &str) -> Result<Self, BrowserError> {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|error| {
                BrowserError::Launch(format!("could not connect to Chrome: {error}"))
            })?;
        let (mut writer, mut reader) = socket.split();
        let (sender, mut receiver) = mpsc::channel::<OutgoingCommand>(32);

        tokio::spawn(async move {
            let mut next_id = 1u64;
            let mut pending = HashMap::<u64, PendingCommand>::new();
            loop {
                tokio::select! {
                    outgoing = receiver.recv() => {
                        let Some(outgoing) = outgoing else { break };
                        let id = next_id;
                        next_id += 1;
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
                        if writer.send(Message::Text(encoded.into())).await.is_err() {
                            let _ = outgoing.response.send(Err(BrowserError::BrowserDisconnected));
                            break;
                        }
                        pending.insert(id, PendingCommand { method: outgoing.method, response: outgoing.response });
                    }
                    incoming = reader.next() => {
                        let Some(Ok(message)) = incoming else { break };
                        let Message::Text(text) = message else { continue };
                        let Ok(message) = serde_json::from_str::<IncomingMessage>(&text) else { continue };
                        let Some(id) = message.id else { continue };
                        let Some(pending_command) = pending.remove(&id) else { continue };
                        let result = if let Some(error) = message.error {
                            Err(BrowserError::Protocol {
                                method: pending_command.method,
                                message: format!("{} ({})", error.message, error.code),
                            })
                        } else {
                            Ok(message.result.unwrap_or(Value::Null))
                        };
                        let _ = pending_command.response.send(result);
                    }
                }
            }
            for (_, command) in pending {
                let _ = command
                    .response
                    .send(Err(BrowserError::BrowserDisconnected));
            }
        });

        Ok(Self { sender })
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
            })
            .await
            .map_err(|_| BrowserError::BrowserDisconnected)?;
        receive
            .await
            .map_err(|_| BrowserError::BrowserDisconnected)?
    }
}

struct CdpBrowserSession {
    _process: ChromeProcess,
    connection: CdpConnection,
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
        }))
    }
}

struct CdpPage {
    connection: CdpConnection,
    session_id: String,
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

        let deadline = Instant::now() + LOAD_TIMEOUT;
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
                return Err(BrowserError::NavigationFailed {
                    url: url.to_owned(),
                    reason: "timed out waiting for the document to load".into(),
                });
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError> {
        let Locator::Id(id) = locator;
        let id_json = serde_json::to_string(id).map_err(|error| BrowserError::Protocol {
            method: "Runtime.evaluate".into(),
            message: error.to_string(),
        })?;
        let expression = format!(
            "(() => {{ const element = document.getElementById({id_json}); if (!element) return {{ found: false }}; element.click(); return {{ found: true }}; }})()"
        );
        let result = self
            .connection
            .command(
                "Runtime.evaluate",
                Some(json!({ "expression": expression, "returnByValue": true })),
                Some(&self.session_id),
            )
            .await?;
        let found = result
            .pointer("/result/value/found")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if found {
            Ok(())
        } else {
            Err(BrowserError::LocatorNotFound {
                locator: locator.clone(),
            })
        }
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

fn find_installed_chrome() -> Option<PathBuf> {
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn real_chrome_clicks_and_reports_missing_ids_when_available() {
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
            let body =
                "<!doctype html><html><body><button id=\"submit\">Submit</button></body></html>";
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
        let missing = page.click(&Locator::Id("missing".into())).await;
        assert!(matches!(missing, Err(BrowserError::LocatorNotFound { .. })));
    }
}
