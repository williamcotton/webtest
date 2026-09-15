//! Deterministic trace archives over the authoritative observation journal.
//!
//! This component never executes project code, opens paths from events, or
//! extracts archive members to disk. Callers supply already-redacted captures
//! and explicitly opt into source inclusion. Storage/retention policy and UI
//! belong to adapters. An exceeded limit is an error, never silent data loss.
mod archive;
mod validation;

use std::collections::BTreeMap;
use webtest_observation::{EventIdentity, ExecutionId, RecordedEvent};
use webtest_text::{FileId, SourceRevision};

pub use archive::{read, write};

pub const TRACE_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug)]
pub struct TraceLimits {
    pub max_archive_bytes: usize,
    pub max_total_bytes: usize,
    pub max_entry_bytes: usize,
    pub max_manifest_bytes: usize,
    pub max_record_bytes: usize,
    pub max_entries: usize,
    pub max_events: usize,
}

impl Default for TraceLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 104_857_600,
            max_total_bytes: 104_857_600,
            max_entry_bytes: 100_000_000,
            max_manifest_bytes: 1_048_576,
            max_record_bytes: 1_048_576,
            max_entries: 10_000,
            max_events: 100_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePolicy {
    pub sources: bool,
    pub attachments: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceMetadata {
    pub compiler_version: String,
    pub project_identity: [u8; 32],
    /// Digest only: resolved configuration values and credentials do not belong here.
    pub configuration_fingerprint: [u8; 32],
    pub browser_version: Option<String>,
    pub provider_versions: BTreeMap<String, String>,
    pub capture: CapturePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceFile {
    pub path: String,
    pub byte_length: u64,
    pub blake3: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSource {
    pub execution_id: ExecutionId,
    pub file_id: FileId,
    pub revision: SourceRevision,
    pub project_path: String,
    pub file: Option<TraceFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceAttachment {
    /// Lookup uses this immutable identity, never the historical host path in an event.
    pub event: EventIdentity,
    pub file: Option<TraceFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceManifest {
    pub format_version: u32,
    pub event_schema_version: u32,
    pub metadata: TraceMetadata,
    pub executions: Vec<ExecutionId>,
    pub events: TraceFile,
    pub sources: Vec<TraceSource>,
    pub attachments: Vec<TraceAttachment>,
}

pub struct SourceInput<'a> {
    pub execution_id: ExecutionId,
    pub file_id: FileId,
    pub revision: SourceRevision,
    pub project_path: &'a str,
    pub text: Option<&'a str>,
}

pub struct AttachmentInput<'a> {
    pub event: EventIdentity,
    pub bytes: &'a [u8],
}

pub struct TraceInput<'a> {
    pub metadata: TraceMetadata,
    pub records: &'a [RecordedEvent],
    pub sources: &'a [SourceInput<'a>],
    pub attachments: &'a [AttachmentInput<'a>],
}

/// Constructed only after every member and journal record has been validated.
pub struct VerifiedTrace {
    manifest: TraceManifest,
    records: Vec<RecordedEvent>,
    files: BTreeMap<String, Vec<u8>>,
}

impl VerifiedTrace {
    pub fn manifest(&self) -> &TraceManifest {
        &self.manifest
    }
    pub fn records(&self) -> &[RecordedEvent] {
        &self.records
    }
    pub fn attachment(&self, identity: EventIdentity) -> Option<&[u8]> {
        let file = self
            .manifest
            .attachments
            .iter()
            .find(|a| a.event == identity)?
            .file
            .as_ref()?;
        self.files.get(&file.path).map(Vec::as_slice)
    }
    pub fn source(&self, execution: ExecutionId, file_id: FileId) -> Option<&str> {
        let file = self
            .manifest
            .sources
            .iter()
            .find(|s| s.execution_id == execution && s.file_id == file_id)?
            .file
            .as_ref()?;
        std::str::from_utf8(self.files.get(&file.path)?).ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    ArchiveBytes,
    TotalBytes,
    EntryBytes,
    ManifestBytes,
    RecordBytes,
    Entries,
    Events,
}

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("trace limit exceeded: {kind:?} (limit {limit})")]
    Limit { kind: LimitKind, limit: usize },
    #[error("unsupported trace format {0}")]
    UnsupportedFormat(u32),
    #[error("unsupported trace event schema {0}")]
    UnsupportedEventSchema(u32),
    #[error("unsafe trace path: {0}")]
    UnsafePath(String),
    #[error("unsupported archive member: {0}")]
    UnsupportedMember(String),
    #[error("duplicate trace member or reference: {0}")]
    Duplicate(String),
    #[error("missing trace member or reference: {0}")]
    Missing(String),
    #[error("unreferenced trace member: {0}")]
    Unreferenced(String),
    #[error("trace length or checksum mismatch: {0}")]
    Integrity(String),
    #[error("invalid trace source: {0}")]
    Source(String),
    #[error("invalid attachment reference: {0:?}")]
    Attachment(EventIdentity),
    #[error("capture content does not agree with its policy")]
    CapturePolicy,
    #[error("invalid journal record at line {line}: {error:?}")]
    Journal {
        line: usize,
        error: webtest_observation::ReplayDecodeError,
    },
    #[error("trace journal is empty or has an invalid JSONL frame")]
    Framing,
    #[error("trace execution membership does not agree with the journal")]
    Executions,
    #[error("invalid authoritative journal order or loss interval: {0:?}")]
    Sequence(EventIdentity),
    #[error("trace execution has no terminal record: {0:?}")]
    IncompleteExecution(ExecutionId),
    #[error("trace JSON is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("trace archive framing is invalid")]
    ArchiveFraming,
    #[error("trace IO failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests;
