use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use webtest_browser::{BrowserContext, BrowserContextOptions, BrowserError, BrowserSession, Page};

use crate::{connection::CdpConnection, page::CdpPage, process::ChromeProcess, wire::string_field};

pub(crate) struct CdpBrowserSession {
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
        .command(
            "Page.setLifecycleEventsEnabled",
            Some(json!({ "enabled": true })),
            Some(&session_id),
        )
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
        CdpPage::new(
            connection.clone(),
            session_id,
            navigation_timeout,
            options.test_id_attribute.clone(),
        ),
        target_id,
    ))
}

impl CdpBrowserSession {
    pub(crate) fn new(
        process: ChromeProcess,
        connection: CdpConnection,
        navigation_timeout: Duration,
    ) -> Self {
        Self {
            process: Some(process),
            connection,
            navigation_timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio_tungstenite::{accept_async, tungstenite::Message};
    use webtest_browser::Viewport;

    use super::*;

    async fn fake_cdp() -> Option<(
        CdpConnection,
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    )> {
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return None;
        };
        let address = listener.local_addr().expect("fake CDP address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept CDP client");
            accept_async(stream).await.expect("accept websocket")
        });
        let connection = CdpConnection::connect(&format!("ws://{address}"), Duration::from_secs(1))
            .await
            .expect("connect fake CDP");
        Some((connection, server.await.expect("server task")))
    }

    async fn receive_command(
        server: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) -> Value {
        let Message::Text(text) = server.next().await.expect("command").expect("frame") else {
            panic!("expected text command");
        };
        serde_json::from_str(&text).expect("command JSON")
    }

    async fn respond(
        server: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        command: &Value,
        result: Value,
    ) {
        let mut response = json!({"id": command["id"], "result": result});
        if let Some(session_id) = command.get("sessionId") {
            response["sessionId"] = session_id.clone();
        }
        server
            .send(Message::Text(response.to_string().into()))
            .await
            .expect("response");
    }

    #[tokio::test]
    async fn page_setup_preserves_target_session_domain_and_viewport_order() {
        let Some((connection, mut server)) = fake_cdp().await else {
            return;
        };
        let options = BrowserContextOptions {
            viewport: Viewport {
                width: 900,
                height: 700,
            },
            test_id_attribute: "data-qa".into(),
        };
        let task_connection = connection.clone();
        let task_options = options.clone();
        let page = tokio::spawn(async move {
            create_page(
                &task_connection,
                Duration::from_secs(15),
                Some("browser-context"),
                &task_options,
            )
            .await
        });

        let create = receive_command(&mut server).await;
        assert_eq!(create["method"], "Target.createTarget");
        assert_eq!(
            create["params"],
            json!({"url": "about:blank", "browserContextId": "browser-context"})
        );
        assert!(create.get("sessionId").is_none());
        respond(&mut server, &create, json!({"targetId": "target"})).await;

        let attach = receive_command(&mut server).await;
        assert_eq!(attach["method"], "Target.attachToTarget");
        assert_eq!(
            attach["params"],
            json!({"targetId": "target", "flatten": true})
        );
        respond(&mut server, &attach, json!({"sessionId": "page-session"})).await;

        let page_enable = receive_command(&mut server).await;
        assert_eq!(page_enable["method"], "Page.enable");
        assert_eq!(page_enable["sessionId"], "page-session");
        assert!(page_enable.get("params").is_none());
        respond(&mut server, &page_enable, json!({})).await;

        let lifecycle = receive_command(&mut server).await;
        assert_eq!(lifecycle["method"], "Page.setLifecycleEventsEnabled");
        assert_eq!(lifecycle["sessionId"], "page-session");
        assert_eq!(lifecycle["params"], json!({"enabled": true}));
        respond(&mut server, &lifecycle, json!({})).await;

        for method in ["Runtime.enable"] {
            let command = receive_command(&mut server).await;
            assert_eq!(command["method"], method);
            assert_eq!(command["sessionId"], "page-session");
            assert!(command.get("params").is_none());
            respond(&mut server, &command, json!({})).await;
        }

        let viewport = receive_command(&mut server).await;
        assert_eq!(viewport["method"], "Emulation.setDeviceMetricsOverride");
        assert_eq!(viewport["sessionId"], "page-session");
        assert_eq!(
            viewport["params"],
            json!({
                "width": 900,
                "height": 700,
                "deviceScaleFactor": 1,
                "mobile": false
            })
        );
        respond(&mut server, &viewport, json!({})).await;

        let (_page, target_id) = page.await.expect("page task").expect("page setup");
        assert_eq!(target_id, "target");
    }

    #[tokio::test]
    async fn context_close_is_idempotent_and_use_after_close_is_structured() {
        let Some((connection, mut server)) = fake_cdp().await else {
            return;
        };
        let mut context = CdpBrowserContext {
            connection,
            context_id: Some("browser-context".into()),
            options: BrowserContextOptions::default(),
            navigation_timeout: Duration::from_secs(15),
            target_ids: vec!["target".into()],
        };
        let close = tokio::spawn(async move {
            let first = context.close().await;
            let second = context.close().await;
            let page = context.new_page().await;
            (first, second, page)
        });

        let command = receive_command(&mut server).await;
        assert_eq!(command["method"], "Target.disposeBrowserContext");
        assert_eq!(
            command["params"],
            json!({"browserContextId": "browser-context"})
        );
        assert!(command.get("sessionId").is_none());
        respond(&mut server, &command, json!({})).await;

        let (first, second, page) = close.await.expect("close task");
        first.expect("first close");
        second.expect("idempotent close");
        assert!(matches!(
            page,
            Err(BrowserError::Protocol { method, message })
                if method == "BrowserContext.new_page" && message.contains("already closed")
        ));
    }
}
