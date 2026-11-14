//! Protocol-neutral event identities, supplied-input limits and declarative payload schemas.
use crate::{RecordField, Type};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    ConsoleMessage,
    ConsoleError,
    NetworkRequest,
    NetworkResponse,
    WebsocketReceived,
    WebsocketSent,
    WebsocketReceivedJson,
    WebsocketSentJson,
    Navigation,
}
impl EventSource {
    pub const ALL: [Self; 9] = [
        Self::ConsoleMessage,
        Self::ConsoleError,
        Self::NetworkRequest,
        Self::NetworkResponse,
        Self::WebsocketReceived,
        Self::WebsocketSent,
        Self::WebsocketReceivedJson,
        Self::WebsocketSentJson,
        Self::Navigation,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::ConsoleMessage => "console.message",
            Self::ConsoleError => "console.error",
            Self::NetworkRequest => "network.request",
            Self::NetworkResponse => "network.response",
            Self::WebsocketReceived => "websocket.received",
            Self::WebsocketSent => "websocket.sent",
            Self::WebsocketReceivedJson => "websocket.received_json",
            Self::WebsocketSentJson => "websocket.sent_json",
            Self::Navigation => "navigation",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.name() == name)
    }
    pub fn payload_type(self) -> Type {
        let mut fields = match self {
            Self::ConsoleMessage | Self::ConsoleError => vec![
                ("level", Type::String),
                ("message", Type::String),
                ("url", url_type()),
                ("line", Type::Int),
                ("column", Type::Int),
            ],
            Self::NetworkRequest => vec![
                ("request_id", Type::String),
                ("method", Type::String),
                ("url", url_type()),
                ("resource_kind", Type::String),
                ("headers", Type::Json),
            ],
            Self::NetworkResponse => vec![
                ("request_id", Type::String),
                ("url", url_type()),
                ("status", Type::Int),
                ("resource_kind", Type::String),
                ("headers", Type::Json),
            ],
            Self::WebsocketReceived | Self::WebsocketSent => vec![
                ("connection_id", Type::String),
                ("url", url_type()),
                ("text", Type::String),
                ("opcode", Type::Int),
                ("direction", Type::String),
            ],
            Self::WebsocketReceivedJson | Self::WebsocketSentJson => vec![
                ("connection_id", Type::String),
                ("url", url_type()),
                ("value", Type::Json),
                ("direction", Type::String),
            ],
            Self::Navigation => vec![
                ("from_url", url_type()),
                ("to_url", url_type()),
                ("same_document", Type::Bool),
                ("navigation_kind", Type::String),
            ],
        };
        fields.push(("timestamp", Type::Float));
        record(fields)
    }
}
fn record(fields: Vec<(&str, Type)>) -> Type {
    Type::Record(
        fields
            .into_iter()
            .map(|(name, ty)| {
                (
                    name.into(),
                    RecordField {
                        ty,
                        optional: false,
                        documentation: String::new(),
                        secret: false,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
    )
}
fn url_type() -> Type {
    record(vec![
        ("value", Type::String),
        ("path", Type::String),
        ("query", Type::String),
        ("host", Type::String),
        ("scheme", Type::String),
    ])
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReactiveLimits {
    pub max_events_per_actor: usize,
    pub max_event_bytes_per_actor: usize,
    pub max_single_event_bytes: usize,
    pub max_unmatched_evidence: usize,
}
impl Default for ReactiveLimits {
    fn default() -> Self {
        Self {
            max_events_per_actor: 10000,
            max_event_bytes_per_actor: 8388608,
            max_single_event_bytes: 262144,
            max_unmatched_evidence: 20,
        }
    }
}
impl ReactiveLimits {
    pub fn valid(self) -> bool {
        (1..=100000).contains(&self.max_events_per_actor)
            && (1024..=67108864).contains(&self.max_event_bytes_per_actor)
            && (256..=1048576).contains(&self.max_single_event_bytes)
            && self.max_single_event_bytes <= self.max_event_bytes_per_actor
            && self.max_unmatched_evidence <= 100
    }
}
