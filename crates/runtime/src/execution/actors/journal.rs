use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Notify;
use webtest_browser::{
    BrowserError,
    reactive::{BrowserEvent, ReactiveObserver},
};
use webtest_model::{ActorId, ExecutionScopeId, ReactiveLimits};
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JournalError {
    Overflow,
    Closed(BrowserError),
    WrongGeneration,
}
type JournalEntry = (u64, u64, Arc<BrowserEvent>);
type Publication = (ExecutionScopeId, u64, Vec<JournalEntry>);
struct State {
    sequence: u64,
    published: u64,
    bytes: usize,
    events: VecDeque<JournalEntry>,
    pins: BTreeMap<u64, usize>,
    lost_through: u64,
    closed: Option<BrowserError>,
}
pub(crate) struct ActorJournal {
    actor: ActorId,
    generation: ExecutionScopeId,
    limits: ReactiveLimits,
    ingress: Arc<std::sync::atomic::AtomicU64>,
    state: Mutex<State>,
    pub notify: Notify,
}
#[derive(Clone)]
pub(crate) struct Cursor {
    pub name: String,
    pub attempt: Option<webtest_model::AttemptId>,
    pub actor: ActorId,
    pub generation: ExecutionScopeId,
    pub sequence: u64,
    _pin: Arc<Pin>,
}
struct Pin {
    journal: Weak<ActorJournal>,
    sequence: u64,
}
impl Drop for Pin {
    fn drop(&mut self) {
        if let Some(journal) = self.journal.upgrade() {
            let mut state = journal.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(count) = state.pins.get_mut(&self.sequence) {
                *count -= 1;
                if *count == 0 {
                    state.pins.remove(&self.sequence);
                }
            }
        }
    }
}
impl ActorJournal {
    pub fn new(
        actor: ActorId,
        generation: ExecutionScopeId,
        limits: ReactiveLimits,
        ingress: Arc<std::sync::atomic::AtomicU64>,
    ) -> Arc<Self> {
        Arc::new(Self {
            actor,
            generation,
            limits,
            ingress,
            state: Mutex::new(State {
                sequence: 0,
                published: 0,
                bytes: 0,
                events: VecDeque::new(),
                pins: BTreeMap::new(),
                lost_through: 0,
                closed: None,
            }),
            notify: Notify::new(),
        })
    }
    pub fn unpublished(&self) -> Publication {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let loss = if state.lost_through > state.published {
            state.lost_through
        } else {
            0
        };
        let events = state
            .events
            .iter()
            .filter(|(sequence, _, _)| *sequence > state.published)
            .cloned()
            .collect();
        state.published = state.sequence;
        (self.generation, loss, events)
    }
    pub fn checkpoint(self: &Arc<Self>) -> Result<Cursor, JournalError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = &state.closed {
            return Err(JournalError::Closed(error.clone()));
        }
        let sequence = state.sequence;
        *state.pins.entry(sequence).or_default() += 1;
        Ok(Cursor {
            name: String::new(),
            attempt: None,
            actor: self.actor,
            generation: self.generation,
            sequence,
            _pin: Arc::new(Pin {
                journal: Arc::downgrade(self),
                sequence,
            }),
        })
    }
    pub fn read(
        &self,
        cursor: &Cursor,
        after: u64,
    ) -> Result<Vec<(u64, u64, Arc<BrowserEvent>)>, JournalError> {
        if cursor.actor != self.actor || cursor.generation != self.generation {
            return Err(JournalError::WrongGeneration);
        }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = &state.closed {
            return Err(JournalError::Closed(error.clone()));
        }
        if after < state.lost_through {
            return Err(JournalError::Overflow);
        }
        let events = state
            .events
            .iter()
            .filter(|(sequence, _, _)| *sequence > after)
            .cloned()
            .collect::<Vec<_>>();
        if events.iter().any(|(_, _, event)| event.truncated) {
            return Err(JournalError::Overflow);
        }
        Ok(events)
    }
    pub fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed
            .get_or_insert(BrowserError::BrowserDisconnected);
        self.notify.notify_waiters();
    }
}
impl ReactiveObserver for ActorJournal {
    fn limits(&self) -> ReactiveLimits {
        self.limits
    }
    fn closed(&self, error: BrowserError) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed
            .get_or_insert(error);
        self.notify.notify_waiters();
    }
    fn observe(&self, event: BrowserEvent) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed.is_some() {
            return;
        }
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        if event.captured_bytes > self.limits.max_single_event_bytes {
            state.lost_through = sequence;
            drop(state);
            self.notify.notify_waiters();
            return;
        }
        while state.events.len() >= self.limits.max_events_per_actor
            || state.bytes.saturating_add(event.captured_bytes)
                > self.limits.max_event_bytes_per_actor
        {
            let pinned = state.pins.keys().next().copied();
            if state
                .events
                .front()
                .is_some_and(|(seq, _, _)| pinned.is_some_and(|pin| *seq > pin))
            {
                state.lost_through = sequence;
                drop(state);
                self.notify.notify_waiters();
                return;
            }
            if let Some((lost, _, event)) = state.events.pop_front() {
                state.bytes = state.bytes.saturating_sub(event.captured_bytes);
                state.lost_through = state.lost_through.max(lost);
            } else {
                break;
            }
        }
        state.bytes += event.captured_bytes;
        let ingress = self
            .ingress
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        state.events.push_back((sequence, ingress, Arc::new(event)));
        drop(state);
        self.notify.notify_waiters();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use webtest_model::{EventSource, Value};
    #[test]
    fn pinned_history_overflow_and_generations_are_explicit() {
        let journal = ActorJournal::new(
            ActorId::default(),
            ExecutionScopeId(1),
            ReactiveLimits {
                max_events_per_actor: 1,
                ..Default::default()
            },
            Default::default(),
        );
        let cursor = journal.checkpoint().unwrap();
        for _ in 0..2 {
            journal.observe(BrowserEvent {
                source: EventSource::Navigation,
                payload: Value::Null,
                captured_bytes: 1,
                truncated: false,
            });
        }
        assert_eq!(
            journal.read(&cursor, 0).unwrap_err(),
            JournalError::Overflow
        );
        let other = ActorJournal::new(
            ActorId::default(),
            ExecutionScopeId(2),
            ReactiveLimits::default(),
            Default::default(),
        );
        assert_eq!(
            other.read(&cursor, 0).unwrap_err(),
            JournalError::WrongGeneration
        );
        drop(cursor);
        let current = journal.checkpoint().unwrap();
        journal.close();
        assert_eq!(
            journal.read(&current, current.sequence).unwrap_err(),
            JournalError::Closed(BrowserError::BrowserDisconnected)
        );
    }
}

#[cfg(test)]
mod stress {
    use super::*;
    use webtest_model::{EventSource, Value};
    #[test]
    fn ingress_history_is_bounded_and_consumers_share_immutable_event_storage() {
        let journal = ActorJournal::new(
            ActorId::default(),
            ExecutionScopeId(1),
            ReactiveLimits {
                max_events_per_actor: 32,
                ..Default::default()
            },
            Default::default(),
        );
        let cursor = journal.checkpoint().unwrap();
        journal.observe(BrowserEvent {
            source: EventSource::ConsoleMessage,
            payload: Value::String("hello".into()),
            captured_bytes: 5,
            truncated: false,
        });
        let first = journal.read(&cursor, 0).unwrap();
        let second = journal.read(&cursor, 0).unwrap();
        assert!(Arc::ptr_eq(&first[0].2, &second[0].2));
        for _ in 0..100000 {
            journal.observe(BrowserEvent {
                source: EventSource::ConsoleMessage,
                payload: Value::Null,
                captured_bytes: 4,
                truncated: false,
            });
        }
        let state = journal.state.lock().unwrap();
        assert!(state.events.len() <= 32);
        assert!(state.bytes <= journal.limits.max_event_bytes_per_actor);
        drop(state);
        assert_eq!(
            journal.read(&cursor, 0).unwrap_err(),
            JournalError::Overflow
        );
    }
}
