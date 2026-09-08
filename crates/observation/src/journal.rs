//! Native event records and replay identity. The caller supplies both clocks;
//! this layer neither reads ambient time nor interprets execution semantics.
use std::{collections::BTreeMap, num::NonZeroUsize, time::Duration};

use crate::{ExecutionEvent, ExecutionId};

pub const EVENT_JOURNAL_SCHEMA_VERSION: u32 = 2;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct EventSequence(pub u64);

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct EventIdentity {
    pub execution_id: ExecutionId,
    pub event_sequence: EventSequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EventTime {
    /// Wall-clock timestamp; clock adjustment may make this go backwards.
    pub since_unix_epoch: Duration,
    /// Monotonic elapsed time from this collector's start, distinct from order.
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedEvent {
    pub schema_version: u32,
    pub identity: EventIdentity,
    pub timestamp: EventTime,
    pub metadata: crate::EventMetadata,
    /// Original typed fact, including scope, source and resource identities.
    pub event: ExecutionEvent,
}

/// A central native collector. Each execution has its own zero-based sequence;
/// arrival order across executions has no semantic ordering guarantee.
/// Retention/backpressure policy is deliberately separate from record identity.
#[derive(Default)]
pub struct EventJournal {
    records: Vec<RecordedEvent>,
    sequences: BTreeMap<ExecutionId, u64>,
}

impl EventJournal {
    pub fn record(&mut self, event: ExecutionEvent, timestamp: EventTime) -> &RecordedEvent {
        let metadata = event.metadata();
        self.record_with_metadata(event, timestamp, metadata)
    }

    pub fn record_with_metadata(
        &mut self,
        event: ExecutionEvent,
        timestamp: EventTime,
        metadata: crate::EventMetadata,
    ) -> &RecordedEvent {
        let identity = self.omit(event.execution_id());
        let index = self.records.len();
        self.records.push(RecordedEvent {
            schema_version: EVENT_JOURNAL_SCHEMA_VERSION,
            identity,
            timestamp,
            metadata,
            event,
        });
        &self.records[index]
    }

    /// Allocates an identity without retaining a payload. A bounded collector
    /// must separately expose the resulting gap as an explicit failure.
    pub fn omit(&mut self, execution_id: ExecutionId) -> EventIdentity {
        let next = self.sequences.entry(execution_id).or_default();
        let identity = EventIdentity {
            execution_id,
            event_sequence: EventSequence(*next),
        };
        *next += 1;
        identity
    }

    pub fn records(&self) -> &[RecordedEvent] {
        &self.records
    }

    pub fn into_records(self) -> Vec<RecordedEvent> {
        self.records
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayOutcome {
    Inserted,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    UnsupportedSchema {
        found: u32,
    },
    ExecutionMismatch {
        identity: EventIdentity,
        payload_execution: ExecutionId,
    },
    ConflictingIdentity {
        identity: EventIdentity,
    },
    CapacityExceeded {
        capacity: usize,
    },
    MetadataMismatch {
        identity: EventIdentity,
    },
}

/// Bounded replay index. Out-of-order delivery is legal, including gaps in a
/// subscriber projection. Exact duplicates never consume additional capacity.
pub struct ReplayJournal {
    capacity: NonZeroUsize,
    records: BTreeMap<EventIdentity, RecordedEvent>,
}

impl ReplayJournal {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            records: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, record: RecordedEvent) -> Result<ReplayOutcome, ReplayError> {
        if record.schema_version != EVENT_JOURNAL_SCHEMA_VERSION {
            return Err(ReplayError::UnsupportedSchema {
                found: record.schema_version,
            });
        }
        if record.identity.execution_id != record.event.execution_id() {
            return Err(ReplayError::ExecutionMismatch {
                identity: record.identity,
                payload_execution: record.event.execution_id(),
            });
        }
        if !record.metadata.agrees_with(&record.event) {
            return Err(ReplayError::MetadataMismatch {
                identity: record.identity,
            });
        }
        if let Some(existing) = self.records.get(&record.identity) {
            return if existing == &record {
                Ok(ReplayOutcome::Duplicate)
            } else {
                Err(ReplayError::ConflictingIdentity {
                    identity: record.identity,
                })
            };
        }
        if self.records.len() >= self.capacity.get() {
            return Err(ReplayError::CapacityExceeded {
                capacity: self.capacity.get(),
            });
        }
        self.records.insert(record.identity, record);
        Ok(ReplayOutcome::Inserted)
    }

    pub fn records_for(&self, execution_id: ExecutionId) -> impl Iterator<Item = &RecordedEvent> {
        self.records
            .range(
                EventIdentity {
                    execution_id,
                    event_sequence: EventSequence(0),
                }..=EventIdentity {
                    execution_id,
                    event_sequence: EventSequence(u64::MAX),
                },
            )
            .map(|(_, event)| event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(wall: u64, elapsed: u64) -> EventTime {
        EventTime {
            since_unix_epoch: Duration::from_secs(wall),
            elapsed: Duration::from_secs(elapsed),
        }
    }

    #[test]
    fn execution_sequences_are_independent_of_wall_time_and_delivery_order() {
        let mut source = EventJournal::default();
        source.record(
            ExecutionEvent::RunStarted {
                execution_id: ExecutionId(9),
            },
            time(100, 0),
        );
        source.record(
            ExecutionEvent::RunStarted {
                execution_id: ExecutionId(3),
            },
            time(90, 1),
        );
        source.record(
            ExecutionEvent::RunFinished {
                execution_id: ExecutionId(9),
                outcome: crate::RunOutcomeKind::Completed,
                failure_class: None,
            },
            time(80, 2),
        );
        let records = source.into_records();
        assert_eq!(
            records
                .iter()
                .map(|record| record.identity.event_sequence.0)
                .collect::<Vec<_>>(),
            [0, 0, 1]
        );
        let mut replay = ReplayJournal::new(NonZeroUsize::new(3).unwrap());
        for record in records.iter().rev() {
            assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Inserted));
        }
        assert_eq!(
            replay
                .records_for(ExecutionId(9))
                .cloned()
                .collect::<Vec<_>>(),
            [records[0].clone(), records[2].clone()]
        );
        assert_eq!(replay.records_for(ExecutionId(3)).count(), 1);
        for record in &records {
            assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Duplicate));
        }
    }

    #[test]
    fn replay_rejects_conflicts_and_overflow_without_changing_accepted_facts() {
        let mut source = EventJournal::default();
        let record = source
            .record(
                ExecutionEvent::RunStarted {
                    execution_id: ExecutionId(1),
                },
                time(10, 0),
            )
            .clone();
        let mut replay = ReplayJournal::new(NonZeroUsize::new(1).unwrap());
        assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Inserted));
        let mut changed = record.clone();
        changed.timestamp = time(11, 0);
        assert!(matches!(
            replay.insert(changed),
            Err(ReplayError::ConflictingIdentity { .. })
        ));
        let mut changed = record.clone();
        changed.event = ExecutionEvent::RunFinished {
            execution_id: ExecutionId(1),
            outcome: crate::RunOutcomeKind::Completed,
            failure_class: None,
        };
        assert!(matches!(
            replay.insert(changed),
            Err(ReplayError::ConflictingIdentity { .. })
        ));
        let mut changed = record.clone();
        changed.identity.execution_id = ExecutionId(2);
        assert!(matches!(
            replay.insert(changed),
            Err(ReplayError::ExecutionMismatch { .. })
        ));
        let mut changed = record.clone();
        changed.schema_version += 1;
        assert!(matches!(
            replay.insert(changed),
            Err(ReplayError::UnsupportedSchema { .. })
        ));
        let next = source
            .record(
                ExecutionEvent::RunStarted {
                    execution_id: ExecutionId(2),
                },
                time(12, 2),
            )
            .clone();
        assert_eq!(
            replay.insert(next),
            Err(ReplayError::CapacityExceeded { capacity: 1 })
        );
        assert_eq!(
            replay
                .records_for(ExecutionId(1))
                .cloned()
                .collect::<Vec<_>>(),
            [record]
        );
        assert_eq!(replay.records_for(ExecutionId(2)).count(), 0);
    }
}
