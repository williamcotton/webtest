//! Generic acknowledged acquisition/body/teardown protocol. Handles remain typed host values.
use crate::{
    ResourceInvariant, ResourceRegistry, ScopeContext, WaitCleanupFailure, WaitCompletion,
    WaitRegistry, WaitSource, cleanup::CleanupDeadline,
};
use async_trait::async_trait;
use std::{future::Future, pin::Pin, time::Duration};
use webtest_host::Cancellation;
use webtest_observation::{ResourceAccess, ResourceEvent, ResourceKey, ResourceKind, WaitEvent};

#[derive(Debug, PartialEq, Eq)]
pub enum ResourceFailure<E> {
    Host(E),
    Invariant(ResourceInvariant),
}
impl<E> From<ResourceInvariant> for ResourceFailure<E> {
    fn from(error: ResourceInvariant) -> Self {
        Self::Invariant(error)
    }
}

/// The adapter acknowledges ownership immediately when it obtains a host handle,
/// before awaiting a readiness barrier that might be interrupted.
pub struct AcquisitionOwnership<'a> {
    registry: &'a ResourceRegistry,
    key: ResourceKey,
}
impl AcquisitionOwnership<'_> {
    pub fn acquired(&self) -> Result<(), ResourceInvariant> {
        self.registry.acknowledge_ownership(self.key)
    }
}

#[async_trait]
pub trait ResourceAdapter: Send {
    type Handle: Send;
    type Error: Send;
    /// True only when the body can observe cancellation and host interruption does
    /// not need Rust guards held by that body. Both operations are then awaited.
    fn cooperative_body_interruption(&self) -> bool {
        false
    }
    async fn acquire(
        &mut self,
        context: &ScopeContext,
        ownership: &AcquisitionOwnership<'_>,
    ) -> Result<Self::Handle, ResourceFailure<Self::Error>>;
    async fn interrupt(&mut self, cause: Cancellation) -> Result<(), Self::Error>;
    async fn teardown(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResourceCleanupFailure<E> {
    Wait(WaitCleanupFailure<ResourceFailure<E>>),
    Invariant(ResourceInvariant),
    Host(E),
    TimedOut,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ResourceScopeOutcome<T, E> {
    pub primary: WaitCompletion<T, ResourceFailure<E>>,
    pub secondary: Vec<ResourceCleanupFailure<E>>,
    pub cleanup_deadline: tokio::time::Instant,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceScopeEvent {
    Resource(ResourceEvent),
    Wait(WaitEvent),
}

pub struct ResourceScope<'a> {
    pub registry: &'a ResourceRegistry,
    pub waits: &'a WaitRegistry,
    pub context: &'a ScopeContext,
    pub kind: ResourceKind,
    pub access: ResourceAccess,
    pub cleanup_timeout: Duration,
}

impl ResourceScope<'_> {
    pub async fn run<A, B, F, T>(
        &self,
        adapter: A,
        body: B,
        emit: impl FnMut(ResourceScopeEvent) + Send,
    ) -> ResourceScopeOutcome<T, A::Error>
    where
        A: ResourceAdapter,
        B: FnOnce(A::Handle) -> F,
        F: Future<Output = Result<T, A::Error>> + Send,
        T: Send,
    {
        self.run_observed(adapter, body, emit, |_| {}).await
    }

    /// Reports the primary execution result before resource teardown. The observer
    /// may signal a structured scheduler but does not own or detach cleanup.
    pub async fn run_observed<A, B, F, T>(
        &self,
        mut adapter: A,
        body: B,
        mut emit: impl FnMut(ResourceScopeEvent) + Send,
        primary_known: impl FnOnce(&WaitCompletion<T, ResourceFailure<A::Error>>) + Send,
    ) -> ResourceScopeOutcome<T, A::Error>
    where
        A: ResourceAdapter,
        B: FnOnce(A::Handle) -> F,
        F: Future<Output = Result<T, A::Error>> + Send,
        T: Send,
    {
        let event = self
            .registry
            .begin_acquisition(self.context.scope_id, self.kind, self.access);
        let key = event.resource.key;
        emit(ResourceScopeEvent::Resource(event));
        let ownership = AcquisitionOwnership {
            registry: self.registry,
            key,
        };
        let acquired = self
            .waits
            .wait(
                self.context,
                &mut AcquireWait {
                    adapter: &mut adapter,
                    context: self.context,
                    ownership: &ownership,
                },
                self.cleanup_timeout,
                |event| emit(ResourceScopeEvent::Wait(event)),
            )
            .await;
        let mut secondary = acquired
            .secondary
            .into_iter()
            .map(ResourceCleanupFailure::Wait)
            .collect::<Vec<_>>();
        let mut cleanup_deadline = acquired.cleanup_deadline;
        let mut lease = None;
        let acquisition = match (self.context.cancellation.cause(), acquired.primary) {
            (Some(cause), _) => WaitCompletion::Cancelled(cause),
            (_, primary) => primary,
        };
        let mut primary = match acquisition {
            WaitCompletion::Ready(handle) => match self.registry.ready(key) {
                Ok(event) => {
                    emit(ResourceScopeEvent::Resource(event));
                    match self.registry.lease(key, self.context.scope_id, self.access) {
                        Ok(acquired_lease) => {
                            lease = Some(acquired_lease);
                            let outcome = self
                                .waits
                                .wait(
                                    self.context,
                                    &mut BodyWait {
                                        adapter: &mut adapter,
                                        body: Some(Box::pin(body(handle))),
                                    },
                                    self.cleanup_timeout,
                                    |event| emit(ResourceScopeEvent::Wait(event)),
                                )
                                .await;
                            cleanup_deadline = outcome.cleanup_deadline;
                            secondary.extend(
                                outcome
                                    .secondary
                                    .into_iter()
                                    .map(ResourceCleanupFailure::Wait),
                            );
                            outcome.primary
                        }
                        Err(error) => WaitCompletion::Failed(ResourceFailure::Invariant(error)),
                    }
                }
                Err(error) => WaitCompletion::Failed(ResourceFailure::Invariant(error)),
            },
            WaitCompletion::Failed(error) => WaitCompletion::Failed(error),
            WaitCompletion::Cancelled(cause) => WaitCompletion::Cancelled(cause),
            WaitCompletion::Rejected(error) => WaitCompletion::Rejected(error),
        };
        primary_known(&primary);
        if let WaitCompletion::Cancelled(cause) = &primary
            && let Err(error) = self.registry.cancel(key, *cause)
        {
            secondary.push(ResourceCleanupFailure::Invariant(error));
        }
        if let Some(lease) = lease
            && let Err(error) = self.registry.release_lease(lease)
        {
            secondary.push(ResourceCleanupFailure::Invariant(error));
        }
        match self.registry.entry(key) {
            Ok(entry) => {
                if entry.acquisition_state == webtest_observation::AcquisitionState::Acquiring {
                    match self.registry.acquire_failed(key) {
                        Ok(event) => emit(ResourceScopeEvent::Resource(event)),
                        Err(error) => secondary.push(ResourceCleanupFailure::Invariant(error)),
                    }
                }
                if entry.ownership_acquired {
                    match self.registry.begin_release(key) {
                        Ok(event) => {
                            emit(ResourceScopeEvent::Resource(event));
                            let released =
                                CleanupDeadline::at(cleanup_deadline, self.cleanup_timeout)
                                    .bound(adapter.teardown())
                                    .await;
                            if let Some(cause) = self.context.cancellation.cause() {
                                if let Err(error) = self.registry.cancel(key, cause) {
                                    secondary.push(ResourceCleanupFailure::Invariant(error));
                                }
                                if matches!(primary, WaitCompletion::Ready(_)) {
                                    primary = WaitCompletion::Cancelled(cause);
                                }
                            }
                            let succeeded = matches!(released, Ok(Ok(())));
                            match released {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => {
                                    secondary.push(ResourceCleanupFailure::Host(error))
                                }
                                Err(_) => secondary.push(ResourceCleanupFailure::TimedOut),
                            }
                            match self.registry.finish_release(key, succeeded) {
                                Ok(event) => emit(ResourceScopeEvent::Resource(event)),
                                Err(error) => {
                                    secondary.push(ResourceCleanupFailure::Invariant(error))
                                }
                            }
                        }
                        Err(error) => secondary.push(ResourceCleanupFailure::Invariant(error)),
                    }
                }
            }
            Err(error) => secondary.push(ResourceCleanupFailure::Invariant(error)),
        }
        ResourceScopeOutcome {
            primary,
            secondary,
            cleanup_deadline,
        }
    }
}

struct AcquireWait<'a, A> {
    adapter: &'a mut A,
    context: &'a ScopeContext,
    ownership: &'a AcquisitionOwnership<'a>,
}
#[async_trait]
impl<A: ResourceAdapter> WaitSource for AcquireWait<'_, A> {
    type Output = A::Handle;
    type Error = ResourceFailure<A::Error>;
    async fn ready(&mut self) -> Result<Self::Output, Self::Error> {
        self.adapter.acquire(self.context, self.ownership).await
    }
    async fn interrupt(&mut self, cause: Cancellation) -> Result<(), Self::Error> {
        self.adapter
            .interrupt(cause)
            .await
            .map_err(ResourceFailure::Host)
    }
    async fn cleanup(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
struct BodyWait<'a, A, F> {
    adapter: &'a mut A,
    body: Option<Pin<Box<F>>>,
}
#[async_trait]
impl<A, F, T> WaitSource for BodyWait<'_, A, F>
where
    A: ResourceAdapter,
    F: Future<Output = Result<T, A::Error>> + Send,
    T: Send,
{
    type Output = T;
    type Error = ResourceFailure<A::Error>;
    async fn ready(&mut self) -> Result<T, Self::Error> {
        let body = self.body.as_mut().ok_or(ResourceFailure::Invariant(
            ResourceInvariant::InvalidTransition,
        ))?;
        body.await.map_err(ResourceFailure::Host)
    }
    async fn interrupt(&mut self, cause: Cancellation) -> Result<(), Self::Error> {
        if self.adapter.cooperative_body_interruption()
            && let Some(body) = self.body.as_mut()
        {
            let (body, interrupted) = tokio::join!(body, self.adapter.interrupt(cause));
            self.body.take();
            interrupted.map_err(ResourceFailure::Host)?;
            body.map_err(ResourceFailure::Host)?;
            return Ok(());
        }
        // Release borrowed handle guards before asking the adapter to stop host work.
        self.body.take();
        self.adapter
            .interrupt(cause)
            .await
            .map_err(ResourceFailure::Host)
    }
    async fn cleanup(&mut self) -> Result<(), Self::Error> {
        self.body.take();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::time::Instant;
    use webtest_host::CancellationReason;
    use webtest_model::ExecutionScopeId;
    use webtest_observation::{ResourceEventKind, TeardownState};

    #[derive(Default)]
    struct HostState {
        log: Vec<&'static str>,
        owned: bool,
    }
    struct FakeResource {
        state: Arc<Mutex<HostState>>,
        handle_lock: Arc<tokio::sync::Mutex<()>>,
        acknowledgement_delay: Duration,
        acquisition_fails: bool,
        teardown_fails: bool,
        interrupt_delay: Duration,
        teardown_delay: Duration,
    }
    impl FakeResource {
        fn new() -> Self {
            Self {
                state: Arc::default(),
                handle_lock: Arc::default(),
                acknowledgement_delay: Duration::ZERO,
                acquisition_fails: false,
                teardown_fails: false,
                interrupt_delay: Duration::ZERO,
                teardown_delay: Duration::ZERO,
            }
        }
    }
    #[async_trait]
    impl ResourceAdapter for FakeResource {
        type Handle = Arc<tokio::sync::Mutex<()>>;
        type Error = &'static str;
        async fn acquire(
            &mut self,
            _: &ScopeContext,
            ownership: &AcquisitionOwnership<'_>,
        ) -> Result<Self::Handle, ResourceFailure<Self::Error>> {
            {
                let mut state = self.state.lock().unwrap();
                state.owned = true;
                state.log.push("acquire");
            }
            ownership.acquired()?;
            tokio::time::sleep(self.acknowledgement_delay).await;
            if self.acquisition_fails {
                return Err(ResourceFailure::Host("acquisition"));
            }
            self.state.lock().unwrap().log.push("acknowledge");
            Ok(self.handle_lock.clone())
        }
        async fn interrupt(&mut self, _: Cancellation) -> Result<(), Self::Error> {
            // Cancellation must drop the suspended body's guard before interruption.
            let _guard = self.handle_lock.lock().await;
            self.state.lock().unwrap().log.push("interrupt");
            tokio::time::sleep(self.interrupt_delay).await;
            Ok(())
        }
        async fn teardown(&mut self) -> Result<(), Self::Error> {
            {
                let mut state = self.state.lock().unwrap();
                assert!(
                    state.owned,
                    "teardown exactly once after acquiring ownership"
                );
                state.owned = false;
                state.log.push("teardown");
            }
            tokio::time::sleep(self.teardown_delay).await;
            if self.teardown_fails {
                Err("teardown")
            } else {
                Ok(())
            }
        }
    }
    fn context(seconds: u64) -> ScopeContext {
        ScopeContext {
            scope_id: ExecutionScopeId(1),
            cancellation: crate::CancellationToken::default(),
            deadline: Some(Instant::now() + Duration::from_secs(seconds)),
            deadline_scope_id: Some(ExecutionScopeId(1)),
        }
    }
    fn resource_events(events: &[ResourceScopeEvent]) -> Vec<ResourceEventKind> {
        events
            .iter()
            .filter_map(|event| match event {
                ResourceScopeEvent::Resource(event) => Some(event.kind),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn body_runs_only_after_acknowledgement_and_teardown_precedes_completion() {
        let registry = ResourceRegistry::default();
        let waits = WaitRegistry::default();
        let context = context(10);
        let mut adapter = FakeResource::new();
        adapter.acknowledgement_delay = Duration::from_secs(2);
        let state = adapter.state.clone();
        let mut events = vec![];
        let outcome = ResourceScope {
            registry: &registry,
            waits: &waits,
            context: &context,
            kind: ResourceKind::Conformance,
            access: ResourceAccess::Exclusive,
            cleanup_timeout: Duration::from_secs(3),
        }
        .run(
            adapter,
            |_| async {
                assert_eq!(state.lock().unwrap().log, ["acquire", "acknowledge"]);
                state.lock().unwrap().log.push("body");
                Ok(42)
            },
            |event| events.push(event),
        )
        .await;
        assert_eq!(outcome.primary, WaitCompletion::Ready(42));
        assert!(outcome.secondary.is_empty());
        assert_eq!(
            state.lock().unwrap().log,
            ["acquire", "acknowledge", "body", "teardown"]
        );
        assert_eq!(
            resource_events(&events),
            [
                ResourceEventKind::AcquireStarted,
                ResourceEventKind::Ready,
                ResourceEventKind::ReleaseStarted,
                ResourceEventKind::Released
            ]
        );
        registry.validate_owner_finished(context.scope_id).unwrap();
        waits.validate_owner_finished(context.scope_id).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_during_acquisition_and_body_interrupts_and_releases_owned_resources() {
        for during_acquisition in [true, false] {
            let registry = ResourceRegistry::default();
            let waits = WaitRegistry::default();
            let context = context(2);
            let mut adapter = FakeResource::new();
            if during_acquisition {
                adapter.acknowledgement_delay = Duration::from_secs(10);
            }
            let state = adapter.state.clone();
            let mut events = vec![];
            let outcome = ResourceScope {
                registry: &registry,
                waits: &waits,
                context: &context,
                kind: ResourceKind::Conformance,
                access: ResourceAccess::Exclusive,
                cleanup_timeout: Duration::from_secs(3),
            }
            .run(
                adapter,
                |handle| async move {
                    let _guard = handle.lock().await;
                    std::future::pending::<Result<(), &'static str>>().await
                },
                |event| events.push(event),
            )
            .await;
            assert_eq!(
                outcome.primary,
                WaitCompletion::Cancelled(Cancellation {
                    reason: CancellationReason::Timeout,
                    causing_scope_id: context.scope_id
                })
            );
            assert!(outcome.secondary.is_empty());
            assert!(!state.lock().unwrap().owned);
            assert_eq!(
                state.lock().unwrap().log,
                if during_acquisition {
                    vec!["acquire", "interrupt", "teardown"]
                } else {
                    vec!["acquire", "acknowledge", "interrupt", "teardown"]
                }
            );
            assert_eq!(
                resource_events(&events),
                [
                    ResourceEventKind::AcquireStarted,
                    if during_acquisition {
                        ResourceEventKind::AcquireFailed
                    } else {
                        ResourceEventKind::Ready
                    },
                    ResourceEventKind::ReleaseStarted,
                    ResourceEventKind::Released
                ]
            );
            registry.validate_owner_finished(context.scope_id).unwrap();
            waits.validate_owner_finished(context.scope_id).unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn host_failures_and_teardown_failure_remain_independent_typed_facts() {
        for acquisition_fails in [true, false] {
            let registry = ResourceRegistry::default();
            let waits = WaitRegistry::default();
            let context = context(10);
            let mut adapter = FakeResource::new();
            adapter.acquisition_fails = acquisition_fails;
            adapter.teardown_fails = true;
            let mut events = vec![];
            let outcome = ResourceScope {
                registry: &registry,
                waits: &waits,
                context: &context,
                kind: ResourceKind::Conformance,
                access: ResourceAccess::Exclusive,
                cleanup_timeout: Duration::from_secs(3),
            }
            .run(
                adapter,
                |_| async { Err::<(), _>("body") },
                |event| events.push(event),
            )
            .await;
            assert_eq!(
                outcome.primary,
                WaitCompletion::Failed(ResourceFailure::Host(if acquisition_fails {
                    "acquisition"
                } else {
                    "body"
                }))
            );
            assert_eq!(
                outcome.secondary,
                [ResourceCleanupFailure::Host("teardown")]
            );
            assert_eq!(
                resource_events(&events).last(),
                Some(&ResourceEventKind::ReleaseFailed)
            );
            registry.validate_owner_finished(context.scope_id).unwrap();
            waits.validate_owner_finished(context.scope_id).unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn interruption_and_teardown_share_one_cleanup_budget() {
        let registry = ResourceRegistry::default();
        let waits = WaitRegistry::default();
        let context = context(2);
        let mut adapter = FakeResource::new();
        adapter.interrupt_delay = Duration::from_secs(2);
        adapter.teardown_delay = Duration::from_secs(2);
        let mut events = vec![];
        let started = Instant::now();
        let outcome = ResourceScope {
            registry: &registry,
            waits: &waits,
            context: &context,
            kind: ResourceKind::Conformance,
            access: ResourceAccess::Exclusive,
            cleanup_timeout: Duration::from_secs(3),
        }
        .run(
            adapter,
            |_| std::future::pending::<Result<(), &'static str>>(),
            |event| events.push(event),
        )
        .await;
        assert!(matches!(outcome.primary, WaitCompletion::Cancelled(_)));
        assert_eq!(outcome.secondary, [ResourceCleanupFailure::TimedOut]);
        assert_eq!(Instant::now() - started, Duration::from_secs(5));
        let ResourceScopeEvent::Resource(event) = events.last().unwrap() else {
            panic!("terminal resource event")
        };
        assert_eq!(event.resource.teardown_state, TeardownState::Failed);
        assert_eq!(event.kind, ResourceEventKind::ReleaseFailed);
        registry.validate_owner_finished(context.scope_id).unwrap();
        waits.validate_owner_finished(context.scope_id).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_during_release_is_recorded_without_interrupting_teardown() {
        let registry = ResourceRegistry::default();
        let waits = WaitRegistry::default();
        let context = context(10);
        let mut adapter = FakeResource::new();
        adapter.teardown_delay = Duration::from_secs(2);
        let state = adapter.state.clone();
        let cause = Cancellation {
            reason: CancellationReason::DebugDisconnect,
            causing_scope_id: ExecutionScopeId(9),
        };
        let mut events = vec![];
        let outcome = ResourceScope { registry: &registry, waits: &waits, context: &context,
            kind: ResourceKind::Conformance, access: ResourceAccess::Exclusive, cleanup_timeout: Duration::from_secs(3) }
            .run(adapter, |_| async { Ok(()) }, |event| {
                if matches!(&event, ResourceScopeEvent::Resource(event) if event.kind == ResourceEventKind::ReleaseStarted) {
                    context.cancellation.cancel(cause);
                }
                events.push(event);
            }).await;
        assert_eq!(outcome.primary, WaitCompletion::Cancelled(cause));
        assert!(outcome.secondary.is_empty());
        assert_eq!(
            state.lock().unwrap().log,
            ["acquire", "acknowledge", "teardown"]
        );
        let ResourceScopeEvent::Resource(terminal) = events.last().unwrap() else {
            panic!("terminal resource fact")
        };
        assert_eq!(terminal.kind, ResourceEventKind::Released);
        assert_eq!(terminal.resource.cancellation, Some(cause));
        assert_eq!(
            registry.cancel(terminal.resource.key, cause),
            Err(ResourceInvariant::InvalidTransition)
        );
        registry.validate_owner_finished(context.scope_id).unwrap();
        waits.validate_owner_finished(context.scope_id).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn rejected_wait_does_not_acquire_a_handle_or_run_the_body() {
        let registry = ResourceRegistry::default();
        let waits = WaitRegistry::new(0);
        let context = context(10);
        let adapter = FakeResource::new();
        let state = adapter.state.clone();
        let mut events = vec![];
        let outcome = ResourceScope {
            registry: &registry,
            waits: &waits,
            context: &context,
            kind: ResourceKind::Conformance,
            access: ResourceAccess::Exclusive,
            cleanup_timeout: Duration::from_secs(3),
        }
        .run(
            adapter,
            |_| async {
                panic!("unacquired body");
                #[allow(unreachable_code)]
                Ok(())
            },
            |event| events.push(event),
        )
        .await;
        assert_eq!(
            outcome.primary,
            WaitCompletion::Rejected(crate::WaitInvariant::CapacityExceeded)
        );
        assert!(outcome.secondary.is_empty());
        assert!(state.lock().unwrap().log.is_empty());
        assert_eq!(
            resource_events(&events),
            [
                ResourceEventKind::AcquireStarted,
                ResourceEventKind::AcquireFailed
            ]
        );
        registry.validate_owner_finished(context.scope_id).unwrap();
        waits.validate_owner_finished(context.scope_id).unwrap();
    }
}
