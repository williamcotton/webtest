//! Owned fixture acquisition and lifetime closure. Waiting clients never own setup futures.
use crate::{CancellationToken, ScopeContext, StepError};
use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::{mpsc, oneshot};
use webtest_model::{FixtureId, FixtureInstanceId, FixtureLifetime, Value};
use webtest_observation::CleanupFailure;
use webtest_plan::FixtureDefinition;

pub const MAX_FIXTURE_INSTANCES: usize = 16_384;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_RESULT_BYTES: usize = 1024 * 1024;

pub use webtest_observation::FixtureOwnerKey;
#[derive(Clone, Copy, Debug)]
pub struct FixtureRequestEnvironment {
    pub execution: webtest_observation::ExecutionId,
    pub worker: u32,
}
#[derive(Clone, Debug)]
pub struct FixtureOwner {
    pub environment: FixtureRequestEnvironment,
    pub key: FixtureOwnerKey,
    pub context: ScopeContext,
}
/// Redaction metadata travels with the cached value, including into later consumers.
#[derive(Clone)]
pub struct FixtureValue {
    pub owner: FixtureOwnerKey,
    pub acquisition_execution_id: webtest_observation::ExecutionId,
    pub instance: FixtureInstanceId,
    pub value: Value,
    pub secrets: Vec<String>,
    pub redacted_fields: Vec<String>,
}
#[derive(Clone, Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("fixture definition is invalid")]
    InvalidDefinition,
    #[error("fixture definition differs from the first acquisition")]
    DefinitionMismatch,
    #[error("fixture arguments violate their declared types or byte budget")]
    InvalidArguments,
    #[error("fixture result violates its declared type or byte budget")]
    InvalidResult,
    #[error(transparent)]
    BudgetExceeded(webtest_observation::FixtureBudgetExceeded),
    #[error("fixture lifetime has ended")]
    OwnerClosed,
    #[error("fixture lifetime owner does not match its declaration")]
    OwnerMismatch,
    #[error("fixture coordinator has stopped")]
    CoordinatorStopped,
    #[error("fixture acquisition was cancelled: {0:?}")]
    Cancelled(webtest_host::Cancellation),
    #[error("fixture {fixture} {phase:?} failed: {cause}")]
    Phase {
        fixture: FixtureId,
        instance: FixtureInstanceId,
        phase: webtest_observation::FixturePhase,
        origin: webtest_text::SyntaxOrigin,
        revision: webtest_text::SourceRevision,
        cause: Box<FixtureError>,
        artifacts: Vec<webtest_observation::Artifact>,
    },
    #[error("fixture execution aborted: {0}")]
    Aborted(crate::RunError),
    #[error("fixture execution timed out after {timeout_ms}ms")]
    TimedOut { timeout_ms: u64 },
    #[error("fixture phase did not complete successfully")]
    Outcome(Box<crate::TestOutcome>),
    #[error("fixture setup failed: {0}")]
    Setup(StepError),
    #[error("fixture acquisition failed: {primary}; cleanup: {cleanup:?}")]
    AcquisitionFailed {
        primary: Box<FixtureError>,
        cleanup: Vec<FixtureError>,
    },
    #[error("fixture teardown failed: {0:?}")]
    Cleanup(CleanupFailure),
}
pub struct FixtureSetup {
    pub definition: Arc<FixtureDefinition>,
    pub owner: FixtureOwner,
    pub instance: FixtureInstanceId,
    pub arguments: BTreeMap<String, Value>,
    pub secrets: Vec<String>,
    pub redacted_fields: Vec<String>,
    pub cancellation: CancellationToken,
}
pub struct FixtureReady<'a> {
    pub value: Value,
    pub secrets: Vec<String>,
    pub redacted_fields: Vec<String>,
    pub lease: Box<dyn FixtureLease<'a> + 'a>,
}
/// Implementations retain setup locals and native ownership until this terminal future ends.
pub trait FixtureLease<'a>: Send {
    fn teardown(self: Box<Self>) -> BoxFuture<'a, Vec<FixtureError>>;
}
/// A failed setup must finish its partial cleanup before returning an error.
pub trait FixtureDriver: Send + Sync {
    fn setup<'a>(
        &'a self,
        request: FixtureSetup,
    ) -> BoxFuture<'a, Result<FixtureReady<'a>, FixtureError>>;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    fixture: FixtureId,
    owner: FixtureOwnerKey,
    arguments: [u8; 32],
}
enum Command {
    Acquire {
        definition: Arc<FixtureDefinition>,
        owner: FixtureOwner,
        arguments: BTreeMap<String, Value>,
        secrets: Vec<String>,
        redacted_fields: Vec<String>,
        reply: oneshot::Sender<Result<FixtureValue, FixtureError>>,
    },
    Close {
        owner: FixtureOwnerKey,
        reply: oneshot::Sender<Vec<FixtureError>>,
    },
}
#[derive(Clone)]
pub struct FixtureClient {
    sender: mpsc::Sender<Command>,
}
impl FixtureClient {
    pub async fn acquire(
        &self,
        definition: Arc<FixtureDefinition>,
        owner: FixtureOwner,
        arguments: BTreeMap<String, Value>,
    ) -> Result<FixtureValue, FixtureError> {
        self.acquire_with_metadata(definition, owner, arguments, Vec::new(), Vec::new())
            .await
    }
    pub async fn acquire_with_metadata(
        &self,
        definition: Arc<FixtureDefinition>,
        owner: FixtureOwner,
        arguments: BTreeMap<String, Value>,
        secrets: Vec<String>,
        redacted_fields: Vec<String>,
    ) -> Result<FixtureValue, FixtureError> {
        let (reply, receive) = oneshot::channel();
        self.sender
            .send(Command::Acquire {
                definition,
                owner,
                arguments,
                secrets,
                redacted_fields,
                reply,
            })
            .await
            .map_err(|_| FixtureError::CoordinatorStopped)?;
        receive
            .await
            .map_err(|_| FixtureError::CoordinatorStopped)?
    }
    /// Closure is acknowledged only after pending setup and reverse teardown finish.
    pub async fn close(&self, owner: FixtureOwnerKey) -> Result<Vec<FixtureError>, FixtureError> {
        let (reply, receive) = oneshot::channel();
        self.sender
            .send(Command::Close { owner, reply })
            .await
            .map_err(|_| FixtureError::CoordinatorStopped)?;
        receive.await.map_err(|_| FixtureError::CoordinatorStopped)
    }
}
enum Phase<'a> {
    Pending,
    Ready {
        value: FixtureValue,
        lease: Box<dyn FixtureLease<'a> + 'a>,
    },
    Failed(FixtureError),
    TearingDown {
        failure: Option<FixtureError>,
    },
    Closed,
}
struct Entry<'a> {
    definition: Arc<FixtureDefinition>,
    owner: FixtureOwner,
    instance: FixtureInstanceId,
    cancellation: CancellationToken,
    waiters: Vec<oneshot::Sender<Result<FixtureValue, FixtureError>>>,
    phase: Phase<'a>,
    cleanup_errors: Vec<FixtureError>,
}
enum Finished<'a> {
    Setup(Key, Result<FixtureReady<'a>, FixtureError>),
    Teardown(Key, Vec<FixtureError>),
}
struct Closing {
    replies: Vec<oneshot::Sender<Vec<FixtureError>>>,
    errors: Vec<FixtureError>,
}
pub struct FixtureCoordinator<'a> {
    driver: &'a dyn FixtureDriver,
    receive: mpsc::Receiver<Command>,
    entries: HashMap<Key, Entry<'a>>,
    closing: BTreeMap<FixtureOwnerKey, Closing>,
    closed: BTreeMap<FixtureOwnerKey, Vec<FixtureError>>,
    readiness: Vec<Key>,
    active: FuturesUnordered<BoxFuture<'a, Finished<'a>>>,
    next_instance: u64,
}
impl<'a> FixtureCoordinator<'a> {
    pub fn new(driver: &'a dyn FixtureDriver) -> (FixtureClient, Self) {
        let (sender, receive) = mpsc::channel(256);
        (
            FixtureClient { sender },
            Self {
                driver,
                receive,
                entries: HashMap::new(),
                closing: BTreeMap::new(),
                closed: BTreeMap::new(),
                readiness: Vec::new(),
                active: FuturesUnordered::new(),
                next_instance: 0,
            },
        )
    }
    /// Every setup/teardown future stays in this structured owner, even when a
    /// request future is dropped. The root must await explicit lifetime closure
    /// before emitting that owner's terminal fact. A final drain protects exits.
    pub async fn drive<R>(
        mut self,
        root: impl std::future::Future<Output = R>,
    ) -> (R, Vec<FixtureError>) {
        let mut root = std::pin::pin!(root);
        let mut receiving = true;
        let result = loop {
            tokio::select! {
                command = self.receive.recv(), if receiving => { if let Some(command) = command { self.command(command); } else { receiving = false; } }
                Some(finished) = self.active.next(), if !self.active.is_empty() => self.finished(finished),
                result = &mut root => break result,
            }
            self.advance_closures();
        };
        let acknowledged = self.closed.keys().copied().collect::<HashSet<_>>();
        // Short lifetimes close first: a longer-lived fixture cannot depend on
        // a shorter-lived one. Successful dependencies close in reverse readiness.
        let mut owners = self
            .entries
            .keys()
            .map(|key| key.owner)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        owners.sort_by_key(|owner| (owner.lifetime().rank(), *owner));
        for owner in owners {
            self.begin_close(owner, None);
            while self.closing.contains_key(&owner) {
                self.advance_closures();
                if !self.closing.contains_key(&owner) {
                    break;
                }
                tokio::select! {
                    Some(finished) = self.active.next(), if !self.active.is_empty() => self.finished(finished),
                    Some(command) = self.receive.recv() => match command {
                        Command::Acquire { reply, .. } => { let _ = reply.send(Err(FixtureError::OwnerClosed)); }
                        Command::Close { owner, reply } => self.begin_close(owner, Some(reply)),
                    },
                }
            }
        }
        let errors = self
            .closed
            .into_iter()
            .filter(|(owner, _)| !acknowledged.contains(owner))
            .flat_map(|(_, errors)| errors)
            .collect();
        (result, errors)
    }
    fn command(&mut self, command: Command) {
        match command {
            Command::Close { owner, reply } => self.begin_close(owner, Some(reply)),
            Command::Acquire {
                definition,
                owner,
                arguments,
                secrets,
                redacted_fields,
                reply,
            } => {
                let validation = self.validate_request(&definition, &owner, &arguments);
                let key = match validation {
                    Ok(key) => key,
                    Err(error) => {
                        let _ = reply.send(Err(error));
                        return;
                    }
                };
                if self
                    .entries
                    .values()
                    .map(|entry| entry.waiters.len())
                    .sum::<usize>()
                    >= MAX_FIXTURE_INSTANCES
                {
                    let _ = reply.send(Err(FixtureError::BudgetExceeded(
                        webtest_observation::FixtureBudgetExceeded {
                            budget: webtest_observation::FixtureBudgetKind::Waiters,
                            limit: MAX_FIXTURE_INSTANCES,
                        },
                    )));
                    return;
                }
                if let Some(entry) = self.entries.get_mut(&key) {
                    if *entry.definition != *definition {
                        let _ = reply.send(Err(FixtureError::DefinitionMismatch));
                        return;
                    }
                    if entry.owner.context.scope_id != owner.context.scope_id {
                        let _ = reply.send(Err(FixtureError::OwnerMismatch));
                        return;
                    }
                    match &entry.phase {
                        Phase::Ready { value, .. } => {
                            let _ = reply.send(Ok(value.clone()));
                        }
                        Phase::Failed(error) => {
                            let _ = reply.send(Err(error.clone()));
                        }
                        Phase::Closed => {
                            let _ = reply.send(Err(FixtureError::OwnerClosed));
                        }
                        Phase::Pending | Phase::TearingDown { .. } => entry.waiters.push(reply),
                    }
                    return;
                }
                if self.entries.len() >= MAX_FIXTURE_INSTANCES {
                    let _ = reply.send(Err(FixtureError::BudgetExceeded(
                        webtest_observation::FixtureBudgetExceeded {
                            budget: webtest_observation::FixtureBudgetKind::Instances,
                            limit: MAX_FIXTURE_INSTANCES,
                        },
                    )));
                    return;
                }
                let instance = FixtureInstanceId(self.next_instance);
                self.next_instance += 1;
                let cancellation = owner.context.cancellation.child();
                let request = FixtureSetup {
                    definition: definition.clone(),
                    owner: owner.clone(),
                    instance,
                    arguments,
                    secrets,
                    redacted_fields,
                    cancellation: cancellation.clone(),
                };
                self.entries.insert(
                    key.clone(),
                    Entry {
                        definition,
                        owner,
                        instance,
                        cancellation,
                        waiters: vec![reply],
                        phase: Phase::Pending,
                        cleanup_errors: Vec::new(),
                    },
                );
                let driver = self.driver;
                self.active.push(Box::pin(async move {
                    Finished::Setup(key, driver.setup(request).await)
                }));
            }
        }
    }
    fn validate_request(
        &self,
        definition: &FixtureDefinition,
        owner: &FixtureOwner,
        arguments: &BTreeMap<String, Value>,
    ) -> Result<Key, FixtureError> {
        if self.closing.contains_key(&owner.key) || self.closed.contains_key(&owner.key) {
            return Err(FixtureError::OwnerClosed);
        }
        if definition.lifetime != owner.key.lifetime() {
            return Err(FixtureError::OwnerMismatch);
        }
        definition
            .validate()
            .map_err(|_| FixtureError::InvalidDefinition)?;
        if arguments.len() != definition.parameters.len()
            || definition.parameters.iter().any(|parameter| {
                arguments.get(&parameter.name).is_none_or(|value| {
                    crate::evaluation::decode_value(value, &parameter.ty, &parameter.name, None)
                        .is_err()
                        || (definition.lifetime != FixtureLifetime::Test
                            && !crate::evaluation::runtime_transferable(value))
                })
            })
        {
            return Err(FixtureError::InvalidArguments);
        }
        let bytes = serde_json::to_vec(arguments).map_err(|_| FixtureError::InvalidArguments)?;
        if bytes.len() > MAX_ARGUMENT_BYTES {
            return Err(FixtureError::BudgetExceeded(
                webtest_observation::FixtureBudgetExceeded {
                    budget: webtest_observation::FixtureBudgetKind::ArgumentBytes,
                    limit: MAX_ARGUMENT_BYTES,
                },
            ));
        }
        Ok(Key {
            fixture: definition.id,
            owner: owner.key,
            arguments: *blake3::hash(&bytes).as_bytes(),
        })
    }
    fn begin_close(
        &mut self,
        owner: FixtureOwnerKey,
        reply: Option<oneshot::Sender<Vec<FixtureError>>>,
    ) {
        if let Some(errors) = self.closed.get(&owner) {
            if let Some(reply) = reply {
                let _ = reply.send(errors.clone());
            }
            return;
        }
        for entry in self
            .entries
            .values()
            .filter(|entry| entry.owner.key == owner)
        {
            entry
                .owner
                .context
                .cancellation
                .start_cleanup_at(tokio::time::Instant::now());
        }
        let mut entries = self
            .entries
            .values()
            .filter(|entry| entry.owner.key == owner)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.instance);
        let errors = entries
            .into_iter()
            .flat_map(|entry| entry.cleanup_errors.iter().cloned())
            .collect();
        let closing = self.closing.entry(owner).or_insert_with(|| Closing {
            replies: Vec::new(),
            errors,
        });
        closing.replies.extend(reply);
        for entry in self
            .entries
            .values()
            .filter(|entry| entry.owner.key == owner && matches!(entry.phase, Phase::Pending))
        {
            entry
                .cancellation
                .cancel(entry.owner.context.cancellation.cause().unwrap_or(
                    webtest_host::Cancellation {
                        reason: webtest_host::CancellationReason::RunnerShutdown,
                        causing_scope_id: entry.owner.context.scope_id,
                    },
                ));
        }
    }
    fn finished(&mut self, finished: Finished<'a>) {
        match finished {
            Finished::Setup(key, result) => {
                let Some(entry) = self.entries.get_mut(&key) else {
                    return;
                };
                match result {
                    Err(error) => {
                        for waiter in entry.waiters.drain(..) {
                            let _ = waiter.send(Err(error.clone()));
                        }
                        entry.phase = Phase::Failed(error);
                    }
                    Ok(ready) => {
                        let oversized = serde_json::to_vec(&ready.value)
                            .is_ok_and(|bytes| bytes.len() > MAX_RESULT_BYTES);
                        let valid = crate::evaluation::decode_value(
                            &ready.value,
                            &entry.definition.result_type,
                            "fixture",
                            None,
                        )
                        .is_ok()
                            && (entry.definition.lifetime == FixtureLifetime::Test
                                || crate::evaluation::runtime_transferable(&ready.value))
                            && serde_json::to_vec(&ready.value)
                                .is_ok_and(|bytes| bytes.len() <= MAX_RESULT_BYTES);
                        if !valid || self.closing.contains_key(&key.owner) {
                            entry.phase = Phase::TearingDown {
                                failure: Some(if valid {
                                    FixtureError::OwnerClosed
                                } else if oversized {
                                    FixtureError::BudgetExceeded(
                                        webtest_observation::FixtureBudgetExceeded {
                                            budget:
                                                webtest_observation::FixtureBudgetKind::ResultBytes,
                                            limit: MAX_RESULT_BYTES,
                                        },
                                    )
                                } else {
                                    FixtureError::InvalidResult
                                }),
                            };
                            self.active.push(Box::pin(async move {
                                Finished::Teardown(key, ready.lease.teardown().await)
                            }));
                        } else {
                            let value = FixtureValue {
                                owner: entry.owner.key,
                                acquisition_execution_id: entry.owner.environment.execution,
                                instance: entry.instance,
                                value: ready.value,
                                secrets: ready.secrets,
                                redacted_fields: ready.redacted_fields,
                            };
                            for waiter in entry.waiters.drain(..) {
                                let _ = waiter.send(Ok(value.clone()));
                            }
                            entry.phase = Phase::Ready {
                                value,
                                lease: ready.lease,
                            };
                            self.readiness.push(key);
                        }
                    }
                }
            }
            Finished::Teardown(key, errors) => {
                let Some(entry) = self.entries.get_mut(&key) else {
                    return;
                };
                let phase = std::mem::replace(&mut entry.phase, Phase::Closed);
                if let Phase::TearingDown {
                    failure: Some(error),
                } = phase
                {
                    let error = if errors.is_empty() {
                        error
                    } else {
                        FixtureError::AcquisitionFailed {
                            primary: Box::new(error),
                            cleanup: errors.clone(),
                        }
                    };
                    for waiter in entry.waiters.drain(..) {
                        let _ = waiter.send(Err(error.clone()));
                    }
                    entry.phase = Phase::Failed(error);
                }
                entry.cleanup_errors.extend(errors.iter().cloned());
                if let Some(closing) = self.closing.get_mut(&key.owner) {
                    closing.errors.extend(errors);
                }
            }
        }
    }
    fn advance_closures(&mut self) {
        let owners = self.closing.keys().copied().collect::<Vec<_>>();
        for owner in owners {
            if self.entries.values().any(|entry| {
                entry.owner.key == owner
                    && matches!(entry.phase, Phase::Pending | Phase::TearingDown { .. })
            }) {
                continue;
            }
            let next = self
                .readiness
                .iter()
                .rev()
                .find(|key| {
                    key.owner == owner
                        && self
                            .entries
                            .get(*key)
                            .is_some_and(|entry| matches!(entry.phase, Phase::Ready { .. }))
                })
                .cloned();
            if let Some(key) = next {
                if let Some(entry) = self.entries.get_mut(&key)
                    && let Phase::Ready { lease, .. } =
                        std::mem::replace(&mut entry.phase, Phase::TearingDown { failure: None })
                {
                    self.active.push(Box::pin(async move {
                        Finished::Teardown(key, lease.teardown().await)
                    }));
                }
            } else if let Some(closing) = self.closing.remove(&owner) {
                for reply in closing.replies {
                    let _ = reply.send(closing.errors.clone());
                }
                self.closed.insert(owner, closing.errors);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use webtest_model::{ExecutionScopeId, PlanDeclarationId, StepId, TestExecutionId, Type};
    use webtest_observation::ExecutionId;
    use webtest_plan::{PlanExpr, PlanNode, PlannedStep, ProvideOperation, TestOperation};
    use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange};

    fn definition(id: u8, lifetime: FixtureLifetime) -> Arc<FixtureDefinition> {
        let id = FixtureId([id; 32]);
        let declaration = PlanDeclarationId(id.0);
        let origin = SyntaxOrigin::new(FileId::new(1), TextRange::new(0.into(), 1.into()));
        let revision = SourceRevision::of("fixture");
        Arc::new(FixtureDefinition {
            id,
            name: format!("fixture{}", id.0[0]),
            origin,
            source_revision: revision,
            input_fingerprint: revision,
            lifetime,
            shareable: lifetime != FixtureLifetime::Test,
            parameters: Vec::new(),
            result_type: Type::Int,
            setup: PlanNode::sequence(
                declaration,
                origin,
                revision,
                vec![0],
                vec![PlanNode::operation(
                    declaration,
                    revision,
                    vec![0, 0],
                    PlannedStep {
                        id: StepId(0),
                        origin,
                        origin_chain: Vec::new(),
                        operation: TestOperation::Return(ProvideOperation {
                            expression: PlanExpr::Literal(Value::Int(7)),
                            result_type: Type::Int,
                        }),
                    },
                )],
            ),
            teardown: PlanNode::sequence(declaration, origin, revision, vec![1], Vec::new()),
        })
    }
    fn owner(key: FixtureOwnerKey, scope: u64) -> FixtureOwner {
        FixtureOwner {
            environment: FixtureRequestEnvironment {
                execution: ExecutionId::from_u128(1),
                worker: 0,
            },
            key,
            context: ScopeContext {
                scope_id: ExecutionScopeId(scope),
                cancellation: CancellationToken::default(),
                deadline: None,
                deadline_scope_id: None,
            },
        }
    }
    fn file_owner() -> FixtureOwner {
        owner(
            FixtureOwnerKey::File {
                execution: ExecutionId::from_u128(1),
            },
            10,
        )
    }
    #[derive(Default)]
    struct Driver {
        client: Mutex<Option<FixtureClient>>,
        dependency: Option<Arc<FixtureDefinition>>,
        starts: AtomicUsize,
        entered: tokio::sync::Notify,
        cleaned_partial: AtomicBool,
        slow: bool,
        invalid: bool,
        teardowns: Mutex<Vec<(u8, FixtureInstanceId)>>,
    }
    struct Lease<'a> {
        driver: &'a Driver,
        id: u8,
        instance: FixtureInstanceId,
    }
    impl<'a> FixtureLease<'a> for Lease<'a> {
        fn teardown(self: Box<Self>) -> BoxFuture<'a, Vec<FixtureError>> {
            Box::pin(async move {
                self.driver
                    .teardowns
                    .lock()
                    .unwrap()
                    .push((self.id, self.instance));
                Vec::new()
            })
        }
    }
    impl FixtureDriver for Driver {
        fn setup<'a>(
            &'a self,
            request: FixtureSetup,
        ) -> BoxFuture<'a, Result<FixtureReady<'a>, FixtureError>> {
            Box::pin(async move {
                self.starts.fetch_add(1, Ordering::SeqCst);
                self.entered.notify_one();
                if self.slow {
                    let cause = request.cancellation.cancelled().await;
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    self.cleaned_partial.store(true, Ordering::SeqCst);
                    return Err(FixtureError::Cancelled(cause));
                }
                if request.definition.id.0[0] == 2
                    && let Some(dependency) = &self.dependency
                {
                    let client = self.client.lock().unwrap().clone().unwrap();
                    client
                        .acquire(dependency.clone(), request.owner.clone(), BTreeMap::new())
                        .await?;
                }
                Ok(FixtureReady {
                    value: if self.invalid {
                        Value::String("wrong type".into())
                    } else {
                        Value::Int(7)
                    },
                    secrets: vec!["fixture-secret".into()],
                    redacted_fields: vec!["token".into()],
                    lease: Box::new(Lease {
                        driver: self,
                        id: request.definition.id.0[0],
                        instance: request.instance,
                    }),
                })
            })
        }
    }
    #[tokio::test]
    async fn simultaneous_waiters_share_one_acquisition_and_redaction_and_close_is_idempotent() {
        let driver = Driver::default();
        let (client, coordinator) = FixtureCoordinator::new(&driver);
        let recipe = definition(1, FixtureLifetime::File);
        let owner = file_owner();
        let (_, errors) = coordinator
            .drive(async {
                let (first, second) = tokio::join!(
                    client.acquire(recipe.clone(), owner.clone(), BTreeMap::new()),
                    client.acquire(recipe, owner.clone(), BTreeMap::new())
                );
                let first = first.unwrap();
                let second = second.unwrap();
                assert_eq!(first.instance, second.instance);
                assert_eq!(first.secrets, vec!["fixture-secret"]);
                assert_eq!(second.redacted_fields, vec!["token"]);
                assert!(client.close(owner.key).await.unwrap().is_empty());
                assert!(client.close(owner.key).await.unwrap().is_empty());
            })
            .await;
        assert!(errors.is_empty());
        assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
        assert_eq!(driver.teardowns.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn abandoned_waiter_does_not_drop_setup_and_owner_close_waits_for_partial_cleanup() {
        let driver = Driver {
            slow: true,
            ..Default::default()
        };
        let (client, coordinator) = FixtureCoordinator::new(&driver);
        let owner = file_owner();
        let (_, errors) = coordinator
            .drive(async {
                let mut waiting = Box::pin(client.acquire(
                    definition(1, FixtureLifetime::File),
                    owner.clone(),
                    BTreeMap::new(),
                ));
                tokio::select! {
                    _ = driver.entered.notified() => {},
                    _ = &mut waiting => panic!("setup should still be pending"),
                }
                drop(waiting);
                assert!(!driver.cleaned_partial.load(Ordering::SeqCst));
                assert!(client.close(owner.key).await.unwrap().is_empty());
                assert!(driver.cleaned_partial.load(Ordering::SeqCst));
            })
            .await;
        assert!(errors.is_empty());
        assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn successful_dependencies_teardown_in_reverse_order() {
        let driver = Driver {
            dependency: Some(definition(1, FixtureLifetime::File)),
            ..Default::default()
        };
        let (client, coordinator) = FixtureCoordinator::new(&driver);
        *driver.client.lock().unwrap() = Some(client.clone());
        let owner = file_owner();
        coordinator
            .drive(async {
                client
                    .acquire(
                        definition(2, FixtureLifetime::File),
                        owner.clone(),
                        BTreeMap::new(),
                    )
                    .await
                    .unwrap();
                client.close(owner.key).await.unwrap();
            })
            .await;
        assert_eq!(
            driver
                .teardowns
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
    }
    #[tokio::test]
    async fn rejected_result_is_cleaned_before_failure_and_failure_is_cached() {
        let driver = Driver {
            invalid: true,
            ..Default::default()
        };
        let (client, coordinator) = FixtureCoordinator::new(&driver);
        let recipe = definition(1, FixtureLifetime::File);
        let owner = file_owner();
        coordinator
            .drive(async {
                for _ in 0..2 {
                    assert!(matches!(
                        client
                            .acquire(recipe.clone(), owner.clone(), BTreeMap::new())
                            .await,
                        Err(FixtureError::InvalidResult)
                    ));
                    assert_eq!(driver.teardowns.lock().unwrap().len(), 1);
                }
                client.close(owner.key).await.unwrap();
            })
            .await;
        assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
        assert_eq!(driver.teardowns.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn four_lifetimes_have_distinct_cache_owners_and_final_drain_joins_remaining_leases() {
        let driver = Driver::default();
        let (client, coordinator) = FixtureCoordinator::new(&driver);
        let invocation = ExecutionId::from_u128(1);
        let owners = [
            owner(
                FixtureOwnerKey::Test {
                    test: TestExecutionId(1),
                    attempt: None,
                },
                1,
            ),
            owner(
                FixtureOwnerKey::File {
                    execution: invocation,
                },
                2,
            ),
            owner(
                FixtureOwnerKey::Worker {
                    invocation,
                    worker: 0,
                },
                3,
            ),
            owner(FixtureOwnerKey::Suite { invocation }, 4),
        ];
        let (_, errors) = coordinator
            .drive(async {
                let mut instances = HashSet::new();
                for owner in owners {
                    let value = client
                        .acquire(definition(1, owner.key.lifetime()), owner, BTreeMap::new())
                        .await
                        .unwrap();
                    assert!(instances.insert(value.instance));
                }
            })
            .await;
        assert!(errors.is_empty());
        assert_eq!(driver.starts.load(Ordering::SeqCst), 4);
        assert_eq!(driver.teardowns.lock().unwrap().len(), 4);
    }
}
