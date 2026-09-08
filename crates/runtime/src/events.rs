use webtest_observation::{EventJournal, EventTime, ExecutionEvent, RecordedEvent};

/// Receives structured execution events as they occur.
///
/// Implementations must return quickly. The runtime retains every event in the
/// final [`crate::RunResult`] regardless of whether a sink is configured.
pub trait RunEventSink: Send + Sync {
    fn publish(&self, event: &ExecutionEvent);

    /// Receives the authoritative identity assigned before publication. Existing
    /// sinks may keep consuming the typed event through the default projection.
    fn publish_record(&self, record: &RecordedEvent) {
        self.publish(&record.event);
    }
}

pub(crate) fn emit_event(
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    event: ExecutionEvent,
) {
    let publication = {
        let mut journal = events
            .journal
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let record = journal.record(
            event,
            EventTime {
                since_unix_epoch: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default(),
                elapsed: events.started.elapsed(),
            },
        );
        sink.map(|sink| (sink, record.clone()))
    };
    if let Some((sink, record)) = publication {
        sink.publish_record(&record);
    }
}

/// Only the append service is shared. Scope/branch state remains independently
/// owned. The authoritative record exists before any optional sink sees it.
pub(crate) struct EventBuffer {
    journal: std::sync::Mutex<EventJournal>,
    started: tokio::time::Instant,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self {
            journal: Default::default(),
            started: tokio::time::Instant::now(),
        }
    }
}

impl EventBuffer {
    pub(crate) fn into_records(self) -> Vec<RecordedEvent> {
        self.journal
            .into_inner()
            .unwrap_or_else(|error| error.into_inner())
            .into_records()
    }

    #[cfg(test)]
    pub(crate) fn into_events(self) -> Vec<ExecutionEvent> {
        self.into_records()
            .into_iter()
            .map(|record| record.event)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use webtest_observation::{ExecutionId, ReplayJournal, ReplayOutcome};

    struct CheckPublication(Arc<EventBuffer>);
    impl RunEventSink for CheckPublication {
        fn publish(&self, _: &ExecutionEvent) {
            panic!("record-aware callback should be used");
        }
        fn publish_record(&self, record: &RecordedEvent) {
            let journal = self.0.journal.lock().unwrap();
            assert!(
                journal.records().contains(record),
                "publication preceded authoritative retention"
            );
        }
    }

    #[test]
    fn concurrent_producers_assign_unique_per_execution_identities_before_publication() {
        let events = Arc::new(EventBuffer::default());
        let sink = CheckPublication(events.clone());
        std::thread::scope(|scope| {
            for producer in 0..4 {
                let events = &events;
                let sink = &sink;
                scope.spawn(move || {
                    for _ in 0..32 {
                        emit_event(
                            events,
                            Some(sink),
                            ExecutionEvent::RunStarted {
                                execution_id: ExecutionId(producer % 2),
                            },
                        );
                    }
                });
            }
        });
        drop(sink);
        let records = match Arc::try_unwrap(events) {
            Ok(events) => events.into_records(),
            Err(_) => panic!("collector retained"),
        };
        let mut replay = ReplayJournal::new(std::num::NonZeroUsize::new(128).unwrap());
        for record in records.iter().rev() {
            assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Inserted));
        }
        for execution in [ExecutionId(0), ExecutionId(1)] {
            let sequence: Vec<_> = replay
                .records_for(execution)
                .map(|record| record.identity.event_sequence.0)
                .collect();
            assert_eq!(sequence, (0..64).collect::<Vec<_>>());
        }
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].timestamp.elapsed <= pair[1].timestamp.elapsed)
        );
    }
}
