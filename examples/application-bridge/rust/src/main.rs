use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn main() {
    if env::var("WEBTEST").as_deref() != Ok("1") {
        panic!("test-only bridge")
    }
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(".webtest/app-schema.json").unwrap())
            .unwrap();
    let users = Arc::new(Mutex::new(HashMap::<String, Value>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let http_users = users.clone();
    let http_stop = stop.clone();
    let port = env::var("PORT").unwrap_or_else(|_| "3108".into());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let http = thread::spawn(move || {
        while !http_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    handle_http(stream, &http_users)
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
    });
    bridge(&manifest, &users);
    stop.store(true, Ordering::Relaxed);
    http.join().unwrap();
}
fn handle_http(mut stream: TcpStream, users: &Arc<Mutex<HashMap<String, Value>>>) {
    let request_bytes = read_http_request(&mut stream);
    let request = String::from_utf8_lossy(&request_bytes);
    let first = request.lines().next().unwrap_or("");
    let mut request_line = first.split_whitespace();
    let method = request_line.next().unwrap_or("");
    let target = request_line.next().unwrap_or("");
    let target = target.split_once("://").map_or(target, |(_, authority)| {
        authority.find('/').map_or("/", |index| &authority[index..])
    });
    let target = target
        .split(['?', '#'])
        .next()
        .unwrap_or(target)
        .trim_end_matches('/');
    let body = if method == "GET" && target == "/health" {
        "ok".into()
    } else if method == "GET" && target == "/login" {
        r#"<form method="post"><label>Email <input name="email"></label><button>Sign in</button></form>"#.into()
    } else if method == "POST" && target == "/login" {
        let raw = request.split("\r\n\r\n").nth(1).unwrap_or("");
        let email = url_decode(raw.strip_prefix("email=").unwrap_or(""));
        if users.lock().unwrap().contains_key(&email) {
            format!("<p>Welcome, {email}</p>")
        } else {
            "<p>Invalid sign in</p>".into()
        }
    } else {
        "not found".into()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = stream.read(&mut buffer).unwrap_or(0);
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}
fn bridge(manifest: &Value, users: &Arc<Mutex<HashMap<String, Value>>>) {
    let endpoint = env::var("WEBTEST_BRIDGE").unwrap();
    let address = endpoint.strip_prefix("tcp://").expect("tcp endpoint");
    assert!(address.starts_with("127.0.0.1:"));
    let mut stream = TcpStream::connect(address).unwrap();
    send(
        &mut stream,
        json!({"type":"hello","protocol_versions":[1],"sdk":"webtest-rust-example","sdk_version":"0.1.0","token":env::var("WEBTEST_TOKEN").unwrap(),"capabilities":{"cancel":false,"events":false}}),
    );
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    if !line.contains("hello_ok") {
        return;
    }
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            return;
        }
        let msg: Value = serde_json::from_str(&line).unwrap();
        let kind = msg["type"].as_str().unwrap();
        let id = &msg["id"];
        match kind {
            "describe" => send(
                &mut stream,
                json!({"type":"schema","id":id,"protocol":1,"schema_hash":manifest["schema_hash"],"functions":manifest["functions"]}),
            ),
            "call" => {
                let args = &msg["arguments"];
                let email = args["email"].as_str().unwrap();
                let mut map = users.lock().unwrap();
                if map.contains_key(email) {
                    send(
                        &mut stream,
                        json!({"type":"error","id":id,"code":"user.email_taken","message":"email already exists","retryable":false,"data":{}}),
                    )
                } else {
                    let user = json!({"id":map.len()+1,"email":email,"admin":args["admin"].as_bool().unwrap_or(false)});
                    map.insert(email.into(), user.clone());
                    send(&mut stream, json!({"type":"result","id":id,"value":user}))
                }
            }
            "ping" => send(&mut stream, json!({"type":"pong","id":id})),
            "shutdown" => {
                send(&mut stream, json!({"type":"shutdown_ok","id":id}));
                return;
            }
            _ => return,
        }
    }
}
fn send(stream: &mut TcpStream, value: Value) {
    writeln!(stream, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    stream.flush().unwrap()
}
fn url_decode(value: &str) -> String {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1
    }
    String::from_utf8_lossy(&out).into()
}
