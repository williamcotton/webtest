//! Structured sibling scheduling. Futures own branch-local execution state; this
//! driver only observes completion and signals sibling cancellation. It never
//! drops unfinished siblings when a branch fails.
use crate::ScopeContext;
use futures::{StreamExt, stream::FuturesUnordered};
use std::future::Future;
use webtest_feedback::FailureClass;
use webtest_host::{Cancellation, CancellationReason};

pub(super) async fn parallel<F, T>(
    parent: &ScopeContext,
    children: Vec<(ScopeContext, F)>,
    classify: impl Fn(&T) -> Option<FailureClass>,
    mut on_completion: impl FnMut(usize, T),
)
where
    F: Future<Output = T>,
{
    let contexts: Vec<_> = children
        .iter()
        .map(|(context, _)| context.clone())
        .collect();
    let mut pending: FuturesUnordered<_> = children
        .into_iter()
        .enumerate()
        .map(|(ordinal, (_, future))| async move { (ordinal, future.await) })
        .collect();
    let mut completed = vec![false; contexts.len()];
    while let Some((ordinal, outcome)) = pending.next().await {
        if matches!(
            classify(&outcome),
            Some(FailureClass::Infrastructure | FailureClass::Internal)
        ) {
            for (sibling, context) in contexts.iter().enumerate() {
                if sibling != ordinal && !completed[sibling] {
                    context.cancellation.cancel(Cancellation {
                        reason: CancellationReason::ParentFailed,
                        causing_scope_id: parent.scope_id,
                    });
                }
            }
        }
        completed[ordinal] = true;
        on_completion(ordinal, outcome);
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
                    (context, future)
                })
                .collect();
            let mut outcomes = Vec::new();
            parallel(&parent, children, |outcome| outcome.failure, |_, outcome| outcomes.push(outcome)).await;
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
