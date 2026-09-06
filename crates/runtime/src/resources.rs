//! Synchronous ownership metadata; host acquisition and teardown never hold the registry lock.
use std::{collections::BTreeMap, sync::Mutex};
use webtest_model::{ExecutionScopeId, ResourceGenerationId, RuntimeResourceId};
use webtest_observation::{
    AcquisitionState, ResourceAccess, ResourceEvent, ResourceEventKind, ResourceKey, ResourceKind,
    RuntimeResourceEntry, TeardownState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceInvariant {
    MissingResource,
    StaleGeneration,
    InvalidTransition,
    NotReady,
    AccessConflict,
    OutstandingLeases,
    WrongLeaseOwner,
    LiveResource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLease {
    id: u64,
    pub key: ResourceKey,
    pub owner: ExecutionScopeId,
    pub access: ResourceAccess,
}

#[derive(Default)]
struct State {
    next_id: u64,
    entries: BTreeMap<RuntimeResourceId, RuntimeResourceEntry>,
    leases: BTreeMap<u64, ResourceLease>,
}
impl State {
    fn next(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    fn entry(&mut self, key: ResourceKey) -> Result<&mut RuntimeResourceEntry, ResourceInvariant> {
        let entry = self
            .entries
            .get_mut(&key.resource_id)
            .ok_or(ResourceInvariant::MissingResource)?;
        if entry.key != key {
            return Err(ResourceInvariant::StaleGeneration);
        }
        Ok(entry)
    }
}

#[derive(Default)]
pub struct ResourceRegistry {
    state: Mutex<State>,
}

impl ResourceRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn entry(&self, key: ResourceKey) -> Result<RuntimeResourceEntry, ResourceInvariant> {
        Ok(self.lock().entry(key)?.clone())
    }

    pub fn begin_acquisition(
        &self,
        owner: ExecutionScopeId,
        kind: ResourceKind,
        access: ResourceAccess,
    ) -> ResourceEvent {
        let mut state = self.lock();
        let key = ResourceKey {
            resource_id: RuntimeResourceId(state.next()),
            generation_id: ResourceGenerationId(state.next()),
        };
        let resource = RuntimeResourceEntry {
            key,
            owner_scope_id: owner,
            resource_kind: kind,
            access_policy: access,
            acquisition_state: AcquisitionState::Acquiring,
            teardown_state: TeardownState::Pending,
            ownership_acquired: false,
            cancellation: None,
        };
        state.entries.insert(key.resource_id, resource.clone());
        ResourceEvent {
            kind: ResourceEventKind::AcquireStarted,
            resource,
        }
    }

    /// Ownership can precede readiness; cancellation during an acknowledgement barrier still cleans up.
    pub fn acknowledge_ownership(&self, key: ResourceKey) -> Result<(), ResourceInvariant> {
        let mut state = self.lock();
        let entry = state.entry(key)?;
        if entry.acquisition_state != AcquisitionState::Acquiring || entry.ownership_acquired {
            return Err(ResourceInvariant::InvalidTransition);
        }
        entry.ownership_acquired = true;
        Ok(())
    }

    pub fn ready(&self, key: ResourceKey) -> Result<ResourceEvent, ResourceInvariant> {
        let mut state = self.lock();
        let entry = state.entry(key)?;
        if entry.acquisition_state != AcquisitionState::Acquiring
            || entry.teardown_state != TeardownState::Pending
            || !entry.ownership_acquired
            || entry.cancellation.is_some()
        {
            return Err(ResourceInvariant::InvalidTransition);
        }
        entry.acquisition_state = AcquisitionState::Ready;
        Ok(ResourceEvent {
            kind: ResourceEventKind::Ready,
            resource: entry.clone(),
        })
    }

    pub fn acquire_failed(&self, key: ResourceKey) -> Result<ResourceEvent, ResourceInvariant> {
        let mut state = self.lock();
        let entry = state.entry(key)?;
        if entry.acquisition_state != AcquisitionState::Acquiring {
            return Err(ResourceInvariant::InvalidTransition);
        }
        entry.acquisition_state = AcquisitionState::Failed;
        if !entry.ownership_acquired {
            entry.teardown_state = TeardownState::Released;
        }
        Ok(ResourceEvent {
            kind: ResourceEventKind::AcquireFailed,
            resource: entry.clone(),
        })
    }

    pub fn lease(
        &self,
        key: ResourceKey,
        owner: ExecutionScopeId,
        access: ResourceAccess,
    ) -> Result<ResourceLease, ResourceInvariant> {
        let mut state = self.lock();
        let entry = state.entry(key)?;
        if entry.acquisition_state != AcquisitionState::Ready
            || entry.teardown_state != TeardownState::Pending
            || entry.cancellation.is_some()
        {
            return Err(ResourceInvariant::NotReady);
        }
        if access == ResourceAccess::Shared && entry.access_policy != ResourceAccess::Shared {
            return Err(ResourceInvariant::AccessConflict);
        }
        if state.leases.values().any(|lease| {
            lease.key == key
                && (access == ResourceAccess::Exclusive
                    || lease.access == ResourceAccess::Exclusive)
        }) {
            return Err(ResourceInvariant::AccessConflict);
        }
        let lease = ResourceLease {
            id: state.next(),
            key,
            owner,
            access,
        };
        state.leases.insert(lease.id, lease);
        Ok(lease)
    }

    pub fn release_lease(&self, lease: ResourceLease) -> Result<(), ResourceInvariant> {
        let mut state = self.lock();
        state.entry(lease.key)?;
        if state.leases.get(&lease.id) != Some(&lease) {
            return Err(ResourceInvariant::WrongLeaseOwner);
        }
        state.leases.remove(&lease.id);
        Ok(())
    }

    pub fn cancel(
        &self,
        key: ResourceKey,
        cause: webtest_host::Cancellation,
    ) -> Result<(), ResourceInvariant> {
        let mut state = self.lock();
        let entry = state.entry(key)?;
        if matches!(
            entry.teardown_state,
            TeardownState::Released | TeardownState::Failed
        ) {
            return Err(ResourceInvariant::InvalidTransition);
        }
        entry.cancellation.get_or_insert(cause);
        Ok(())
    }

    pub fn begin_release(&self, key: ResourceKey) -> Result<ResourceEvent, ResourceInvariant> {
        let mut state = self.lock();
        state.entry(key)?;
        if state.leases.values().any(|lease| lease.key == key) {
            return Err(ResourceInvariant::OutstandingLeases);
        }
        let entry = state.entry(key)?;
        if entry.acquisition_state == AcquisitionState::Acquiring
            || !entry.ownership_acquired
            || entry.teardown_state != TeardownState::Pending
        {
            return Err(ResourceInvariant::InvalidTransition);
        }
        entry.teardown_state = TeardownState::Releasing;
        Ok(ResourceEvent {
            kind: ResourceEventKind::ReleaseStarted,
            resource: entry.clone(),
        })
    }

    pub fn finish_release(
        &self,
        key: ResourceKey,
        succeeded: bool,
    ) -> Result<ResourceEvent, ResourceInvariant> {
        let mut state = self.lock();
        let entry = state.entry(key)?;
        if entry.teardown_state != TeardownState::Releasing {
            return Err(ResourceInvariant::InvalidTransition);
        }
        entry.teardown_state = if succeeded {
            TeardownState::Released
        } else {
            TeardownState::Failed
        };
        Ok(ResourceEvent {
            kind: if succeeded {
                ResourceEventKind::Released
            } else {
                ResourceEventKind::ReleaseFailed
            },
            resource: entry.clone(),
        })
    }

    pub fn reacquire(
        &self,
        key: ResourceKey,
        owner: ExecutionScopeId,
    ) -> Result<ResourceEvent, ResourceInvariant> {
        let mut state = self.lock();
        if state.entry(key)?.teardown_state != TeardownState::Released {
            return Err(ResourceInvariant::InvalidTransition);
        }
        let generation = ResourceGenerationId(state.next());
        let entry = state.entry(key)?;
        entry.key.generation_id = generation;
        entry.owner_scope_id = owner;
        entry.acquisition_state = AcquisitionState::Acquiring;
        entry.teardown_state = TeardownState::Pending;
        entry.ownership_acquired = false;
        entry.cancellation = None;
        Ok(ResourceEvent {
            kind: ResourceEventKind::AcquireStarted,
            resource: entry.clone(),
        })
    }

    pub fn validate_owner_finished(
        &self,
        owner: ExecutionScopeId,
    ) -> Result<(), ResourceInvariant> {
        let state = self.lock();
        if state.leases.values().any(|lease| lease.owner == owner)
            || state.entries.values().any(|entry| {
                entry.owner_scope_id == owner
                    && matches!(
                        entry.teardown_state,
                        TeardownState::Pending | TeardownState::Releasing
                    )
            })
        {
            return Err(ResourceInvariant::LiveResource);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledgement_leases_generation_and_exactly_once_release_are_enforced() {
        let registry = ResourceRegistry::default();
        let owner = ExecutionScopeId(1);
        let mut events = vec![registry.begin_acquisition(
            owner,
            ResourceKind::Conformance,
            ResourceAccess::Shared,
        )];
        let key = events[0].resource.key;
        assert_eq!(
            registry.ready(key),
            Err(ResourceInvariant::InvalidTransition)
        );
        assert_eq!(
            registry.lease(key, owner, ResourceAccess::Shared),
            Err(ResourceInvariant::NotReady)
        );
        registry.acknowledge_ownership(key).expect("owned");
        events.push(registry.ready(key).expect("acknowledged"));
        let first = registry
            .lease(key, owner, ResourceAccess::Shared)
            .expect("shared");
        let second = registry
            .lease(key, ExecutionScopeId(2), ResourceAccess::Shared)
            .expect("shared sibling");
        assert_eq!(
            registry.lease(key, owner, ResourceAccess::Exclusive),
            Err(ResourceInvariant::AccessConflict)
        );
        assert_eq!(
            registry.begin_release(key),
            Err(ResourceInvariant::OutstandingLeases)
        );
        registry.release_lease(first).expect("release first");
        registry.release_lease(second).expect("release second");
        events.push(registry.begin_release(key).expect("teardown"));
        assert_eq!(
            registry.begin_release(key),
            Err(ResourceInvariant::InvalidTransition)
        );
        events.push(registry.finish_release(key, true).expect("released"));
        registry
            .validate_owner_finished(owner)
            .expect("no survivors");
        let next = registry
            .reacquire(key, ExecutionScopeId(3))
            .expect("fresh generation");
        assert_ne!(key.generation_id, next.resource.key.generation_id);
        assert_eq!(registry.ready(key), Err(ResourceInvariant::StaleGeneration));
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            [
                ResourceEventKind::AcquireStarted,
                ResourceEventKind::Ready,
                ResourceEventKind::ReleaseStarted,
                ResourceEventKind::Released
            ]
        );
    }

    #[test]
    fn cancellation_during_acquisition_keeps_partial_ownership_until_cleanup() {
        let registry = ResourceRegistry::default();
        let owner = ExecutionScopeId(1);
        let key = registry
            .begin_acquisition(owner, ResourceKind::Conformance, ResourceAccess::Exclusive)
            .resource
            .key;
        registry
            .acknowledge_ownership(key)
            .expect("partial ownership");
        registry
            .cancel(
                key,
                webtest_host::Cancellation {
                    reason: webtest_host::CancellationReason::Timeout,
                    causing_scope_id: owner,
                },
            )
            .expect("cancel");
        assert_eq!(
            registry.ready(key),
            Err(ResourceInvariant::InvalidTransition)
        );
        registry.acquire_failed(key).expect("failed barrier");
        assert_eq!(
            registry.validate_owner_finished(owner),
            Err(ResourceInvariant::LiveResource)
        );
        registry
            .begin_release(key)
            .expect("release partial resource");
        let terminal = registry.finish_release(key, false).expect("failed release");
        assert_eq!(terminal.kind, ResourceEventKind::ReleaseFailed);
        registry
            .validate_owner_finished(owner)
            .expect("terminal failure retained");
        assert_eq!(
            registry.reacquire(key, owner),
            Err(ResourceInvariant::InvalidTransition)
        );
    }
}
