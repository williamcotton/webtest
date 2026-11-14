use super::select::{Decision, SelectionError, SelectionWait};
use super::*;
use futures::{StreamExt, stream::FuturesUnordered};
use webtest_plan::{PlanNode, semantics::GuardClause};
impl TreeExecution<'_, '_> {
    pub(super) async fn guard_node(
        &mut self,
        clauses: &[GuardClause],
        failures: [usize; 3],
        children: &[PlanNode],
        scope: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let Some(body) = children.first() else {
            return guard_internal("guard scope has no body");
        };
        let services = self.services;
        let watcher_context = scope.context.child(scope.id(), None);
        let mut watchers = FuturesUnordered::new();
        let mut observed = Vec::new();
        let environment = std::sync::Arc::new(self.branch.bindings.environment().clone());
        // Cursor installation is synchronous and precedes polling any body work.
        let mut sources = Vec::new();
        for clause in clauses {
            let Some(handle) = self.branch.actors.get(&clause.actor) else {
                return guard_internal("guard actor is outside its scope");
            };
            let Ok(cursor) = handle.1.checkpoint() else {
                return self
                    .selection_failure(children, failures[1], None, scope)
                    .await;
            };
            super::semantics::emit_semantic(
                services,
                scope,
                webtest_observation::SemanticEvent::GuardActivated {
                    guard: clause.guard,
                    actor: clause.actor,
                    alternative: clause.alternative.id,
                    since: cursor.sequence,
                },
            );
            sources.push(SelectionWait {
                journal: handle.1.clone(),
                cursor,
                alternatives: std::slice::from_ref(&clause.alternative),
                environment: environment.clone(),
                duration: webtest_plan::MAX_CONTROL_TIMEOUT,
                evidence: Default::default(),
            });
        }
        for (ordinal, mut source) in sources.into_iter().enumerate() {
            let context = watcher_context.clone();
            watchers.push(async move {
                let result = services
                    .waits
                    .wait_observed(
                        &context,
                        &mut source,
                        scope.cleanup_timeout(services.options.cleanup_timeout),
                        |event| {
                            crate::events::emit_wait_notification(
                                &scope.event,
                                services.events,
                                services.event_sink,
                                services.execution_id,
                                event,
                            )
                        },
                    )
                    .await;
                (ordinal, result.primary)
            });
        }
        let visible = self.branch.bindings.binding_checkpoint();
        let mut parent = scope.clone();
        parent.context = scope.context.child(scope.id(), None);
        let body_context = parent.context.clone();
        let (body_result, mut trigger) = {
            let body = self.node(body, &parent);
            tokio::pin!(body);
            tokio::select! {biased;
                _=scope.context.cancellation.cancelled()=>{body_context.cancellation.cancel(webtest_host::Cancellation{reason:scope.context.cancellation.cause().map(|c|c.reason).unwrap_or(CancellationReason::ParentFailed),causing_scope_id:scope.id()});(body.await,None)},
                trigger=watchers.next(),if !watchers.is_empty()=>{let trigger=earliest(trigger,&mut watchers,&mut observed);body_context.cancellation.cancel_observed(webtest_host::Cancellation{reason:CancellationReason::GuardFailed,causing_scope_id:scope.id()},|cause|crate::events::emit_cancellation_request(&scope.event,services.events,services.event_sink,services.execution_id,cause));(body.await,trigger)},
                result=&mut body=>(result,None),
            }
        };
        // A ready body can publish a forbidden event in its final poll. Drain
        // ingress through a backend barrier before declaring guards inactive.
        let mut barrier_error = None;
        if trigger.is_none()
            && scope.context.cancellation.cause().is_none()
            && !body_result
                .failure_class()
                .is_some_and(|class| class != FailureClass::Test)
        {
            for clause in clauses {
                if self.branch.current_actor == Some(clause.actor) {
                    if let Some(page) = self.branch.page.as_mut()
                        && let Err(error) = page.reactive_barrier().await
                    {
                        barrier_error = Some((clause.actor, error));
                        break;
                    }
                } else if let Some(handle) = self.branch.actors.get(&clause.actor) {
                    let mut page = handle.0.lock().await;
                    if let Some(page) = page.as_mut()
                        && let Err(error) = page.reactive_barrier().await
                    {
                        barrier_error = Some((clause.actor, error));
                        break;
                    }
                }
            }
            trigger = match futures::poll!(watchers.next()) {
                std::task::Poll::Ready(value) => value,
                std::task::Poll::Pending => None,
            };
        }
        trigger = earliest(trigger, &mut watchers, &mut observed);
        for (ordinal, sequence, ingress) in observed.iter().take(32) {
            let clause = &clauses[*ordinal];
            let primary = trigger.as_ref().is_some_and(|(index, _)| index == ordinal);
            super::semantics::emit_semantic(
                services,
                scope,
                webtest_observation::SemanticEvent::GuardTriggered {
                    guard: clause.guard,
                    actor: clause.actor,
                    alternative: clause.alternative.id,
                    sequence: *sequence,
                    ingress: *ingress,
                    primary,
                },
            );
        }
        if observed.len() > 32 {
            super::semantics::emit_semantic(
                services,
                scope,
                webtest_observation::SemanticEvent::GuardSecondaryTruncated {
                    omitted: observed.len() - 32,
                },
            );
        }
        watcher_context
            .cancellation
            .cancel(webtest_host::Cancellation {
                reason: CancellationReason::RunnerShutdown,
                causing_scope_id: scope.id(),
            });
        while watchers.next().await.is_some() {}
        for clause in clauses {
            if let Some(handle) = self.branch.actors.get(&clause.actor) {
                self.flush_actor(clause.actor, handle);
            }
            super::semantics::emit_semantic(
                services,
                scope,
                webtest_observation::SemanticEvent::GuardDeactivated {
                    guard: clause.guard,
                    actor: clause.actor,
                    alternative: clause.alternative.id,
                },
            );
        }
        let owned = self.branch.scopes.subtree_ids(scope);
        self.branch.cleanup_failures.extend(
            self.branch
                .temporary
                .release(
                    Some(&owned),
                    &mut self.branch.bindings,
                    crate::cleanup::CleanupDeadline::new(
                        scope.cleanup_timeout(services.options.cleanup_timeout),
                    ),
                    &temporary::ResourceEvents {
                        registry: services.resources,
                        execution_id: services.execution_id,
                        events: services.events,
                        sink: services.event_sink,
                    },
                )
                .await,
        );
        self.branch.bindings.restore_bindings(&visible);
        if let Some((actor, error)) = barrier_error {
            let mut actor_scope = scope.clone();
            actor_scope.event.execution_context.actor = Some(actor);
            return self
                .selection_failure(
                    children,
                    failures[2],
                    Some(crate::StepError::Browser(error)),
                    &actor_scope,
                )
                .await;
        }
        let Some((ordinal, trigger)) = trigger else {
            return body_result;
        };
        // Cleanup/infrastructure evidence from the body retains precedence.
        if body_result
            .failure_class()
            .is_some_and(|class| class != FailureClass::Test)
        {
            return body_result;
        }
        match trigger {
            crate::WaitCompletion::Ready(Decision::Selected { payload, .. }) => {
                let clause = &clauses[ordinal];
                let Some(child) = children.get(clause.child) else {
                    return guard_internal("guard clause has no failure child");
                };
                self.branch.bindings.accept_event_metadata(
                    &payload,
                    &services.options.inspection.redacted_query_parameters,
                );
                self.branch.bindings.bind(
                    clause.alternative.binding,
                    Some(&clause.alternative.name),
                    payload,
                );
                let mut actor_scope = scope.clone();
                actor_scope.event.execution_context.actor = Some(clause.actor);
                let outcome = self.node(child, &actor_scope).await;
                self.branch.bindings.restore_bindings(&visible);
                outcome
            }
            crate::WaitCompletion::Failed(SelectionError::Journal(error)) => {
                let (index, error) = match error {
                    actors::JournalError::Overflow => (0, None),
                    actors::JournalError::WrongGeneration => (1, None),
                    actors::JournalError::Closed(error) => {
                        (2, Some(crate::StepError::Browser(error)))
                    }
                };
                let mut actor_scope = scope.clone();
                actor_scope.event.execution_context.actor = Some(clauses[ordinal].actor);
                self.selection_failure(children, failures[index], error, &actor_scope)
                    .await
            }
            crate::WaitCompletion::Failed(SelectionError::Predicate(error)) => {
                self.selection_failure(children, failures[0], Some(error), scope)
                    .await
            }
            _ => body_result,
        }
    }
}
fn guard_internal(message: &str) -> TestBodyOutcome {
    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
        failure: RunError::Internal(message.into()),
    })
}

type GuardCompletion = (usize, crate::WaitCompletion<Decision, SelectionError>);
fn earliest<F: std::future::Future<Output = GuardCompletion>>(
    mut first: Option<GuardCompletion>,
    watchers: &mut FuturesUnordered<F>,
    observed: &mut Vec<(usize, u64, u64)>,
) -> Option<GuardCompletion> {
    fn order(result: &GuardCompletion) -> (u8, u64, usize) {
        match &result.1 {
            crate::WaitCompletion::Failed(_) => (0, 0, result.0),
            crate::WaitCompletion::Ready(Decision::Selected { ingress, .. }) => {
                (1, *ingress, result.0)
            }
            _ => (2, u64::MAX, result.0),
        }
    }
    let remember = |value: &GuardCompletion, observed: &mut Vec<(usize, u64, u64)>| {
        if let crate::WaitCompletion::Ready(Decision::Selected {
            sequence, ingress, ..
        }) = &value.1
            && !observed.iter().any(|(ordinal, _, _)| *ordinal == value.0)
        {
            observed.push((value.0, *sequence, *ingress));
        }
    };
    if let Some(value) = &first {
        remember(value, observed);
    }
    let mut context = std::task::Context::from_waker(futures::task::noop_waker_ref());
    while let std::task::Poll::Ready(Some(candidate)) = watchers.poll_next_unpin(&mut context) {
        remember(&candidate, observed);
        if first
            .as_ref()
            .is_none_or(|prior| order(&candidate) < order(prior))
        {
            first = Some(candidate);
        }
    }
    first
}
