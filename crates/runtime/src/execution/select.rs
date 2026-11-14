use super::actors::{ActorJournal, Cursor, JournalError};
use super::*;
use std::{collections::HashMap, sync::Arc};
use webtest_model::{ActorId, BindingId, Value};
use webtest_plan::{PlanNode, semantics::SelectAlternative};
pub(super) enum Decision {
    Selected {
        alternative: usize,
        payload: Value,
        sequence: u64,
        ingress: u64,
    },
    Timeout,
}
pub(super) enum SelectionError {
    Journal(JournalError),
    Predicate(crate::StepError),
}
#[derive(Default)]
pub(super) struct SelectionEvidence {
    pub limit: usize,
    observed: u64,
    unmatched: u64,
    sequences: Vec<u64>,
}
impl SelectionEvidence {
    fn reject(&mut self, sequence: u64) {
        self.unmatched = self.unmatched.saturating_add(1);
        if self.sequences.len() < self.limit {
            self.sequences.push(sequence);
        }
    }
}
pub(super) struct SelectionWait<'a> {
    pub(super) journal: Arc<ActorJournal>,
    pub(super) cursor: Cursor,
    pub(super) alternatives: &'a [SelectAlternative],
    pub(super) environment: Arc<HashMap<BindingId, Value>>,
    pub(super) duration: Duration,
    pub(super) evidence: SelectionEvidence,
}
#[async_trait::async_trait]
impl crate::WaitSource for SelectionWait<'_> {
    type Output = Decision;
    type Error = SelectionError;
    async fn ready(&mut self) -> Result<Decision, SelectionError> {
        let observation = async {
            let mut after = self.cursor.sequence;
            loop {
                let notification = self.journal.notify.notified();
                tokio::pin!(notification);
                notification.as_mut().enable();
                let events = self
                    .journal
                    .read(&self.cursor, after)
                    .map_err(SelectionError::Journal)?;
                for (ordinal, (sequence, ingress, event)) in events.into_iter().enumerate() {
                    if ordinal > 0 && ordinal % 64 == 0 {
                        tokio::task::yield_now().await;
                    }
                    after = sequence;
                    self.evidence.observed = self.evidence.observed.saturating_add(1);
                    let mut relevant = false;
                    for (index, alternative) in self.alternatives.iter().enumerate() {
                        if !event.matches(alternative.source) {
                            continue;
                        }
                        relevant = true;
                        let environment = EventBindings {
                            parent: self.environment.as_ref(),
                            binding: alternative.binding,
                            value: &event.payload,
                        };
                        let matches = match &alternative.filter {
                            Some(filter) => {
                                match webtest_plan::evaluation::evaluate(filter, &environment)
                                    .map_err(crate::StepError::from)
                                    .map_err(SelectionError::Predicate)?
                                {
                                    Value::Bool(value) => value,
                                    _ => {
                                        return Err(SelectionError::Predicate(
                                            crate::StepError::Internal(
                                                "select filter is not Bool".into(),
                                            ),
                                        ));
                                    }
                                }
                            }
                            None => true,
                        };
                        if matches {
                            return Ok(Decision::Selected {
                                alternative: index,
                                payload: event.payload.clone(),
                                sequence,
                                ingress,
                            });
                        }
                    }
                    if relevant {
                        self.evidence.reject(sequence);
                    }
                }
                notification.await;
            }
        };
        match tokio::time::timeout(self.duration, observation).await {
            Ok(result) => result,
            Err(_) => Ok(Decision::Timeout),
        }
    }
    async fn interrupt(&mut self, _: webtest_host::Cancellation) -> Result<(), SelectionError> {
        Ok(())
    }
    async fn cleanup(&mut self) -> Result<(), SelectionError> {
        Ok(())
    }
}
impl TreeExecution<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn select_node(
        &mut self,
        actor: ActorId,
        cursor: Option<BindingId>,
        duration: Duration,
        alternatives: &[SelectAlternative],
        timeout: Option<usize>,
        failures: [usize; 4],
        result: Option<&webtest_plan::RaceBinding>,
        children: &[PlanNode],
        scope: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let Some(handle) = self.branch.actors.get(&actor).cloned() else {
            return selection_internal("select actor is outside its owning scope");
        };
        let explicit_cursor = cursor.is_some();
        let cursor = match cursor {
            Some(binding) => self
                .branch
                .cursors
                .get(&binding)
                .cloned()
                .ok_or(JournalError::WrongGeneration),
            None => handle.1.checkpoint(),
        };
        let cursor = match cursor {
            Ok(cursor) => cursor,
            Err(error) => {
                let (index, error) = match error {
                    JournalError::Closed(error) => (3, Some(crate::StepError::Browser(error))),
                    JournalError::Overflow => (1, None),
                    JournalError::WrongGeneration => (2, None),
                };
                return self
                    .selection_failure(children, failures[index], error, scope)
                    .await;
            }
        };
        if explicit_cursor && cursor.attempt != scope.event.execution_context.attempt_id {
            return self
                .selection_failure(children, failures[2], None, scope)
                .await;
        }
        super::semantics::emit_semantic(
            self.services,
            scope,
            webtest_observation::SemanticEvent::SelectionStarted {
                actor,
                since: cursor.sequence,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
                alternatives: alternatives.iter().map(|a| a.id).collect(),
            },
        );
        let mut source = SelectionWait {
            journal: handle.1.clone(),
            cursor,
            alternatives,
            environment: Arc::new(self.branch.bindings.environment().clone()),
            duration,
            evidence: SelectionEvidence {
                limit: self.services.options.reactive.max_unmatched_evidence,
                ..Default::default()
            },
        };
        let output = self
            .services
            .waits
            .wait_observed(
                &scope.context,
                &mut source,
                scope.cleanup_timeout(self.services.options.cleanup_timeout),
                |event| {
                    crate::events::emit_wait_notification(
                        &scope.event,
                        self.services.events,
                        self.services.event_sink,
                        self.services.execution_id,
                        event,
                    )
                },
            )
            .await;
        self.flush_actor(actor, &handle);
        super::semantics::emit_semantic(
            self.services,
            scope,
            webtest_observation::SemanticEvent::SelectionObserved {
                actor,
                observed: source.evidence.observed,
                unmatched: source.evidence.unmatched,
                truncated: source.evidence.unmatched > source.evidence.sequences.len() as u64,
                unmatched_sequences: std::mem::take(&mut source.evidence.sequences),
            },
        );
        let (index, event) = match output.primary {
            crate::WaitCompletion::Ready(Decision::Selected {
                alternative,
                payload,
                sequence,
                ingress,
            }) => {
                self.flush_actor(actor, &handle);
                super::semantics::emit_semantic(
                    self.services,
                    scope,
                    webtest_observation::SemanticEvent::SelectionMatched {
                        actor,
                        sequence,
                        ingress,
                        alternative: alternatives[alternative].id,
                        source: alternatives[alternative].source,
                    },
                );
                (alternative, Some(payload))
            }
            crate::WaitCompletion::Ready(Decision::Timeout) => {
                if let Some(index) = timeout {
                    (index, None)
                } else {
                    return self
                        .selection_failure(children, failures[0], None, scope)
                        .await;
                }
            }
            crate::WaitCompletion::Failed(SelectionError::Journal(error)) => {
                let (index, error) = match error {
                    JournalError::Overflow => (1, None),
                    JournalError::WrongGeneration => (2, None),
                    JournalError::Closed(error) => (3, Some(crate::StepError::Browser(error))),
                };
                return self
                    .selection_failure(children, failures[index], error, scope)
                    .await;
            }
            crate::WaitCompletion::Failed(SelectionError::Predicate(error)) => {
                return self
                    .selection_failure(children, failures[0], Some(error), scope)
                    .await;
            }
            crate::WaitCompletion::Cancelled(cause) => {
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                    reason: cause.reason,
                });
            }
            crate::WaitCompletion::Rejected(_) => {
                return selection_internal("select wait registration rejected");
            }
        };
        let Some(child) = children.get(index) else {
            return selection_internal("select alternative has no executable child");
        };
        let visible = self.branch.bindings.binding_checkpoint();
        if let Some(event) = event {
            let alternative = &alternatives[index];
            self.branch
                .bindings
                .bind(alternative.binding, Some(&alternative.name), event);
        }
        let outcome = self.node(child, scope).await;
        let provided = self.branch.provided.take();
        self.branch.bindings.restore_bindings(&visible);
        if matches!(
            outcome,
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
        ) && let Some(binding) = result
        {
            let Some(value) = provided else {
                return selection_internal("successful select did not provide its result");
            };
            self.branch
                .bindings
                .bind(binding.id, Some(&binding.name), value);
        }
        outcome
    }
    pub(super) async fn selection_failure(
        &mut self,
        children: &[PlanNode],
        index: usize,
        error: Option<crate::StepError>,
        scope: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let Some(child) = children.get(index) else {
            return selection_internal("select failure operation is absent");
        };
        if let Some(error) = error
            && let webtest_plan::PlanNodeKind::Operation { step } = &child.kind
        {
            return TestBodyOutcome::PendingFailure(Box::new(PendingFailure::primary(
                step,
                &scope.event,
                error,
                0,
            )));
        }
        self.node(child, scope).await
    }
}
fn selection_internal(message: &str) -> TestBodyOutcome {
    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
        failure: RunError::Internal(message.into()),
    })
}

struct EventBindings<'a> {
    parent: &'a HashMap<BindingId, Value>,
    binding: BindingId,
    value: &'a Value,
}
impl webtest_plan::evaluation::EvaluationBindings for EventBindings<'_> {
    fn get(&self, binding: &BindingId) -> Option<&Value> {
        if *binding == self.binding {
            Some(self.value)
        } else {
            self.parent.get(binding)
        }
    }
}
