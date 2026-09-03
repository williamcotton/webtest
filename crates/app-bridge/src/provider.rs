use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex, RwLock as StdRwLock,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncWrite, BufReader, ReadBuf, ReadHalf, WriteHalf},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, Notify, oneshot},
};
use tracing::{info, warn};
use webtest_provider::{
    CallContext, ProviderCall, ProviderError, ProviderResult, ProviderSchema, ServerProvider,
    Value, value_to_json,
};

use crate::{
    AppManifest, AppSchemaError, BridgeMessage, DEFAULT_MAX_MESSAGE_BYTES, PROTOCOL_VERSION,
    SchemaLimits, TypeSchema, canonical_schema_hash, read_frame, write_frame,
};

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_STDERR_BYTES: usize = 65_536;
const DEFAULT_MAX_PENDING_CALLS: usize = 1_024;
const DEFAULT_MAX_EVENTS_PER_CALL: usize = 1_024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppAdapter {
    #[default]
    Bridge,
    Command,
    Http,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppTransport {
    #[default]
    Auto,
    Unix,
    NamedPipe,
    Tcp,
    Stdio,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthCheck {
    pub url: String,
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppProcessConfig {
    pub command: String,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub owned: bool,
    pub health: Option<HealthCheck>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpOperation {
    pub method: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppHttpConfig {
    pub base_url: String,
    pub operations: BTreeMap<String, HttpOperation>,
}

#[derive(Clone, Debug)]
pub struct AppProviderConfig {
    pub adapter: AppAdapter,
    pub transport: AppTransport,
    /// Persistent stdio bridge command, or the executable used once per call by `command`.
    pub command: Vec<String>,
    pub application: Option<AppProcessConfig>,
    pub http: AppHttpConfig,
    pub startup_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub max_message_bytes: usize,
    pub max_stderr_bytes: usize,
    pub max_pending_calls: usize,
    pub max_events_per_call: usize,
    pub value_limits: SchemaLimits,
}

impl Default for AppProviderConfig {
    fn default() -> Self {
        Self {
            adapter: AppAdapter::Bridge,
            transport: AppTransport::Auto,
            command: Vec::new(),
            application: None,
            http: AppHttpConfig::default(),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
            max_pending_calls: DEFAULT_MAX_PENDING_CALLS,
            max_events_per_call: DEFAULT_MAX_EVENTS_PER_CALL,
            value_limits: SchemaLimits::default(),
        }
    }
}

pub struct AppProvider {
    manifest: AppManifest,
    schema: ProviderSchema,
    config: AppProviderConfig,
    state: Mutex<AppState>,
    active_transport: StdRwLock<Option<String>>,
}

enum AppState {
    Stopped,
    Bridge(ActiveBridge),
    Compatibility(ProcessResources),
    Failed(ProviderError),
}

struct ActiveBridge {
    client: Arc<BridgeClient>,
    resources: ProcessResources,
    transport: &'static str,
}

#[derive(Default)]
struct ProcessResources {
    children: Vec<ManagedProcess>,
    endpoint_directory: Option<tempfile::TempDir>,
}

struct ManagedProcess {
    child: Child,
    stderr: Option<StderrCapture>,
}

/// Owns one configured application process independently of an application provider.
pub struct ApplicationLifecycle {
    config: AppProcessConfig,
    shutdown_timeout: Duration,
    max_stderr_bytes: usize,
    state: Mutex<ApplicationState>,
}

enum ApplicationState {
    Stopped,
    Started(ProcessResources),
    Failed(ProviderError),
}

struct StderrCapture {
    state: Arc<StdMutex<CapturedStderr>>,
    finished: Arc<Notify>,
}

#[derive(Default)]
struct CapturedStderr {
    bytes: Vec<u8>,
    total: usize,
    done: bool,
}

impl ApplicationLifecycle {
    pub fn new(config: AppProcessConfig) -> Self {
        Self {
            config,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
            state: Mutex::new(ApplicationState::Stopped),
        }
    }

    pub async fn start(&self, project_root: &std::path::Path) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        match &*state {
            ApplicationState::Started(_) => return Ok(()),
            ApplicationState::Failed(error) => return Err(error.clone()),
            ApplicationState::Stopped => {}
        }
        let started = start_application_process(
            &self.config,
            project_root,
            self.max_stderr_bytes,
            self.shutdown_timeout,
        )
        .await;
        match started {
            Ok(resources) => {
                *state = ApplicationState::Started(resources);
                Ok(())
            }
            Err(error) => {
                *state = ApplicationState::Failed(error.clone());
                Err(error)
            }
        }
    }

    pub async fn shutdown(&self) -> Result<(), ProviderError> {
        let state = std::mem::replace(&mut *self.state.lock().await, ApplicationState::Stopped);
        if let ApplicationState::Started(mut resources) = state {
            terminate_process(&mut resources, self.shutdown_timeout).await;
        }
        Ok(())
    }
}

impl AppProvider {
    pub fn new(manifest: AppManifest, config: AppProviderConfig) -> Result<Self, AppSchemaError> {
        manifest.validate()?;
        if config.max_message_bytes == 0
            || config.max_pending_calls == 0
            || config.max_events_per_call == 0
        {
            return Err(AppSchemaError::Invalid(
                "bridge message, pending-call, and event limits must be positive".into(),
            ));
        }
        let schema = manifest.provider_schema();
        Ok(Self {
            manifest,
            schema,
            config,
            state: Mutex::new(AppState::Stopped),
            active_transport: StdRwLock::new(None),
        })
    }

    pub fn manifest(&self) -> &AppManifest {
        &self.manifest
    }

    /// Starts and verifies the configured application bridge without dispatching a call.
    pub async fn start(&self, project_root: &std::path::Path) -> Result<(), ProviderError> {
        info!(
            adapter = ?self.config.adapter,
            transport = ?self.config.transport,
            "application bridge configuration resolved"
        );
        self.ensure_started(project_root).await
    }

    pub async fn shutdown(&self) -> Result<(), ProviderError> {
        info!("draining application bridge");
        *self
            .active_transport
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let state = std::mem::replace(&mut *self.state.lock().await, AppState::Stopped);
        let result = match state {
            AppState::Bridge(mut active) => {
                let bridge_result = active.client.shutdown(self.config.shutdown_timeout).await;
                terminate_process(&mut active.resources, self.config.shutdown_timeout).await;
                bridge_result
            }
            AppState::Compatibility(mut resources) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                Ok(())
            }
            AppState::Stopped | AppState::Failed(_) => Ok(()),
        };
        info!("application bridge resources finalized");
        result
    }

    fn function(&self, name: &str) -> Result<&crate::FunctionSchema, ProviderError> {
        self.manifest
            .functions
            .get(name)
            .ok_or_else(|| ProviderError::UnknownOperation {
                provider: "app".into(),
                operation: name.into(),
            })
    }

    fn checked_arguments(&self, call: &ProviderCall) -> Result<serde_json::Value, ProviderError> {
        if !call.schema_hash.is_empty() && call.schema_hash != self.manifest.schema_hash {
            return Err(ProviderError::BridgeSchemaDrift {
                expected: call.schema_hash.clone(),
                live: self.manifest.schema_hash.clone(),
            });
        }
        let function = self.function(&call.operation.0)?;
        let TypeSchema::Object { fields } = &function.params else {
            return Err(ProviderError::BridgeProtocol {
                code: "invalid_offline_schema".into(),
                message: "function parameters were not an object".into(),
            });
        };
        let mut arguments = call
            .arguments
            .iter()
            .map(|(name, value)| {
                value_to_json(value)
                    .map(|value| (name.clone(), value))
                    .ok_or_else(|| ProviderError::BridgeValidation {
                        path: format!("$.arguments.{name}"),
                        message: "value is not transferable".into(),
                    })
            })
            .collect::<Result<serde_json::Map<_, _>, _>>()?;
        for (name, field) in fields {
            if !arguments.contains_key(name)
                && let Some(default) = &field.default
            {
                arguments.insert(name.clone(), default.clone());
            }
        }
        let arguments = serde_json::Value::Object(arguments);
        function
            .params
            .validate_json(&arguments, "$.arguments", self.config.value_limits)
            .map_err(schema_validation_error)?;
        Ok(arguments)
    }

    fn checked_result(
        &self,
        function: &str,
        value: serde_json::Value,
    ) -> Result<Value, ProviderError> {
        self.function(function)?
            .returns
            .validate_json(&value, "$.result", self.config.value_limits)
            .map_err(schema_validation_error)?;
        Ok(self.function(function)?.returns.value_from_json(value))
    }

    async fn ensure_started(&self, project_root: &std::path::Path) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        match &*state {
            AppState::Bridge(_) | AppState::Compatibility(_) => return Ok(()),
            AppState::Failed(error) => return Err(error.clone()),
            AppState::Stopped => {}
        }
        let started = match self.config.adapter {
            AppAdapter::Bridge => self.start_bridge(project_root).await.map(AppState::Bridge),
            AppAdapter::Command | AppAdapter::Http => self
                .start_compatibility_process(project_root)
                .await
                .map(AppState::Compatibility),
        };
        match started {
            Ok(started) => {
                *state = started;
                Ok(())
            }
            Err(error) => {
                *state = AppState::Failed(error.clone());
                Err(error)
            }
        }
    }

    async fn start_bridge(
        &self,
        project_root: &std::path::Path,
    ) -> Result<ActiveBridge, ProviderError> {
        let token = random_secret()?;
        let run_id = random_secret()?;
        let (io, mut resources, transport) = match self.config.transport {
            AppTransport::Stdio => self.start_stdio_bridge(project_root, &token).await?,
            AppTransport::Auto => self.start_auto_bridge(project_root, &token).await?,
            AppTransport::Unix => self.start_unix_bridge(project_root, &token).await?,
            AppTransport::Tcp => self.start_tcp_bridge(project_root, &token).await?,
            AppTransport::NamedPipe => self.start_named_pipe_bridge(project_root, &token).await?,
        };
        let client =
            match BridgeClient::handshake(io, token, run_id, &self.manifest, &self.config).await {
                Ok(client) => client,
                Err(error) => {
                    terminate_process(&mut resources, self.config.shutdown_timeout).await;
                    return Err(error);
                }
            };
        info!(
            transport,
            "application bridge connected and schema verified"
        );
        *self
            .active_transport
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(transport.into());
        Ok(ActiveBridge {
            client: Arc::new(client),
            resources,
            transport,
        })
    }

    async fn start_stdio_bridge(
        &self,
        project_root: &std::path::Path,
        token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        let (program, args) = command_parts(&self.config.command)?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.env("WEBTEST_TOKEN", token);
        command.env("WEBTEST_PROTOCOL", PROTOCOL_VERSION.to_string());
        let mut child = command
            .spawn()
            .map_err(|error| ProviderError::BridgeTransport {
                message: format!("could not spawn stdio bridge: {error}"),
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::BridgeTransport {
                message: "stdio bridge stdin was unavailable".into(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::BridgeTransport {
                message: "stdio bridge stdout was unavailable".into(),
            })?;
        if let Some(stderr) = child.stderr.take() {
            drain_stderr(stderr, self.config.max_stderr_bytes);
        }
        let mut resources = ProcessResources {
            children: vec![ManagedProcess {
                child,
                stderr: None,
            }],
            endpoint_directory: None,
        };
        if let Some(application) = &self.config.application {
            if application.owned {
                match spawn_application(
                    application,
                    project_root,
                    &[],
                    self.config.max_stderr_bytes,
                ) {
                    Ok(application) => resources.children.push(application),
                    Err(error) => {
                        terminate_process(&mut resources, self.config.shutdown_timeout).await;
                        return Err(error);
                    }
                }
            }
            if let Err(error) = wait_for_health(application.health.as_ref()).await {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(error);
            }
        }
        Ok((
            Box::new(StdioIo {
                reader: stdout,
                writer: stdin,
            }),
            resources,
            "stdio",
        ))
    }

    async fn start_compatibility_process(
        &self,
        project_root: &std::path::Path,
    ) -> Result<ProcessResources, ProviderError> {
        let Some(application) = &self.config.application else {
            return Ok(ProcessResources::default());
        };
        start_application_process(
            application,
            project_root,
            self.config.max_stderr_bytes,
            self.config.shutdown_timeout,
        )
        .await
    }

    #[cfg(unix)]
    async fn start_auto_bridge(
        &self,
        project_root: &std::path::Path,
        token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        match self.start_unix_bridge(project_root, token).await {
            Ok(bridge) => Ok(bridge),
            Err(error) if is_local_ipc_unavailable(&error) => {
                warn!(%error, "Unix bridge unavailable; falling back to loopback TCP");
                self.start_tcp_bridge(project_root, token).await
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(windows)]
    async fn start_auto_bridge(
        &self,
        project_root: &std::path::Path,
        token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        match self.start_named_pipe_bridge(project_root, token).await {
            Ok(bridge) => Ok(bridge),
            Err(error) if is_local_ipc_unavailable(&error) => {
                warn!(%error, "named-pipe bridge unavailable; falling back to loopback TCP");
                self.start_tcp_bridge(project_root, token).await
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(not(any(unix, windows)))]
    async fn start_auto_bridge(
        &self,
        project_root: &std::path::Path,
        token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        self.start_tcp_bridge(project_root, token).await
    }

    #[cfg(unix)]
    async fn start_unix_bridge(
        &self,
        project_root: &std::path::Path,
        token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;

        let run_root = project_root.join(".webtest");
        std::fs::create_dir_all(&run_root).map_err(local_ipc_error)?;
        let directory = tempfile::Builder::new()
            .prefix("bridge-")
            .tempdir_in(&run_root)
            .map_err(local_ipc_error)?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(local_ipc_error)?;
        let socket_path = directory.path().join("app.sock");
        let listener = UnixListener::bind(&socket_path).map_err(local_ipc_error)?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
            .map_err(local_ipc_error)?;
        let endpoint = format!("unix:{}", socket_path.display());
        info!(endpoint = %endpoint, "application bridge endpoint created");
        let mut resources = ProcessResources {
            children: self
                .spawn_managed_application(project_root, &endpoint, token)?
                .into_iter()
                .collect(),
            endpoint_directory: Some(directory),
        };
        let accepted = if let Some(process) = resources.children.first_mut() {
            tokio::select! {
                accepted = tokio::time::timeout(self.config.startup_timeout, listener.accept()) => accepted,
                status = process.child.wait() => {
                    let error = bridge_process_error(process, status).await;
                    terminate_process(&mut resources, self.config.shutdown_timeout).await;
                    return Err(error);
                }
            }
        } else {
            tokio::time::timeout(self.config.startup_timeout, listener.accept()).await
        };
        let stream = match accepted {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(error)) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(transport_error(error));
            }
            Err(_) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(ProviderError::BridgeHandshake {
                    code: "bridge_readiness_timeout".into(),
                    message: format!(
                        "application did not connect within {}ms",
                        self.config.startup_timeout.as_millis()
                    ),
                });
            }
        };
        if let Some(application) = &self.config.application
            && let Err(error) = wait_for_health(application.health.as_ref()).await
        {
            terminate_process(&mut resources, self.config.shutdown_timeout).await;
            return Err(error);
        }
        Ok((Box::new(stream), resources, "unix"))
    }

    #[cfg(not(unix))]
    async fn start_unix_bridge(
        &self,
        _project_root: &std::path::Path,
        _token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        Err(ProviderError::BridgeTransport {
            message: "Unix-domain sockets are unavailable on this host".into(),
        })
    }

    async fn start_tcp_bridge(
        &self,
        project_root: &std::path::Path,
        token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(transport_error)?;
        let address = listener.local_addr().map_err(transport_error)?;
        if !address.ip().is_loopback() {
            return Err(ProviderError::BridgeTransport {
                message: "refusing a non-loopback bridge listener".into(),
            });
        }
        let endpoint = format!("tcp://{address}");
        info!(endpoint = %endpoint, "application bridge endpoint created");
        let mut resources = ProcessResources {
            children: self
                .spawn_managed_application(project_root, &endpoint, token)?
                .into_iter()
                .collect(),
            endpoint_directory: None,
        };
        let accepted = if let Some(process) = resources.children.first_mut() {
            tokio::select! {
                accepted = tokio::time::timeout(self.config.startup_timeout, listener.accept()) => accepted,
                status = process.child.wait() => {
                    let error = bridge_process_error(process, status).await;
                    terminate_process(&mut resources, self.config.shutdown_timeout).await;
                    return Err(error);
                }
            }
        } else {
            tokio::time::timeout(self.config.startup_timeout, listener.accept()).await
        };
        let stream = match accepted {
            Ok(Ok((stream, peer))) if peer.ip().is_loopback() => stream,
            Ok(Ok((_, peer))) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(ProviderError::BridgeHandshake {
                    code: "non_loopback_peer".into(),
                    message: format!("rejected bridge peer {peer}"),
                });
            }
            Ok(Err(error)) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(transport_error(error));
            }
            Err(_) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(ProviderError::BridgeHandshake {
                    code: "bridge_readiness_timeout".into(),
                    message: format!(
                        "application did not connect within {}ms",
                        self.config.startup_timeout.as_millis()
                    ),
                });
            }
        };
        if let Some(application) = &self.config.application
            && let Err(error) = wait_for_health(application.health.as_ref()).await
        {
            terminate_process(&mut resources, self.config.shutdown_timeout).await;
            return Err(error);
        }
        Ok((Box::new(stream), resources, "tcp"))
    }

    #[cfg(windows)]
    async fn start_named_pipe_bridge(
        &self,
        project_root: &std::path::Path,
        token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let name = format!(r"\\.\pipe\webtest-{}", random_secret()?);
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&name)
            .map_err(local_ipc_error)?;
        let endpoint = format!("pipe:{name}");
        info!("application bridge named-pipe endpoint created");
        let mut resources = ProcessResources {
            children: self
                .spawn_managed_application(project_root, &endpoint, token)?
                .into_iter()
                .collect(),
            endpoint_directory: None,
        };
        let connected = if let Some(process) = resources.children.first_mut() {
            tokio::select! {
                connected = tokio::time::timeout(self.config.startup_timeout, server.connect()) => connected,
                status = process.child.wait() => {
                    let error = bridge_process_error(process, status).await;
                    terminate_process(&mut resources, self.config.shutdown_timeout).await;
                    return Err(error);
                }
            }
        } else {
            tokio::time::timeout(self.config.startup_timeout, server.connect()).await
        };
        match connected {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(transport_error(error));
            }
            Err(_) => {
                terminate_process(&mut resources, self.config.shutdown_timeout).await;
                return Err(ProviderError::BridgeHandshake {
                    code: "bridge_readiness_timeout".into(),
                    message: "application did not connect to the named pipe".into(),
                });
            }
        }
        if let Some(application) = &self.config.application
            && let Err(error) = wait_for_health(application.health.as_ref()).await
        {
            terminate_process(&mut resources, self.config.shutdown_timeout).await;
            return Err(error);
        }
        Ok((Box::new(server), resources, "named_pipe"))
    }

    #[cfg(not(windows))]
    async fn start_named_pipe_bridge(
        &self,
        _project_root: &std::path::Path,
        _token: &str,
    ) -> Result<(DynIo, ProcessResources, &'static str), ProviderError> {
        Err(ProviderError::BridgeTransport {
            message: "Windows named pipes are unavailable on this host".into(),
        })
    }

    fn spawn_managed_application(
        &self,
        project_root: &std::path::Path,
        endpoint: &str,
        token: &str,
    ) -> Result<Option<ManagedProcess>, ProviderError> {
        let application =
            self.config
                .application
                .as_ref()
                .ok_or_else(|| ProviderError::BridgeTransport {
                    message: "runner-managed bridge transport requires [app].command".into(),
                })?;
        if !application.owned {
            return Ok(None);
        }
        spawn_application(
            application,
            project_root,
            &[
                ("WEBTEST_BRIDGE", endpoint),
                ("WEBTEST_TOKEN", token),
                ("WEBTEST_PROTOCOL", "1"),
            ],
            self.config.max_stderr_bytes,
        )
        .map(Some)
    }

    async fn command_call(
        &self,
        function: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProviderError> {
        use tokio::io::AsyncWriteExt;
        let (program, args) = command_parts(&self.config.command)?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(transport_error)?;
        let request = serde_json::to_vec(&serde_json::json!({
            "function": function,
            "arguments": arguments,
            "deadline_ms": duration_millis(timeout),
            "schema_hash": self.manifest.schema_hash,
        }))
        .map_err(|error| protocol_error("encode", error))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&request).await.map_err(transport_error)?;
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::BridgeTransport {
                message: "command adapter stdout was unavailable".into(),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProviderError::BridgeTransport {
                message: "command adapter stderr was unavailable".into(),
            })?;
        let stdout_task = tokio::spawn(read_bounded_stream(stdout, self.config.max_message_bytes));
        let stderr_task = tokio::spawn(read_bounded_stream(stderr, self.config.max_stderr_bytes));
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(result) => result.map_err(transport_error)?,
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::BridgeTimeout {
                    timeout_ms: duration_millis(timeout),
                });
            }
        };
        let (stdout, _, stdout_exceeded) = stdout_task
            .await
            .map_err(|error| ProviderError::BridgeTransport {
                message: format!("command adapter output task failed: {error}"),
            })?
            .map_err(transport_error)?;
        let (_, stderr_bytes, stderr_exceeded) = stderr_task
            .await
            .map_err(|error| ProviderError::BridgeTransport {
                message: format!("command adapter log task failed: {error}"),
            })?
            .map_err(transport_error)?;
        if stderr_bytes > 0 {
            warn!(
                bytes = stderr_bytes,
                truncated = stderr_exceeded,
                "command adapter wrote to stderr"
            );
        }
        if stdout_exceeded {
            return Err(ProviderError::BridgeProtocol {
                code: "frame_too_large".into(),
                message: "command adapter output exceeded the configured limit".into(),
            });
        }
        if !status.success() && stdout.is_empty() {
            return Err(ProviderError::BridgeTransport {
                message: format!("command adapter exited with {status}"),
            });
        }
        compatibility_response(&stdout, self.config.value_limits)
    }

    async fn http_call(
        &self,
        function: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProviderError> {
        let operation = self.config.http.operations.get(function).ok_or_else(|| {
            ProviderError::BridgeTransport {
                message: format!("no HTTP endpoint is configured for app.{function}"),
            }
        })?;
        let base = url::Url::parse(&self.config.http.base_url).map_err(|error| {
            ProviderError::BridgeTransport {
                message: format!("invalid application adapter base URL: {error}"),
            }
        })?;
        let url = base
            .join(&operation.path)
            .map_err(|error| ProviderError::BridgeTransport {
                message: format!("invalid application adapter endpoint: {error}"),
            })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ProviderError::BridgeTransport {
                message: "application HTTP adapter requires http or https".into(),
            });
        }
        let method = if operation.method.is_empty() {
            "POST"
        } else {
            operation.method.as_str()
        };
        let body =
            serde_json::to_string(&arguments).map_err(|error| protocol_error("encode", error))?;
        let max = self.config.max_message_bytes;
        let value_limits = self.config.value_limits;
        let url = url.to_string();
        let method = method.to_owned();
        tokio::task::spawn_blocking(move || {
            let response = match ureq::request(&method, &url)
                .timeout(timeout)
                .set("content-type", "application/json")
                .send_string(&body)
            {
                Ok(response) | Err(ureq::Error::Status(_, response)) => response,
                Err(error) => {
                    return Err(ProviderError::BridgeTransport {
                        message: error.to_string(),
                    });
                }
            };
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(max.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
                .map_err(transport_error)?;
            if bytes.len() > max {
                return Err(ProviderError::BridgeProtocol {
                    code: "frame_too_large".into(),
                    message: "HTTP adapter response exceeded the configured limit".into(),
                });
            }
            compatibility_response(&bytes, value_limits)
        })
        .await
        .map_err(|error| ProviderError::BridgeTransport {
            message: format!("HTTP adapter task failed: {error}"),
        })?
    }
}

#[async_trait]
impl ServerProvider for AppProvider {
    fn schema(&self) -> ProviderSchema {
        self.schema.clone()
    }

    fn transport_kind(&self) -> Option<String> {
        Some(match self.config.adapter {
            AppAdapter::Bridge => self
                .active_transport
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .unwrap_or_else(|| match self.config.transport {
                    AppTransport::Auto => "local_ipc_auto".into(),
                    AppTransport::Unix => "unix".into(),
                    AppTransport::NamedPipe => "named_pipe".into(),
                    AppTransport::Tcp => "tcp".into(),
                    AppTransport::Stdio => "stdio".into(),
                }),
            AppAdapter::Command => "command".into(),
            AppAdapter::Http => "http".into(),
        })
    }

    async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        let arguments = self.checked_arguments(&call)?;
        self.ensure_started(&context.project_root).await?;
        let timeout = context.timeout;
        let value = match self.config.adapter {
            AppAdapter::Bridge => {
                let (client, transport) = {
                    let state = self.state.lock().await;
                    let AppState::Bridge(active) = &*state else {
                        return Err(ProviderError::BridgeTransport {
                            message: "bridge lost its active state".into(),
                        });
                    };
                    (Arc::clone(&active.client), active.transport)
                };
                info!(transport, function = %call.operation.0, "calling application bridge");
                client.call(&call.operation.0, arguments, timeout).await?
            }
            AppAdapter::Command => {
                self.command_call(&call.operation.0, arguments, timeout)
                    .await?
            }
            AppAdapter::Http => {
                self.http_call(&call.operation.0, arguments, timeout)
                    .await?
            }
        };
        Ok(ProviderResult {
            value: self.checked_result(&call.operation.0, value)?,
        })
    }
}

struct BridgeClient {
    writer: Arc<Mutex<WriteHalf<DynIo>>>,
    pending: Arc<Mutex<PendingState>>,
    next_id: AtomicU64,
    max_message_bytes: usize,
    max_pending_calls: usize,
    supports_cancel: bool,
}

#[derive(Default)]
struct PendingState {
    requests: HashMap<u64, PendingRequest>,
    cancelled: HashMap<u64, ExpectedResponse>,
    event_counts: HashMap<u64, usize>,
}

struct PendingRequest {
    sender: oneshot::Sender<Result<BridgeMessage, ProviderError>>,
    expected: ExpectedResponse,
}

struct PendingRequestCancellation {
    id: u64,
    expected: ExpectedResponse,
    pending: Arc<Mutex<PendingState>>,
    writer: Arc<Mutex<WriteHalf<DynIo>>>,
    max_message_bytes: usize,
    supports_cancel: bool,
    armed: bool,
}

impl PendingRequestCancellation {
    fn new(client: &BridgeClient, id: u64, expected: ExpectedResponse) -> Self {
        Self {
            id,
            expected,
            pending: Arc::clone(&client.pending),
            writer: Arc::clone(&client.writer),
            max_message_bytes: client.max_message_bytes,
            supports_cancel: client.supports_cancel,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingRequestCancellation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let id = self.id;
        let expected = self.expected;
        let pending = Arc::clone(&self.pending);
        let writer = Arc::clone(&self.writer);
        let max_message_bytes = self.max_message_bytes;
        let supports_cancel = self.supports_cancel;
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            let removed = {
                let mut state = pending.lock().await;
                let request = state.requests.remove(&id);
                if let Some(request) = request {
                    state.cancelled.insert(id, request.expected);
                    true
                } else {
                    state.event_counts.remove(&id);
                    false
                }
            };
            if removed && expected == ExpectedResponse::Call && supports_cancel {
                let _ = write_frame(
                    &mut *writer.lock().await,
                    &BridgeMessage::Cancel {
                        id,
                        reason: "deadline".into(),
                    },
                    max_message_bytes,
                )
                .await;
            }
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedResponse {
    Call,
    Shutdown,
}

impl BridgeClient {
    async fn handshake(
        io: DynIo,
        token: String,
        run_id: String,
        manifest: &AppManifest,
        config: &AppProviderConfig,
    ) -> Result<Self, ProviderError> {
        let (read, mut write) = tokio::io::split(io);
        let mut reader = BufReader::new(read);
        let hello = tokio::time::timeout(
            config.startup_timeout,
            read_frame(&mut reader, config.max_message_bytes),
        )
        .await
        .map_err(|_| ProviderError::BridgeHandshake {
            code: "hello_timeout".into(),
            message: "bridge did not send hello before the readiness deadline".into(),
        })?
        .map_err(protocol_from_frame)?
        .ok_or_else(|| ProviderError::BridgeHandshake {
            code: "eof_before_hello".into(),
            message: "bridge closed before hello".into(),
        })?;
        let BridgeMessage::Hello {
            protocol_versions,
            sdk: _,
            sdk_version: _,
            token: supplied_token,
            capabilities,
        } = hello
        else {
            send_hello_error(
                &mut write,
                "expected_hello",
                "only hello is accepted before readiness",
                config.max_message_bytes,
            )
            .await;
            return Err(ProviderError::BridgeHandshake {
                code: "expected_hello".into(),
                message: "only hello is accepted before readiness".into(),
            });
        };
        if !constant_time_equal(token.as_bytes(), supplied_token.as_bytes()) {
            send_hello_error(
                &mut write,
                "authentication_failed",
                "bridge token was rejected",
                config.max_message_bytes,
            )
            .await;
            return Err(ProviderError::BridgeHandshake {
                code: "authentication_failed".into(),
                message: "bridge token was rejected".into(),
            });
        }
        if !protocol_versions.contains(&PROTOCOL_VERSION) {
            send_hello_error(
                &mut write,
                "unsupported_protocol",
                "runner and bridge have no protocol version in common",
                config.max_message_bytes,
            )
            .await;
            return Err(ProviderError::BridgeHandshake {
                code: "unsupported_protocol".into(),
                message: "runner and bridge have no protocol version in common".into(),
            });
        }
        write_frame(
            &mut write,
            &BridgeMessage::HelloOk {
                protocol: PROTOCOL_VERSION,
                run_id,
                max_message_bytes: config.max_message_bytes,
            },
            config.max_message_bytes,
        )
        .await
        .map_err(protocol_from_frame)?;
        write_frame(
            &mut write,
            &BridgeMessage::Describe { id: 1 },
            config.max_message_bytes,
        )
        .await
        .map_err(protocol_from_frame)?;
        let live = tokio::time::timeout(
            config.startup_timeout,
            read_frame(&mut reader, config.max_message_bytes),
        )
        .await
        .map_err(|_| ProviderError::BridgeHandshake {
            code: "describe_timeout".into(),
            message: "bridge did not return its schema before the readiness deadline".into(),
        })?
        .map_err(protocol_from_frame)?
        .ok_or_else(|| ProviderError::BridgeHandshake {
            code: "eof_during_describe".into(),
            message: "bridge closed during schema discovery".into(),
        })?;
        let BridgeMessage::Schema {
            id: 1,
            protocol,
            schema_hash,
            functions,
        } = live
        else {
            return Err(ProviderError::BridgeProtocol {
                code: "expected_schema".into(),
                message: "describe did not receive a matching schema response".into(),
            });
        };
        if protocol != PROTOCOL_VERSION {
            return Err(ProviderError::BridgeProtocol {
                code: "schema_protocol_mismatch".into(),
                message: format!("live schema declared protocol {protocol}"),
            });
        }
        let computed = canonical_schema_hash(&functions).map_err(schema_validation_error)?;
        if computed != schema_hash {
            tracing::debug!(
                declared = %schema_hash,
                canonical = %computed,
                "using canonical live application schema hash"
            );
        }
        if computed != manifest.schema_hash {
            return Err(ProviderError::BridgeSchemaDrift {
                expected: manifest.schema_hash.clone(),
                live: computed,
            });
        }

        let writer = Arc::new(Mutex::new(write));
        let pending = Arc::new(Mutex::new(PendingState::default()));
        spawn_reader(
            reader,
            Arc::clone(&writer),
            Arc::clone(&pending),
            config.max_message_bytes,
            config.max_events_per_call,
            config.value_limits,
        );
        Ok(Self {
            writer,
            pending,
            next_id: AtomicU64::new(2),
            max_message_bytes: config.max_message_bytes,
            max_pending_calls: config.max_pending_calls,
            supports_cancel: capabilities.cancel,
        })
    }

    async fn call(
        &self,
        function: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProviderError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let response = self
            .request(
                id,
                BridgeMessage::Call {
                    id,
                    function: function.into(),
                    arguments,
                    deadline_ms: duration_millis(timeout),
                },
                timeout,
            )
            .await;
        let response = match response {
            Err(ProviderError::BridgeTimeout { .. }) => {
                if self.supports_cancel {
                    let _ = write_frame(
                        &mut *self.writer.lock().await,
                        &BridgeMessage::Cancel {
                            id,
                            reason: "deadline".into(),
                        },
                        self.max_message_bytes,
                    )
                    .await;
                }
                return Err(ProviderError::BridgeTimeout {
                    timeout_ms: duration_millis(timeout),
                });
            }
            value => value?,
        };
        match response {
            BridgeMessage::Result {
                id: response_id,
                value,
            } if response_id == id => Ok(value),
            BridgeMessage::Error {
                id: response_id,
                code,
                message,
                retryable,
                data,
                ..
            } if response_id == id => Err(ProviderError::Application {
                code,
                message,
                retryable,
                data,
            }),
            _ => Err(ProviderError::BridgeProtocol {
                code: "unexpected_call_response".into(),
                message: format!("call {id} received a non-terminal or mismatched response"),
            }),
        }
    }

    async fn request(
        &self,
        id: u64,
        message: BridgeMessage,
        timeout: Duration,
    ) -> Result<BridgeMessage, ProviderError> {
        let (sender, receiver) = oneshot::channel();
        let expected = match &message {
            BridgeMessage::Call { .. } => ExpectedResponse::Call,
            BridgeMessage::Shutdown { .. } => ExpectedResponse::Shutdown,
            _ => {
                return Err(ProviderError::BridgeProtocol {
                    code: "invalid_runner_request".into(),
                    message: "bridge client attempted an unsupported request type".into(),
                });
            }
        };
        {
            let mut pending = self.pending.lock().await;
            if pending.requests.len() + pending.cancelled.len() >= self.max_pending_calls {
                return Err(ProviderError::BridgeProtocol {
                    code: "too_many_pending_calls".into(),
                    message: format!(
                        "bridge exceeded the {} pending-call limit",
                        self.max_pending_calls
                    ),
                });
            }
            if pending.requests.contains_key(&id) || pending.cancelled.contains_key(&id) {
                return Err(ProviderError::BridgeProtocol {
                    code: "duplicate_id".into(),
                    message: format!("request ID {id} is already in flight"),
                });
            }
            pending
                .requests
                .insert(id, PendingRequest { sender, expected });
            if expected == ExpectedResponse::Call {
                pending.event_counts.insert(id, 0);
            }
        }
        let mut cancellation = PendingRequestCancellation::new(self, id, expected);
        if let Err(error) = write_frame(
            &mut *self.writer.lock().await,
            &message,
            self.max_message_bytes,
        )
        .await
        {
            let mut pending = self.pending.lock().await;
            pending.requests.remove(&id);
            pending.event_counts.remove(&id);
            cancellation.disarm();
            return Err(protocol_from_frame(error));
        }
        let result = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ProviderError::BridgeTransport {
                message: "bridge response channel closed".into(),
            }),
            Err(_) => {
                let mut pending = self.pending.lock().await;
                let request = pending.requests.remove(&id);
                if let Some(request) = request {
                    pending.cancelled.insert(id, request.expected);
                } else {
                    pending.event_counts.remove(&id);
                }
                Err(ProviderError::BridgeTimeout {
                    timeout_ms: duration_millis(timeout),
                })
            }
        };
        cancellation.disarm();
        result
    }

    async fn shutdown(&self, timeout: Duration) -> Result<(), ProviderError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        match self
            .request(id, BridgeMessage::Shutdown { id }, timeout)
            .await?
        {
            BridgeMessage::ShutdownOk { id: response_id } if response_id == id => Ok(()),
            _ => Err(ProviderError::BridgeProtocol {
                code: "expected_shutdown_ok".into(),
                message: "bridge did not confirm shutdown".into(),
            }),
        }
    }
}

fn spawn_reader(
    mut reader: BufReader<ReadHalf<DynIo>>,
    writer: Arc<Mutex<WriteHalf<DynIo>>>,
    pending: Arc<Mutex<PendingState>>,
    max_message_bytes: usize,
    max_events_per_call: usize,
    value_limits: SchemaLimits,
) {
    tokio::spawn(async move {
        loop {
            let message = match read_frame(&mut reader, max_message_bytes).await {
                Ok(Some(message)) => message,
                Ok(None) => {
                    fail_pending(
                        &pending,
                        ProviderError::BridgeTransport {
                            message: "bridge closed with requests pending".into(),
                        },
                    )
                    .await;
                    break;
                }
                Err(error) => {
                    fail_pending(&pending, protocol_from_frame(error)).await;
                    break;
                }
            };
            if let Err(error) = validate_auxiliary_message(&message, value_limits) {
                fail_pending(&pending, error).await;
                break;
            }
            match &message {
                BridgeMessage::Result { id, .. } | BridgeMessage::Error { id, .. } => {
                    let mut state = pending.lock().await;
                    if state
                        .requests
                        .get(id)
                        .is_some_and(|request| request.expected != ExpectedResponse::Call)
                        || state
                            .cancelled
                            .get(id)
                            .is_some_and(|expected| *expected != ExpectedResponse::Call)
                    {
                        drop(state);
                        fail_pending(
                            &pending,
                            ProviderError::BridgeProtocol {
                                code: "unexpected_response_type".into(),
                                message: format!("request {id} received a call response"),
                            },
                        )
                        .await;
                        break;
                    }
                    let request = state.requests.remove(id);
                    state.event_counts.remove(id);
                    if let Some(request) = request {
                        drop(state);
                        let _ = request.sender.send(Ok(message));
                    } else if state.cancelled.remove(id).is_some() {
                        // A timed-out request remains reserved until its one terminal
                        // response arrives, so it cannot be confused with a later call.
                    } else {
                        drop(state);
                        fail_pending(
                            &pending,
                            ProviderError::BridgeProtocol {
                                code: "unknown_response_id".into(),
                                message: format!("bridge returned unknown or duplicate ID {id}"),
                            },
                        )
                        .await;
                        break;
                    }
                }
                BridgeMessage::ShutdownOk { id } => {
                    let mut state = pending.lock().await;
                    if state
                        .requests
                        .get(id)
                        .is_some_and(|request| request.expected != ExpectedResponse::Shutdown)
                        || state
                            .cancelled
                            .get(id)
                            .is_some_and(|expected| *expected != ExpectedResponse::Shutdown)
                    {
                        drop(state);
                        fail_pending(
                            &pending,
                            ProviderError::BridgeProtocol {
                                code: "unexpected_response_type".into(),
                                message: format!("request {id} received shutdown_ok"),
                            },
                        )
                        .await;
                        break;
                    }
                    let request = state.requests.remove(id);
                    if let Some(request) = request {
                        drop(state);
                        let _ = request.sender.send(Ok(message));
                    } else if state.cancelled.remove(id).is_none() {
                        drop(state);
                        fail_pending(
                            &pending,
                            ProviderError::BridgeProtocol {
                                code: "unknown_response_id".into(),
                                message: format!("bridge returned unknown or duplicate ID {id}"),
                            },
                        )
                        .await;
                        break;
                    }
                }
                BridgeMessage::Ping { id } => {
                    if let Err(error) = write_frame(
                        &mut *writer.lock().await,
                        &BridgeMessage::Pong { id: *id },
                        max_message_bytes,
                    )
                    .await
                    {
                        fail_pending(&pending, protocol_from_frame(error)).await;
                        break;
                    }
                }
                BridgeMessage::Event { call_id, .. } => {
                    let mut state = pending.lock().await;
                    let Some(count) = state.event_counts.get_mut(call_id) else {
                        drop(state);
                        fail_pending(
                            &pending,
                            ProviderError::BridgeProtocol {
                                code: "unknown_event_call_id".into(),
                                message: format!(
                                    "bridge event referenced unknown call ID {call_id}"
                                ),
                            },
                        )
                        .await;
                        break;
                    };
                    *count += 1;
                    if *count > max_events_per_call {
                        drop(state);
                        fail_pending(
                            &pending,
                            ProviderError::BridgeProtocol {
                                code: "too_many_events".into(),
                                message: format!(
                                    "call {call_id} exceeded the {max_events_per_call} event limit"
                                ),
                            },
                        )
                        .await;
                        break;
                    }
                }
                BridgeMessage::Pong { .. } => {}
                BridgeMessage::Schema { .. } => {
                    fail_pending(
                        &pending,
                        ProviderError::BridgeProtocol {
                            code: "unexpected_message".into(),
                            message: "bridge sent schema after readiness".into(),
                        },
                    )
                    .await;
                    break;
                }
                _ => {
                    fail_pending(
                        &pending,
                        ProviderError::BridgeProtocol {
                            code: "unexpected_message".into(),
                            message: "bridge sent a message invalid in the ready state".into(),
                        },
                    )
                    .await;
                    break;
                }
            }
        }
    });
}

async fn fail_pending(pending: &Mutex<PendingState>, error: ProviderError) {
    let mut state = pending.lock().await;
    let values = std::mem::take(&mut state.requests);
    state.cancelled.clear();
    state.event_counts.clear();
    drop(state);
    for (_, request) in values {
        let _ = request.sender.send(Err(error.clone()));
    }
}

fn validate_auxiliary_message(
    message: &BridgeMessage,
    limits: SchemaLimits,
) -> Result<(), ProviderError> {
    match message {
        BridgeMessage::Error {
            code,
            message,
            data,
            debug,
            ..
        } => {
            validate_bounded_text(code, "$.code", limits.max_string_bytes)?;
            validate_bounded_text(message, "$.message", limits.max_string_bytes)?;
            if let Some(debug) = debug {
                validate_bounded_text(debug, "$.debug", limits.max_string_bytes)?;
            }
            validate_untyped_json(data, "$.data", limits, 0)
        }
        BridgeMessage::Event { kind, value, .. } => {
            validate_bounded_text(kind, "$.kind", limits.max_string_bytes)?;
            validate_untyped_json(value, "$.value", limits, 0)
        }
        _ => Ok(()),
    }
}

fn validate_bounded_text(value: &str, path: &str, max: usize) -> Result<(), ProviderError> {
    if value.len() <= max {
        Ok(())
    } else {
        Err(ProviderError::BridgeValidation {
            path: path.into(),
            message: format!("{path} exceeded the {max} byte string limit"),
        })
    }
}

fn validate_untyped_json(
    value: &serde_json::Value,
    path: &str,
    limits: SchemaLimits,
    depth: usize,
) -> Result<(), ProviderError> {
    if depth > limits.max_depth {
        return Err(ProviderError::BridgeValidation {
            path: path.into(),
            message: format!(
                "{path} exceeded the maximum value depth {}",
                limits.max_depth
            ),
        });
    }
    match value {
        serde_json::Value::String(value) => {
            validate_bounded_text(value, path, limits.max_string_bytes)
        }
        serde_json::Value::Array(values) => {
            if values.len() > limits.max_collection_items {
                return Err(ProviderError::BridgeValidation {
                    path: path.into(),
                    message: format!(
                        "{path} exceeded the {} item collection limit",
                        limits.max_collection_items
                    ),
                });
            }
            for (index, value) in values.iter().enumerate() {
                validate_untyped_json(value, &format!("{path}[{index}]"), limits, depth + 1)?;
            }
            Ok(())
        }
        serde_json::Value::Object(values) => {
            if values.len() > limits.max_collection_items {
                return Err(ProviderError::BridgeValidation {
                    path: path.into(),
                    message: format!(
                        "{path} exceeded the {} field collection limit",
                        limits.max_collection_items
                    ),
                });
            }
            for (name, value) in values {
                validate_bounded_text(name, path, limits.max_string_bytes)?;
                validate_untyped_json(value, &format!("{path}.{name}"), limits, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn send_hello_error(
    writer: &mut WriteHalf<DynIo>,
    code: &str,
    message: &str,
    max_message_bytes: usize,
) {
    let _ = write_frame(
        writer,
        &BridgeMessage::HelloError {
            code: code.into(),
            message: message.into(),
        },
        max_message_bytes,
    )
    .await;
}

trait BridgeIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> BridgeIo for T {}
type DynIo = Box<dyn BridgeIo>;

struct StdioIo {
    reader: ChildStdout,
    writer: ChildStdin,
}

impl AsyncRead for StdioIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.reader).poll_read(context, buffer)
    }
}

impl AsyncWrite for StdioIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.writer).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.writer).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.writer).poll_shutdown(context)
    }
}

async fn start_application_process(
    application: &AppProcessConfig,
    project_root: &std::path::Path,
    max_stderr_bytes: usize,
    shutdown_timeout: Duration,
) -> Result<ProcessResources, ProviderError> {
    if !application.owned {
        wait_for_health(application.health.as_ref()).await?;
        return Ok(ProcessResources::default());
    }
    if application.command.is_empty() {
        return Err(ProviderError::BridgeProcess {
            message: "runner-owned [app] requires a non-empty command".into(),
        });
    }
    let child = spawn_application(application, project_root, &[], max_stderr_bytes)?;
    let mut resources = ProcessResources {
        children: vec![child],
        endpoint_directory: None,
    };
    if let Err(error) = wait_for_health(application.health.as_ref()).await {
        terminate_process(&mut resources, shutdown_timeout).await;
        return Err(error);
    }
    Ok(resources)
}

fn spawn_application(
    application: &AppProcessConfig,
    project_root: &std::path::Path,
    bridge_environment: &[(&str, &str)],
    max_stderr_bytes: usize,
) -> Result<ManagedProcess, ProviderError> {
    let working_directory = if application.working_directory.is_absolute() {
        application.working_directory.clone()
    } else {
        project_root.join(&application.working_directory)
    };
    let mut command = Command::new(&application.command);
    command
        .args(&application.args)
        .current_dir(&working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(application.owned)
        .envs(&application.environment);
    for (name, value) in bridge_environment {
        command.env(name, value);
    }
    info!(
        executable = %application.command,
        working_directory = %working_directory.display(),
        "spawning configured application"
    );
    let mut child = command
        .spawn()
        .map_err(|error| ProviderError::BridgeTransport {
            message: format!(
                "could not start application executable `{}`: {error}",
                application.command
            ),
        })?;
    let stderr = child
        .stderr
        .take()
        .map(|stderr| capture_stderr(stderr, max_stderr_bytes));
    Ok(ManagedProcess { child, stderr })
}

fn capture_stderr(mut stderr: tokio::process::ChildStderr, limit: usize) -> StderrCapture {
    let state = Arc::new(StdMutex::new(CapturedStderr::default()));
    let output = Arc::clone(&state);
    let finished = Arc::new(Notify::new());
    let completion = Arc::clone(&finished);
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;

        let mut buffer = [0u8; 8_192];
        loop {
            let read = match stderr.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    warn!(%error, "could not read runner-owned application stderr");
                    break;
                }
            };
            let mut captured = output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            captured.total = captured.total.saturating_add(read);
            if read >= limit {
                captured.bytes.clear();
                captured
                    .bytes
                    .extend_from_slice(&buffer[read.saturating_sub(limit)..read]);
            } else {
                let overflow = captured
                    .bytes
                    .len()
                    .saturating_add(read)
                    .saturating_sub(limit);
                if overflow > 0 {
                    captured.bytes.drain(..overflow);
                }
                captured.bytes.extend_from_slice(&buffer[..read]);
            }
        }
        output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .done = true;
        completion.notify_one();
    });
    StderrCapture { state, finished }
}

async fn wait_for_health(health: Option<&HealthCheck>) -> Result<(), ProviderError> {
    let Some(health) = health else {
        return Ok(());
    };
    let started = Instant::now();
    while started.elapsed() < health.timeout {
        let url = health.url.clone();
        let ready = tokio::task::spawn_blocking(move || {
            ureq::get(&url)
                .timeout(Duration::from_millis(250))
                .call()
                .is_ok()
        })
        .await
        .unwrap_or(false);
        if ready {
            info!(url = %health.url, "application health check ready");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(ProviderError::BridgeHandshake {
        code: "health_timeout".into(),
        message: format!(
            "application health check `{}` was not ready within {}ms",
            health.url,
            health.timeout.as_millis()
        ),
    })
}

async fn terminate_process(resources: &mut ProcessResources, timeout: Duration) {
    for process in &mut resources.children {
        if let Ok(Some(_)) = process.child.try_wait() {
            continue;
        }
        let _ = process.child.start_kill();
        info!("terminating runner-owned application process");
        if tokio::time::timeout(timeout, process.child.wait())
            .await
            .is_err()
        {
            warn!("application process did not exit before the teardown deadline");
        }
    }
    resources.children.clear();
    resources.endpoint_directory = None;
}

fn drain_stderr(stderr: tokio::process::ChildStderr, limit: usize) {
    tokio::spawn(async move {
        if let Ok((_, total, truncated)) = read_bounded_stream(stderr, limit).await
            && total > 0
        {
            warn!(
                bytes = total,
                truncated, "application bridge wrote to stderr"
            );
        }
    });
}

async fn read_bounded_stream(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, usize, bool)> {
    use tokio::io::AsyncReadExt;
    let mut captured = Vec::with_capacity(limit.min(8_192));
    let mut total = 0usize;
    let mut buffer = [0u8; 8_192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        let remaining = limit.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
    }
    Ok((captured, total, total > limit))
}

fn compatibility_response(
    bytes: &[u8],
    limits: SchemaLimits,
) -> Result<serde_json::Value, ProviderError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| ProviderError::BridgeProtocol {
            code: "invalid_json".into(),
            message: error.to_string(),
        })?;
    if let Some(value) = value.get("value") {
        return Ok(value.clone());
    }
    if let Some(error) = value.get("error") {
        let data = error
            .get("data")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        validate_untyped_json(&data, "$.error.data", limits, 0)?;
        return Err(ProviderError::Application {
            code: error
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("application.error")
                .into(),
            message: error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("application function failed")
                .into(),
            retryable: error
                .get("retryable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            data,
        });
    }
    Err(ProviderError::BridgeProtocol {
        code: "invalid_compatibility_response".into(),
        message: "adapter response must contain `value` or `error`".into(),
    })
}

fn command_parts(command: &[String]) -> Result<(&str, &[String]), ProviderError> {
    command
        .split_first()
        .map(|(program, args)| (program.as_str(), args))
        .ok_or_else(|| ProviderError::BridgeTransport {
            message: "application adapter command is empty".into(),
        })
}

fn schema_validation_error(error: AppSchemaError) -> ProviderError {
    let message = error.to_string();
    let path = message
        .split_whitespace()
        .find(|part| part.starts_with('$'))
        .unwrap_or("$")
        .trim_end_matches(':')
        .to_owned();
    ProviderError::BridgeValidation { path, message }
}

fn protocol_from_frame(error: crate::ProtocolError) -> ProviderError {
    ProviderError::BridgeProtocol {
        code: error.code.into(),
        message: error.message,
    }
}

fn protocol_error(code: &str, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::BridgeProtocol {
        code: code.into(),
        message: error.to_string(),
    }
}

fn transport_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::BridgeTransport {
        message: error.to_string(),
    }
}

fn local_ipc_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::BridgeTransport {
        message: format!("local IPC unavailable: {error}"),
    }
}

fn is_local_ipc_unavailable(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::BridgeTransport { message }
            if message.starts_with("local IPC unavailable:")
    )
}

async fn bridge_process_error(
    process: &ManagedProcess,
    status: std::io::Result<std::process::ExitStatus>,
) -> ProviderError {
    let mut message = match status {
        Ok(status) => format!("owned application exited with {status}"),
        Err(error) => format!("could not observe owned application status: {error}"),
    };
    if let Some(stderr) = &process.stderr {
        let finished = stderr.finished.notified();
        let done = stderr
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .done;
        if !done {
            let _ = tokio::time::timeout(Duration::from_millis(100), finished).await;
        }
        let captured = stderr
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let text = String::from_utf8_lossy(&captured.bytes);
        let lines = text.trim().lines().rev().take(10).collect::<Vec<_>>();
        if !lines.is_empty() {
            let summary = lines.into_iter().rev().collect::<Vec<_>>().join("\n  ");
            message.push_str("\nstderr:\n  ");
            message.push_str(&summary);
        }
        if captured.total > captured.bytes.len() {
            message.push_str(&format!(
                "\n  … {} additional stderr byte(s) omitted",
                captured.total - captured.bytes.len()
            ));
        }
    }
    ProviderError::BridgeProcess { message }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn random_secret() -> Result<String, ProviderError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| ProviderError::BridgeTransport {
        message: format!("could not generate per-run bridge credentials: {error}"),
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::{FieldSchema, FunctionSchema};

    fn application_config(command: &str, owned: bool) -> AppProcessConfig {
        AppProcessConfig {
            command: command.into(),
            args: Vec::new(),
            working_directory: PathBuf::from("."),
            environment: BTreeMap::new(),
            owned,
            health: None,
        }
    }

    #[tokio::test]
    async fn standalone_application_lifecycle_rejects_an_owned_empty_command() {
        let project = tempfile::tempdir().expect("project");
        let lifecycle = ApplicationLifecycle::new(application_config("", true));
        let error = lifecycle
            .start(project.path())
            .await
            .expect_err("empty owned command");
        assert!(matches!(
            error,
            ProviderError::BridgeProcess { message }
                if message.contains("requires a non-empty command")
        ));
        lifecycle.shutdown().await.expect("shutdown failed state");
    }

    #[tokio::test]
    async fn standalone_external_application_waits_for_health_without_spawning() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("health listener");
        let address = listener.local_addr().expect("health address");
        let response = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("health request");
            let mut request = [0u8; 512];
            let _ = stream.read(&mut request).expect("read health request");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .expect("write health response");
        });
        let mut config = application_config("", false);
        config.health = Some(HealthCheck {
            url: format!("http://{address}/health"),
            timeout: Duration::from_secs(2),
        });
        let project = tempfile::tempdir().expect("project");
        let lifecycle = ApplicationLifecycle::new(config);
        lifecycle.start(project.path()).await.expect("health ready");
        lifecycle
            .start(project.path())
            .await
            .expect("idempotent start");
        lifecycle.shutdown().await.expect("shutdown external app");
        response.join().expect("health thread");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn standalone_owned_application_is_finalized() {
        let project = tempfile::tempdir().expect("project");
        let mut config = application_config("sh", true);
        config.args = vec!["-c".into(), "while :; do sleep 1; done".into()];
        let lifecycle = ApplicationLifecycle::new(config);
        lifecycle.start(project.path()).await.expect("start app");
        let process_id = {
            let state = lifecycle.state.lock().await;
            let ApplicationState::Started(resources) = &*state else {
                panic!("started application state")
            };
            resources.children[0].child.id().expect("process id")
        };
        lifecycle.shutdown().await.expect("shutdown app");
        let status = std::process::Command::new("kill")
            .args(["-0", &process_id.to_string()])
            .stderr(Stdio::null())
            .status()
            .expect("probe process");
        assert!(!status.success(), "application process still exists");
    }

    fn manifest() -> AppManifest {
        AppManifest {
            manifest_version: 1,
            protocol: 1,
            provider: "app".into(),
            sdk: "fixture".into(),
            sdk_version: "1".into(),
            schema_hash: String::new(),
            functions: [(
                "echo".into(),
                FunctionSchema {
                    documentation: "Echo a value.".into(),
                    retry_safe: true,
                    params: TypeSchema::Object {
                        fields: [(
                            "value".into(),
                            FieldSchema {
                                ty: TypeSchema::String,
                                documentation: String::new(),
                                optional: false,
                                secret: false,
                                default: None,
                            },
                        )]
                        .into_iter()
                        .collect(),
                    },
                    returns: TypeSchema::String,
                },
            )]
            .into_iter()
            .collect(),
        }
        .with_computed_hash()
        .expect("hash")
    }

    async fn fixture_bridge(stream: tokio::io::DuplexStream, token: String, manifest: AppManifest) {
        let (read, mut write) = tokio::io::split(stream);
        let mut read = BufReader::new(read);
        write_frame(
            &mut write,
            &BridgeMessage::Hello {
                protocol_versions: vec![1],
                sdk: "fixture".into(),
                sdk_version: "1".into(),
                token,
                capabilities: Default::default(),
            },
            DEFAULT_MAX_MESSAGE_BYTES,
        )
        .await
        .expect("hello");
        assert!(matches!(
            read_frame(&mut read, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .expect("hello ok"),
            Some(BridgeMessage::HelloOk { .. })
        ));
        assert!(matches!(
            read_frame(&mut read, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .expect("describe"),
            Some(BridgeMessage::Describe { id: 1 })
        ));
        write_frame(
            &mut write,
            &BridgeMessage::Schema {
                id: 1,
                protocol: 1,
                schema_hash: manifest.schema_hash.clone(),
                functions: manifest.functions.clone(),
            },
            DEFAULT_MAX_MESSAGE_BYTES,
        )
        .await
        .expect("schema");
        while let Some(message) = read_frame(&mut read, DEFAULT_MAX_MESSAGE_BYTES)
            .await
            .expect("request")
        {
            match message {
                BridgeMessage::Call { id, arguments, .. } => {
                    write_frame(
                        &mut write,
                        &BridgeMessage::Result {
                            id,
                            value: arguments["value"].clone(),
                        },
                        DEFAULT_MAX_MESSAGE_BYTES,
                    )
                    .await
                    .expect("result");
                }
                BridgeMessage::Shutdown { id } => {
                    write_frame(
                        &mut write,
                        &BridgeMessage::ShutdownOk { id },
                        DEFAULT_MAX_MESSAGE_BYTES,
                    )
                    .await
                    .expect("shutdown");
                    break;
                }
                _ => {}
            }
        }
    }

    async fn delayed_bridge(stream: tokio::io::DuplexStream, token: String, manifest: AppManifest) {
        let (read, mut write) = tokio::io::split(stream);
        let mut read = BufReader::new(read);
        write_frame(
            &mut write,
            &BridgeMessage::Hello {
                protocol_versions: vec![1],
                sdk: "fixture".into(),
                sdk_version: "1".into(),
                token,
                capabilities: crate::BridgeCapabilities {
                    cancel: true,
                    events: false,
                },
            },
            DEFAULT_MAX_MESSAGE_BYTES,
        )
        .await
        .expect("hello");
        let _ = read_frame(&mut read, DEFAULT_MAX_MESSAGE_BYTES)
            .await
            .expect("hello ok");
        let Some(BridgeMessage::Describe { id }) = read_frame(&mut read, DEFAULT_MAX_MESSAGE_BYTES)
            .await
            .expect("describe")
        else {
            panic!("describe request")
        };
        write_frame(
            &mut write,
            &BridgeMessage::Schema {
                id,
                protocol: 1,
                schema_hash: manifest.schema_hash,
                functions: manifest.functions,
            },
            DEFAULT_MAX_MESSAGE_BYTES,
        )
        .await
        .expect("schema");
        let write = Arc::new(Mutex::new(write));
        while let Some(message) = read_frame(&mut read, DEFAULT_MAX_MESSAGE_BYTES)
            .await
            .expect("request")
        {
            match message {
                BridgeMessage::Call { id, arguments, .. } => {
                    let delay = if arguments["value"] == "slow" { 40 } else { 0 };
                    let value = arguments["value"].clone();
                    let write = Arc::clone(&write);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        write_frame(
                            &mut *write.lock().await,
                            &BridgeMessage::Result { id, value },
                            DEFAULT_MAX_MESSAGE_BYTES,
                        )
                        .await
                        .expect("result");
                    });
                }
                BridgeMessage::Cancel { .. } => {}
                BridgeMessage::Shutdown { id } => {
                    write_frame(
                        &mut *write.lock().await,
                        &BridgeMessage::ShutdownOk { id },
                        DEFAULT_MAX_MESSAGE_BYTES,
                    )
                    .await
                    .expect("shutdown");
                    break;
                }
                _ => {}
            }
        }
    }

    async fn hello_only(stream: tokio::io::DuplexStream, token: &str) -> Option<BridgeMessage> {
        let (read, mut write) = tokio::io::split(stream);
        let mut read = BufReader::new(read);
        write_frame(
            &mut write,
            &BridgeMessage::Hello {
                protocol_versions: vec![1],
                sdk: "fixture".into(),
                sdk_version: "1".into(),
                token: token.into(),
                capabilities: Default::default(),
            },
            DEFAULT_MAX_MESSAGE_BYTES,
        )
        .await
        .expect("hello");
        read_frame(&mut read, DEFAULT_MAX_MESSAGE_BYTES)
            .await
            .expect("response")
    }

    fn echo_call(manifest: &AppManifest, value: &str) -> ProviderCall {
        ProviderCall {
            provider: webtest_provider::ProviderName("app".into()),
            operation: webtest_provider::OperationName("echo".into()),
            arguments: [("value".into(), Value::String(value.into()))]
                .into_iter()
                .collect(),
            schema_hash: manifest.schema_hash.clone(),
        }
    }

    fn call_context() -> CallContext {
        CallContext {
            project_root: std::env::current_dir().expect("current directory"),
            timeout: Duration::from_secs(2),
            redacted_json_fields: Vec::new(),
        }
    }

    #[tokio::test]
    async fn in_memory_transport_handshakes_calls_and_shuts_down() {
        let manifest = manifest();
        let token = "secret".to_owned();
        let (runner, bridge) = tokio::io::duplex(16_384);
        tokio::spawn(fixture_bridge(bridge, token.clone(), manifest.clone()));
        let client = BridgeClient::handshake(
            Box::new(runner),
            token,
            "run".into(),
            &manifest,
            &AppProviderConfig::default(),
        )
        .await
        .expect("handshake");
        assert_eq!(
            client
                .call(
                    "echo",
                    serde_json::json!({"value": "héllo"}),
                    Duration::from_secs(1)
                )
                .await
                .expect("call"),
            serde_json::json!("héllo")
        );
        client
            .shutdown(Duration::from_secs(1))
            .await
            .expect("shutdown");
    }

    #[tokio::test]
    async fn late_terminal_after_cancellation_does_not_poison_later_calls() {
        let manifest = manifest();
        let token = "secret".to_owned();
        let (runner, bridge) = tokio::io::duplex(16_384);
        tokio::spawn(delayed_bridge(bridge, token.clone(), manifest.clone()));
        let client = BridgeClient::handshake(
            Box::new(runner),
            token,
            "run".into(),
            &manifest,
            &AppProviderConfig::default(),
        )
        .await
        .expect("handshake");
        assert!(matches!(
            client
                .call(
                    "echo",
                    serde_json::json!({"value": "slow"}),
                    Duration::from_millis(5),
                )
                .await,
            Err(ProviderError::BridgeTimeout { .. })
        ));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            client
                .call(
                    "echo",
                    serde_json::json!({"value": "fast"}),
                    Duration::from_secs(1),
                )
                .await
                .expect("later call"),
            serde_json::json!("fast")
        );
        client
            .shutdown(Duration::from_secs(1))
            .await
            .expect("shutdown");
    }

    #[tokio::test]
    async fn dropping_a_call_future_reserves_correlation_and_sends_cancel_cleanup() {
        let manifest = manifest();
        let token = "secret".to_owned();
        let (runner, bridge) = tokio::io::duplex(16_384);
        tokio::spawn(delayed_bridge(bridge, token.clone(), manifest.clone()));
        let client = Arc::new(
            BridgeClient::handshake(
                Box::new(runner),
                token,
                "run".into(),
                &manifest,
                &AppProviderConfig::default(),
            )
            .await
            .expect("handshake"),
        );
        let active = {
            let client = Arc::clone(&client);
            tokio::spawn(async move {
                client
                    .call(
                        "echo",
                        serde_json::json!({"value": "slow"}),
                        Duration::from_secs(30),
                    )
                    .await
            })
        };
        for _ in 0..20 {
            if !client.pending.lock().await.requests.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        active.abort();
        let _ = active.await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        {
            let pending = client.pending.lock().await;
            assert!(pending.requests.is_empty());
            assert_eq!(pending.cancelled.get(&2), Some(&ExpectedResponse::Call));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(client.pending.lock().await.cancelled.is_empty());
        client
            .shutdown(Duration::from_secs(1))
            .await
            .expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_adapter_uses_one_bounded_json_document_per_call() {
        let manifest = manifest();
        let provider = AppProvider::new(
            manifest.clone(),
            AppProviderConfig {
                adapter: AppAdapter::Command,
                command: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf '%s' '{\"value\":\"from-command\"}'".into(),
                ],
                ..AppProviderConfig::default()
            },
        )
        .expect("command provider");
        let result = provider
            .call(echo_call(&manifest, "input"), call_context())
            .await
            .expect("command call");
        assert_eq!(result.value, Value::String("from-command".into()));
        provider.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn http_adapter_calls_only_the_explicit_function_endpoint() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("read timeout");
            let mut request = Vec::new();
            let mut buffer = [0u8; 4_096];
            let (header_end, content_length) = loop {
                let count = stream.read(&mut buffer).expect("request bytes");
                assert!(count > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                {
                    let headers = std::str::from_utf8(&request[..header_end]).expect("headers");
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("content length"))
                        })
                        .expect("content-length header");
                    break (header_end + 4, content_length);
                }
            };
            while request.len() < header_end + content_length {
                let count = stream.read(&mut buffer).expect("request body");
                assert!(count > 0, "request ended before body");
                request.extend_from_slice(&buffer[..count]);
            }
            let request_text = String::from_utf8(request).expect("UTF-8 request");
            assert!(request_text.starts_with("POST /fixture HTTP/1.1\r\n"));
            assert!(request_text.ends_with("{\"value\":\"input\"}"));
            let body = br#"{"value":"from-http"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("response headers");
            stream.write_all(body).expect("response body");
        });

        let manifest = manifest();
        let provider = AppProvider::new(
            manifest.clone(),
            AppProviderConfig {
                adapter: AppAdapter::Http,
                http: AppHttpConfig {
                    base_url: format!("http://{address}/"),
                    operations: [(
                        "echo".into(),
                        HttpOperation {
                            method: "POST".into(),
                            path: "fixture".into(),
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
                ..AppProviderConfig::default()
            },
        )
        .expect("HTTP provider");
        let result = provider
            .call(echo_call(&manifest, "input"), call_context())
            .await
            .expect("HTTP call");
        assert_eq!(result.value, Value::String("from-http".into()));
        provider.shutdown().await.expect("shutdown");
        server.join().expect("server");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owned_process_death_fails_readiness_promptly_without_transport_fallback() {
        let project = tempfile::tempdir().expect("project");
        let provider = AppProvider::new(
            manifest(),
            AppProviderConfig {
                transport: AppTransport::Auto,
                application: Some(AppProcessConfig {
                    command: "/bin/sh".into(),
                    args: vec!["-c".into(), "exit 23".into()],
                    working_directory: PathBuf::from("."),
                    environment: BTreeMap::new(),
                    owned: true,
                    health: None,
                }),
                startup_timeout: Duration::from_secs(5),
                ..AppProviderConfig::default()
            },
        )
        .expect("provider");
        let started = Instant::now();
        assert!(matches!(
            provider.start(project.path()).await,
            Err(ProviderError::BridgeProcess { ref message }) if message.contains("23")
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        provider.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owned_process_death_captures_and_surfaces_stderr() {
        let project = tempfile::tempdir().expect("project");
        let provider = AppProvider::new(
            manifest(),
            AppProviderConfig {
                transport: AppTransport::Auto,
                application: Some(AppProcessConfig {
                    command: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        "echo 'Error: listen EADDRINUSE :::3000' >&2; exit 1".into(),
                    ],
                    working_directory: PathBuf::from("."),
                    environment: BTreeMap::new(),
                    owned: true,
                    health: None,
                }),
                startup_timeout: Duration::from_secs(5),
                ..AppProviderConfig::default()
            },
        )
        .expect("provider");
        let result = provider.start(project.path()).await;
        match result {
            Err(ProviderError::BridgeProcess { message }) => {
                assert!(message.contains("owned application exited with exit status: 1"));
                assert!(message.contains("stderr:"));
                assert!(message.contains("Error: listen EADDRINUSE :::3000"));
            }
            res => panic!("expected BridgeProcess error with stderr, got {res:?}"),
        }
        provider.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owned_process_stderr_capture_is_bounded() {
        let project = tempfile::tempdir().expect("project");
        let provider = AppProvider::new(
            manifest(),
            AppProviderConfig {
                transport: AppTransport::Auto,
                application: Some(AppProcessConfig {
                    command: "/bin/sh".into(),
                    args: vec!["-c".into(), "printf 'abcdefghijk\\n' >&2; exit 1".into()],
                    working_directory: PathBuf::from("."),
                    environment: BTreeMap::new(),
                    owned: true,
                    health: None,
                }),
                startup_timeout: Duration::from_secs(5),
                max_stderr_bytes: 8,
                ..AppProviderConfig::default()
            },
        )
        .expect("provider");
        let result = provider.start(project.path()).await;
        match result {
            Err(ProviderError::BridgeProcess { message }) => {
                assert!(message.contains("stderr:\n  efghijk"));
                assert!(message.contains("additional stderr byte(s) omitted"));
                assert!(!message.contains("abcdefghijk"));
            }
            res => panic!("expected BridgeProcess error with bounded stderr, got {res:?}"),
        }
        provider.shutdown().await.expect("shutdown");
    }

    #[test]
    fn token_comparison_handles_different_lengths() {
        assert!(constant_time_equal(b"same", b"same"));
        assert!(!constant_time_equal(b"same", b"same!"));
        assert!(!constant_time_equal(b"left", b"right"));
    }

    #[tokio::test]
    async fn authentication_failure_is_distinct_and_returns_hello_error() {
        let expected = manifest();
        let (runner, bridge) = tokio::io::duplex(4_096);
        let peer = tokio::spawn(async move { hello_only(bridge, "wrong").await });
        let Err(error) = BridgeClient::handshake(
            Box::new(runner),
            "expected".into(),
            "run".into(),
            &expected,
            &AppProviderConfig::default(),
        )
        .await
        else {
            panic!("authentication must fail")
        };
        assert!(matches!(
            error,
            ProviderError::BridgeHandshake { ref code, .. } if code == "authentication_failed"
        ));
        assert!(matches!(
            peer.await.expect("peer"),
            Some(BridgeMessage::HelloError { code, .. }) if code == "authentication_failed"
        ));
    }

    #[tokio::test]
    async fn live_schema_drift_is_rejected_before_calls() {
        let expected = manifest();
        let mut live = expected.clone();
        live.functions.get_mut("echo").expect("echo").documentation = "Changed live schema.".into();
        live.schema_hash = live.computed_hash().expect("live hash");
        let token = "secret".to_owned();
        let (runner, bridge) = tokio::io::duplex(16_384);
        tokio::spawn(fixture_bridge(bridge, token.clone(), live.clone()));
        let Err(error) = BridgeClient::handshake(
            Box::new(runner),
            token,
            "run".into(),
            &expected,
            &AppProviderConfig::default(),
        )
        .await
        else {
            panic!("schema drift")
        };
        assert_eq!(
            error,
            ProviderError::BridgeSchemaDrift {
                expected: expected.schema_hash,
                live: live.schema_hash,
            }
        );
    }

    #[tokio::test]
    async fn stale_declared_live_hash_uses_the_canonical_functions_hash() {
        let expected = manifest();
        let mut live = expected.clone();
        live.schema_hash = format!("blake3:{}", "0".repeat(64));
        let token = "secret".to_owned();
        let (runner, bridge) = tokio::io::duplex(16_384);
        let peer = tokio::spawn(fixture_bridge(bridge, token.clone(), live));

        let client = BridgeClient::handshake(
            Box::new(runner),
            token,
            "run".into(),
            &expected,
            &AppProviderConfig::default(),
        )
        .await
        .expect("canonical live functions match the offline manifest");
        client
            .shutdown(Duration::from_secs(1))
            .await
            .expect("shutdown");
        peer.await.expect("bridge task");
    }

    #[test]
    fn invalid_live_result_is_a_structured_validation_failure() {
        let provider =
            AppProvider::new(manifest(), AppProviderConfig::default()).expect("provider");
        assert!(matches!(
            provider.checked_result("echo", serde_json::json!(42)),
            Err(ProviderError::BridgeValidation { .. })
        ));
    }
}
