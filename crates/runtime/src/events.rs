use webtest_observation::ExecutionEvent;

/// Receives structured execution events as they occur.
///
/// Implementations must return quickly. The runtime retains every event in the
/// final [`crate::RunResult`] regardless of whether a sink is configured.
pub trait RunEventSink: Send + Sync {
    fn publish(&self, event: &ExecutionEvent);
}

pub(crate) fn emit_event(
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    event: ExecutionEvent,
) {
    if let Some(sink) = sink {
        sink.publish(&event);
    }
    events
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(event);
}

/// Shared append ownership lets resource callbacks and descendant scopes publish
/// without holding an execution-state borrow across an await.
#[derive(Default)]
pub(crate) struct EventBuffer(std::sync::Mutex<Vec<ExecutionEvent>>);
impl EventBuffer {
    pub(crate) fn into_events(self) -> Vec<ExecutionEvent> {
        self.0
            .into_inner()
            .unwrap_or_else(|error| error.into_inner())
    }
}
