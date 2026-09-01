use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
use webtest_browser::BrowserError;

use super::CdpConnection;

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
async fn operation_remainder_caps_the_browser_command_budget() {
    let Some((connection, mut server)) = fake_cdp(Duration::from_secs(5)).await else {
        return;
    };
    let server_task = tokio::spawn(async move {
        let _ = server.next().await.expect("command").expect("websocket");
        sleep(Duration::from_millis(100)).await;
    });

    let error = connection
        .command_with_timeout("Capped", None, None, Duration::from_millis(20))
        .await
        .expect_err("operation cap");
    assert_eq!(
        error,
        BrowserError::CommandTimeout {
            method: "Capped".into(),
            timeout_ms: 20,
        }
    );
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
async fn response_sessions_must_match_exactly() {
    let Some((connection, mut server)) = fake_cdp(Duration::from_secs(1)).await else {
        return;
    };
    let call = tokio::spawn(async move {
        connection
            .command("Page.enable", None, Some("expected-session"))
            .await
    });
    let command = server.next().await.expect("command").expect("frame");
    let id = command_id(command);
    server
        .send(Message::Text(
            json!({"id": id, "sessionId": "wrong-session", "result": {}})
                .to_string()
                .into(),
        ))
        .await
        .expect("response");
    assert!(matches!(
        call.await.expect("call task"),
        Err(BrowserError::MalformedProtocol { message })
            if message.contains("wrong-session") && message.contains("expected-session")
    ));
}

#[tokio::test]
async fn protocol_errors_and_missing_payloads_remain_distinct() {
    let Some((connection, mut server)) = fake_cdp(Duration::from_secs(1)).await else {
        return;
    };
    let first_connection = connection.clone();
    let protocol =
        tokio::spawn(async move { first_connection.command("Runtime.fail", None, None).await });
    let id = command_id(server.next().await.expect("command").expect("frame"));
    server
        .send(Message::Text(
            json!({"id": id, "error": {"code": -1, "message": "bad method"}})
                .to_string()
                .into(),
        ))
        .await
        .expect("protocol error");
    assert!(matches!(
        protocol.await.expect("protocol task"),
        Err(BrowserError::Protocol { method, message })
            if method == "Runtime.fail" && message == "bad method (-1)"
    ));

    let missing_connection = connection.clone();
    let missing = tokio::spawn(async move {
        missing_connection
            .command("Runtime.empty", None, None)
            .await
    });
    let id = command_id(server.next().await.expect("command").expect("frame"));
    server
        .send(Message::Text(json!({"id": id}).to_string().into()))
        .await
        .expect("empty response");
    assert!(matches!(
        missing.await.expect("missing task"),
        Err(BrowserError::MalformedProtocol { message })
            if message.contains("neither result nor error")
    ));
    assert_eq!(connection.in_flight(), 0);
}

#[tokio::test]
async fn binary_frames_are_terminal_and_fail_every_pending_command() {
    let Some((connection, mut server)) = fake_cdp(Duration::from_secs(1)).await else {
        return;
    };
    let first_connection = connection.clone();
    let second_connection = connection.clone();
    let first = tokio::spawn(async move { first_connection.command("First", None, None).await });
    let second = tokio::spawn(async move { second_connection.command("Second", None, None).await });
    let _ = server.next().await.expect("first command").expect("frame");
    let _ = server.next().await.expect("second command").expect("frame");
    server
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .expect("binary response");
    for result in [
        first.await.expect("first task"),
        second.await.expect("second task"),
    ] {
        assert!(matches!(
            result,
            Err(BrowserError::MalformedProtocol { ref message })
                if message.contains("binary CDP response")
        ));
    }
    assert_eq!(connection.in_flight(), 0);
}

#[tokio::test]
async fn dropped_command_receivers_are_removed_by_the_sweep() {
    let Some((connection, mut server)) = fake_cdp(Duration::from_secs(1)).await else {
        return;
    };
    let pending_connection = connection.clone();
    let pending =
        tokio::spawn(async move { pending_connection.command("Dropped", None, None).await });
    let _ = server.next().await.expect("command").expect("frame");
    pending.abort();
    sleep(Duration::from_millis(30)).await;
    assert_eq!(connection.in_flight(), 0);
}

#[tokio::test]
async fn console_entries_are_bounded_ordered_and_filter_unrelated_events() {
    let Some((connection, mut server)) = fake_cdp(Duration::from_secs(1)).await else {
        return;
    };
    let call_connection = connection.clone();
    let call =
        tokio::spawn(async move { call_connection.command("After.events", None, None).await });
    let id = command_id(server.next().await.expect("command").expect("frame"));
    server
        .send(Message::Text(
            json!({"method": "Page.unrelated", "params": {}})
                .to_string()
                .into(),
        ))
        .await
        .expect("unrelated event");
    for sequence in 0..25 {
        server
            .send(Message::Text(
                json!({
                    "method": "Runtime.exceptionThrown",
                    "params": {"exceptionDetails": {"text": format!("exception-{sequence}")}}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("exception event");
    }
    server
        .send(Message::Text(
            json!({
                "method": "Runtime.consoleAPICalled",
                "params": {"type": "error"}
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("console event");
    server
        .send(Message::Text(
            json!({"id": id, "result": {}}).to_string().into(),
        ))
        .await
        .expect("response");
    call.await.expect("call task").expect("response result");

    let errors = connection.console_errors().await;
    assert_eq!(errors.len(), 20);
    assert_eq!(errors.first().map(String::as_str), Some("exception-6"));
    assert_eq!(errors.last().map(String::as_str), Some("console.error"));
}
