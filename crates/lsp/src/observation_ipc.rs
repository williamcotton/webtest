//! Authenticated, bounded local publication. Independent of App Bridge Protocol.
use super::{DocumentStore, diagnostic_to_lsp};
use futures::{StreamExt, stream::FuturesUnordered};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tower_lsp_server::Client;
use webtest_observation::{ObservationPublication, RuntimeObservationKind};
use webtest_text::SourceRevision;

const PROTOCOL: u32 = webtest_observation::OBSERVATION_IPC_VERSION;
const MAX_FRAME: usize = 8 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    protocol: u32,
    pid: u32,
    workspace_id: String,
    endpoint: SocketAddr,
    token: String,
    created_at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: u32,
    token: String,
    workspace_id: String,
    path: String,
    revision: SourceRevision,
    operation: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Begin {
        provider_schema_hashes: std::collections::BTreeMap<String, String>,
    },
    Complete {
        ticket: u64,
        batch: ObservationPublication,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Accepted { ticket: u64 },
    Rejected,
}

pub struct Publication {
    metadata: Metadata,
    workspace: PathBuf,
    path: String,
    revision: SourceRevision,
    ticket: u64,
}

/// Missing/stale endpoints and rejected optional publication never affect execution.
pub async fn begin(
    project: &Path,
    file: &Path,
    revision: SourceRevision,
    provider_schema_hashes: &std::collections::BTreeMap<String, String>,
) -> Option<Publication> {
    let file = file.canonicalize().ok()?;
    for root in project.canonicalize().ok()?.ancestors() {
        let id = workspace_id(root);
        let directory = private_directory().ok()?;
        let Ok(metadata) = read_metadata(&directory.join(format!("{id}.json"))) else {
            continue;
        };
        if metadata.protocol != PROTOCOL
            || metadata.workspace_id != id
            || !live(metadata.pid)
            || metadata.endpoint.ip() != Ipv4Addr::LOCALHOST
        {
            continue;
        }
        let path = file.strip_prefix(root).ok()?.to_str()?.replace('\\', "/");
        let request = Request {
            protocol: PROTOCOL,
            token: metadata.token.clone(),
            workspace_id: id,
            path: path.clone(),
            revision,
            operation: Operation::Begin {
                provider_schema_hashes: provider_schema_hashes.clone(),
            },
        };
        if let Ok(Reply::Accepted { ticket }) = exchange(&metadata, &request).await {
            return Some(Publication {
                metadata,
                workspace: root.to_path_buf(),
                path,
                revision,
                ticket,
            });
        }
    }
    None
}

impl Publication {
    pub async fn complete(self, mut batch: ObservationPublication) {
        // Convert acknowledged evidence references to workspace-relative names. They
        // remain data: publication never asks the receiver to open evidence files.
        for observation in &mut batch.observations {
            if let RuntimeObservationKind::BrowserFailure { artifacts, .. } = &mut observation.kind
            {
                *artifacts = artifacts
                    .iter()
                    .filter_map(|path| {
                        let path = Path::new(path).canonicalize().ok()?;
                        Some(
                            path.strip_prefix(&self.workspace)
                                .ok()?
                                .to_str()?
                                .replace('\\', "/"),
                        )
                    })
                    .collect();
            }
        }
        let request = Request {
            protocol: PROTOCOL,
            token: self.metadata.token.clone(),
            workspace_id: self.metadata.workspace_id.clone(),
            path: self.path,
            revision: self.revision,
            operation: Operation::Complete {
                ticket: self.ticket,
                batch,
            },
        };
        if !matches!(
            exchange(&self.metadata, &request).await,
            Ok(Reply::Accepted { .. })
        ) {
            tracing::debug!("editor rejected or could not receive completed observations");
        }
    }
}

async fn exchange(metadata: &Metadata, request: &Request) -> io::Result<Reply> {
    tokio::time::timeout(DEADLINE, async {
        let mut stream = TcpStream::connect(metadata.endpoint).await?;
        write_frame(&mut stream, request).await?;
        read_frame(&mut stream).await
    })
    .await
    .map_err(|_| invalid("publication timed out"))?
}

pub(super) struct Endpoint {
    listener: TcpListener,
    root: PathBuf,
    metadata: Metadata,
    metadata_path: PathBuf,
    _lock: File,
}

impl Endpoint {
    pub(super) async fn bind(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        let directory = private_directory()?;
        let id = workspace_id(&root);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.join(format!("{id}.lock")))?;
        private_file(&lock)?;
        if unsafe {
            libc::flock(
                std::os::fd::AsRawFd::as_raw_fd(&lock),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let mut token = [0; 32];
        getrandom::fill(&mut token).map_err(|_| invalid("cannot allocate publication secret"))?;
        let metadata = Metadata {
            protocol: PROTOCOL,
            pid: std::process::id(),
            workspace_id: id.clone(),
            endpoint: listener.local_addr()?,
            token: hex(&token),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        let metadata_path = directory.join(format!("{id}.json"));
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
        serde_json::to_writer(&mut temporary, &metadata).map_err(invalid)?;
        temporary.flush()?;
        temporary
            .persist(&metadata_path)
            .map_err(|error| error.error)?;
        Ok(Self {
            listener,
            root,
            metadata,
            metadata_path,
            _lock: lock,
        })
    }

    pub(super) async fn serve(&self, client: Client, documents: Arc<DocumentStore>) {
        let mut connections = FuturesUnordered::new();
        loop {
            tokio::select! {
                Some(()) = connections.next(), if !connections.is_empty() => {},
                connection = self.listener.accept(), if connections.len() < 8 => {
                    let Ok((stream, _)) = connection else { return; };
                    connections.push(self.handle(stream, &client, &documents));
                }
            }
        }
    }

    async fn handle(&self, mut stream: TcpStream, client: &Client, documents: &DocumentStore) {
        let _ = tokio::time::timeout(DEADLINE, async {
            let request: Request = read_frame(&mut stream).await?;
            let reply = self.apply(request, client, documents).await;
            write_frame(&mut stream, &reply).await
        })
        .await;
    }

    async fn apply(&self, request: Request, client: &Client, documents: &DocumentStore) -> Reply {
        if request.protocol != PROTOCOL
            || request.workspace_id != self.metadata.workspace_id
            || !secret_equal(&request.token, &self.metadata.token)
        {
            return Reply::Rejected;
        }
        let Some(path) = canonical_member(&self.root, &request.path) else {
            return Reply::Rejected;
        };
        let Some(document) = documents.all().into_iter().find(|document| {
            document
                .uri
                .to_file_path()
                .and_then(|path| path.canonicalize().ok())
                .as_ref()
                == Some(&path)
        }) else {
            return Reply::Rejected;
        };
        let ticket = match request.operation {
            Operation::Begin {
                provider_schema_hashes,
            } => document.editor.begin_publication(
                document.file,
                request.revision,
                &provider_schema_hashes,
            ),
            Operation::Complete { ticket, batch } => {
                let artifacts_valid =
                    batch
                        .observations
                        .iter()
                        .all(|observation| match &observation.kind {
                            RuntimeObservationKind::BrowserFailure { artifacts, .. } => artifacts
                                .iter()
                                .all(|path| canonical_member(&self.root, path).is_some()),
                            _ => true,
                        });
                if batch.revision != request.revision || !artifacts_valid {
                    return Reply::Rejected;
                }
                document
                    .editor
                    .complete_publication(document.file, ticket, batch)
                    .then_some(ticket)
            }
        };
        let Some(ticket) = ticket else {
            return Reply::Rejected;
        };
        if let (Ok(source), Ok(diagnostics)) = (
            document.editor.source(document.file),
            document.editor.diagnostics(document.file),
        ) {
            client
                .publish_diagnostics(
                    document.uri,
                    diagnostics
                        .into_iter()
                        .map(|diagnostic| diagnostic_to_lsp(&source, diagnostic))
                        .collect::<Vec<_>>(),
                    Some(document.version),
                )
                .await;
        }
        Reply::Accepted { ticket }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if read_metadata(&self.metadata_path)
            .is_ok_and(|metadata| metadata.token == self.metadata.token)
        {
            let _ = std::fs::remove_file(&self.metadata_path);
        }
        // Keep the lock inode: unlinking a lock permits competing process locks.
    }
}

fn relative_member(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0'])
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}
fn canonical_member(root: &Path, path: &str) -> Option<PathBuf> {
    if !relative_member(path) {
        return None;
    }
    let resolved = root.join(path).canonicalize().ok()?;
    (resolved.starts_with(root) && resolved.is_file()).then_some(resolved)
}
fn workspace_id(root: &Path) -> String {
    hex(SourceRevision::of(&root.to_string_lossy()).as_bytes())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn live(pid: u32) -> bool {
    pid > 0 && pid <= i32::MAX as u32 && unsafe { libc::kill(pid as i32, 0) } == 0
}
fn secret_equal(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}
fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
fn private_directory() -> io::Result<PathBuf> {
    let directory = std::env::temp_dir().join(format!("webtest-observations-{}", unsafe {
        libc::geteuid()
    }));
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(&directory)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid("observation runtime directory is not user-private"));
    }
    Ok(directory)
}
fn private_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid("observation metadata is not user-private"));
    }
    Ok(())
}
fn read_metadata(path: &Path) -> io::Result<Metadata> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    private_file(&file)?;
    let mut bytes = Vec::new();
    file.take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(invalid("oversized endpoint metadata"));
    }
    serde_json::from_slice(&bytes).map_err(invalid)
}
async fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut TcpStream) -> io::Result<T> {
    let length = stream.read_u32().await? as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid("invalid publication frame size"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(invalid)
}
async fn write_frame(stream: &mut TcpStream, value: &impl Serialize) -> io::Result<()> {
    struct Buffer(Vec<u8>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > MAX_FRAME {
                return Err(invalid("publication exceeds byte limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut bytes = Buffer(Vec::new());
    serde_json::to_writer(&mut bytes, value).map_err(invalid)?;
    stream.write_u32(bytes.0.len() as u32).await?;
    stream.write_all(&bytes.0).await
}
