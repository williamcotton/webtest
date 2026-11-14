//! Explicit actor references share ownership, never clone backend pages. Static
//! resource analysis rejects conflicting access before these leases are reached.
mod journal;
use super::*;
pub(super) use journal::{ActorJournal, Cursor, JournalError};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};
use webtest_browser::Page;
use webtest_model::ActorId;

#[derive(Clone)]
pub(super) struct ActorHandle(
    pub Arc<Mutex<Option<Box<dyn Page>>>>,
    pub Arc<ActorJournal>,
    pub webtest_observation::ScopeEvent,
    pub String,
);
impl ActorHandle {
    pub fn new(
        page: Box<dyn Page>,
        journal: Arc<ActorJournal>,
        scope: webtest_observation::ScopeEvent,
        name: String,
    ) -> Self {
        Self(Arc::new(Mutex::new(Some(page))), journal, scope, name)
    }
}
pub(super) struct ActorOwner(pub ActorHandle);
impl Drop for ActorOwner {
    fn drop(&mut self) {
        self.0.1.close();
        if let Ok(mut page) = self.0.0.try_lock() {
            drop(page.take());
        }
    }
}
struct ActorLease<'a> {
    branch: &'a mut branch::BranchState,
    page: OwnedMutexGuard<Option<Box<dyn Page>>>,
    previous: Option<Box<dyn Page>>,
    previous_actor: Option<ActorId>,
}
impl Drop for ActorLease<'_> {
    fn drop(&mut self) {
        *self.page = self.branch.page.take();
        self.branch.page = self.previous.take();
        self.branch.current_actor = self.previous_actor;
    }
}
impl TreeExecution<'_, '_> {
    pub(super) async fn actor_access(
        &mut self,
        actor: ActorId,
        body: &webtest_plan::PlanNode,
        scope: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        if self.branch.current_actor == Some(actor) {
            return self.node(body, scope).await;
        }
        let Some(handle) = self.branch.actors.get(&actor).cloned() else {
            return actor_internal("actor is outside its owning scope");
        };
        // A conflict in a validated plan is an invariant violation, not a wait
        // whose order would change the meaning of parallel browser mutations.
        let Ok(mut page) = handle.0.try_lock_owned() else {
            return actor_internal("exclusive actor access conflict");
        };
        let Some(owned) = page.take() else {
            return actor_internal("actor generation has closed");
        };
        let previous = self.branch.page.replace(owned);
        let previous_actor = self.branch.current_actor.replace(actor);
        let lease = ActorLease {
            branch: self.branch,
            page,
            previous,
            previous_actor,
        };
        TreeExecution {
            services: self.services,
            branch: lease.branch,
        }
        .node(body, scope)
        .await
    }
}
fn actor_internal(message: &str) -> TestBodyOutcome {
    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
        failure: RunError::Internal(message.into()),
    })
}

impl TreeExecution<'_, '_> {
    pub(super) fn flush_actor(&self, actor: ActorId, handle: &ActorHandle) {
        let (generation, loss, events) = handle.1.unpublished();
        let emit = |event| {
            emit_event(
                self.services.events,
                self.services.event_sink,
                ExecutionEvent::Semantic {
                    execution_id: self.services.execution_id,
                    scope: handle.2.clone(),
                    event,
                },
            )
        };
        if loss > 0 {
            emit(webtest_observation::SemanticEvent::EventLoss {
                actor,
                through: loss,
            });
        }
        let (fields, secrets) = self.branch.bindings.redaction();
        for (sequence, ingress, event) in events {
            let payload = crate::redaction::reactive_payload(
                &event.payload,
                fields,
                secrets,
                &self.services.options.inspection.redacted_query_parameters,
            );
            let truncated = event.truncated
                || webtest_model::value_to_json(&event.payload).is_some_and(|v| {
                    serde_json::to_vec(&v)
                        .map(|v| v.len() > 16 * 1024)
                        .unwrap_or(true)
                });
            emit(webtest_observation::SemanticEvent::BrowserEvent {
                actor,
                generation,
                sequence,
                ingress,
                source: event.source,
                payload: webtest_model::value_to_json(&payload).unwrap_or(serde_json::Value::Null),
                captured_bytes: event.captured_bytes,
                truncated,
            });
        }
    }
}
