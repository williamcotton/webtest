use async_trait::async_trait;
use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};
use webtest_host::{Cancellation, OperationContext};
use webtest_model::ExecutionScopeId;

#[derive(Debug, Default)]
struct CancellationState {
    cause: Option<Cancellation>,
    children: Vec<Weak<CancellationInner>>,
}
#[derive(Debug, Default)]
struct CancellationInner {
    state: Mutex<CancellationState>,
    wake: Notify,
}

/// A parent keeps only weak references to its children, so completed subtrees are released.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<CancellationInner>);

impl CancellationToken {
    pub fn child(&self) -> Self {
        let child = Self::default();
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.children.retain(|child| child.strong_count() != 0);
        state.children.push(Arc::downgrade(&child.0));
        if let Some(cause) = state.cause {
            child.cancel(cause);
        }
        child
    }

    /// First cause wins. Cancelling a child never changes an ancestor or sibling.
    pub fn cancel(&self, cause: Cancellation) {
        let children = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.cause.is_some() {
                return;
            }
            state.cause = Some(cause);
            std::mem::take(&mut state.children)
        };
        self.0.wake.notify_waiters();
        for child in children.into_iter().filter_map(|child| child.upgrade()) {
            Self(child).cancel(cause);
        }
    }

    pub fn cause(&self) -> Option<Cancellation> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cause
    }

    pub async fn cancelled(&self) -> Cancellation {
        loop {
            let notified = self.0.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(cause) = self.cause() {
                return cause;
            }
            notified.await;
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScopeContext {
    pub scope_id: ExecutionScopeId,
    pub cancellation: CancellationToken,
    pub deadline: Option<Instant>,
    pub deadline_scope_id: Option<ExecutionScopeId>,
}

impl ScopeContext {
    pub fn child(&self, scope_id: ExecutionScopeId, local_deadline: Option<Instant>) -> Self {
        let (deadline, deadline_scope_id) = match (self.deadline, local_deadline) {
            (Some(inherited), Some(local)) if local < inherited => (Some(local), Some(scope_id)),
            (Some(inherited), _) => (Some(inherited), self.deadline_scope_id),
            (None, Some(local)) => (Some(local), Some(scope_id)),
            (None, None) => (None, None),
        };
        Self {
            scope_id,
            cancellation: self.cancellation.child(),
            deadline,
            deadline_scope_id,
        }
    }
}

#[async_trait]
impl OperationContext for ScopeContext {
    fn scope_id(&self) -> ExecutionScopeId {
        self.scope_id
    }
    fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }
    fn cancellation(&self) -> Option<Cancellation> {
        self.cancellation.cause()
    }
    async fn cancelled(&self) -> Cancellation {
        self.cancellation.cancelled().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_host::CancellationReason;

    #[tokio::test(start_paused = true)]
    async fn parent_cancellation_wakes_existing_and_late_children_and_retains_first_cause() {
        let root = CancellationToken::default();
        let child = root.child();
        let grandchild = child.child();
        let cause = Cancellation {
            reason: CancellationReason::Timeout,
            causing_scope_id: ExecutionScopeId(1),
        };
        root.cancel(cause);
        child.cancel(Cancellation {
            reason: CancellationReason::RaceLost,
            ..cause
        });
        assert_eq!(grandchild.cancelled().await, cause);
        assert_eq!(root.child().cancelled().await, cause);
        let root = CancellationToken::default();
        root.child().cancel(cause);
        assert!(root.cause().is_none());
        assert!(root.child().cause().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn inherited_deadlines_can_only_shrink() {
        let root = ScopeContext {
            scope_id: ExecutionScopeId(1),
            cancellation: CancellationToken::default(),
            deadline: Some(Instant::now() + Duration::from_secs(5)),
            deadline_scope_id: Some(ExecutionScopeId(1)),
        };
        let child = root.child(
            ExecutionScopeId(2),
            Some(Instant::now() + Duration::from_secs(10)),
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(child.remaining(), Some(Duration::from_secs(3)));
        let child = child.child(
            ExecutionScopeId(3),
            Some(Instant::now() + Duration::from_secs(1)),
        );
        assert_eq!(child.remaining(), Some(Duration::from_secs(1)));
    }
}
