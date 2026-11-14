//! Fixture execution composes ordinary plan traversal with owned lifetime handoff.
use super::*;
use crate::ScopeContext;
use crate::fixtures::{
    FixtureClient, FixtureError, FixtureOwner, FixtureOwnerKey, FixtureRequestEnvironment,
    FixtureValue,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use webtest_model::{ExecutionScopeId, FixtureLifetime};

#[derive(Clone)]
pub(crate) struct FixtureAccess {
    pub client: FixtureClient,
    pub invocation: ExecutionId,
    pub environment: FixtureRequestEnvironment,
    pub contexts: Arc<BTreeMap<FixtureOwnerKey, ScopeContext>>,
    pub test_owner: Option<FixtureOwner>,
}
impl FixtureAccess {
    pub(super) fn owner(
        &self,
        lifetime: FixtureLifetime,
        scope: &scopes::ExecutionScope,
    ) -> Result<FixtureOwner, FixtureError> {
        let key = match lifetime {
            FixtureLifetime::Test => {
                let original = self.test_owner.as_ref().and_then(|owner| match owner.key {
                    FixtureOwnerKey::Test { test, attempt } => Some((test, attempt)),
                    _ => None,
                });
                let test = scope
                    .event
                    .execution_context
                    .test_execution_id
                    .or_else(|| original.map(|owner| owner.0))
                    .ok_or(FixtureError::OwnerMismatch)?;
                let attempt = scope
                    .event
                    .execution_context
                    .attempt_id
                    .or_else(|| original.and_then(|owner| owner.1));
                FixtureOwnerKey::Test { test, attempt }
            }
            FixtureLifetime::File => FixtureOwnerKey::File {
                execution: self.environment.execution,
            },
            FixtureLifetime::Worker => FixtureOwnerKey::Worker {
                invocation: self.invocation,
                worker: self.environment.worker,
            },
            FixtureLifetime::Suite => FixtureOwnerKey::Suite {
                invocation: self.invocation,
            },
        };
        let context = if lifetime == FixtureLifetime::Test {
            scope
                .fixture_boundary
                .clone()
                .or_else(|| self.test_owner.as_ref().map(|owner| owner.context.clone()))
        } else {
            self.contexts.get(&key).cloned()
        }
        .ok_or(FixtureError::OwnerMismatch)?;
        Ok(FixtureOwner {
            key,
            context,
            environment: self.environment,
        })
    }
    pub(crate) fn shared_contexts(
        invocation: ExecutionId,
        environments: &[FixtureRequestEnvironment],
        ids: &scopes::ExecutionIds,
    ) -> Arc<BTreeMap<FixtureOwnerKey, ScopeContext>> {
        let mut keys = BTreeSet::from([FixtureOwnerKey::Suite { invocation }]);
        for environment in environments {
            keys.insert(FixtureOwnerKey::File {
                execution: environment.execution,
            });
            keys.insert(FixtureOwnerKey::Worker {
                invocation,
                worker: environment.worker,
            });
        }
        Arc::new(
            keys.into_iter()
                .map(|key| {
                    (
                        key,
                        ScopeContext {
                            scope_id: ExecutionScopeId(ids.next()),
                            cancellation: crate::CancellationToken::default(),
                            deadline: None,
                            deadline_scope_id: None,
                        },
                    )
                })
                .collect(),
        )
    }
    pub(super) async fn acquire(
        &self,
        plan: &TestPlan,
        call: &webtest_plan::FixtureAcquire,
        scope: &scopes::ExecutionScope,
        state: &state::TestExecutionState,
    ) -> Result<FixtureValue, FixtureAcquisitionError> {
        let definition = plan
            .fixtures
            .iter()
            .find(|definition| definition.id == call.fixture)
            .ok_or(FixtureAcquisitionError::Fixture(
                FixtureError::InvalidDefinition,
            ))?;
        let mut arguments = BTreeMap::new();
        for argument in &call.arguments {
            arguments.insert(
                argument.name.clone(),
                crate::evaluation::evaluate(&argument.value, state.environment())
                    .map_err(FixtureAcquisitionError::Argument)?,
            );
        }
        let owner = self
            .owner(call.lifetime, scope)
            .map_err(FixtureAcquisitionError::Fixture)?;
        let (fields, secrets) = state.redaction();
        self.client
            .acquire_with_metadata(
                Arc::new(definition.clone()),
                owner,
                arguments,
                secrets.to_vec(),
                fields.to_vec(),
            )
            .await
            .map_err(FixtureAcquisitionError::Fixture)
    }
}
pub(super) enum FixtureAcquisitionError {
    Argument(crate::StepError),
    Fixture(FixtureError),
}

pub(super) struct FixtureReadiness {
    value: webtest_model::Value,
    secrets: Vec<String>,
    redacted_fields: Vec<String>,
}
pub(super) struct FixtureBody<'a> {
    definition: &'a webtest_plan::FixtureDefinition,
    ready: tokio::sync::oneshot::Sender<FixtureReadiness>,
    close: tokio::sync::oneshot::Receiver<()>,
}
pub(super) enum BrowserBody<'a> {
    Actor {
        actor: webtest_model::ActorId,
        sources: &'a [webtest_model::EventSource],
        name: &'a str,
        body: &'a webtest_plan::PlanNode,
    },
    Node(&'a webtest_plan::PlanNode),
    Fixture(FixtureBody<'a>),
}

impl TreeExecution<'_, '_> {
    pub(super) async fn fixture_lifetime_body(
        &mut self,
        body: FixtureBody<'_>,
        root: &scopes::ExecutionScope,
    ) -> TestBodyOutcome {
        let setup = self
            .fixture_phase_body(
                &body.definition.setup,
                webtest_observation::FixturePhase::Setup,
                root,
            )
            .await;
        if !matches!(setup, TestOutcome::Passed) {
            root.context.cancellation.start_cleanup_at(Instant::now());
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(Box::new(
                setup,
            )));
        }
        if let Some(cause) = root.context.cancellation.cause() {
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                reason: cause.reason,
            });
        }
        let Some(value) = self.branch.provided.take() else {
            root.context.cancellation.start_cleanup_at(Instant::now());
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                failure: RunError::Internal(
                    "fixture setup completed without its planned result".into(),
                ),
            });
        };
        let (fields, secrets) = self.branch.bindings.redaction();
        if body
            .ready
            .send(FixtureReadiness {
                value,
                secrets: secrets.to_vec(),
                redacted_fields: fields.to_vec(),
            })
            .is_err()
        {
            root.context.cancellation.start_cleanup_at(Instant::now());
            return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                failure: RunError::Internal("fixture readiness receiver disappeared".into()),
            });
        }
        tokio::select! { biased; _ = root.context.cancellation.cancelled() => {}, _ = body.close => {} }
        root.context.cancellation.start_cleanup_at(Instant::now());
        let teardown = self
            .fixture_phase_body(
                &body.definition.teardown,
                webtest_observation::FixturePhase::Teardown,
                root,
            )
            .await;
        if !matches!(teardown, TestOutcome::Passed) {
            self.branch.cleanup_failures.push(fixture_phase_cleanup(
                self.services.plan,
                body.definition,
                root,
                teardown,
            ));
        }
        TestBodyOutcome::Provisional(ProvisionalTestOutcome::Passed)
    }

    async fn fixture_phase_body(
        &mut self,
        node: &webtest_plan::PlanNode,
        phase: webtest_observation::FixturePhase,
        root: &scopes::ExecutionScope,
    ) -> TestOutcome {
        let services = self.services;
        let timeout = if phase == webtest_observation::FixturePhase::Teardown {
            services.options.cleanup_timeout
        } else {
            services.options.test_timeout
        };
        let at = if phase == webtest_observation::FixturePhase::Teardown {
            root.context.cancellation.cleanup_deadline(timeout)
        } else {
            Instant::now() + timeout
        };
        let scope = services.scopes.fixture_phase(root, node, phase, at);
        let deadline = TestDeadline { at };
        let phase_services = ExecutionServices {
            deadline: &deadline,
            ..*services
        };
        self.branch.primary_failure = None;
        self.branch.active_step = None;
        self.branch.active_operation = None;
        let cleanup_start = self.branch.cleanup_failures.len();
        self.branch.scopes.start(
            &scope,
            services.execution_id,
            services.events,
            services.event_sink,
        );
        let body = {
            let mut tree = TreeExecution {
                services: &phase_services,
                branch: self.branch,
            };
            let mut source = wait::TestBodyWait::new(tree.body(node, &scope));
            let mut result = services
                .waits
                .wait_observed(
                    &scope.context,
                    &mut source,
                    services.options.cleanup_timeout,
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
            if let Some(primary) = source.interrupted_failure.take() {
                result.primary = crate::WaitCompletion::Ready(primary);
            }
            drop(source);
            tree.branch
                .cleanup_failures
                .extend(result.secondary.into_iter().map(|failure| CleanupFailure {
                    resource: CleanupResource::ExecutionScope {
                        scope_id: scope.id(),
                    },
                    cause: match failure {
                        crate::WaitCleanupFailure::Failed { error, .. } => error,
                        crate::WaitCleanupFailure::TimedOut { .. } => CleanupCause::TimedOut {
                            timeout_ms: duration_millis(services.options.cleanup_timeout),
                        },
                    },
                }));
            match result.primary {
                crate::WaitCompletion::Ready(body) => body,
                crate::WaitCompletion::Cancelled(cause)
                    if cause.reason == CancellationReason::Timeout
                        && cause.causing_scope_id == scope.id() =>
                {
                    TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
                        timeout,
                        active_step: tree.branch.active_step,
                        origin: tree
                            .branch
                            .active_operation
                            .as_ref()
                            .map(|operation| operation.origin),
                    })
                }
                crate::WaitCompletion::Cancelled(cause) => {
                    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                        reason: cause.reason,
                    })
                }
                crate::WaitCompletion::Failed(error) => {
                    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                        failure: RunError::Cleanup(CleanupFailure {
                            resource: CleanupResource::ExecutionScope {
                                scope_id: scope.id(),
                            },
                            cause: error,
                        }),
                    })
                }
                crate::WaitCompletion::Rejected(error) => {
                    TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                        failure: RunError::Internal(format!(
                            "fixture phase wait was rejected: {error:?}"
                        )),
                    })
                }
            }
        };
        let body = self.branch.primary_failure.take().unwrap_or(body);
        let provisional = child::finalize_body(
            &phase_services,
            body,
            &scope.event,
            self.branch.active_operation.as_ref(),
        )
        .await;
        let owned = self.branch.scopes.subtree_ids(&scope);
        self.branch.cleanup_failures.extend(
            self.branch
                .temporary
                .release(
                    Some(&owned),
                    &mut self.branch.bindings,
                    crate::cleanup::CleanupDeadline::at(
                        scope
                            .context
                            .cancellation
                            .cleanup_deadline(services.options.cleanup_timeout),
                        services.options.cleanup_timeout,
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
        let cleanup = self.branch.cleanup_failures.split_off(cleanup_start);
        let outcome = combine_test_outcome(provisional, cleanup);
        emit_scope_aborted(
            &scope.event,
            &outcome,
            services.execution_id,
            services.events,
            services.event_sink,
        );
        self.branch.scopes.finish_descendants(
            &scope,
            services.execution_id,
            services.events,
            services.event_sink,
        );
        self.branch.scopes.finish(
            &scope,
            scope_outcome(&outcome),
            services.execution_id,
            services.events,
            services.event_sink,
        );
        outcome
    }
}

fn fixture_phase_cleanup(
    plan: &TestPlan,
    definition: &webtest_plan::FixtureDefinition,
    root: &scopes::ExecutionScope,
    outcome: TestOutcome,
) -> CleanupFailure {
    let Some(context) = root.event.execution_context.fixture else {
        return CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: root.id(),
            },
            cause: CleanupCause::Internal {
                message: "fixture cleanup has no fixture occurrence".into(),
            },
        };
    };
    let (origin, cause) = match outcome {
        TestOutcome::Failed(failure) => (
            failure.step.origin,
            CleanupCause::Runtime {
                failure: Box::new(failure::runtime_failure(&failure.error)),
                failure_class: failure.error.failure_class(),
            },
        ),
        TestOutcome::TimedOut {
            timeout,
            active_step,
        } => (
            active_step
                .and_then(|id| {
                    definition
                        .teardown
                        .steps()
                        .into_iter()
                        .find(|step| step.id == id)
                })
                .map_or(definition.teardown.origin, |step| step.origin),
            CleanupCause::TimedOut {
                timeout_ms: duration_millis(timeout),
            },
        ),
        TestOutcome::Aborted { failure, .. } => (
            definition.teardown.origin,
            CleanupCause::Execution {
                failure: Box::new(failure.to_execution_failure()),
                failure_class: failure.failure_class(),
            },
        ),
        _ => (
            definition.teardown.origin,
            CleanupCause::Internal {
                message: "fixture teardown did not complete".into(),
            },
        ),
    };
    let Some(revision) = plan.source_revision_for(origin.file) else {
        return CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: root.id(),
            },
            cause: CleanupCause::Internal {
                message: "fixture cleanup source is absent from its plan".into(),
            },
        };
    };
    CleanupFailure {
        resource: CleanupResource::Fixture(Box::new(webtest_observation::FixtureCleanup {
            fixture: definition.id,
            instance: context.instance,
            phase: webtest_observation::FixturePhase::Teardown,
            origin,
            revision,
        })),
        cause,
    }
}

/// Native composition supplies stable per-file/worker services. The coordinator
/// owns setup and leases; no detached task is used for an idle fixture lifetime.
pub(crate) struct FixtureEnvironment<'a> {
    pub environment: FixtureRequestEnvironment,
    pub plan: &'a TestPlan,
    pub events: &'a EventBuffer,
    pub sink: Option<&'a dyn RunEventSink>,
    pub browser: &'a dyn BrowserHost,
    pub control: Option<&'a dyn RunControl>,
    pub options: &'a RunnerOptions,
    pub providers: &'a ProviderRegistry,
}
pub(crate) struct NativeFixtureDriver<'a> {
    pub environments: Vec<FixtureEnvironment<'a>>,
    pub invocation: ExecutionId,
    pub contexts: Arc<BTreeMap<FixtureOwnerKey, ScopeContext>>,
    pub ids: scopes::ExecutionIds,
    pub client: std::sync::OnceLock<FixtureClient>,
    pub resources: crate::ResourceRegistry,
    pub waits: crate::WaitRegistry,
}
struct LifetimeCompletion {
    outcome: TestOutcome,
    cleanup: Vec<FixtureError>,
}
struct NativeFixtureLease<'a> {
    close: Option<tokio::sync::oneshot::Sender<()>>,
    lifetime: futures::future::BoxFuture<'a, LifetimeCompletion>,
}
impl<'a> crate::fixtures::FixtureLease<'a> for NativeFixtureLease<'a> {
    fn teardown(mut self: Box<Self>) -> futures::future::BoxFuture<'a, Vec<FixtureError>> {
        Box::pin(async move {
            if let Some(close) = self.close.take() {
                let _ = close.send(());
            }
            self.lifetime.await.cleanup
        })
    }
}
impl crate::fixtures::FixtureDriver for NativeFixtureDriver<'_> {
    fn setup<'a>(
        &'a self,
        request: crate::fixtures::FixtureSetup,
    ) -> futures::future::BoxFuture<'a, Result<crate::fixtures::FixtureReady<'a>, FixtureError>>
    {
        Box::pin(async move {
            let (ready, mut receive) = tokio::sync::oneshot::channel();
            let (close, closed) = tokio::sync::oneshot::channel();
            let definition = request.definition.clone();
            let instance = request.instance;
            let mut lifetime = Box::pin(self.run_lifetime(request, ready, closed));
            let completion = tokio::select! { biased;
                completion = &mut lifetime => completion,
                value = &mut receive => match value {
                    Ok(value) => return Ok(crate::fixtures::FixtureReady {
                        value: value.value, secrets: value.secrets, redacted_fields: value.redacted_fields,
                        lease: Box::new(NativeFixtureLease { close: Some(close), lifetime }),
                    }),
                    Err(_) => lifetime.await,
                },
            };
            let (origin, artifacts) = match &completion.outcome {
                TestOutcome::Failed(failure) => (failure.step.origin, failure.artifacts.clone()),
                TestOutcome::TimedOut { active_step, .. } => (
                    active_step
                        .and_then(|id| {
                            definition
                                .setup
                                .steps()
                                .into_iter()
                                .find(|step| step.id == id)
                        })
                        .map_or(definition.setup.origin, |step| step.origin),
                    Vec::new(),
                ),
                _ => (definition.setup.origin, Vec::new()),
            };
            let revision = self
                .environments
                .iter()
                .find_map(|environment| environment.plan.source_revision_for(origin.file))
                .ok_or(FixtureError::InvalidDefinition)?;
            let primary = FixtureError::Phase {
                fixture: definition.id,
                instance,
                phase: webtest_observation::FixturePhase::Setup,
                origin,
                revision,
                artifacts,
                cause: Box::new(FixtureError::Outcome(Box::new(completion.outcome))),
            };
            if completion.cleanup.is_empty() {
                Err(primary)
            } else {
                Err(FixtureError::AcquisitionFailed {
                    primary: Box::new(primary),
                    cleanup: completion.cleanup,
                })
            }
        })
    }
}
impl NativeFixtureDriver<'_> {
    async fn run_lifetime(
        &self,
        request: crate::fixtures::FixtureSetup,
        ready: tokio::sync::oneshot::Sender<FixtureReadiness>,
        close: tokio::sync::oneshot::Receiver<()>,
    ) -> LifetimeCompletion {
        let Some(environment) = self.environments.iter().find(|environment| {
            environment.environment.execution == request.owner.environment.execution
                && environment.environment.worker == request.owner.environment.worker
        }) else {
            return LifetimeCompletion {
                outcome: TestOutcome::Aborted {
                    failure: RunError::Internal("fixture execution environment is absent".into()),
                    prior_outcome: None,
                },
                cleanup: Vec::new(),
            };
        };
        let Some(client) = self.client.get() else {
            return LifetimeCompletion {
                outcome: TestOutcome::Aborted {
                    failure: RunError::Internal("fixture driver has no coordinator".into()),
                    prior_outcome: None,
                },
                cleanup: Vec::new(),
            };
        };
        let context = webtest_observation::FixtureExecutionContext {
            fixture: request.definition.id,
            instance: request.instance,
            owner: request.owner.key,
            phase: webtest_observation::FixturePhase::Lifetime,
        };
        let scopes = scopes::ScopeFactory::for_fixture(self.ids.clone(), context);
        let node = request.definition.lifetime_node();
        let root = scopes.fixture_root(
            &node,
            &request.owner.context,
            request.cancellation,
            &request.definition.name,
        );
        let access = FixtureAccess {
            client: client.clone(),
            invocation: self.invocation,
            environment: environment.environment,
            contexts: self.contexts.clone(),
            test_owner: Some(request.owner),
        };
        let observations = ObservationStore::default();
        let deadline = TestDeadline::new(environment.options.test_timeout);
        let control = FixtureControl(environment.control);
        let services = ExecutionServices {
            plan: environment.plan,
            observations: &observations,
            test: None,
            fixture: Some(&request.definition),
            fixture_access: Some(&access),
            execution_id: environment.environment.execution,
            events: environment.events,
            event_sink: environment.sink,
            control: Some(&control),
            options: environment.options,
            providers: environment.providers,
            deadline: &deadline,
            waits: &self.waits,
            resources: &self.resources,
            scopes: &scopes,
            browser: environment.browser,
        };
        let _debug = crate::control::DebugTaskGuard::new(
            Some(&control),
            None,
            Some(&request.definition),
            root.debug_context(services.execution_id),
        );
        let _journal = environment.events.register_root(
            &root.context,
            &root.event,
            services.execution_id,
            environment.sink,
        );
        let mut branch = branch::BranchState::new(environment.options);
        branch
            .bindings
            .seed_metadata(request.secrets, request.redacted_fields);
        for parameter in &request.definition.parameters {
            if let Some(value) = request.arguments.get(&parameter.name) {
                branch
                    .bindings
                    .bind(parameter.binding, Some(&parameter.name), value.clone());
            }
        }
        branch.session_owner = Some(root.event.clone());
        branch.scopes.start(
            &root,
            services.execution_id,
            services.events,
            services.event_sink,
        );
        let body = FixtureBody {
            definition: &request.definition,
            ready,
            close,
        };
        let result = {
            let mut tree = TreeExecution {
                services: &services,
                branch: &mut branch,
            };
            if node
                .required_resources()
                .iter()
                .any(|usage| usage.resource == webtest_plan::ResourceReference::BrowserContext)
            {
                tree.browser_resource_body(BrowserBody::Fixture(body), &root)
                    .await
            } else {
                tree.fixture_lifetime_body(body, &root).await
            }
        };
        let provisional = child::finalize_body(
            &services,
            result,
            &root.event,
            branch.active_operation.as_ref(),
        )
        .await;
        root.context.cancellation.start_cleanup_at(Instant::now());
        drop(branch.page.take());
        let cleanup = crate::cleanup::CleanupDeadline::at(
            root.context
                .cancellation
                .cleanup_deadline(environment.options.cleanup_timeout),
            environment.options.cleanup_timeout,
        );
        let events = temporary::ResourceEvents {
            registry: &self.resources,
            execution_id: services.execution_id,
            events: services.events,
            sink: services.event_sink,
        };
        branch.cleanup_failures.extend(
            branch
                .temporary
                .release(None, &mut branch.bindings, cleanup, &events)
                .await,
        );
        if let Some(mut session) = branch.session.take()
            && let Err(error) = session
                .close(cleanup, &events, root.context.cancellation.cause())
                .await
        {
            branch.cleanup_failures.push(error);
        }
        if let Err(error) = self.resources.validate_owner_finished(root.id()) {
            branch
                .cleanup_failures
                .push(resource_cleanup_invariant(error));
        }
        if let Err(error) = self.waits.validate_owner_finished(root.id()) {
            branch.cleanup_failures.push(CleanupFailure {
                resource: CleanupResource::ExecutionScope {
                    scope_id: root.id(),
                },
                cause: CleanupCause::Internal {
                    message: format!("fixture wait ownership invariant: {error:?}"),
                },
            });
        }
        let (_, secrets) = branch.bindings.redaction();
        let failures = branch
            .cleanup_failures
            .into_iter()
            .map(|error| {
                crate::redaction::redact_cleanup_failure(
                    error,
                    secrets,
                    &environment.options.inspection.redacted_query_parameters,
                )
            })
            .collect::<Vec<_>>();
        for error in &failures {
            emit_cleanup_failed(
                services.events,
                services.event_sink,
                services.execution_id,
                None,
                Some(&root.event),
                error,
            );
        }
        let outcome = combine_test_outcome(provisional, Vec::new());
        let outcome = redact_aborted_outcome(
            outcome,
            secrets,
            &environment.options.inspection.redacted_query_parameters,
        );
        let terminal = if failures.is_empty() {
            scope_outcome(&outcome)
        } else {
            webtest_observation::ScopeOutcome::Aborted
        };
        emit_scope_aborted(
            &root.event,
            &combine_test_outcome(
                ProvisionalTestOutcome::Finalized(Box::new(outcome.clone())),
                failures.clone(),
            ),
            services.execution_id,
            services.events,
            services.event_sink,
        );
        branch.scopes.finish_descendants(
            &root,
            services.execution_id,
            services.events,
            services.event_sink,
        );
        branch.scopes.finish(
            &root,
            terminal,
            services.execution_id,
            services.events,
            services.event_sink,
        );
        LifetimeCompletion {
            outcome,
            cleanup: failures.into_iter().map(FixtureError::Cleanup).collect(),
        }
    }
}

impl FixtureAccess {
    pub(super) async fn close_test(&self, scope: &scopes::ExecutionScope) -> Vec<CleanupFailure> {
        let owner = match self.owner(FixtureLifetime::Test, scope) {
            Ok(owner) => owner,
            Err(error) => return vec![fixture_error_cleanup(scope, error)],
        };
        match self.client.close(owner.key).await {
            Ok(errors) => errors
                .into_iter()
                .map(|error| fixture_error_cleanup(scope, error))
                .collect(),
            Err(error) => vec![fixture_error_cleanup(scope, error)],
        }
    }
}
fn fixture_error_cleanup(scope: &scopes::ExecutionScope, error: FixtureError) -> CleanupFailure {
    match error {
        FixtureError::Cleanup(error) => error,
        FixtureError::Aborted(error) => CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: scope.id(),
            },
            cause: CleanupCause::Execution {
                failure_class: error.failure_class(),
                failure: Box::new(error.to_execution_failure()),
            },
        },
        _ => CleanupFailure {
            resource: CleanupResource::ExecutionScope {
                scope_id: scope.id(),
            },
            cause: CleanupCause::Internal {
                message: error.to_string(),
            },
        },
    }
}
pub(super) fn acquisition_failure(
    plan: &TestPlan,
    error: FixtureError,
    step: &PlannedStep,
    scope: &scopes::ExecutionScope,
    cleanup: &mut Vec<CleanupFailure>,
) -> TestBodyOutcome {
    match error {
        FixtureError::AcquisitionFailed {
            primary,
            cleanup: errors,
        } => {
            cleanup.extend(
                errors
                    .into_iter()
                    .map(|error| fixture_error_cleanup(scope, error)),
            );
            acquisition_failure(plan, *primary, step, scope, cleanup)
        }
        FixtureError::Phase {
            origin,
            revision,
            cause,
            ..
        } => {
            let Some(source) = plan
                .source_files
                .iter()
                .find(|source| source.file == origin.file && source.revision == revision)
            else {
                return TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
                    failure: RunError::Internal("fixture failure source is absent".into()),
                });
            };
            if let FixtureError::Outcome(outcome) = *cause {
                match *outcome {
                    TestOutcome::Failed(failure) => {
                        TestBodyOutcome::PendingFailure(Box::new(PendingFailure::from_fixture(
                            step,
                            &scope.event,
                            *failure,
                            webtest_text::SourceLocation {
                                path: source.path.clone(),
                                origin,
                                revision,
                            },
                        )))
                    }
                    TestOutcome::TimedOut { timeout, .. } => {
                        TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
                            timeout,
                            active_step: Some(step.id),
                            origin: Some(origin),
                        })
                    }
                    outcome => TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(
                        Box::new(outcome),
                    )),
                }
            } else {
                acquisition_failure(plan, *cause, step, scope, cleanup)
            }
        }
        FixtureError::Setup(error) => TestBodyOutcome::PendingFailure(Box::new(
            PendingFailure::primary(step, &scope.event, error, 0),
        )),
        FixtureError::Cancelled(cause) => {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Cancelled {
                reason: cause.reason,
            })
        }
        FixtureError::Aborted(failure) => {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted { failure })
        }
        FixtureError::Outcome(outcome) => {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::Finalized(outcome))
        }
        FixtureError::TimedOut { timeout_ms } => {
            TestBodyOutcome::Provisional(ProvisionalTestOutcome::TimedOut {
                timeout: Duration::from_millis(timeout_ms),
                active_step: Some(step.id),
                origin: Some(step.origin),
            })
        }
        error => TestBodyOutcome::Provisional(ProvisionalTestOutcome::Aborted {
            failure: RunError::Internal(error.to_string()),
        }),
    }
}

/// Lifetime ownership controls cancellation. A cancelled consumer must not cancel
/// a shared setup that other test roots are still waiting for.
struct FixtureControl<'a>(Option<&'a dyn RunControl>);
#[async_trait::async_trait]
impl RunControl for FixtureControl<'_> {
    async fn before_step(&self, _: &PlannedTest, _: &PlannedStep) {}
    fn fixture_task_started(
        &self,
        fixture: &webtest_plan::FixtureDefinition,
        context: &crate::DebugContext,
    ) {
        if let Some(control) = self.0 {
            control.fixture_task_started(fixture, context);
        }
    }
    fn task_finished(&self, context: &crate::DebugContext) {
        if let Some(control) = self.0 {
            control.task_finished(context);
        }
    }
    fn should_capture_fixture_bindings(
        &self,
        fixture: &webtest_plan::FixtureDefinition,
        step: &PlannedStep,
    ) -> bool {
        self.0
            .is_some_and(|control| control.should_capture_fixture_bindings(fixture, step))
    }
    async fn before_fixture_step(
        &self,
        fixture: &webtest_plan::FixtureDefinition,
        step: &PlannedStep,
        context: &crate::DebugContext,
        bindings: Option<BTreeMap<String, webtest_model::Value>>,
    ) {
        if let Some(control) = self.0 {
            control
                .before_fixture_step(fixture, step, context, bindings)
                .await;
        }
    }
    async fn after_fixture_step_failure(
        &self,
        fixture: &webtest_plan::FixtureDefinition,
        step: &PlannedStep,
        context: &crate::DebugContext,
        error: &crate::StepError,
        bindings: &BTreeMap<String, webtest_model::Value>,
    ) {
        if let Some(control) = self.0 {
            control
                .after_fixture_step_failure(fixture, step, context, error, bindings)
                .await;
        }
    }
}
