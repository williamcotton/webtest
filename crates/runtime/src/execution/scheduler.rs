//! Structured sibling scheduling. Futures own branch-local execution state; this
//! driver only observes completion and signals sibling cancellation. It never
//! drops unfinished siblings when a branch fails.
use crate::ScopeContext;
use futures::{StreamExt, stream::FuturesUnordered};
use std::future::Future;
use webtest_feedback::FailureClass;
use webtest_host::{Cancellation, CancellationReason};

/// A bounded, coalescing primary-failure notification. This is a runtime service,
/// not a container for mutable branch state. Ancestor schedulers can observe the
/// same failure before nested scopes finish tearing down.
#[derive(Clone)]
pub(crate) struct FailureSignal(tokio::sync::watch::Sender<Option<FailureClass>>);
impl FailureSignal {
    pub fn channel() -> (Self, tokio::sync::watch::Receiver<Option<FailureClass>>) {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        (Self(sender), receiver)
    }
    pub fn report(&self, class: FailureClass) {
        if matches!(class, FailureClass::Infrastructure | FailureClass::Internal) {
            self.0.send_if_modified(|value| {
                if value.is_some() {
                    false
                } else {
                    *value = Some(class);
                    true
                }
            });
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SiblingPolicy {
    All,
    FirstSuccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Completion {
    Passed,
    Failed(FailureClass),
    Cancelled,
}

pub(super) async fn schedule<F, T>(
    parent: &ScopeContext,
    policy: SiblingPolicy,
    children: Vec<(
        ScopeContext,
        tokio::sync::watch::Receiver<Option<FailureClass>>,
        F,
    )>,
    classify: impl Fn(&T) -> Completion,
    mut on_completion: impl FnMut(usize, T),
) -> Option<usize>
where
    F: Future<Output = T>,
{
    let contexts: Vec<_> = children
        .iter()
        .map(|(context, _, _)| context.clone())
        .collect();
    let failures: Vec<_> = children
        .iter()
        .map(|(_, receiver, _)| receiver.clone())
        .collect();
    let mut notices = FuturesUnordered::new();
    let mut pending = FuturesUnordered::new();
    for (ordinal, (_, mut receiver, future)) in children.into_iter().enumerate() {
        notices.push(async move {
            let class = receiver
                .wait_for(Option::is_some)
                .await
                .ok()
                .and_then(|value| *value);
            (ordinal, class)
        });
        pending.push(async move { (ordinal, future.await) });
    }
    let mut completed = vec![false; contexts.len()];
    let mut winner = None;
    let mut unhealthy = false;
    while !pending.is_empty() {
        tokio::select! {
            biased;
            Some((ordinal, class)) = notices.next(), if !notices.is_empty() => {
                if class.is_some() {
                    unhealthy = true;
                    winner = None;
                    cancel_siblings(parent, &contexts, &completed, ordinal, CancellationReason::ParentFailed);
                }
            }
            Some((ordinal, outcome)) = pending.next() => {
                // Polling child futures can publish a failure after select has
                // polled notices but before it returns a completed sibling.
                // Observe those reports before awarding a race winner.
                for (failed, receiver) in failures.iter().enumerate() {
                    if receiver.borrow().is_some() {
                        unhealthy = true;
                        winner = None;
                        cancel_siblings(parent, &contexts, &completed, failed, CancellationReason::ParentFailed);
                    }
                }
                // Also catches infrastructure failures discovered during cleanup.
                let completion = classify(&outcome);
                if matches!(completion, Completion::Failed(FailureClass::Infrastructure | FailureClass::Internal)) {
                    unhealthy = true;
                    winner = None;
                    cancel_siblings(parent, &contexts, &completed, ordinal, CancellationReason::ParentFailed);
                } else if policy == SiblingPolicy::FirstSuccess
                    && completion == Completion::Passed
                    && winner.is_none()
                    && !unhealthy
                    && parent.cancellation.cause().is_none()
                {
                    winner = Some(ordinal);
                    cancel_siblings(parent, &contexts, &completed, ordinal, CancellationReason::RaceLost);
                }
                completed[ordinal] = true;
                on_completion(ordinal, outcome);
            }
        }
    }
    winner.filter(|_| parent.cancellation.cause().is_none())
}

fn cancel_siblings(
    parent: &ScopeContext,
    contexts: &[ScopeContext],
    completed: &[bool],
    failed: usize,
    reason: CancellationReason,
) {
    for (ordinal, context) in contexts.iter().enumerate() {
        if ordinal != failed && !completed[ordinal] {
            context.cancellation.cancel(Cancellation {
                reason,
                causing_scope_id: parent.scope_id,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::sync::Barrier;
    use webtest_model::ExecutionScopeId;

    fn parent() -> ScopeContext {
        ScopeContext {
            scope_id: ExecutionScopeId(0),
            cancellation: Default::default(),
            deadline: None,
            deadline_scope_id: None,
        }
    }

    type Task = std::pin::Pin<Box<dyn Future<Output = Completion> + Send>>;

    #[tokio::test(start_paused = true)]
    async fn race_observes_primary_failure_published_in_the_same_poll_as_a_success() {
        let parent = parent();
        let failed = parent.child(ExecutionScopeId(1), None);
        let successful = parent.child(ExecutionScopeId(2), None);
        let (signal, receiver) = FailureSignal::channel();
        let (_, other_receiver) = FailureSignal::channel();
        let failure: Task = Box::pin(async move {
            signal.report(FailureClass::Infrastructure);
            tokio::time::sleep(Duration::from_millis(100)).await;
            Completion::Failed(FailureClass::Infrastructure)
        });
        let success: Task = Box::pin(async { Completion::Passed });
        let mut completed = Vec::new();
        let started = tokio::time::Instant::now();
        let winner = schedule(
            &parent,
            SiblingPolicy::FirstSuccess,
            vec![
                (failed, receiver, failure),
                (successful.clone(), other_receiver, success),
            ],
            |outcome| *outcome,
            |ordinal, outcome| completed.push((ordinal, outcome)),
        )
        .await;
        assert_eq!(winner, None);
        assert_eq!(
            successful.cancellation.cause(),
            Some(Cancellation {
                reason: CancellationReason::ParentFailed,
                causing_scope_id: parent.scope_id,
            })
        );
        assert_eq!(completed.len(), 2);
        assert_eq!(started.elapsed(), Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn race_keeps_the_first_winner_when_another_success_is_already_ready() {
        let parent = parent();
        let children = (1..=2)
            .map(|id| {
                let (_, receiver) = FailureSignal::channel();
                let future: Task = Box::pin(async { Completion::Passed });
                (parent.child(ExecutionScopeId(id), None), receiver, future)
            })
            .collect();
        let mut results = Vec::new();
        let winner = schedule(
            &parent,
            SiblingPolicy::FirstSuccess,
            children,
            |outcome| *outcome,
            |ordinal, outcome| results.push((ordinal, outcome)),
        )
        .await;
        assert_eq!(winner, Some(results[0].0));
        assert_eq!(results.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn race_cannot_award_success_after_parent_cancellation() {
        for reason in [
            CancellationReason::UserCancelled,
            CancellationReason::Timeout,
            CancellationReason::ParentFailed,
            CancellationReason::FailFast,
            CancellationReason::DebugDisconnect,
            CancellationReason::RunnerShutdown,
            CancellationReason::RaceLost,
        ] {
            let parent = parent();
            parent.cancellation.cancel(Cancellation {
                reason,
                causing_scope_id: parent.scope_id,
            });
            let child = parent.child(ExecutionScopeId(1), None);
            let (_, receiver) = FailureSignal::channel();
            let mut completed = 0;
            let winner = schedule(
                &parent,
                SiblingPolicy::FirstSuccess,
                vec![(child.clone(), receiver, async { Completion::Passed })],
                |outcome| *outcome,
                |_, _| completed += 1,
            )
            .await;
            assert_eq!(winner, None);
            assert_eq!(completed, 1);
            assert_eq!(child.cancellation.cause().unwrap().reason, reason);
        }
    }

    struct Finished {
        ordinal: usize,
        failure: Option<FailureClass>,
        cancellation: Option<Cancellation>,
    }

    async fn branch(
        ordinal: usize,
        context: ScopeContext,
        barrier: Arc<Barrier>,
        failure: Option<FailureClass>,
        cleanup: Arc<Mutex<Vec<usize>>>,
    ) -> Finished {
        barrier.wait().await;
        let cancellation = tokio::select! {
            biased;
            cause = context.cancellation.cancelled() => Some(cause),
            _ = tokio::time::sleep(Duration::from_millis((3 - ordinal) as u64 * 10)) => None,
        };
        // Teardown is part of the future, including after explicit cancellation.
        tokio::time::sleep(Duration::from_millis(5)).await;
        cleanup.lock().unwrap().push(ordinal);
        Finished {
            ordinal,
            failure,
            cancellation,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn parallel_collects_test_failures_and_waits_for_every_child_teardown_in_source_order() {
        for failure in [
            FailureClass::Test,
            FailureClass::Infrastructure,
            FailureClass::Internal,
        ] {
            let parent = ScopeContext {
                scope_id: ExecutionScopeId(0),
                cancellation: Default::default(),
                deadline: None,
                deadline_scope_id: None,
            };
            let barrier = Arc::new(Barrier::new(3));
            let cleanup = Arc::new(Mutex::new(Vec::new()));
            let children = (0..3)
                .map(|ordinal| {
                    let context = parent.child(ExecutionScopeId(ordinal as u64 + 1), None);
                    let future = branch(
                        ordinal,
                        context.clone(),
                        barrier.clone(),
                        (ordinal == 2).then_some(failure),
                        cleanup.clone(),
                    );
                    {
                        let (_signal, receiver) = FailureSignal::channel();
                        (context, receiver, future)
                    }
                })
                .collect();
            let mut outcomes = Vec::new();
            schedule(
                &parent,
                SiblingPolicy::All,
                children,
                |outcome| {
                    outcome
                        .failure
                        .map_or(Completion::Passed, Completion::Failed)
                },
                |_, outcome| outcomes.push(outcome),
            )
            .await;
            outcomes.sort_by_key(|outcome| outcome.ordinal);
            assert_eq!(
                outcomes
                    .iter()
                    .map(|outcome| outcome.ordinal)
                    .collect::<Vec<_>>(),
                [0, 1, 2]
            );
            let cleaned = cleanup.lock().unwrap();
            assert_eq!(cleaned.len(), 3);
            assert_eq!(
                cleaned[0], 2,
                "completion order differs from presentation order"
            );
            assert!(outcomes[2].cancellation.is_none());
            if failure == FailureClass::Test {
                assert!(
                    outcomes
                        .iter()
                        .all(|outcome| outcome.cancellation.is_none())
                );
            } else {
                assert!(outcomes[..2].iter().all(|outcome| {
                    outcome.cancellation.is_some_and(|cause| {
                        cause.reason == CancellationReason::ParentFailed
                            && cause.causing_scope_id == parent.scope_id
                    })
                }));
            }
            assert!(parent.cancellation.cause().is_none());
        }
    }
}
