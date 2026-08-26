use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use webtest_app_bridge::{AppManifest, FieldSchema, FunctionSchema, TypeSchema};

struct ProtocolProcess {
    child: Child,
    input: ChildStdin,
    messages: Receiver<Value>,
    pending: Mutex<VecDeque<Value>>,
}

impl ProtocolProcess {
    fn spawn(arguments: &[&str], directory: &std::path::Path) -> Self {
        Self::spawn_with_chrome(arguments, directory, None)
    }

    fn spawn_with_chrome(
        arguments: &[&str],
        directory: &std::path::Path,
        chrome: Option<&std::path::Path>,
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_webtest"));
        command
            .args(arguments)
            .current_dir(directory)
            .env_remove("WEBTEST_CHROME_PATH")
            .env("WEBTEST_CACHE_DIR", directory.join("cache"));
        if let Some(chrome) = chrome {
            command.env("WEBTEST_CHROME_PATH", chrome);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn protocol server");
        let input = child.stdin.take().expect("protocol stdin");
        let output = child.stdout.take().expect("protocol stdout");
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(output);
            while let Some(message) = read_frame(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            messages,
            pending: Mutex::new(VecDeque::new()),
        }
    }

    fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).expect("serialize protocol message");
        write!(self.input, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
        self.input.write_all(&body).expect("write body");
        self.input.flush().expect("flush protocol message");
    }

    fn receive(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        {
            let mut pending = self.pending.lock().expect("pending protocol messages");
            if let Some(index) = pending.iter().position(&predicate) {
                return pending.remove(index).expect("matched pending message");
            }
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .messages
                .recv_timeout(remaining)
                .unwrap_or_else(|error| {
                    let pending = self.pending.lock().expect("pending protocol messages");
                    panic!("timed out waiting for protocol response: {error}; pending={pending:#?}")
                });
            if predicate(&message) {
                return message;
            }
            self.pending
                .lock()
                .expect("pending protocol messages")
                .push_back(message);
        }
    }

    fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.child.try_wait().expect("server status").is_some() {
                return;
            }
            if Instant::now() >= deadline {
                self.child.kill().expect("terminate hung protocol server");
                panic!("protocol server did not exit after shutdown");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn available_chrome() -> Option<std::path::PathBuf> {
    webtest_browser_manager::BrowserManager::new()
        .ok()
        .and_then(|manager| manager.current().ok().flatten())
        .map(|installed| installed.executable)
        .or_else(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/protocol-browser-cache");
            webtest_browser_manager::BrowserManager::with_cache_root(root)
                .current()
                .ok()
                .flatten()
                .map(|installed| installed.executable)
        })
        .or_else(webtest_browser_cdp::find_system_chrome)
}

fn runtime_protocol_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fixture_server() -> Option<(std::net::SocketAddr, Arc<AtomicBool>)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    listener.set_nonblocking(true).ok()?;
    let address = listener.local_addr().ok()?;
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    thread::spawn(move || {
        while !server_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0u8; 2048];
                    let _ = stream.read(&mut request);
                    let body = "<!doctype html><button id=\"submit\">Submit</button>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    Some((address, stop))
}

fn json_fixture_server() -> Option<(std::net::SocketAddr, Arc<AtomicBool>)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    listener.set_nonblocking(true).ok()?;
    let address = listener.local_addr().ok()?;
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    thread::spawn(move || {
        while !server_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0u8; 2048];
                    let _ = stream.read(&mut request);
                    let body = r#"{"id":7,"email":"alice@example.test"}"#;
                    let response = format!(
                        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    Some((address, stop))
}

impl Drop for ProtocolProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
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
    let mut body = vec![0; content_length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

#[test]
fn lsp_stdio_covers_document_features_and_shutdown() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("buffer.webtest");
    std::fs::write(&path, "this on-disk source is deliberately invalid")
        .expect("write on-disk source");
    let uri = format!("file://{}", path.display());
    let mut lsp = ProtocolProcess::spawn(&["lsp"], directory.path());

    lsp.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"capabilities": {}}
    }));
    let initialized = lsp.receive(|message| message["id"] == 1);
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        "utf-16"
    );
    assert!(initialized["result"]["capabilities"]["semanticTokensProvider"].is_object());
    lsp.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));

    let broken = "test \"broken\" {\n";
    lsp.send(json!({
        "jsonrpc":"2.0",
        "method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":uri,"languageId":"webtest","version":1,"text":broken}}
    }));
    let diagnostics = lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["version"] == 1
    });
    assert!(
        !diagnostics["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty()
    );

    lsp.send(json!({
        "jsonrpc":"2.0","id":2,"method":"textDocument/formatting",
        "params":{"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}}
    }));
    assert!(lsp.receive(|message| message["id"] == 2)["result"].is_array());

    lsp.send(json!({
        "jsonrpc":"2.0","id":3,"method":"textDocument/semanticTokens/full",
        "params":{"textDocument":{"uri":uri}}
    }));
    assert!(lsp.receive(|message| message["id"] == 3)["result"]["data"].is_array());

    lsp.send(json!({
        "jsonrpc":"2.0","id":4,"method":"workspace/executeCommand",
        "params":{"command":"webtest.runFile","arguments":[uri]}
    }));
    assert!(
        lsp.receive(|message| message["id"] == 4)
            .get("result")
            .is_some()
    );

    let valid = "test \"synchronized buffer\" {}\n";
    lsp.send(json!({
        "jsonrpc":"2.0","method":"textDocument/didChange",
        "params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":valid}]}
    }));
    let cleared = lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["version"] == 2
    });
    assert!(
        cleared["params"]["diagnostics"]
            .as_array()
            .expect("cleared diagnostics")
            .is_empty()
    );

    lsp.send(json!({"jsonrpc":"2.0","id":5,"method":"shutdown","params":null}));
    assert!(
        lsp.receive(|message| message["id"] == 5)
            .get("result")
            .is_some()
    );
    lsp.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    lsp.wait_for_exit();
}

#[test]
fn lsp_resolves_app_provider_from_the_open_documents_nearest_project() {
    let directory = tempfile::tempdir().expect("temp directory");
    let project = directory.path().join("nested-app-project");
    let schema_directory = project.join(".webtest");
    std::fs::create_dir_all(&schema_directory).expect("create schema directory");
    let manifest = AppManifest {
        manifest_version: 1,
        protocol: 1,
        provider: "app".into(),
        sdk: "protocol-test".into(),
        sdk_version: "1.0.0".into(),
        schema_hash: String::new(),
        functions: [(
            "create_user".into(),
            FunctionSchema {
                documentation: "Create a user for a test.".into(),
                retry_safe: false,
                params: TypeSchema::Object {
                    fields: [(
                        "email".into(),
                        FieldSchema {
                            ty: TypeSchema::String,
                            documentation: "Unique email address.".into(),
                            optional: false,
                            secret: false,
                            default: None,
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
                returns: TypeSchema::Object {
                    fields: [(
                        "email".into(),
                        FieldSchema {
                            ty: TypeSchema::String,
                            documentation: String::new(),
                            optional: false,
                            secret: false,
                            default: None,
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            },
        )]
        .into_iter()
        .collect(),
    }
    .with_computed_hash()
    .expect("compute manifest hash");
    std::fs::write(
        schema_directory.join("app-schema.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    std::fs::write(
        project.join("webtest.toml"),
        "[project]\ntest_roots = [\"case.webtest\"]\n\n[server.app]\nschema = \".webtest/app-schema.json\"\n",
    )
    .expect("write project configuration");

    let source = "test \"app\" { server { let user = app.create_user(email: \"a@example.test\") expect user.email == \"a@example.test\" } }";
    let path = project.join("case.webtest");
    std::fs::write(&path, source).expect("write source");
    let uri = format!("file://{}", path.display());
    let mut lsp = ProtocolProcess::spawn(&["lsp"], directory.path());
    lsp.send(json!({
        "jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}
    }));
    lsp.receive(|message| message["id"] == 1);
    lsp.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    lsp.send(json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":uri,"languageId":"webtest","version":1,"text":source}}
    }));
    let diagnostics = lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["version"] == 1
    });
    assert!(
        diagnostics["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty(),
        "{diagnostics:#?}"
    );

    let partial = "test \"app\" { server { let user = app. } }";
    lsp.send(json!({
        "jsonrpc":"2.0","method":"textDocument/didChange",
        "params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":partial}]}
    }));
    let partial_diagnostics = lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["version"] == 2
    });
    assert!(
        partial_diagnostics["params"]["diagnostics"]
            .as_array()
            .expect("partial diagnostics")
            .iter()
            .all(|diagnostic| {
                diagnostic["code"] != "semantic.reserved_provider"
                    && diagnostic["code"] != "semantic.unknown_provider"
            }),
        "{partial_diagnostics:#?}"
    );

    let completion_character = partial.find("app.").expect("app call") + "app.".len();
    lsp.send(json!({
        "jsonrpc":"2.0","id":2,"method":"textDocument/completion",
        "params":{"textDocument":{"uri":uri},"position":{"line":0,"character":completion_character}}
    }));
    let completions = lsp.receive(|message| message["id"] == 2);
    assert!(
        completions["result"]
            .as_array()
            .expect("completion items")
            .iter()
            .any(|completion| completion["label"] == "create_user"),
        "{completions:#?}"
    );

    lsp.send(json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}));
    lsp.receive(|message| message["id"] == 3);
    lsp.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    lsp.wait_for_exit();
}

#[test]
fn dap_stdio_covers_launch_breakpoint_continue_and_disconnect() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("debug.webtest");
    std::fs::write(
        &path,
        "test \"debug\" {\n    browser {\n        open \"about:blank\"\n    }\n}\n",
    )
    .expect("write debug source");
    let mut dap = ProtocolProcess::spawn(&["dap", "--headless"], directory.path());
    let mut seq = 1;
    let mut request = |dap: &mut ProtocolProcess, command: &str, arguments: Value| {
        let request_seq = seq;
        seq += 1;
        dap.send(json!({
            "seq":request_seq,"type":"request","command":command,"arguments":arguments
        }));
        dap.receive(|message| {
            message["type"] == "response" && message["request_seq"] == request_seq
        })
    };

    let initialize = request(&mut dap, "initialize", json!({"adapterID":"webtest"}));
    assert_eq!(initialize["success"], true);
    assert_eq!(initialize["body"]["supportsConfigurationDoneRequest"], true);
    assert_eq!(
        request(
            &mut dap,
            "launch",
            json!({"program":path,"stopOnEntry":true,"headed":false})
        )["success"],
        true
    );
    let breakpoints = request(
        &mut dap,
        "setBreakpoints",
        json!({"source":{"path":path},"breakpoints":[{"line":3}]}),
    );
    assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], true);
    assert_eq!(
        request(&mut dap, "configurationDone", json!({}))["success"],
        true
    );
    assert_eq!(
        request(&mut dap, "continue", json!({"threadId":1}))["success"],
        true
    );
    assert_eq!(
        request(&mut dap, "disconnect", json!({"terminateDebuggee":true}))["success"],
        true
    );
    dap.wait_for_exit();
}

#[test]
fn dap_project_path_loads_the_test_files_nearest_configuration() {
    let _runtime_test = runtime_protocol_lock();
    let Some((address, stop_server)) = fixture_server() else {
        return;
    };
    let directory = tempfile::tempdir().expect("temp directory");
    let project = directory.path().join("nested-project");
    std::fs::create_dir(&project).expect("create nested project");
    std::fs::write(
        project.join("webtest.toml"),
        format!("[server]\nbase_url = \"http://{address}\"\n"),
    )
    .expect("write project configuration");
    let path = project.join("debug.webtest");
    std::fs::write(
        &path,
        "test \"configured provider\" {\n    server {\n        let response = http.get(\"/health\")\n        expect response.status == 200\n    }\n}\n",
    )
    .expect("write debug source");
    let project_argument = path.to_string_lossy().into_owned();
    let mut dap = ProtocolProcess::spawn(
        &["dap", "--headless", "--project", &project_argument],
        directory.path(),
    );
    let mut seq = 1;
    let mut request = |dap: &mut ProtocolProcess, command: &str, arguments: Value| {
        let request_seq = seq;
        seq += 1;
        dap.send(json!({
            "seq":request_seq,"type":"request","command":command,"arguments":arguments
        }));
        dap.receive(|message| {
            message["type"] == "response" && message["request_seq"] == request_seq
        })
    };

    assert_eq!(
        request(&mut dap, "initialize", json!({"adapterID":"webtest"}))["success"],
        true
    );
    assert_eq!(
        request(
            &mut dap,
            "launch",
            json!({"program":path,"stopOnEntry":false,"headed":false})
        )["success"],
        true
    );
    assert_eq!(
        request(&mut dap, "configurationDone", json!({}))["success"],
        true
    );
    let output = dap.receive(|message| {
        message["type"] == "event"
            && message["event"] == "output"
            && message["body"]["output"]
                .as_str()
                .is_some_and(|output| output.contains("test \"configured provider\" ... ok"))
    });
    assert_eq!(output["body"]["category"], "stdout");
    dap.receive(|message| message["type"] == "event" && message["event"] == "terminated");
    dap.wait_for_exit();
    stop_server.store(true, Ordering::Release);
}

//
// flaky unless single threaded
//
// #[test]
// fn dap_variables_keep_server_bindings_visible_at_a_browser_step() {
//     let _runtime_test = runtime_protocol_lock();
//     let Some(chrome) = available_chrome() else {
//         return;
//     };
//     let Some((address, stop_server)) = json_fixture_server() else {
//         return;
//     };
//     let directory = tempfile::tempdir().expect("temp directory");
//     let project = directory.path().join("nested-project");
//     std::fs::create_dir(&project).expect("create nested project");
//     std::fs::write(
//         project.join("webtest.toml"),
//         format!(
//             "[server]\nbase_url = \"http://{address}\"\n[browser]\nbase_url = \"http://{address}\"\n"
//         ),
//     )
//     .expect("write project configuration");
//     let path = project.join("debug.webtest");
//     std::fs::write(
//         &path,
//         "test \"server variables\" {\n    server {\n        let response = http.get(\"/api/user\")\n        expect response.status == 201\n        let user: { id: Int, email: String } = response.json\n        expect user.id == 7\n    }\n    browser {\n        open \"/login\"\n    }\n}\n",
//     )
//     .expect("write debug source");
//     let project_argument = path.to_string_lossy().into_owned();
//     let mut dap = ProtocolProcess::spawn_with_chrome(
//         &["dap", "--headless", "--project", &project_argument],
//         directory.path(),
//         Some(&chrome),
//     );
//     let mut seq = 1;
//     let mut request = |dap: &mut ProtocolProcess, command: &str, arguments: Value| {
//         let request_seq = seq;
//         seq += 1;
//         dap.send(json!({
//             "seq":request_seq,"type":"request","command":command,"arguments":arguments
//         }));
//         dap.receive(|message| {
//             message["type"] == "response" && message["request_seq"] == request_seq
//         })
//     };

//     assert_eq!(
//         request(&mut dap, "initialize", json!({"adapterID":"webtest"}))["success"],
//         true
//     );
//     assert_eq!(
//         request(
//             &mut dap,
//             "launch",
//             json!({"program":path,"stopOnEntry":false,"headed":false})
//         )["success"],
//         true
//     );
//     let breakpoints = request(
//         &mut dap,
//         "setBreakpoints",
//         json!({"source":{"path":path},"breakpoints":[{"line":9}]}),
//     );
//     assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], true);
//     request(&mut dap, "configurationDone", json!({}));
//     dap.receive(|message| message["type"] == "event" && message["event"] == "stopped");
//     let variables = request(&mut dap, "variables", json!({"variablesReference":1}));
//     let variables = variables["body"]["variables"]
//         .as_array()
//         .expect("debug variables");
//     let response = variables
//         .iter()
//         .find(|variable| variable["name"] == "response")
//         .expect("response variable");
//     assert_eq!(response["type"], "response");
//     assert!(
//         response["value"]
//             .as_str()
//             .is_some_and(|value| value.contains("\"status\":201"))
//     );
//     let response_reference = response["variablesReference"]
//         .as_i64()
//         .expect("expandable response reference");
//     assert!(response_reference > 1);
//     let user = variables
//         .iter()
//         .find(|variable| variable["name"] == "user")
//         .expect("user variable");
//     assert_eq!(user["type"], "object");
//     assert!(
//         user["value"]
//             .as_str()
//             .is_some_and(|value| value.contains("alice@example.test"))
//     );
//     let user_reference = user["variablesReference"]
//         .as_i64()
//         .expect("expandable user reference");
//     assert!(user_reference > 1);

//     let response_children = request(
//         &mut dap,
//         "variables",
//         json!({"variablesReference":response_reference}),
//     );
//     let response_children = response_children["body"]["variables"]
//         .as_array()
//         .expect("response children");
//     assert!(
//         response_children
//             .iter()
//             .any(|child| { child["name"] == "status" && child["value"] == "201" })
//     );
//     let json = response_children
//         .iter()
//         .find(|child| child["name"] == "json")
//         .expect("response JSON child");
//     let json_reference = json["variablesReference"]
//         .as_i64()
//         .expect("expandable JSON reference");
//     let json_children = request(
//         &mut dap,
//         "variables",
//         json!({"variablesReference":json_reference}),
//     );
//     let json_children = json_children["body"]["variables"]
//         .as_array()
//         .expect("JSON children");
//     assert!(
//         json_children.iter().any(|child| {
//             child["name"] == "email" && child["value"] == "\"alice@example.test\""
//         })
//     );

//     let user_children = request(
//         &mut dap,
//         "variables",
//         json!({"variablesReference":user_reference}),
//     );
//     let user_children = user_children["body"]["variables"]
//         .as_array()
//         .expect("user children");
//     assert!(
//         user_children
//             .iter()
//             .any(|child| { child["name"] == "id" && child["value"] == "7" })
//     );
//     assert_eq!(
//         request(&mut dap, "disconnect", json!({"terminateDebuggee":true}))["success"],
//         true
//     );
//     dap.wait_for_exit();
//     stop_server.store(true, Ordering::Release);
// }

#[test]
fn lsp_real_run_publishes_and_then_clears_runtime_diagnostic() {
    let _runtime_test = runtime_protocol_lock();
    let Some(chrome) = available_chrome() else {
        return;
    };
    let Some((address, stop_server)) = fixture_server() else {
        return;
    };
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("runtime.webtest");
    std::fs::write(&path, "on-disk text is not the synchronized test")
        .expect("write runtime source");
    let uri = format!("file://{}", path.display());
    let mut lsp = ProtocolProcess::spawn_with_chrome(&["lsp"], directory.path(), Some(&chrome));

    lsp.send(json!({
        "jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}
    }));
    lsp.receive(|message| message["id"] == 1);
    lsp.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    let failing = format!(
        "test \"runtime\" {{ browser {{ open \"http://{address}\" click id(\"does-not-exist\") }} }}"
    );
    lsp.send(json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":uri,"languageId":"webtest","version":1,"text":failing}}
    }));
    lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["version"] == 1
    });
    lsp.send(json!({
        "jsonrpc":"2.0","id":2,"method":"workspace/executeCommand",
        "params":{"command":"webtest.runFile","arguments":[uri]}
    }));
    let runtime = lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| {
                    diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic["code"] == "runtime.locator_not_found")
                })
    });
    let diagnostic = runtime["params"]["diagnostics"]
        .as_array()
        .expect("runtime diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "runtime.locator_not_found")
        .expect("runtime locator diagnostic");
    assert_eq!(
        diagnostic["message"],
        "No element with id \"does-not-exist\" was found during the last test run."
    );
    assert!(
        diagnostic["range"]["end"]["character"]
            .as_u64()
            .expect("end")
            > diagnostic["range"]["start"]["character"]
                .as_u64()
                .expect("start")
    );
    lsp.receive(|message| message["id"] == 2);

    let passing = failing.replace("does-not-exist", "submit");
    lsp.send(json!({
        "jsonrpc":"2.0","method":"textDocument/didChange",
        "params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":passing}]}
    }));
    let edit_clear = lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["version"] == 2
    });
    assert!(
        edit_clear["params"]["diagnostics"]
            .as_array()
            .expect("edit diagnostics")
            .is_empty()
    );
    lsp.send(json!({
        "jsonrpc":"2.0","id":3,"method":"workspace/executeCommand",
        "params":{"command":"webtest.runFile","arguments":[uri]}
    }));
    let rerun_clear = lsp.receive(|message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["version"] == 2
    });
    assert!(
        rerun_clear["params"]["diagnostics"]
            .as_array()
            .expect("rerun diagnostics")
            .is_empty()
    );
    lsp.receive(|message| message["id"] == 3);
    lsp.send(json!({"jsonrpc":"2.0","id":4,"method":"shutdown","params":null}));
    lsp.receive(|message| message["id"] == 4);
    lsp.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    lsp.wait_for_exit();
    stop_server.store(true, Ordering::Release);
}

#[test]
fn dap_headed_session_stops_at_a_real_source_breakpoint() {
    let _runtime_test = runtime_protocol_lock();
    let Some(chrome) = available_chrome() else {
        return;
    };
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }

    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("headed-debug.webtest");
    std::fs::write(
        &path,
        "test \"headed breakpoint\" {\n    browser {\n        open \"about:blank\"\n    }\n}\n",
    )
    .expect("write headed debug source");
    let mut dap = ProtocolProcess::spawn_with_chrome(&["dap"], directory.path(), Some(&chrome));
    let mut seq = 1;
    let mut request = |dap: &mut ProtocolProcess, command: &str, arguments: Value| {
        let request_seq = seq;
        seq += 1;
        dap.send(json!({
            "seq":request_seq,"type":"request","command":command,"arguments":arguments
        }));
        dap.receive(|message| {
            message["type"] == "response" && message["request_seq"] == request_seq
        })
    };

    assert_eq!(
        request(&mut dap, "initialize", json!({"adapterID":"webtest"}))["success"],
        true
    );
    assert_eq!(
        request(
            &mut dap,
            "launch",
            json!({"program":path,"stopOnEntry":false,"headed":true})
        )["success"],
        true
    );
    assert_eq!(
        request(
            &mut dap,
            "setBreakpoints",
            json!({"source":{"path":path},"breakpoints":[{"line":3}]})
        )["body"]["breakpoints"][0]["verified"],
        true
    );
    request(&mut dap, "configurationDone", json!({}));
    let stopped =
        dap.receive(|message| message["type"] == "event" && message["event"] == "stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");
    let stack = request(&mut dap, "stackTrace", json!({"threadId":1}));
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
    assert!(
        stack["body"]["stackFrames"][0]["name"]
            .as_str()
            .expect("frame name")
            .starts_with("open")
    );
    request(&mut dap, "continue", json!({"threadId":1}));
    dap.receive(|message| message["type"] == "event" && message["event"] == "terminated");
    dap.wait_for_exit();
}
