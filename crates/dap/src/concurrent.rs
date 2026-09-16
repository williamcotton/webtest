//! DAP projections and cooperative controls over runtime-owned tasks.
//! The mutex protects debugger metadata only; no execution state or host handles enter it.
use super::*;
use webtest_model::ExecutionScopeId;
use webtest_observation::ExecutionId;
use webtest_runtime::{DebugContext, DebugScopeKind};

type TaskKey = (ExecutionId, ExecutionScopeId);

#[derive(Clone, Copy)]
pub(super) enum ResumeCommand {
    Continue,
    In,
    Over,
    Out,
}

struct StepTarget {
    command: ResumeCommand,
    context: DebugContext,
}
impl StepTarget {
    fn matches(&self, candidate: &DebugContext) -> bool {
        if candidate.execution_id != self.context.execution_id {
            return false;
        }
        let previous = &self.context.frames;
        let current = &candidate.frames;
        let contains = |frames: &[webtest_runtime::DebugScope], scope| {
            frames
                .iter()
                .any(|frame| frame.scope.execution_context.scope_id == scope)
        };
        let same = candidate.task_id == self.context.task_id;
        let descendant = contains(current, self.context.task_id);
        let ancestor = contains(previous, candidate.task_id);
        let left_lexical_scope = previous
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|scope| !contains(current, scope.scope.execution_context.scope_id));
        match self.command {
            ResumeCommand::Continue => false,
            ResumeCommand::In => same || descendant || ancestor,
            // Over lets nested scopes and child tasks run, then stops back at the
            // selected lexical depth (or its caller when the task has finished).
            ResumeCommand::Over => {
                (same || ancestor) && (current.len() <= previous.len() || left_lexical_scope)
            }
            ResumeCommand::Out => (same || ancestor) && left_lexical_scope,
        }
    }
}

struct Frame {
    id: i64,
    name: String,
    location: StepLocation,
    variables: DapVariableStore,
}
struct Snapshot {
    context: DebugContext,
    path: PathBuf,
    frames: Vec<Frame>, // innermost first
}
struct Task {
    id: i64,
    name: String,
    snapshot: Option<Snapshot>,
}

pub(super) struct DebugTasks {
    tasks: BTreeMap<TaskKey, Task>,
    next_thread: i64,
    pub pending_pause: Option<&'static str>,
    paused: bool,
    epoch: u64,
    step: Option<StepTarget>,
    events: Vec<Value>,
}
impl Default for DebugTasks {
    fn default() -> Self {
        Self {
            tasks: BTreeMap::new(),
            next_thread: 1,
            pending_pause: None,
            paused: false,
            epoch: 0,
            step: None,
            events: Vec::new(),
        }
    }
}
impl DebugTasks {
    pub fn threads(&self) -> Vec<Value> {
        let mut tasks: Vec<_> = self.tasks.values().collect();
        tasks.sort_by_key(|task| task.id);
        tasks
            .into_iter()
            .map(|task| json!({"id":task.id, "name":task.name}))
            .collect()
    }
    fn resume(&mut self) {
        self.paused = false;
        self.epoch += 1;
        for task in self.tasks.values_mut() {
            task.snapshot = None;
        }
    }
}

/// If a scope deadline or sibling cancellation drops a parked hook, its frame
/// disappears immediately. Other parked siblings retain their own snapshots.
struct Parked<'a> {
    state: &'a DebugState,
    key: TaskKey,
    epoch: u64,
}
impl Drop for Parked<'_> {
    fn drop(&mut self) {
        let mut tasks = lock(&self.state.tasks);
        if tasks.epoch == self.epoch
            && let Some(task) = tasks.tasks.get_mut(&self.key)
        {
            task.snapshot = None;
        }
    }
}

impl DebugState {
    pub(super) fn cancel_execution(&self) {
        self.cancel_with_reason(webtest_host::CancellationReason::DebugDisconnect);
    }
    fn cancel_with_reason(&self, reason: webtest_host::CancellationReason) {
        *lock(&self.cancellation_reason) = reason;
        self.shutting_down.store(true, Ordering::Release);
        lock(&self.tasks).resume();
        self.wake.send_modify(|epoch| *epoch += 1);
    }
    pub(super) async fn join_execution(&self) {
        let task = lock(&self.run_task).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
    pub(super) async fn restart(self: &Arc<Self>, request: &Request) -> Result<(), DapError> {
        if lock(&self.program).is_none() {
            return self
                .writer
                .failure(request, "No debug execution has been launched")
                .await;
        }
        self.cancel_with_reason(webtest_host::CancellationReason::RunnerShutdown);
        self.join_execution().await;
        self.flush_thread_events().await;
        self.shutting_down.store(false, Ordering::Release);
        self.started.store(false, Ordering::Release);
        self.completion.send_replace(false);
        {
            let mut tasks = lock(&self.tasks);
            tasks.step = None;
            tasks.pending_pause = Some("entry");
        }
        self.writer.response(request, json!({})).await?;
        self.start_if_ready();
        Ok(())
    }
    pub(super) async fn flush_thread_events(&self) {
        let events = std::mem::take(&mut lock(&self.tasks).events);
        for event in events {
            let _ = self.writer.event("thread", event).await;
        }
    }
    pub(super) async fn stack_trace(&self, request: &Request) -> Result<(), DapError> {
        let id = request.arguments["threadId"].as_i64();
        let all = lock(&self.tasks)
            .tasks
            .values()
            .find(|task| Some(task.id) == id)
            .and_then(|task| task.snapshot.as_ref())
            .map(|snapshot| {
                snapshot.frames.iter().map(|frame| json!({
                "id":frame.id, "name":frame.name,
                "source":{"name":source_name(&snapshot.path), "path":snapshot.path},
                "line":frame.location.line, "column":frame.location.column,
                "endLine":frame.location.end_line, "endColumn":frame.location.end_column,
            })).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let start = request.arguments["startFrame"].as_u64().unwrap_or(0) as usize;
        let levels = request.arguments["levels"]
            .as_u64()
            .filter(|n| *n != 0)
            .unwrap_or(u64::MAX) as usize;
        let frames: Vec<_> = all.iter().skip(start).take(levels).collect();
        self.writer
            .response(
                request,
                json!({"stackFrames":frames,"totalFrames":all.len()}),
            )
            .await
    }
    pub(super) async fn scopes(&self, request: &Request) -> Result<(), DapError> {
        let id = request.arguments["frameId"].as_i64();
        let scopes = lock(&self.tasks)
            .tasks
            .values()
            .filter_map(|task| task.snapshot.as_ref())
            .flat_map(|snapshot| &snapshot.frames)
            .find(|frame| Some(frame.id) == id)
            .map(|frame| {
                vec![json!({"name":"WebTest", "presentationHint":"locals",
                "variablesReference":frame.variables.root_reference,"expensive":false})]
            })
            .unwrap_or_default();
        self.writer
            .response(request, json!({"scopes":scopes}))
            .await
    }
    pub(super) async fn variables(&self, request: &Request) -> Result<(), DapError> {
        let reference = request.arguments["variablesReference"]
            .as_i64()
            .unwrap_or(0);
        let variables = lock(&self.tasks)
            .tasks
            .values_mut()
            .filter_map(|task| task.snapshot.as_mut())
            .flat_map(|snapshot| &mut snapshot.frames)
            .find(|frame| {
                frame.variables.root_reference == reference
                    || frame.variables.targets.contains_key(&reference)
            })
            .map(|frame| frame.variables.variables(reference))
            .unwrap_or_default();
        self.writer
            .response(request, json!({"variables":variables}))
            .await
    }
    pub(super) async fn resume(
        &self,
        request: &Request,
        command: ResumeCommand,
    ) -> Result<(), DapError> {
        if request.arguments["singleThread"].as_bool() == Some(true) {
            return self
                .writer
                .failure(request, "WebTest resumes all managed tasks together")
                .await;
        }
        let id = request.arguments["threadId"].as_i64();
        let resumed = {
            let mut tasks = lock(&self.tasks);
            let selected = tasks.tasks.values().find(|task| Some(task.id) == id);
            let context = selected
                .and_then(|task| task.snapshot.as_ref())
                .map(|s| s.context.clone());
            if !tasks.paused || (!matches!(command, ResumeCommand::Continue) && context.is_none()) {
                false
            } else {
                tasks.pending_pause = None;
                tasks.step = context.and_then(|context| {
                    (!matches!(command, ResumeCommand::Continue))
                        .then_some(StepTarget { command, context })
                });
                tasks.resume();
                true
            }
        };
        if !resumed {
            return self
                .writer
                .failure(request, "The selected task is not paused at a safe point")
                .await;
        }
        self.writer
            .response(
                request,
                if matches!(command, ResumeCommand::Continue) {
                    json!({"allThreadsContinued":true})
                } else {
                    json!({})
                },
            )
            .await?;
        self.writer
            .event(
                "continued",
                json!({"threadId":id, "allThreadsContinued":true}),
            )
            .await?;
        self.wake.send_modify(|epoch| *epoch += 1);
        Ok(())
    }

    fn snapshot(
        &self,
        program: &LoadedProgram,
        test: &PlannedTest,
        step: &PlannedStep,
        context: &DebugContext,
        bindings: BTreeMap<String, webtest_model::Value>,
        error: Option<&StepError>,
    ) -> Snapshot {
        let mut bindings = Some(bindings);
        let frames = context
            .frames
            .iter()
            .rev()
            .enumerate()
            .map(|(index, scope)| {
                let location = StepLocation::new(&program.source, scope.scope.origin.range);
                let name = if index == 0 {
                    operation_name(&step.operation)
                } else if index == context.frames.len() - 1 {
                    format!("test {:?}", test.name)
                } else if let Some(attempt) = scope.attempt {
                    format!("attempt {}/{}", attempt.ordinal, attempt.max_attempts)
                } else {
                    match scope.kind {
                        DebugScopeKind::Sequence => "sequence",
                        DebugScopeKind::Parallel => "parallel",
                        DebugScopeKind::Race => "race",
                        DebugScopeKind::Retry => "retry",
                        DebugScopeKind::Timeout => "timeout",
                        DebugScopeKind::BrowserContext => "browser context",
                        DebugScopeKind::Operation => "operation",
                    }
                    .into()
                };
                let mut values = if index == 0 {
                    bindings.take().unwrap_or_default()
                } else {
                    BTreeMap::new()
                };
                // Ancestor frames expose their own metadata, never a child's lexical locals.
                values.insert(
                    "$scope".into(),
                    webtest_model::Value::String(
                        scope.scope.execution_context.scope_id.0.to_string(),
                    ),
                );
                if let Some(attempt) = scope.attempt {
                    values.insert(
                        "$attempt".into(),
                        webtest_model::Value::Int(attempt.ordinal.into()),
                    );
                    values.insert(
                        "$maxAttempts".into(),
                        webtest_model::Value::Int(attempt.max_attempts.into()),
                    );
                }
                if index == 0 {
                    values.insert(
                        "test".into(),
                        webtest_model::Value::String(test.name.clone()),
                    );
                    values.insert(
                        "operation".into(),
                        webtest_model::Value::String(name.clone()),
                    );
                    values.insert(
                        "line".into(),
                        webtest_model::Value::Int(location.line.into()),
                    );
                    values.insert(
                        "source".into(),
                        webtest_model::Value::String(
                            program
                                .source
                                .lines()
                                .nth(location.line.saturating_sub(1) as usize)
                                .unwrap_or_default()
                                .trim()
                                .into(),
                        ),
                    );
                    if let Some(error) = error {
                        values.insert(
                            "$failureCode".into(),
                            webtest_model::Value::String(error.code().short_code().into()),
                        );
                        if let StepError::Assertion(assertion) = error {
                            values.insert("$actual".into(), assertion.actual.clone());
                            if let Some(expected) = &assertion.expected {
                                values.insert("$expected".into(), expected.clone());
                            }
                        }
                    }
                }
                let values = values
                    .into_iter()
                    .map(|(key, value)| {
                        (
                            key,
                            value.redacted(&self.runner_options.redacted_json_fields),
                        )
                    })
                    .collect();
                let variables = DapVariableStore::with_handles(values, self.handles.clone());
                let id = self.handles.fetch_add(1, Ordering::Relaxed);
                Frame {
                    id,
                    name,
                    location,
                    variables,
                }
            })
            .collect();
        Snapshot {
            context: context.clone(),
            path: program.path.clone(),
            frames,
        }
    }

    async fn safe_point(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        context: &DebugContext,
        bindings: BTreeMap<String, webtest_model::Value>,
        error: Option<&StepError>,
    ) {
        if self.is_cancelled() {
            return;
        }
        let Some(program) = lock(&self.program).clone() else {
            return;
        };
        self.flush_thread_events().await;
        let mut wake = self.wake.subscribe();
        let location = StepLocation::new(&program.source, step.origin.range);
        let revision_matches = lock(&self.breakpoint_revisions)
            .get(&program.path)
            .is_none_or(|revision| *revision == program.plan.source_revision);
        let breakpoint = error.is_none()
            && revision_matches
            && lock(&self.breakpoints)
                .get(&program.path)
                .is_some_and(|lines| lines.contains(&location.line));
        let exception = error.is_some_and(|error| self.catches(step, error));
        let key = (context.execution_id, context.task_id);
        let parked = {
            let mut tasks = lock(&self.tasks);
            let reason = if tasks.paused {
                None
            } else {
                tasks
                    .pending_pause
                    .take()
                    .or(exception.then_some("exception"))
                    .or(breakpoint.then_some("breakpoint"))
                    .or_else(|| {
                        tasks
                            .step
                            .as_ref()
                            .is_some_and(|target| target.matches(context))
                            .then_some("step")
                    })
            };
            if reason.is_some() {
                tasks.paused = true;
                tasks.step = None;
            }
            if !tasks.paused {
                return;
            }
            let epoch = tasks.epoch;
            let Some(task) = tasks.tasks.get_mut(&key) else {
                return;
            };
            task.snapshot = Some(self.snapshot(&program, test, step, context, bindings, error));
            (task.id, epoch, reason)
        };
        let _parked = Parked {
            state: self,
            key,
            epoch: parked.1,
        };
        // In-flight host operations finish normally and park at the next safe point.
        // Do not claim they have already stopped when the triggering event is sent.
        if let Some(reason) = parked.2 {
            let _ = self.writer.event("stopped", json!({"reason":reason,
                "threadId":parked.0, "allThreadsStopped":false,
                "description": error.map_or_else(|| format!("Paused before {}", operation_name(&step.operation)), ToString::to_string),
                "text": error.map(|error| error.code().short_code()),
            })).await;
        } else {
            let _ = self
                .writer
                .event(
                    "stopped",
                    json!({"reason":"pause", "threadId":parked.0,
                "allThreadsStopped":false, "preserveFocusHint":true}),
                )
                .await;
        }
        loop {
            if self.is_cancelled() || lock(&self.tasks).epoch != parked.1 {
                break;
            }
            if wake.changed().await.is_err() {
                break;
            }
        }
    }
    fn catches(&self, step: &PlannedStep, error: &StepError) -> bool {
        let filters = lock(&self.exception_breakpoints);
        let class = match error.failure_class() {
            webtest_feedback::FailureClass::Infrastructure => "infrastructure",
            webtest_feedback::FailureClass::Internal => "internal",
            webtest_feedback::FailureClass::Test => match error {
                StepError::Provider(_) => "provider",
                _ => "assertion",
            },
        };
        filters.contains(class)
            || (matches!(&step.operation, TestOperation::ServerProviderCall(call) if call.provider == "app")
                && filters.contains(
                    if error.failure_class() == webtest_feedback::FailureClass::Infrastructure {
                        "appInfrastructure"
                    } else {
                        "appProviderFailure"
                    },
                ))
    }
}

#[async_trait]
impl RunControl for DebugState {
    fn task_started(&self, test: &PlannedTest, context: &DebugContext) {
        let mut tasks = lock(&self.tasks);
        let id = tasks.next_thread;
        tasks.next_thread += 1;
        let name = if context.parent_task_id.is_none() {
            test.name.clone()
        } else {
            format!("{} / task {}", test.name, context.task_id.0)
        };
        tasks.tasks.insert(
            (context.execution_id, context.task_id),
            Task {
                id,
                name,
                snapshot: None,
            },
        );
        tasks.events.push(json!({"reason":"started","threadId":id}));
    }
    fn task_finished(&self, context: &DebugContext) {
        let mut tasks = lock(&self.tasks);
        if let Some(task) = tasks.tasks.remove(&(context.execution_id, context.task_id)) {
            tasks
                .events
                .push(json!({"reason":"exited","threadId":task.id}));
        }
        if tasks.tasks.is_empty() {
            tasks.resume();
        }
    }
    fn is_cancelled(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }
    fn cancellation_reason(&self) -> webtest_host::CancellationReason {
        *lock(&self.cancellation_reason)
    }
    async fn cancelled(&self) {
        let mut wake = self.wake.subscribe();
        while !self.is_cancelled() {
            if wake.changed().await.is_err() {
                break;
            }
        }
    }
    async fn before_step(&self, _test: &PlannedTest, _step: &PlannedStep) {}
    fn should_capture_bindings(&self, _test: &PlannedTest, _step: &PlannedStep) -> bool {
        // Any sibling can request a cooperative pause before this hook is polled.
        true
    }
    async fn before_step_in_context(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        context: &DebugContext,
        bindings: Option<BTreeMap<String, webtest_model::Value>>,
    ) {
        self.safe_point(test, step, context, bindings.unwrap_or_default(), None)
            .await;
    }
    async fn after_step_failure_in_context(
        &self,
        test: &PlannedTest,
        step: &PlannedStep,
        context: &DebugContext,
        error: &StepError,
        bindings: &BTreeMap<String, webtest_model::Value>,
    ) {
        if self.catches(step, error) || lock(&self.tasks).paused {
            self.safe_point(test, step, context, bindings.clone(), Some(error))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_browser::{BrowserError, BrowserSession};

    struct NoBrowser;
    #[async_trait]
    impl BrowserHost for NoBrowser {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            panic!("server-only test")
        }
    }
    fn state(source: &str) -> Arc<DebugState> {
        let state = DebugState::new_with_options(
            ProtocolWriter::new(tokio::io::sink()),
            Arc::new(NoBrowser),
            RunnerOptions::default(),
        );
        *lock(&state.program) = Some(
            LoadedProgram::load(PathBuf::from("/debug.webtest"), Some(source.into()))
                .expect("valid source"),
        );
        state
    }
    fn run(state: &Arc<DebugState>) -> tokio::task::JoinHandle<webtest_runtime::RunResult> {
        let state = state.clone();
        tokio::spawn(async move {
            let program = lock(&state.program).clone().unwrap();
            Runner::new(Arc::default())
                .with_options(state.runner_options.clone())
                .run_with_control(&program.plan, &NoBrowser, Some(state.as_ref()))
                .await
        })
    }
    async fn parked(state: &DebugState, count: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if lock(&state.tasks)
                    .tasks
                    .values()
                    .filter(|t| t.snapshot.is_some())
                    .count()
                    == count
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("tasks reach their safe points");
    }
    fn snapshot(state: &DebugState) -> (i64, u32, i64) {
        let tasks = lock(&state.tasks);
        let task = tasks.tasks.values().find(|t| t.snapshot.is_some()).unwrap();
        let frame = &task.snapshot.as_ref().unwrap().frames[0];
        (task.id, frame.location.line, frame.variables.root_reference)
    }
    fn request(command: &str, id: i64) -> Request {
        Request {
            seq: 1,
            message_type: "request".into(),
            command: command.into(),
            arguments: json!({"threadId":id}),
        }
    }
    async fn resume(state: &DebugState, command: ResumeCommand) {
        let (id, _, _) = snapshot(state);
        state
            .resume(&request("continue", id), command)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn siblings_keep_distinct_frames_bindings_and_broadcast_continue() {
        let state = state(
            "test \"parallel\" {\n let seed = 7\n parallel {\n server {\n let local = 11\n expect local == 11\n }\n server {\n let right = 22\n expect right == 22\n }\n }\n expect seed == 7\n}",
        );
        lock(&state.breakpoints).insert(PathBuf::from("/debug.webtest"), HashSet::from([6]));
        let run = run(&state);
        parked(&state, 2).await;
        {
            let mut tasks = lock(&state.tasks);
            assert_eq!(tasks.tasks.len(), 3, "parent and two owned siblings");
            let mut references = HashSet::new();
            let mut local_seen = false;
            for task in tasks.tasks.values_mut() {
                let Some(snapshot) = &mut task.snapshot else {
                    continue;
                };
                assert!(snapshot.frames.iter().any(|f| f.name == "parallel"));
                let frame = &mut snapshot.frames[0];
                assert!(references.insert(frame.variables.root_reference));
                let values = frame.variables.variables(frame.variables.root_reference);
                assert!(
                    values
                        .iter()
                        .any(|v| v["name"] == "seed" && v["value"] == "7")
                );
                if frame.location.line == 6 {
                    assert!(
                        values
                            .iter()
                            .any(|v| v["name"] == "local" && v["value"] == "11")
                    );
                    local_seen = true;
                } else {
                    assert!(!values.iter().any(|v| v["name"] == "local"));
                }
            }
            assert!(local_seen);
        }
        resume(&state, ResumeCommand::Continue).await;
        let result = run.await.unwrap();
        assert_eq!(result.passed(), 1);
        assert!(lock(&state.tasks).tasks.is_empty());
    }

    #[tokio::test]
    async fn step_in_over_and_out_follow_lexical_scopes() {
        let state = state(
            "test \"steps\" {\n let a = 1\n timeout 30s {\n let b = 2\n expect b == 2\n }\n expect a == 1\n}",
        );
        lock(&state.tasks).pending_pause = Some("entry");
        let run = run(&state);
        parked(&state, 1).await;
        let (thread, line, old_ref) = snapshot(&state);
        assert_eq!(line, 2);
        resume(&state, ResumeCommand::In).await;
        parked(&state, 1).await;
        assert_eq!(snapshot(&state).0, thread);
        assert_eq!(snapshot(&state).1, 4);
        assert_ne!(
            snapshot(&state).2,
            old_ref,
            "stale variable handles never alias a later stop"
        );
        resume(&state, ResumeCommand::Over).await;
        parked(&state, 1).await;
        assert_eq!(snapshot(&state).1, 5);
        resume(&state, ResumeCommand::Out).await;
        parked(&state, 1).await;
        assert_eq!(snapshot(&state).1, 7);
        resume(&state, ResumeCommand::Continue).await;
        assert_eq!(run.await.unwrap().passed(), 1);
    }

    #[tokio::test]
    async fn next_skips_sibling_work_and_step_out_returns_to_the_parent_task() {
        let source = "test \"tasks\" {\n let start = 1\n parallel {\n server {\n expect start == 1\n }\n server {\n expect start == 1\n }\n }\n expect start == 1\n}";
        for command in [ResumeCommand::Over, ResumeCommand::In] {
            let state = state(source);
            lock(&state.tasks).pending_pause = Some("entry");
            let run = run(&state);
            parked(&state, 1).await;
            let parent = snapshot(&state).0;
            resume(&state, command).await;
            if matches!(command, ResumeCommand::In) {
                parked(&state, 2).await;
                assert_ne!(snapshot(&state).0, parent);
                resume(&state, ResumeCommand::Out).await;
            }
            parked(&state, 1).await;
            assert_eq!(snapshot(&state).0, parent);
            assert_eq!(snapshot(&state).1, 11);
            resume(&state, ResumeCommand::Continue).await;
            assert_eq!(run.await.unwrap().passed(), 1);
        }
    }

    #[test]
    fn exception_filters_preserve_provider_infrastructure_and_internal_classes() {
        let state = state("test \"filters\" { expect true }");
        let program = lock(&state.program).clone().unwrap();
        let step = program.plan.tests[0].steps()[0];
        for (filter, error) in [
            (
                "provider",
                StepError::Provider(webtest_provider::ProviderError::Application {
                    code: "test".into(),
                    message: "failed".into(),
                    retryable: false,
                    data: Value::Null,
                }),
            ),
            (
                "infrastructure",
                StepError::Browser(BrowserError::Launch("offline".into())),
            ),
            ("internal", StepError::Internal("invariant".into())),
        ] {
            for selected in ["assertion", "provider", "infrastructure", "internal"] {
                *lock(&state.exception_breakpoints) = HashSet::from([selected.to_string()]);
                assert_eq!(state.catches(step, &error), selected == filter);
            }
        }
    }

    #[tokio::test]
    async fn retry_exceptions_have_fresh_threads_attempt_metadata_and_values() {
        let state = state("test \"retry\" { retry 2 { expect 1 == 2 } }");
        lock(&state.exception_breakpoints).insert("assertion".into());
        let run = run(&state);
        let mut ids = HashSet::new();
        for ordinal in 1..=2 {
            parked(&state, 1).await;
            {
                let mut tasks = lock(&state.tasks);
                let task = tasks
                    .tasks
                    .values_mut()
                    .find(|t| t.snapshot.is_some())
                    .unwrap();
                assert!(ids.insert(task.id));
                let snapshot = task.snapshot.as_mut().unwrap();
                assert!(
                    snapshot
                        .context
                        .frames
                        .iter()
                        .any(|f| f.attempt.is_some_and(|a| a.ordinal == ordinal))
                );
                let frame = &mut snapshot.frames[0];
                let values = frame.variables.variables(frame.variables.root_reference);
                assert!(
                    values
                        .iter()
                        .any(|v| v["name"] == "$actual" && v["value"] == "1")
                );
                assert!(
                    values
                        .iter()
                        .any(|v| v["name"] == "$expected" && v["value"] == "2")
                );
            }
            resume(&state, ResumeCommand::Continue).await;
        }
        assert_eq!(run.await.unwrap().failed(), 1);
        assert!(lock(&state.tasks).tasks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_drops_only_owned_pause_snapshots_and_finishes_tasks() {
        let state = state("test \"timeout\" { timeout 1s { expect true } }");
        lock(&state.tasks).pending_pause = Some("entry");
        let run = run(&state);
        parked(&state, 1).await;
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        let result = run.await.unwrap();
        assert_eq!(result.timed_out(), 1);
        assert!(lock(&state.tasks).tasks.is_empty());
    }

    #[tokio::test]
    async fn disconnect_cancels_all_parked_siblings_and_awaits_teardown() {
        let state = state(
            "test \"disconnect\" { server { fs.temp_dir() } parallel { server { expect true } server { expect true } } }",
        );
        lock(&state.breakpoints).insert(PathBuf::from("/debug.webtest"), HashSet::from([1]));
        let run = run(&state);
        parked(&state, 1).await;
        // Advance past acquisition so a live owned resource exists at cancellation.
        lock(&state.breakpoints).clear();
        resume(&state, ResumeCommand::In).await;
        parked(&state, 2).await;
        assert!(state.handle(request("disconnect", 1)).await.unwrap());
        let result = run.await.unwrap();
        assert!(matches!(
            result.outcome,
            RunOutcome::Cancelled {
                reason: webtest_host::CancellationReason::DebugDisconnect
            }
        ));
        assert!(matches!(
            result.tests[0].outcome,
            TestOutcome::Cancelled { .. }
        ));
        assert!(lock(&state.tasks).tasks.is_empty());
        assert!(matches!(
            result.events.last(),
            Some(webtest_observation::ExecutionEvent::RunFinished { .. })
        ));
    }

    #[tokio::test]
    async fn restart_joins_old_execution_before_admitting_fresh_tasks() {
        let state = state("test \"restart\" { expect true }");
        state.configured.store(true, Ordering::Release);
        lock(&state.tasks).pending_pause = Some("entry");
        state.start_if_ready();
        parked(&state, 1).await;
        let old = snapshot(&state).0;
        state.restart(&request("restart", old)).await.unwrap();
        parked(&state, 1).await;
        assert!(snapshot(&state).0 > old);
        assert_eq!(lock(&state.tasks).tasks.len(), 1);
        state.cancel_execution();
        state.join_execution().await;
        assert!(lock(&state.tasks).tasks.is_empty());
    }
}
