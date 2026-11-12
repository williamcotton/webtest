//! Invocation-owned host resources use the same registry and bounded native journal
//! as test resources, without inventing a source or test scope.
use crate::{
    ExecutionIdentityError, ExecutionIdentitySource, ResourceRegistry, RunOutcome, RunResult,
    events::{EventBuffer, emit_event},
};
use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::Instant,
};
use webtest_host::{ResourceKind, ResourceObservation, ResourceObserver};
use webtest_observation::{ExecutionEvent, ResourceAccess, ResourceEvent, ResourceKey};

pub struct HostResourceJournal {
    execution_id: webtest_observation::ExecutionId,
    registry: ResourceRegistry,
    events: EventBuffer,
    started: Instant,
    finished: Mutex<Option<RunResult>>,
    cleanup_failed: std::sync::atomic::AtomicBool,
}

impl HostResourceJournal {
    pub fn new(capacity: NonZeroUsize) -> Result<Arc<Self>, ExecutionIdentityError> {
        let journal = Arc::new(Self {
            execution_id: crate::identity::OsExecutionIdentitySource.allocate()?,
            registry: ResourceRegistry::default(),
            events: EventBuffer::new(capacity, Vec::new()),
            started: Instant::now(),
            finished: Mutex::new(None),
            cleanup_failed: std::sync::atomic::AtomicBool::new(false),
        });
        emit_event(
            &journal.events,
            None,
            ExecutionEvent::RunStarted {
                execution_id: journal.execution_id,
            },
        );
        Ok(journal)
    }

    pub fn overflow(&self) -> Option<crate::JournalOverflow> {
        self.events.overflow()
    }

    pub fn finish(&self, outcome: RunOutcome) -> RunResult {
        let mut finished = self.finished.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(result) = &*finished {
            return result.clone();
        }
        let failure = if let Err(error) = self.registry.validate_finished() {
            Some(crate::RunError::Internal(format!(
                "host resource ownership invariant: {error:?}"
            )))
        } else if self
            .cleanup_failed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Some(crate::RunError::Provider(
                webtest_provider::ProviderError::ProcessCleanup { primary: None },
            ))
        } else {
            None
        };
        let outcome = match (outcome, failure) {
            (
                RunOutcome::Aborted {
                    failure: primary,
                    prior_outcome,
                },
                Some(error),
            ) => RunOutcome::Aborted {
                failure: primary.combine_with_error(error),
                prior_outcome,
            },
            (_, Some(failure)) => RunOutcome::Aborted {
                failure,
                prior_outcome: None,
            },
            (outcome, None) => outcome,
        };
        let result = crate::runner::finish_run(
            self.execution_id,
            outcome,
            Vec::new(),
            &self.events,
            self.started,
            None,
            &[],
        );
        *finished = Some(result.clone());
        result
    }

    pub fn observer(self: &Arc<Self>) -> Arc<dyn ResourceObserver> {
        Arc::new(JournalObserver(self.clone()))
    }

    fn emit(&self, event: ResourceEvent) {
        emit_event(
            &self.events,
            None,
            ExecutionEvent::Resource {
                execution_id: self.execution_id,
                scope: None,
                event,
            },
        );
    }
}

struct JournalObserver(Arc<HostResourceJournal>);

impl ResourceObserver for JournalObserver {
    fn acquired(&self, kind: ResourceKind) -> Box<dyn ResourceObservation> {
        let event = self
            .0
            .registry
            .begin_acquisition(None, kind, ResourceAccess::Exclusive);
        let key = event.resource.key;
        self.0.emit(event);
        // A host calls this synchronously after acquiring physical ownership.
        let _ = self.0.registry.acknowledge_ownership(key);
        Box::new(ObservedResource {
            journal: self.0.clone(),
            key,
            ready: false,
            releasing: false,
            released: false,
        })
    }
}

struct ObservedResource {
    journal: Arc<HostResourceJournal>,
    key: ResourceKey,
    ready: bool,
    releasing: bool,
    released: bool,
}

impl ResourceObservation for ObservedResource {
    fn ready(&mut self) {
        if !self.ready
            && !self.releasing
            && let Ok(event) = self.journal.registry.ready(self.key)
        {
            self.ready = true;
            self.journal.emit(event);
        }
    }
    fn releasing(&mut self) {
        if self.releasing || self.released {
            return;
        }
        if !self.ready
            && let Ok(event) = self.journal.registry.acquire_failed(self.key)
        {
            self.journal.emit(event);
        }
        if let Ok(event) = self.journal.registry.begin_release(self.key) {
            self.releasing = true;
            self.journal.emit(event);
        }
    }
    fn released(&mut self, succeeded: bool) {
        if self.released {
            return;
        }
        self.releasing();
        if !succeeded {
            self.journal
                .cleanup_failed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if let Ok(event) = self.journal.registry.finish_release(self.key, succeeded) {
            self.released = true;
            self.journal.emit(event);
        }
    }
}

impl Drop for ObservedResource {
    fn drop(&mut self) {
        // Drop cannot establish successful async teardown.
        if !self.released {
            self.released(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_observation::{ReplayJournal, ResourceEventKind, TeardownState};

    #[test]
    fn invocation_resources_have_terminal_native_facts_without_fabricated_source_scopes() {
        let journal = HostResourceJournal::new(NonZeroUsize::new(100).unwrap()).unwrap();
        let observer = journal.observer();
        for kind in [
            ResourceKind::ApplicationProcess,
            ResourceKind::BridgeEndpoint,
        ] {
            let mut resource = observer.acquired(kind);
            resource.ready();
            resource.releasing();
            resource.released(true);
        }
        let result = journal.finish(RunOutcome::Completed);
        let mut replay = ReplayJournal::new(NonZeroUsize::new(100).unwrap());
        for record in &result.journal {
            assert!(record.metadata.source_revision.is_none());
            assert!(record.metadata.execution_context.scope_id.is_none());
            replay.insert(record.clone()).unwrap();
        }
        let resources: Vec<_> = result
            .events
            .iter()
            .filter_map(|event| match event {
                ExecutionEvent::Resource { event, scope, .. } => {
                    assert!(scope.is_none());
                    Some(event)
                }
                _ => None,
            })
            .collect();
        assert_eq!(resources.len(), 8);
        assert_eq!(
            resources
                .iter()
                .filter(|event| event.kind == ResourceEventKind::Released
                    && event.resource.teardown_state == TeardownState::Released)
                .count(),
            2
        );
        assert_eq!(
            journal.finish(RunOutcome::Completed).journal,
            result.journal
        );
    }

    #[test]
    fn abandoned_startup_is_failed_acquisition_with_failed_terminal_cleanup() {
        let journal = HostResourceJournal::new(NonZeroUsize::new(100).unwrap()).unwrap();
        drop(
            journal
                .observer()
                .acquired(ResourceKind::ApplicationProcess),
        );
        let result = journal.finish(RunOutcome::Completed);
        let kinds: Vec<_> = result
            .events
            .iter()
            .filter_map(|event| match event {
                ExecutionEvent::Resource { event, .. } => Some(event.kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ResourceEventKind::AcquireStarted,
                ResourceEventKind::AcquireFailed,
                ResourceEventKind::ReleaseStarted,
                ResourceEventKind::ReleaseFailed
            ]
        );
    }

    #[test]
    fn invocation_overflow_retains_reserved_typed_terminal_and_gap() {
        let journal = HostResourceJournal::new(NonZeroUsize::new(3).unwrap()).unwrap();
        let mut resource = journal
            .observer()
            .acquired(ResourceKind::ApplicationProcess);
        resource.ready();
        resource.releasing();
        resource.released(true);
        let result = journal.finish(RunOutcome::Completed);
        assert_eq!(result.journal.len(), 3);
        assert!(matches!(
            result.outcome,
            RunOutcome::Aborted {
                failure: crate::RunError::JournalOverflow(_),
                ..
            }
        ));
        assert!(matches!(
            result.events.last(),
            Some(ExecutionEvent::RunFinished {
                failure: Some(_),
                ..
            })
        ));
    }
}
