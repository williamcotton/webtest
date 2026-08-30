use webtest_observation::ExecutionEvent;

/// Receives structured execution events as they occur.
///
/// Implementations must return quickly. The runtime retains every event in the
/// final [`crate::RunResult`] regardless of whether a sink is configured.
pub trait RunEventSink: Send + Sync {
    fn publish(&self, event: &ExecutionEvent);
}

pub(crate) fn emit_event(
    events: &mut Vec<ExecutionEvent>,
    sink: Option<&dyn RunEventSink>,
    event: ExecutionEvent,
) {
    if let Some(sink) = sink {
        sink.publish(&event);
    }
    events.push(event);
}
