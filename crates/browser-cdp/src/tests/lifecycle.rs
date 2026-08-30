use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};
use webtest_browser::{BrowserContextOptions, BrowserError, BrowserHost, Locator, LocatorState};

use crate::{ChromeHost, connection::CdpConnection, process::ChromeProcess, wire::string_field};

#[test]
fn chrome_is_headless_by_default_and_can_be_headed() {
    assert!(ChromeHost::default().test_configuration().0);
    assert!(
        !ChromeHost::default()
            .with_headed(true)
            .test_configuration()
            .0
    );
    assert!(
        ChromeHost::default()
            .with_headed(false)
            .test_configuration()
            .0
    );
}

#[tokio::test]
async fn chrome_launch_waits_for_webtest_to_create_the_first_page_when_available() {
    let host = ChromeHost::default();
    let Some(executable) = host.locate() else {
        return;
    };
    let Ok((mut process, websocket_url)) = ChromeProcess::launch(&executable, true).await else {
        return;
    };
    let connection = CdpConnection::connect(&websocket_url, Duration::from_secs(5))
        .await
        .expect("connect to Chrome");
    let targets = connection
        .command("Target.getTargets", None, None)
        .await
        .expect("list startup targets");
    let page_targets = targets
        .get("targetInfos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        .count();
    assert_eq!(page_targets, 0, "Chrome created an extra startup page");
    let _ = connection.command("Browser.close", None, None).await;
    process.shutdown().await.expect("reap Chrome");
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
async fn forced_real_chrome_disconnect_fails_pending_call_and_reaps_process() {
    let host = ChromeHost::default();
    let Some(executable) = host.locate() else {
        return;
    };
    let Ok((mut process, websocket_url)) = ChromeProcess::launch(&executable, true).await else {
        return;
    };
    let profile = process
        .profile_path()
        .expect("Chrome profile")
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
    let target_id = string_field(&target, "targetId", "Target.createTarget").expect("target ID");
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

    process.start_kill().expect("force Chrome exit");
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
