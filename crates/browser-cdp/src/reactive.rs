//! Direct bounded ingress from the CDP reader. It never consumes the lossy
//! convenience broadcast used by navigation waits.
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Weak},
};
use webtest_browser::{
    BrowserError,
    reactive::{BrowserEvent, ReactiveObserver},
};
use webtest_model::EventSource;
#[derive(Default)]
pub(crate) struct Captures(HashMap<String, Capture>);
struct Capture {
    observer: Weak<dyn ReactiveObserver>,
    sources: BTreeSet<EventSource>,
    sockets: HashMap<String, String>,
    url: String,
    started: std::time::Instant,
}
impl Captures {
    pub fn insert(
        &mut self,
        session: String,
        observer: Arc<dyn ReactiveObserver>,
        sources: &[EventSource],
    ) {
        self.0.insert(
            session,
            Capture {
                observer: Arc::downgrade(&observer),
                sources: sources.iter().copied().collect(),
                sockets: HashMap::new(),
                url: String::new(),
                started: std::time::Instant::now(),
            },
        );
    }
    pub fn remove(&mut self, session: &str) {
        self.0.remove(session);
    }
    pub fn close(&mut self, error: BrowserError) {
        for capture in self.0.values() {
            if let Some(observer) = capture.observer.upgrade() {
                observer.closed(error.clone());
            }
        }
        self.0.clear();
    }
    pub fn observe(&mut self, session: Option<&str>, method: &str, params: &Value) {
        let Some(capture) = session.and_then(|s| self.0.get_mut(s)) else {
            return;
        };
        let Some(observer) = capture.observer.upgrade() else {
            return;
        };
        let timestamp = capture.started.elapsed().as_secs_f64();
        let limit = observer.limits().max_single_event_bytes;
        let mut truncated = false;
        let clip = |value: &str, truncated: &mut bool| {
            if value.len() > limit {
                *truncated = true;
                value.chars().take(limit / 4).collect::<String>()
            } else {
                value.to_owned()
            }
        };
        let string = |value: &Value, key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let (source, payload) = match method {
            "Runtime.consoleAPICalled" => {
                let level = string(params, "type");
                let mut message = String::new();
                if let Some(args) = params["args"].as_array() {
                    for arg in args.iter().take(20) {
                        if !message.is_empty() {
                            message.push(' ')
                        }
                        let text = arg["value"]
                            .as_str()
                            .or_else(|| arg["description"].as_str())
                            .map(str::to_owned)
                            .unwrap_or_else(|| arg["value"].to_string());
                        message.push_str(&clip(&text, &mut truncated));
                        if message.len() > limit {
                            truncated = true;
                            break;
                        }
                    }
                    if args.len() > 20 {
                        truncated = true
                    }
                }
                let frame = &params["stackTrace"]["callFrames"][0];
                (
                    EventSource::ConsoleMessage,
                    json!({"level":level,"message":message,"url":url(frame["url"].as_str().unwrap_or("")),"line":frame["lineNumber"].as_i64().unwrap_or(0),"column":frame["columnNumber"].as_i64().unwrap_or(0),"timestamp":timestamp}),
                )
            }
            "Network.requestWillBeSent" | "Network.responseReceived" => {
                let response = method == "Network.responseReceived";
                let data = &params[if response { "response" } else { "request" }];
                let mut headers = serde_json::Map::new();
                if let Some(values) = data["headers"].as_object() {
                    for (key, value) in values.iter().take(100) {
                        headers.insert(
                            clip(key, &mut truncated),
                            Value::String(clip(value.as_str().unwrap_or(""), &mut truncated)),
                        );
                    }
                    if values.len() > 100 {
                        truncated = true
                    }
                }
                let mut payload = json!({"request_id":string(params,"requestId"),"url":url(&clip(data["url"].as_str().unwrap_or(""),&mut truncated)),"resource_kind":string(params,"type"),"headers":headers,"timestamp":timestamp});
                if response {
                    payload["status"] = json!(data["status"].as_f64().unwrap_or(0.0) as i64)
                } else {
                    payload["method"] = json!(string(data, "method"))
                }
                (
                    if response {
                        EventSource::NetworkResponse
                    } else {
                        EventSource::NetworkRequest
                    },
                    payload,
                )
            }
            "Network.webSocketCreated" => {
                if capture.sockets.len() < 1000 {
                    capture.sockets.insert(
                        string(params, "requestId"),
                        clip(params["url"].as_str().unwrap_or(""), &mut truncated),
                    );
                }
                return;
            }
            "Network.webSocketClosed" => {
                capture
                    .sockets
                    .remove(params["requestId"].as_str().unwrap_or(""));
                return;
            }
            "Network.webSocketFrameReceived" | "Network.webSocketFrameSent" => {
                let received = method == "Network.webSocketFrameReceived";
                let id = string(params, "requestId");
                let text = clip(
                    params["response"]["payloadData"].as_str().unwrap_or(""),
                    &mut truncated,
                );
                let payload = json!({"connection_id":id,"url":url(capture.sockets.get(&id).map(String::as_str).unwrap_or("")),"text":text,"opcode":params["response"]["opcode"].as_i64().unwrap_or(0),"direction":if received{"received"}else{"sent"},"timestamp":timestamp});
                let raw = if received {
                    EventSource::WebsocketReceived
                } else {
                    EventSource::WebsocketSent
                };
                let derived = if received {
                    EventSource::WebsocketReceivedJson
                } else {
                    EventSource::WebsocketSentJson
                };
                if capture.sources.contains(&derived)
                    && !truncated
                    && payload["opcode"] == 1
                    && let Ok(value) =
                        serde_json::from_str::<Value>(payload["text"].as_str().unwrap_or(""))
                {
                    let mut parsed = payload.clone();
                    parsed.as_object_mut().map(|p| {
                        p.remove("text");
                        p.remove("opcode");
                        p.insert("value".into(), value)
                    });
                    publish(observer.as_ref(), derived, parsed, false);
                }
                (raw, payload)
            }
            "Page.frameNavigated" => {
                if params["frame"].get("parentId").is_some() {
                    return;
                }
                let next = clip(
                    params["frame"]["url"].as_str().unwrap_or(""),
                    &mut truncated,
                );
                let previous = std::mem::replace(&mut capture.url, next.clone());
                (
                    EventSource::Navigation,
                    json!({"from_url":url(&previous),"to_url":url(&next),"same_document":false,"navigation_kind":"document","timestamp":timestamp}),
                )
            }
            "Page.navigatedWithinDocument" => {
                let next = clip(params["url"].as_str().unwrap_or(""), &mut truncated);
                let previous = std::mem::replace(&mut capture.url, next.clone());
                (
                    EventSource::Navigation,
                    json!({"from_url":url(&previous),"to_url":url(&next),"same_document":true,"navigation_kind":"same_document","timestamp":timestamp}),
                )
            }
            _ => return,
        };
        if capture.sources.contains(&source)
            || (source == EventSource::ConsoleMessage
                && capture.sources.contains(&EventSource::ConsoleError))
        {
            publish(observer.as_ref(), source, payload, truncated)
        }
    }
}
fn publish(observer: &dyn ReactiveObserver, source: EventSource, payload: Value, truncated: bool) {
    let bytes = serde_json::to_vec(&payload)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    if bytes > observer.limits().max_single_event_bytes {
        observer.observe(BrowserEvent {
            source,
            payload: webtest_model::Value::Null,
            captured_bytes: bytes,
            truncated: true,
        })
    } else {
        observer.observe(BrowserEvent {
            source,
            payload: webtest_model::value_from_json(payload),
            captured_bytes: bytes,
            truncated,
        })
    }
}
fn url(value: &str) -> Value {
    let parsed = url::Url::parse(value).ok();
    json!({"value":value,"path":parsed.as_ref().map(|u|u.path()).unwrap_or(""),"query":parsed.as_ref().and_then(|u|u.query()).unwrap_or(""),"host":parsed.as_ref().and_then(|u|u.host_str()).unwrap_or(""),"scheme":parsed.as_ref().map(|u|u.scheme()).unwrap_or("")})
}
