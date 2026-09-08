//! Bounded optional projections of the authoritative native journal.
use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use webtest_observation::{EventIdentity, RecordedEvent};

/// The stream ended with a gap. Replay/resynchronization is required; subsequent
/// records, including terminal facts, must be obtained from the run journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionGapReason {
    SubscriberCapacity,
    JournalCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SubscriptionOverflow {
    pub reason: SubscriptionGapReason,
    pub capacity: usize,
    pub first_rejected: EventIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubscriptionItem {
    Event(Arc<RecordedEvent>),
    Overflow(SubscriptionOverflow),
}

/// Drains an accepted prefix, then reports overflow once and ends. A healthy
/// subscription stays open across runs until its runner is dropped.
pub struct EventSubscription {
    receiver: mpsc::Receiver<Arc<RecordedEvent>>,
    overflow: Arc<Mutex<Option<SubscriptionOverflow>>>,
    ended: bool,
}

impl EventSubscription {
    pub async fn next(&mut self) -> Option<SubscriptionItem> {
        if self.ended {
            return None;
        }
        if let Some(record) = self.receiver.recv().await {
            return Some(SubscriptionItem::Event(record));
        }
        self.ended = true;
        self.overflow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(SubscriptionItem::Overflow)
    }
}

struct Producer {
    sender: Option<mpsc::Sender<Arc<RecordedEvent>>>,
    overflow: Arc<Mutex<Option<SubscriptionOverflow>>>,
    capacity: usize,
}

/// This lock owns only a bounded subscriber projection, never execution state.
#[derive(Clone)]
pub(crate) struct SubscriptionSender(Arc<Mutex<Producer>>);

impl SubscriptionSender {
    pub(crate) fn channel(capacity: NonZeroUsize) -> (Self, EventSubscription) {
        let (sender, receiver) = mpsc::channel(capacity.get());
        let overflow = Arc::new(Mutex::new(None));
        (
            Self(Arc::new(Mutex::new(Producer {
                sender: Some(sender),
                overflow: overflow.clone(),
                capacity: capacity.get(),
            }))),
            EventSubscription {
                receiver,
                overflow,
                ended: false,
            },
        )
    }

    pub(crate) fn publish(&self, record: Arc<RecordedEvent>) {
        let mut producer = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(sender) = &producer.sender else {
            return;
        };
        match sender.try_send(record) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(record)) => {
                *producer.overflow.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(SubscriptionOverflow {
                        reason: SubscriptionGapReason::SubscriberCapacity,
                        capacity: producer.capacity,
                        first_rejected: record.identity,
                    });
                // Closing the sole sender wakes the receiver after its accepted
                // prefix, without needing room for the overflow marker itself.
                producer.sender = None;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => producer.sender = None,
        }
    }

    pub(crate) fn journal_failed(&self, first_rejected: EventIdentity, capacity: usize) {
        let mut producer = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if producer.sender.is_some() {
            *producer.overflow.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(SubscriptionOverflow {
                    reason: SubscriptionGapReason::JournalCapacity,
                    first_rejected,
                    capacity,
                });
            producer.sender = None;
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sender
            .as_ref()
            .is_none_or(mpsc::Sender::is_closed)
    }
}
