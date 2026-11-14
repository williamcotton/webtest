//! Normalized browser observations. CDP/transport data never crosses this boundary.
use crate::BrowserError;
use webtest_model::{EventSource, ReactiveLimits, Value};
#[derive(Clone, Debug)]
pub struct BrowserEvent {
    pub source: EventSource,
    pub payload: Value,
    pub captured_bytes: usize,
    pub truncated: bool,
}
impl BrowserEvent {
    pub fn matches(&self, source: EventSource) -> bool {
        self.source == source
            || (source == EventSource::ConsoleError
                && self.source == EventSource::ConsoleMessage
                && self.payload.member("level") == Some(Value::String("error".into())))
    }
}
/// Synchronous bounded ingress only. Implementations must not block on consumers/I/O.
pub trait ReactiveObserver: Send + Sync {
    fn limits(&self) -> ReactiveLimits;
    fn observe(&self, event: BrowserEvent);
    fn closed(&self, error: BrowserError);
}
