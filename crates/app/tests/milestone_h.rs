#[path = "support/http_fixture.rs"]
mod http_fixture;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use webtest_observation::{ExecutionEvent, ObservationStore, SemanticEvent};

#[tokio::test]
async fn chrome_runs_cases_patterns_actors_checkpoints_selection_and_guards() {
    let host = webtest_browser_cdp::ChromeHost::default();
    if host.locate().is_none() {
        eprintln!("skipping H Chrome test: Chrome unavailable");
        return;
    }
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AddrNotAvailable
            ) =>
        {
            eprintln!("skipping H Chrome test: loopback unavailable");
            return;
        }
        Err(error) => panic!("websocket fixture: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let websocket = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {biased;_= &mut stopped=>break,Some(_)=connections.join_next()=>{},accepted=listener.accept()=>{let (stream,_)=accepted.unwrap();connections.spawn(async move{let Ok(mut socket)=tokio_tungstenite::accept_async(stream).await else{return};while let Some(Ok(message))=socket.next().await{if message.is_text()&&socket.send(message).await.is_err(){break}}});}}
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    });
    let body = format!(
        r#"<!doctype html><input id="message"><button id="send" disabled onclick="socket.send(JSON.stringify({{message:document.querySelector('#message').value}}))">Send</button><div id="received"></div><script>const socket=new WebSocket('ws://{address}');socket.onopen=()=>document.querySelector('#send').disabled=false;socket.onmessage=e=>document.querySelector('#received').textContent=JSON.parse(e.data).message;</script>"#
    );
    let fixture =
        http_fixture::HttpFixture::start("200 OK", "text/html", &body).expect("HTTP fixture");
    let mut db = webtest_analysis::AnalysisDatabase::default();
    let source = format!(
        r#"
        pattern Message(expected: String) = subset {{message: expected}}
        guard clean(actor: BrowserActor) {{
            when actor.console.error(e) {{fail e.message}}
            when actor.network.response(r) if r.status >= 500 {{fail "server error"}}
        }}
        test "native H" cases [case "first" {{message: "hello"}}, case "second" {{message: "again"}}] as input {{
            actor alice browser
            actor bob browser
            alice {{open "http://{}?token=browser-secret"}}
            bob {{open "http://{}"}}
            alice {{wait id("send").enabled within 5s}}
            let mark = checkpoint(alice)
            with guards [clean(alice), clean(bob)] {{
                alice {{fill id("message") with input.message click id("send")}}
                let received = select alice since mark within 5s {{
                    when websocket.received_json(m) if m.value matches Message(input.message) {{provide m.value}}
                    timeout {{fail "message missing"}}
                }}
                check {{expect received matches Message(input.message) expect input.message matches String}}
                alice {{expect text(input.message).visible}}
            }}
        }}
    "#,
        fixture.address, fixture.address
    );
    let file = db.open_file("native-h.webtest", source);
    let diagnostics = db.diagnostics(file).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let result = webtest_runtime::Runner::new(Arc::new(ObservationStore::default()))
        .run(&plan, &host)
        .await;
    let _ = stop.send(());
    websocket.await.unwrap();
    assert_eq!(result.passed(), 2, "{:?}", result.tests);
    assert!(result.events.iter().any(|event| matches!(
        event,
        ExecutionEvent::Semantic {
            event: SemanticEvent::SelectionMatched {
                source: webtest_model::EventSource::WebsocketReceivedJson,
                ..
            },
            ..
        }
    )));
    let journal = serde_json::to_string(&result.journal).unwrap();
    assert!(
        !journal.contains("token=browser-secret"),
        "public URL evidence must be redacted"
    );
}

#[test]
fn cli_selects_authored_declarations_and_variants_and_preserves_inconclusive_in_junit() {
    use std::{fs, process::Command};
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("tests")).unwrap();
    fs::write(
        directory.path().join("webtest.toml"),
        "[project]\ntest_roots=[\"tests\"]\n",
    )
    .unwrap();
    fs::write(directory.path().join("tests/cases.webtest"),"test \"rows\" cases [case \"one\" {value: 1}, case \"two\" {value: 2}] as input {expect input.value > 0}").unwrap();
    let command = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_webtest"))
            .current_dir(directory.path())
            .args(args)
            .output()
            .unwrap()
    };
    let built = command(&["build", "--emit", "plan.json"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let plan: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("plan.json")).unwrap()).unwrap();
    let variant = plan["tests"][0]["variant_id"].as_str().unwrap();
    let declaration = plan["tests"][0]["test_declaration_id"].as_str().unwrap();
    for (flag, id, count) in [("--variant", variant, 1), ("--test-id", declaration, 2)] {
        let output = command(&["test", flag, id, "--reporter", "json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["summary"]["tests"], count);
        assert_eq!(result["files"][0]["tests"][0]["case_label"], "one");
    }
    fs::write(
        directory.path().join("tests/cases.webtest"),
        "test \"undecided\" {inconclusive \"external prerequisite\"}",
    )
    .unwrap();
    let output = command(&["test", "--reporter", "junit"]);
    assert_eq!(output.status.code(), Some(1));
    let junit = String::from_utf8(output.stdout).unwrap();
    assert!(junit.contains("webtest.verdict"));
    assert!(junit.contains("webtest.inconclusive"));
}
