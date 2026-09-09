//! Native event records and replay identity. The caller supplies both clocks;
//! this layer neither reads ambient time nor interprets execution semantics.
use std::{collections::BTreeMap, num::NonZeroUsize, time::Duration};

use crate::{ExecutionEvent, ExecutionId};

pub const EVENT_JOURNAL_SCHEMA_VERSION: u32 = 5;

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

/// Complete native wire envelope. Serde checks its shape; replay ingestion also
/// validates version, identity, source/context agreement, and immutable delivery.
/// Optional source/occurrence fields remain absent when they do not apply.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedEvent {
    pub schema_version: u32,
    #[serde(flatten)]
    pub identity: EventIdentity,
    pub timestamp: EventTime,
    #[serde(flatten)]
    pub metadata: crate::EventMetadata,
    /// Original typed fact, including scope, source and resource identities.
    #[serde(flatten)]
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

/// Bounded wire ingestion errors preserve syntax/data locations and typed replay
/// failures. Rejected input never mutates the replay index.
#[derive(Debug)]
pub enum ReplayDecodeError {
    RecordTooLarge { limit: usize, actual: usize },
    Json(serde_json::Error),
    Replay(ReplayError),
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

    /// Decode one complete JSON record within a caller-selected byte budget.
    /// This is independent of retained event count; framing/total stream budgets
    /// remain the caller's responsibility. Check version before typed payloads so
    /// future event kinds are rejected as an unsupported contract, not guessed.
    pub fn insert_json(
        &mut self,
        input: &[u8],
        max_bytes: NonZeroUsize,
    ) -> Result<ReplayOutcome, ReplayDecodeError> {
        if input.len() > max_bytes.get() {
            return Err(ReplayDecodeError::RecordTooLarge {
                limit: max_bytes.get(),
                actual: input.len(),
            });
        }
        #[derive(serde::Deserialize)]
        struct Header {
            schema_version: u32,
        }
        let header: Header = serde_json::from_slice(input).map_err(ReplayDecodeError::Json)?;
        if header.schema_version != EVENT_JOURNAL_SCHEMA_VERSION {
            return Err(ReplayDecodeError::Replay(ReplayError::UnsupportedSchema {
                found: header.schema_version,
            }));
        }
        let record = serde_json::from_slice(input).map_err(ReplayDecodeError::Json)?;
        self.insert(record).map_err(ReplayDecodeError::Replay)
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

#[cfg(test)]
mod wire_tests {
    use super::*;

    fn record() -> RecordedEvent {
        EventJournal::default()
            .record(
                ExecutionEvent::RunStarted {
                    execution_id: ExecutionId(9),
                },
                EventTime {
                    since_unix_epoch: Duration::from_secs(123),
                    elapsed: Duration::from_nanos(5),
                },
            )
            .clone()
    }

    #[test]
    fn bounded_wire_replay_rejects_corruption_without_mutating_accepted_records() {
        let record = record();
        let value = serde_json::to_value(&record).unwrap();
        let bytes = serde_json::to_vec(&record).unwrap();
        let limit = NonZeroUsize::new(16_384).unwrap();
        let mut replay = ReplayJournal::new(NonZeroUsize::new(1).unwrap());
        replay.insert_json(&bytes, limit).unwrap();
        assert!(
            matches!(replay.insert_json(&bytes, NonZeroUsize::new(bytes.len() - 1).unwrap()), Err(ReplayDecodeError::RecordTooLarge { actual, .. }) if actual == bytes.len())
        );
        for field in [
            "kind",
            "execution_id",
            "payload",
            "timestamp",
            "execution_context",
        ] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                matches!(
                    replay.insert_json(&serde_json::to_vec(&missing).unwrap(), limit),
                    Err(ReplayDecodeError::Json(_))
                ),
                "missing {field}"
            );
        }
        let mut unknown = value.clone();
        unknown["kind"] = "future_kind".into();
        assert!(matches!(
            replay.insert_json(&serde_json::to_vec(&unknown).unwrap(), limit),
            Err(ReplayDecodeError::Json(_))
        ));
        for version in [4, 6] {
            unknown["schema_version"] = version.into();
            assert!(
                matches!(replay.insert_json(&serde_json::to_vec(&unknown).unwrap(), limit), Err(ReplayDecodeError::Replay(ReplayError::UnsupportedSchema { found })) if found == version)
            );
        }
        let mut extra = value.clone();
        extra["unrecognized"] = true.into();
        assert!(matches!(
            replay.insert_json(&serde_json::to_vec(&extra).unwrap(), limit),
            Err(ReplayDecodeError::Json(_))
        ));
        let mut wrong = value.clone();
        wrong["payload"]["execution_id"] = 10.into();
        assert!(matches!(
            replay.insert_json(&serde_json::to_vec(&wrong).unwrap(), limit),
            Err(ReplayDecodeError::Replay(
                ReplayError::ExecutionMismatch { .. }
            ))
        ));
        wrong = value.clone();
        wrong["execution_context"]["test_id"] = 2.into();
        assert!(matches!(
            replay.insert_json(&serde_json::to_vec(&wrong).unwrap(), limit),
            Err(ReplayDecodeError::Replay(
                ReplayError::MetadataMismatch { .. }
            ))
        ));
        wrong = value.clone();
        wrong["timestamp"]["elapsed"]["nanos"] = 7.into();
        assert!(matches!(
            replay.insert_json(&serde_json::to_vec(&wrong).unwrap(), limit),
            Err(ReplayDecodeError::Replay(
                ReplayError::ConflictingIdentity { .. }
            ))
        ));
        wrong = value.clone();
        wrong["event_sequence"] = 1.into();
        assert!(matches!(
            replay.insert_json(&serde_json::to_vec(&wrong).unwrap(), limit),
            Err(ReplayDecodeError::Replay(ReplayError::CapacityExceeded {
                capacity: 1
            }))
        ));
        let duplicate_field = String::from_utf8(bytes.clone()).unwrap().replacen(
            "\"execution_id\":9",
            "\"execution_id\":9,\"execution_id\":9",
            1,
        );
        for invalid in [
            b"".to_vec(),
            b"{".to_vec(),
            vec![0xff],
            [bytes.as_slice(), b" {}"].concat(),
            duplicate_field.into_bytes(),
        ] {
            assert!(matches!(
                replay.insert_json(&invalid, limit),
                Err(ReplayDecodeError::Json(_))
            ));
        }
        assert_eq!(
            replay.records_for(ExecutionId(9)).collect::<Vec<_>>(),
            [&record]
        );
        assert_eq!(
            replay.insert_json(&bytes, limit).unwrap(),
            ReplayOutcome::Duplicate
        );
    }

    #[test]
    fn wire_envelope_is_flat_typed_and_replays_idempotently() {
        let record = record();
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schema_version": 5, "execution_id": 9, "event_sequence": 0,
                "timestamp": {"since_unix_epoch": {"secs":123,"nanos":0}, "elapsed":{"secs":0,"nanos":5}},
                "execution_context": {}, "kind": "run_started", "payload": {"execution_id":9}
            })
        );
        let bytes = serde_json::to_vec(&record).unwrap();
        assert_eq!(
            serde_json::from_slice::<RecordedEvent>(&bytes).unwrap(),
            record
        );
        let mut replay = ReplayJournal::new(NonZeroUsize::new(1).unwrap());
        let limit = NonZeroUsize::new(bytes.len()).unwrap();
        assert_eq!(
            replay.insert_json(&bytes, limit).unwrap(),
            ReplayOutcome::Inserted
        );
        assert_eq!(
            replay.insert_json(&bytes, limit).unwrap(),
            ReplayOutcome::Duplicate
        );
        assert_eq!(
            replay.records_for(ExecutionId(9)).collect::<Vec<_>>(),
            [&record]
        );
    }
}
