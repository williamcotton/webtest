use std::{collections::BTreeMap, path::PathBuf};
use webtest_model::ExecutionScopeId;
use webtest_observation::{
    CleanupCause, CleanupFailure, CleanupResource, ExecutionId, ResourceAccess, ResourceKey,
    ResourceKind, ScopeEvent,
};

use super::{emit_resource_event, resource_cleanup_invariant, state::TestExecutionState};
use crate::{
    ResourceRegistry, RunEventSink, ScopeContext, cleanup::CleanupDeadline, events::EventBuffer,
};

type DirectoryRelease =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), std::io::Error>> + Send>>;

struct OwnedDirectory {
    key: ResourceKey,
    scope: ScopeEvent,
    context: ScopeContext,
    release: Option<DirectoryRelease>,
}

/// Provider results transfer ownership to the nearest lexical resource scope.
/// Keeping handles outside suspended child futures also permits bounded cleanup
/// after a non-cooperative child has exhausted its interruption budget.
#[derive(Default)]
pub(super) struct TemporaryResources {
    owned: BTreeMap<PathBuf, OwnedDirectory>,
}

pub(super) struct ResourceEvents<'a> {
    pub registry: &'a ResourceRegistry,
    pub execution_id: ExecutionId,
    pub events: &'a EventBuffer,
    pub sink: Option<&'a dyn RunEventSink>,
}

impl TemporaryResources {
    pub fn adopt(
        &mut self,
        state: &TestExecutionState,
        scope: &ScopeEvent,
        context: &ScopeContext,
        output: &ResourceEvents<'_>,
    ) -> Result<(), crate::ResourceInvariant> {
        for path in state.temporary_directories() {
            if self.owned.contains_key(&path) {
                continue;
            }
            let event = output.registry.begin_acquisition(
                context.scope_id,
                ResourceKind::TemporaryDirectory,
                ResourceAccess::Exclusive,
            );
            let key = event.resource.key;
            // Record the handle before any fallible metadata transition.
            self.owned.insert(
                path,
                OwnedDirectory {
                    key,
                    scope: scope.clone(),
                    context: context.clone(),
                    release: None,
                },
            );
            emit_resource_event(
                output.events,
                output.sink,
                output.execution_id,
                scope,
                event,
            );
            output.registry.acknowledge_ownership(key)?;
            let event = output.registry.ready(key)?;
            emit_resource_event(
                output.events,
                output.sink,
                output.execution_id,
                scope,
                event,
            );
        }
        Ok(())
    }

    pub async fn release(
        &mut self,
        owner: Option<ExecutionScopeId>,
        state: &mut TestExecutionState,
        deadline: CleanupDeadline,
        output: &ResourceEvents<'_>,
    ) -> Vec<CleanupFailure> {
        let paths: Vec<_> = self
            .owned
            .iter()
            .filter(|(_, entry)| owner.is_none_or(|owner| entry.context.scope_id == owner))
            .map(|(path, _)| path.clone())
            .collect();
        let mut failures = Vec::new();
        for path in paths {
            let Some(entry) = self.owned.get_mut(&path) else {
                continue;
            };
            if let Some(cause) = entry.context.cancellation.cause()
                && let Err(error) = output.registry.cancel(entry.key, cause)
            {
                failures.push(resource_cleanup_invariant(error));
            }
            if entry.release.is_none() {
                match output.registry.begin_release(entry.key) {
                    Ok(event) => emit_resource_event(
                        output.events,
                        output.sink,
                        output.execution_id,
                        &entry.scope,
                        event,
                    ),
                    Err(error) => failures.push(resource_cleanup_invariant(error)),
                }
                let path = path.clone();
                entry.release = Some(Box::pin(
                    async move { tokio::fs::remove_dir_all(path).await },
                ));
            }
            let Some(release) = entry.release.as_mut() else {
                continue;
            };
            let result = deadline
                .run(
                    CleanupResource::TemporaryDirectory { path: path.clone() },
                    release,
                    |error| CleanupCause::Io(error.into()),
                )
                .await;
            match output.registry.finish_release(entry.key, result.is_ok()) {
                Ok(event) => emit_resource_event(
                    output.events,
                    output.sink,
                    output.execution_id,
                    &entry.scope,
                    event,
                ),
                Err(error) => failures.push(resource_cleanup_invariant(error)),
            }
            if let Err(error) = result {
                failures.push(error);
            }
            self.owned.remove(&path);
            state.release_temporary_directory(&path);
        }
        failures
    }
}
