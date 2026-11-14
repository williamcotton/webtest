use crate::{
    CancellationReason, CancellationToken, ScopeContext, subscription::SubscriptionSender,
};
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};
use webtest_model::{ExecutionScopeId, StepId, TestId};
use webtest_observation::{EventJournal, EventTime, ExecutionEvent, RecordedEvent};
use webtest_observation::{EventMetadata, ScopeEvent};
use webtest_text::{SourceRevision, SyntaxOrigin};

pub use webtest_observation::JournalOverflow;

/// Trusted synchronous event hook. Implementations must return quickly.
/// For bounded asynchronous consumers use [`crate::Runner::subscribe`].
/// The runtime retains a record before invoking this hook; journal exhaustion
/// stops publication and is exposed through the final [`crate::RunResult`].
pub trait RunEventSink: Send + Sync {
    fn publish(&self, event: &ExecutionEvent);
    /// Receives the identity assigned before publication. Existing sinks may
    /// continue consuming payloads through this default projection.
    fn publish_record(&self, record: &RecordedEvent) {
        self.publish(&record.event);
    }
}

pub(crate) fn emit_event(
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    event: ExecutionEvent,
) {
    let metadata = events.metadata(&event);
    emit_event_with_metadata(metadata, events, sink, event);
}

pub(crate) fn emit_cancellation_request(
    scope: &ScopeEvent,
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    execution_id: webtest_observation::ExecutionId,
    cause: webtest_host::Cancellation,
) {
    emit_event(
        events,
        sink,
        ExecutionEvent::CancellationRequested {
            execution_id,
            scope: scope.clone(),
            cause,
        },
    );
}

pub(crate) fn emit_wait_notification(
    scope: &ScopeEvent,
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    execution_id: webtest_observation::ExecutionId,
    notification: crate::WaitNotification,
) {
    match notification {
        crate::WaitNotification::Wait(event) => emit_event(
            events,
            sink,
            ExecutionEvent::Wait {
                execution_id,
                scope: scope.clone(),
                event,
            },
        ),
        crate::WaitNotification::CancellationRequested(cause) => {
            emit_cancellation_request(scope, events, sink, execution_id, cause)
        }
    }
}

pub(crate) fn emit_event_in_scope(
    scope: &ScopeEvent,
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    event: ExecutionEvent,
) {
    emit_event_with_metadata(EventMetadata::from(scope), events, sink, event);
}

pub(crate) fn emit_event_with_metadata(
    metadata: EventMetadata,
    events: &EventBuffer,
    sink: Option<&dyn RunEventSink>,
    event: ExecutionEvent,
) {
    let mut cancelled_roots = Vec::new();
    let publication = {
        let mut state = events.journal.lock().unwrap_or_else(|e| e.into_inner());
        let terminal = matches!(event, ExecutionEvent::RunFinished { .. });
        if !terminal
            && (state.overflow.is_some()
                || state.journal.records().len() >= events.capacity.get() - 1)
        {
            let identity = state.journal.omit(event.execution_id());
            if let Some(overflow) = &mut state.overflow {
                overflow.last_rejected = identity;
                overflow.rejected_events += 1;
            } else {
                state.overflow = Some(JournalOverflow {
                    capacity: events.capacity.get(),
                    first_rejected: identity,
                    last_rejected: identity,
                    rejected_events: 1,
                });
                for subscriber in &events.subscribers {
                    subscriber.journal_failed(identity, events.capacity.get());
                }
                // Publish and propagate cancellation after releasing the journal lock:
                // request publication itself can encounter the exhausted collector.
                cancelled_roots.extend(state.roots.values().cloned());
            }
            None
        } else {
            let record = state.journal.record_with_metadata(
                event,
                EventTime {
                    since_unix_epoch: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default(),
                    elapsed: events.started.elapsed(),
                },
                metadata,
            );
            if !events.subscribers.is_empty() {
                let record = Arc::new(record.clone());
                for subscriber in &events.subscribers {
                    subscriber.publish(record.clone());
                }
            }
            sink.map(|sink| (sink, record.clone()))
        }
    };
    for root in cancelled_roots {
        cancel_root(&root, events, sink);
    }
    if let Some((sink, record)) = publication {
        sink.publish_record(&record);
    }
}

#[derive(Default)]
struct JournalState {
    journal: EventJournal,
    overflow: Option<JournalOverflow>,
    roots: BTreeMap<ExecutionScopeId, RootCancellation>,
}

/// Only journal retention and cancellation registration are shared. Branches
/// retain their independent execution state and structurally owned resources.
pub(crate) struct EventBuffer {
    journal: Mutex<JournalState>,
    started: tokio::time::Instant,
    capacity: NonZeroUsize,
    subscribers: Vec<SubscriptionSender>,
    source_revision: Option<SourceRevision>,
    source_revisions: std::collections::HashMap<webtest_text::FileId, SourceRevision>,
    tests: BTreeMap<TestId, SyntaxOrigin>,
    test_identities: BTreeMap<
        TestId,
        (
            webtest_model::TestDeclarationId,
            webtest_model::TestVariantId,
        ),
    >,
    steps: BTreeMap<(TestId, StepId), SyntaxOrigin>,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new(
            crate::RunnerOptions::default().journal_max_events,
            Vec::new(),
        )
    }
}

impl EventBuffer {
    pub(crate) fn new(capacity: NonZeroUsize, subscribers: Vec<SubscriptionSender>) -> Self {
        Self {
            journal: Mutex::default(),
            started: tokio::time::Instant::now(),
            capacity,
            subscribers,
            source_revision: None,
            source_revisions: std::collections::HashMap::new(),
            tests: BTreeMap::new(),
            test_identities: BTreeMap::new(),
            steps: BTreeMap::new(),
        }
    }
    pub(crate) fn for_plan(mut self, plan: &webtest_plan::TestPlan) -> Self {
        self.source_revision = Some(plan.source_revision);
        self.source_revisions = plan.sources().into_iter().collect();
        for test in &plan.tests {
            self.tests.insert(test.id, test.origin);
            self.test_identities
                .insert(test.id, (test.test_declaration_id, test.variant_id));
            for step in test.steps() {
                self.steps.insert((test.id, step.id), step.origin);
            }
        }
        self
    }

    fn metadata(&self, event: &ExecutionEvent) -> EventMetadata {
        let mut metadata = event.metadata();
        if event.scope().is_none() {
            metadata.source_revision = self.source_revision;
            metadata.origin = event.test_id().and_then(|test| {
                event
                    .step_id()
                    .and_then(|step| self.steps.get(&(test, step)))
                    .or_else(|| self.tests.get(&test))
                    .copied()
            });
            if let Some(origin) = metadata.origin {
                metadata.source_revision = self.source_revisions.get(&origin.file).copied();
            }
        }
        if let Some((declaration, variant)) = event
            .test_id()
            .and_then(|test| self.test_identities.get(&test))
        {
            metadata.execution_context.test_declaration_id = Some(*declaration);
            metadata.execution_context.variant_id = Some(*variant);
        }
        metadata
    }

    pub(crate) fn overflow(&self) -> Option<JournalOverflow> {
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .overflow
    }
    pub(crate) fn register_root(
        &self,
        context: &ScopeContext,
        scope: &ScopeEvent,
        execution_id: webtest_observation::ExecutionId,
        sink: Option<&dyn RunEventSink>,
    ) -> JournalRoot<'_> {
        let root = RootCancellation {
            token: context.cancellation.clone(),
            scope: scope.clone(),
            execution_id,
        };
        let overflow = {
            let mut state = self.journal.lock().unwrap_or_else(|e| e.into_inner());
            state.roots.insert(context.scope_id, root.clone());
            state.overflow.is_some()
        };
        if overflow {
            cancel_root(&root, self, sink);
        }
        JournalRoot {
            events: self,
            scope_id: context.scope_id,
        }
    }
    pub(crate) fn take_records(&self) -> Vec<RecordedEvent> {
        let mut state = self.journal.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut state.journal).into_records()
    }
    #[cfg(test)]
    pub(crate) fn into_records(self) -> Vec<RecordedEvent> {
        self.take_records()
    }
    #[cfg(test)]
    pub(crate) fn into_events(self) -> Vec<ExecutionEvent> {
        self.into_records()
            .into_iter()
            .map(|record| record.event)
            .collect()
    }
}

#[derive(Clone)]
struct RootCancellation {
    token: CancellationToken,
    scope: ScopeEvent,
    execution_id: webtest_observation::ExecutionId,
}

fn cancel_root(root: &RootCancellation, events: &EventBuffer, sink: Option<&dyn RunEventSink>) {
    root.token.cancel_observed(
        webtest_host::Cancellation {
            reason: CancellationReason::RunnerShutdown,
            causing_scope_id: root.scope.execution_context.scope_id,
        },
        |cause| emit_cancellation_request(&root.scope, events, sink, root.execution_id, cause),
    );
}

pub(crate) struct JournalRoot<'a> {
    events: &'a EventBuffer,
    scope_id: ExecutionScopeId,
}
impl Drop for JournalRoot<'_> {
    fn drop(&mut self) {
        self.events
            .journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .roots
            .remove(&self.scope_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use webtest_observation::{ExecutionId, ReplayJournal, ReplayOutcome};

    #[test]
    fn exhaustion_cancels_registered_and_late_roots_without_replacing_existing_causes() {
        let events = EventBuffer::new(NonZeroUsize::new(1).unwrap(), vec![]);
        let root = |id| ScopeContext {
            scope_id: ExecutionScopeId(id),
            cancellation: CancellationToken::default(),
            deadline: None,
            deadline_scope_id: None,
        };
        let scope = |context: &ScopeContext| ScopeEvent {
            execution_context: webtest_observation::ExecutionContext {
                actor: None,
                fixture: None,
                test_declaration_id: None,
                variant_id: None,
                test_execution_id: Some(webtest_model::TestExecutionId(0)),
                test_id: Some(TestId(0)),
                task_path: vec![],
                scope_id: context.scope_id,
                parent_scope_id: None,
                plan_node_id: webtest_model::PlanNodeId([0; 32]),
                attempt_id: None,
                operation_execution_id: None,
            },
            source_revision: SourceRevision::of("test"),
            origin: SyntaxOrigin::new(
                webtest_text::FileId::new(0),
                webtest_text::TextRange::empty(0.into()),
            ),
            outcome: None,
            cancellation: None,
        };
        let first = root(1);
        let second = root(2);
        let third = root(3);
        let prior = webtest_host::Cancellation {
            reason: CancellationReason::Timeout,
            causing_scope_id: second.scope_id,
        };
        second.cancellation.cancel(prior);
        let first_guard =
            events.register_root(&first, &scope(&first), ExecutionId::from_u128(1), None);
        let second_guard =
            events.register_root(&second, &scope(&second), ExecutionId::from_u128(1), None);
        emit_event(
            &events,
            None,
            ExecutionEvent::RunStarted {
                execution_id: ExecutionId::from_u128(1),
            },
        );
        let third_guard =
            events.register_root(&third, &scope(&third), ExecutionId::from_u128(1), None);
        assert_eq!(
            first.cancellation.cause().unwrap().reason,
            CancellationReason::RunnerShutdown
        );
        assert_eq!(second.cancellation.cause(), Some(prior));
        assert_eq!(
            third.cancellation.cause().unwrap().reason,
            CancellationReason::RunnerShutdown
        );
        let overflow = events.overflow().unwrap();
        assert_eq!(overflow.first_rejected.event_sequence.0, 0);
        assert_eq!(overflow.last_rejected.event_sequence.0, 2);
        assert_eq!(
            overflow.rejected_events, 3,
            "original event plus first and late root requests; the already cancelled root emits no new request"
        );
        drop((first_guard, second_guard, third_guard));
        assert!(events.journal.lock().unwrap().roots.is_empty());
    }

    struct CheckPublication(Arc<EventBuffer>);
    impl RunEventSink for CheckPublication {
        fn publish(&self, _: &ExecutionEvent) {
            panic!("record-aware callback should be used");
        }
        fn publish_record(&self, record: &RecordedEvent) {
            let journal = self.0.journal.lock().unwrap();
            assert!(
                journal.journal.records().contains(record),
                "publication preceded authoritative retention"
            );
        }
    }

    #[tokio::test]
    async fn concurrent_producers_assign_unique_per_execution_identities_before_publication() {
        let (sender, mut subscription) =
            SubscriptionSender::channel(NonZeroUsize::new(128).unwrap());
        let events = Arc::new(EventBuffer::new(
            NonZeroUsize::new(256).unwrap(),
            vec![sender],
        ));
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
                                execution_id: ExecutionId::from_u128((producer % 2) as u128),
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
        for record in &records {
            assert_eq!(
                subscription.next().await,
                Some(crate::SubscriptionItem::Event(Arc::new(record.clone())))
            );
        }
        assert!(subscription.next().await.is_none());
        let mut replay = ReplayJournal::new(std::num::NonZeroUsize::new(128).unwrap());
        for record in records.iter().rev() {
            assert_eq!(replay.insert(record.clone()), Ok(ReplayOutcome::Inserted));
        }
        for execution in [ExecutionId::from_u128(0), ExecutionId::from_u128(1)] {
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
