use std::{borrow::Cow, collections::BTreeMap, io::Write};

use crate::*;
use webtest_observation::{EVENT_JOURNAL_SCHEMA_VERSION, ExecutionEvent};

// Version 1: uncompressed regular-file entries only. No extraction, link,
// compression, platform permission, timestamp, or executable entry semantics.
// Magic; u32 LE entry count; repeated u8 kind (0), u32 LE name length,
// u64 LE content length, UTF-8 name, content. No padding or trailing bytes.
const MAGIC: &[u8] = b"WEBTEST-TRACE\0\x01";

type CapturedFiles<'a> = BTreeMap<String, Cow<'a, [u8]>>;

pub(crate) fn limit(actual: usize, maximum: usize, kind: LimitKind) -> Result<(), TraceError> {
    if actual > maximum {
        Err(TraceError::Limit {
            kind,
            limit: maximum,
        })
    } else {
        Ok(())
    }
}

pub(crate) fn descriptor(path: String, bytes: &[u8]) -> TraceFile {
    TraceFile {
        path,
        byte_length: bytes.len() as u64,
        blake3: *blake3::hash(bytes).as_bytes(),
    }
}

pub(crate) fn content_path(directory: &str, hash: &[u8; 32]) -> String {
    format!("{directory}/{}", blake3::Hash::from(*hash).to_hex())
}

/// Strict capture: any exceeded bound returns an error without publishing an archive.
pub fn write(input: TraceInput<'_>, limits: TraceLimits) -> Result<Vec<u8>, TraceError> {
    let (manifest, files) = prepare(input, limits)?;
    finish(manifest, files, limits)
}

/// Preserve the entire journal and omit optional content with typed export facts
/// when capture bounds are reached. Mandatory metadata/journal failure is an error.
pub fn write_budgeted(input: TraceInput<'_>, limits: TraceLimits) -> Result<Vec<u8>, TraceError> {
    let (mut manifest, mut files) = prepare(input, limits)?;
    // Validate supplied bytes before selection. A budget must never conceal a
    // corrupt digest, invalid source, or missing capture promised by policy.
    validation::validate(manifest.clone(), &files, limits)?;
    capture::select(&mut manifest, &mut files, limits)?;
    finish(manifest, files, limits)
}

fn prepare(
    input: TraceInput<'_>,
    limits: TraceLimits,
) -> Result<(TraceManifest, CapturedFiles<'_>), TraceError> {
    limit(input.records.len(), limits.max_events, LimitKind::Events)?;
    let (event_limit, event_limit_kind) = [
        (limits.max_entry_bytes, LimitKind::EntryBytes),
        (limits.max_total_bytes, LimitKind::TotalBytes),
        (limits.max_archive_bytes, LimitKind::ArchiveBytes),
    ]
    .into_iter()
    .min_by_key(|(bytes, _)| *bytes)
    .ok_or(TraceError::ArchiveFraming)?;
    let mut events = BoundedBytes::new(event_limit, event_limit_kind);
    for record in input.records {
        let bytes = json(record, limits.max_record_bytes, LimitKind::RecordBytes)?;
        events.append(&bytes)?;
        events.append(b"\n")?;
    }
    let mut manifest = TraceManifest {
        format_version: TRACE_FORMAT_VERSION,
        event_schema_version: EVENT_JOURNAL_SCHEMA_VERSION,
        metadata: input.metadata,
        executions: input
            .records
            .iter()
            .map(|r| r.identity.execution_id)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        events: descriptor("events.jsonl".into(), &events.bytes),
        sources: Vec::new(),
        attachments: Vec::new(),
        capture_report: None,
    };
    let mut files = BTreeMap::from([("events.jsonl".into(), Cow::Owned(events.bytes))]);
    for source in input.sources {
        let file = if let Some(text) = source.text {
            let path = content_path("sources", source.revision.as_bytes());
            insert_capture(&mut files, path.clone(), text.as_bytes())?;
            Some(descriptor(path, text.as_bytes()))
        } else {
            None
        };
        manifest.sources.push(TraceSource {
            execution_id: source.execution_id,
            file_id: source.file_id,
            revision: source.revision,
            project_path: source.project_path.into(),
            file,
        });
    }
    manifest
        .sources
        .sort_by_key(|s| (s.execution_id, s.file_id.get()));
    let mut supplied = BTreeMap::new();
    for attachment in input.attachments {
        if supplied
            .insert(attachment.event, attachment.bytes)
            .is_some()
        {
            return Err(TraceError::Attachment(attachment.event));
        }
    }
    for record in input.records {
        if let ExecutionEvent::AttachmentCreated { attachment, .. } = &record.event {
            let file = if let Some(bytes) = supplied.remove(&record.identity) {
                let path = content_path("attachments", &attachment.blake3);
                insert_capture(&mut files, path.clone(), bytes)?;
                Some(descriptor(path, bytes))
            } else {
                None
            };
            manifest.attachments.push(TraceAttachment {
                event: record.identity,
                file,
            });
        }
    }
    if let Some((identity, _)) = supplied.first_key_value() {
        return Err(TraceError::Attachment(*identity));
    }
    manifest.attachments.sort_by_key(|a| a.event);
    Ok((manifest, files))
}

fn insert_capture<'a>(
    files: &mut CapturedFiles<'a>,
    path: String,
    bytes: &'a [u8],
) -> Result<(), TraceError> {
    if let Some(existing) = files.get(&path) {
        if existing.as_ref() != bytes {
            return Err(TraceError::Integrity(path));
        }
    } else {
        files.insert(path, Cow::Borrowed(bytes));
    }
    Ok(())
}

fn finish(
    mut manifest: TraceManifest,
    mut files: CapturedFiles<'_>,
    limits: TraceLimits,
) -> Result<Vec<u8>, TraceError> {
    if let Some(report) = &mut manifest.capture_report {
        report.events.sort_by(|a, b| {
            capture::event_file(a)
                .path
                .cmp(&capture::event_file(b).path)
        });
    }
    let bytes = json(
        &manifest,
        limits.max_manifest_bytes,
        LimitKind::ManifestBytes,
    )?;
    files.insert("manifest.json".into(), Cow::Owned(bytes));
    check_sizes(&files, limits)?;
    validation::validate(manifest, &files, limits)?;
    encode(&files, limits)
}

pub(crate) fn check_sizes(
    files: &BTreeMap<String, impl AsRef<[u8]>>,
    limits: TraceLimits,
) -> Result<(), TraceError> {
    limit(files.len(), limits.max_entries, LimitKind::Entries)?;
    let mut total = 0usize;
    for bytes in files.values() {
        limit(
            bytes.as_ref().len(),
            limits.max_entry_bytes,
            LimitKind::EntryBytes,
        )?;
        total = total.saturating_add(bytes.as_ref().len());
    }
    limit(total, limits.max_total_bytes, LimitKind::TotalBytes)
}

/// Validate a bounded archive without filesystem access or extraction. Historical
/// paths inside journal facts are data only; content lookup uses manifest entries.
pub fn read(bytes: &[u8], limits: TraceLimits) -> Result<VerifiedTrace, TraceError> {
    limit(
        bytes.len(),
        limits.max_archive_bytes,
        LimitKind::ArchiveBytes,
    )?;
    let files = decode(bytes, limits)?;
    let bytes = files
        .get("manifest.json")
        .ok_or_else(|| TraceError::Missing("manifest.json".into()))?;
    limit(
        bytes.len(),
        limits.max_manifest_bytes,
        LimitKind::ManifestBytes,
    )?;
    #[derive(serde::Deserialize)]
    struct Header {
        format_version: u32,
        event_schema_version: u32,
    }
    let header: Header = serde_json::from_slice(bytes)?;
    if !matches!(header.format_version, 1 | TRACE_FORMAT_VERSION) {
        return Err(TraceError::UnsupportedFormat(header.format_version));
    }
    if !matches!(
        header.event_schema_version,
        8 | 9 | EVENT_JOURNAL_SCHEMA_VERSION
    ) {
        return Err(TraceError::UnsupportedEventSchema(
            header.event_schema_version,
        ));
    }
    let manifest: TraceManifest = serde_json::from_slice(bytes)?;
    let records = validation::validate(manifest.clone(), &files, limits)?;
    Ok(VerifiedTrace {
        manifest,
        records,
        files,
    })
}

pub(crate) fn encode(
    files: &BTreeMap<String, impl AsRef<[u8]>>,
    limits: TraceLimits,
) -> Result<Vec<u8>, TraceError> {
    let mut out = BoundedBytes::new(limits.max_archive_bytes, LimitKind::ArchiveBytes);
    out.append(MAGIC)?;
    let count = u32::try_from(files.len()).map_err(|_| TraceError::ArchiveFraming)?;
    out.append(&count.to_le_bytes())?;
    for (name, bytes) in files {
        let bytes = bytes.as_ref();
        let length = u32::try_from(name.len()).map_err(|_| TraceError::ArchiveFraming)?;
        out.append(&[0])?;
        out.append(&length.to_le_bytes())?;
        out.append(&(bytes.len() as u64).to_le_bytes())?;
        out.append(name.as_bytes())?;
        out.append(bytes)?;
    }
    Ok(out.bytes)
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], TraceError> {
    let (head, rest) = bytes
        .split_at_checked(length)
        .ok_or(TraceError::ArchiveFraming)?;
    *bytes = rest;
    Ok(head)
}

fn number<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], TraceError> {
    take(bytes, N)?
        .try_into()
        .map_err(|_| TraceError::ArchiveFraming)
}

pub(crate) fn decode(
    mut bytes: &[u8],
    limits: TraceLimits,
) -> Result<BTreeMap<String, Vec<u8>>, TraceError> {
    if take(&mut bytes, MAGIC.len())? != MAGIC {
        return Err(TraceError::ArchiveFraming);
    }
    let count = u32::from_le_bytes(number(&mut bytes)?) as usize;
    limit(count, limits.max_entries, LimitKind::Entries)?;
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for _ in 0..count {
        let kind = take(&mut bytes, 1)?[0];
        let name_length = u32::from_le_bytes(number(&mut bytes)?) as usize;
        let length = u64::from_le_bytes(number(&mut bytes)?);
        let length = usize::try_from(length).map_err(|_| TraceError::Limit {
            kind: LimitKind::EntryBytes,
            limit: limits.max_entry_bytes,
        })?;
        limit(length, limits.max_entry_bytes, LimitKind::EntryBytes)?;
        total = total.saturating_add(length);
        limit(total, limits.max_total_bytes, LimitKind::TotalBytes)?;
        let name = std::str::from_utf8(take(&mut bytes, name_length)?)
            .map_err(|_| TraceError::ArchiveFraming)?;
        validation::safe_path(name)?;
        if kind != 0 {
            return Err(TraceError::UnsupportedMember(name.into()));
        }
        if name == "manifest.json" {
            limit(length, limits.max_manifest_bytes, LimitKind::ManifestBytes)?;
        }
        if files.contains_key(name) {
            return Err(TraceError::Duplicate(name.into()));
        }
        files.insert(name.into(), take(&mut bytes, length)?.to_vec());
    }
    if !bytes.is_empty() {
        return Err(TraceError::ArchiveFraming);
    }
    Ok(files)
}

struct BoundedBytes {
    bytes: Vec<u8>,
    maximum: usize,
    kind: LimitKind,
    exceeded: bool,
}
impl BoundedBytes {
    fn new(maximum: usize, kind: LimitKind) -> Self {
        Self {
            bytes: Vec::new(),
            maximum,
            kind,
            exceeded: false,
        }
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), TraceError> {
        limit(
            self.bytes.len().saturating_add(bytes.len()),
            self.maximum,
            self.kind,
        )?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.append(bytes).map_err(|_| {
            self.exceeded = true;
            std::io::Error::other("trace serialization limit")
        })?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn json(
    value: &impl serde::Serialize,
    maximum: usize,
    kind: LimitKind,
) -> Result<Vec<u8>, TraceError> {
    let mut out = BoundedBytes::new(maximum, kind);
    let result = serde_json::to_writer(&mut out, value);
    if out.exceeded {
        return Err(TraceError::Limit {
            kind,
            limit: maximum,
        });
    }
    result?;
    Ok(out.bytes)
}
