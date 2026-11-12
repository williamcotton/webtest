use crate::{
    archive::{content_path, descriptor, limit},
    *,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
};
use webtest_observation::{
    EVENT_JOURNAL_SCHEMA_VERSION, ExecutionEvent, ExecutionFailure, ReplayJournal, ReplayOutcome,
};

pub(crate) fn safe_path(path: &str) -> Result<(), TraceError> {
    if path.is_empty()
        || path.len() > 1024
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | "..") || part.ends_with([' ', '.']))
    {
        return Err(TraceError::UnsafePath(path.into()));
    }
    Ok(())
}

fn check_file<'a>(
    file: &TraceFile,
    files: &'a BTreeMap<String, impl AsRef<[u8]>>,
    used: &mut BTreeMap<String, TraceFile>,
) -> Result<&'a [u8], TraceError> {
    safe_path(&file.path)?;
    let bytes = files
        .get(&file.path)
        .ok_or_else(|| TraceError::Missing(file.path.clone()))?
        .as_ref();
    if let Some(verified) = used.get(&file.path) {
        if verified != file {
            return Err(TraceError::Integrity(file.path.clone()));
        }
    } else if descriptor(file.path.clone(), bytes) != *file {
        return Err(TraceError::Integrity(file.path.clone()));
    } else {
        used.insert(file.path.clone(), file.clone());
    }
    Ok(bytes)
}

pub(crate) fn validate(
    manifest: TraceManifest,
    files: &BTreeMap<String, impl AsRef<[u8]>>,
    limits: TraceLimits,
) -> Result<Vec<RecordedEvent>, TraceError> {
    if !matches!(manifest.format_version, 1 | TRACE_FORMAT_VERSION) {
        return Err(TraceError::UnsupportedFormat(manifest.format_version));
    }
    if !matches!(
        manifest.event_schema_version,
        8 | EVENT_JOURNAL_SCHEMA_VERSION
    ) {
        return Err(TraceError::UnsupportedEventSchema(
            manifest.event_schema_version,
        ));
    }
    if manifest.events.path != "events.jsonl" {
        return Err(TraceError::UnsafePath(manifest.events.path));
    }
    let mut used = BTreeMap::new();
    let events = check_file(&manifest.events, files, &mut used)?;
    let records = decode_records(events, limits, manifest.event_schema_version)?;
    let executions: BTreeSet<_> = records.iter().map(|r| r.identity.execution_id).collect();
    if executions.iter().copied().collect::<Vec<_>>() != manifest.executions {
        return Err(TraceError::Executions);
    }
    let omissions = capture::omissions(&manifest, files)?;
    let mut used_omissions = BTreeSet::new();

    let mut sources = BTreeMap::new();
    let mut source_texts = BTreeMap::new();
    let mut text_by_path = BTreeMap::new();
    let mut source_revisions = BTreeSet::new();
    for source in &manifest.sources {
        safe_path(&source.project_path)?;
        if !executions.contains(&source.execution_id) {
            return Err(TraceError::Executions);
        }
        let source_path = content_path("sources", source.revision.as_bytes());
        if source.file.is_none() && manifest.metadata.capture.sources {
            let omitted = omissions
                .get(&source_path)
                .ok_or(TraceError::CapturePolicy)?;
            if &omitted.blake3 != source.revision.as_bytes() {
                return Err(TraceError::Capture(source_path));
            }
            used_omissions.insert(source_path);
        } else if source.file.is_some() != manifest.metadata.capture.sources {
            return Err(TraceError::CapturePolicy);
        }
        source_revisions.insert((source.execution_id, *source.revision.as_bytes()));
        if sources
            .insert((source.execution_id, source.file_id.get()), source)
            .is_some()
        {
            return Err(TraceError::Duplicate(source.project_path.clone()));
        }
        if let Some(file) = &source.file {
            if file.path != content_path("sources", source.revision.as_bytes()) {
                return Err(TraceError::Source(source.project_path.clone()));
            }
            let bytes = check_file(file, files, &mut used)?;
            if &file.blake3 != source.revision.as_bytes() {
                return Err(TraceError::Source(source.project_path.clone()));
            }
            let text = if let Some(text) = text_by_path.get(&file.path) {
                *text
            } else {
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| TraceError::Source(source.project_path.clone()))?;
                text_by_path.insert(file.path.clone(), text);
                text
            };
            source_texts.insert((source.execution_id, source.file_id.get()), text);
        }
    }
    let mut attachments = BTreeMap::new();
    for reference in &manifest.attachments {
        if attachments.insert(reference.event, reference).is_some() {
            return Err(TraceError::Attachment(reference.event));
        }
        if reference.file.is_some() && !manifest.metadata.capture.attachments {
            return Err(TraceError::CapturePolicy);
        }
    }
    for record in &records {
        match (record.metadata.origin, record.metadata.source_revision) {
            (Some(origin), Some(revision)) => {
                let source = sources
                    .get(&(record.identity.execution_id, origin.file.get()))
                    .ok_or_else(|| {
                        TraceError::Source(format!("missing source for {:?}", record.identity))
                    })?;
                if source.revision != revision {
                    return Err(TraceError::Source(source.project_path.clone()));
                }
                if let Some(text) =
                    source_texts.get(&(record.identity.execution_id, origin.file.get()))
                    && (!text.is_char_boundary(u32::from(origin.range.start()) as usize)
                        || !text.is_char_boundary(u32::from(origin.range.end()) as usize))
                {
                    return Err(TraceError::Source(source.project_path.clone()));
                }
            }
            (None, Some(revision)) => {
                // Run-level facts have a revision but no executable source range.
                if !source_revisions.contains(&(record.identity.execution_id, *revision.as_bytes()))
                {
                    return Err(TraceError::Source(format!(
                        "missing run source for {:?}",
                        record.identity
                    )));
                }
            }
            (None, None) => {}
            _ => {
                return Err(TraceError::Source(format!(
                    "incomplete source identity for {:?}",
                    record.identity
                )));
            }
        }
        if let ExecutionEvent::AttachmentCreated { attachment, .. } = &record.event {
            let reference = attachments
                .remove(&record.identity)
                .ok_or(TraceError::Attachment(record.identity))?;
            if let Some(file) = &reference.file {
                if file.path != content_path("attachments", &attachment.blake3)
                    || file.blake3 != attachment.blake3
                    || file.byte_length != attachment.byte_length
                {
                    return Err(TraceError::Attachment(record.identity));
                }
                check_file(file, files, &mut used)?;
            } else if manifest.metadata.capture.attachments {
                let path = content_path("attachments", &attachment.blake3);
                let omitted = omissions.get(&path).ok_or(TraceError::CapturePolicy)?;
                if omitted.blake3 != attachment.blake3
                    || omitted.byte_length != attachment.byte_length
                {
                    return Err(TraceError::Attachment(record.identity));
                }
                used_omissions.insert(path);
            }
        }
    }
    if let Some((identity, _)) = attachments.first_key_value() {
        return Err(TraceError::Attachment(*identity));
    }
    if let Some(path) = omissions
        .keys()
        .find(|path| !used_omissions.contains(*path))
    {
        return Err(TraceError::Capture(path.clone()));
    }
    if let Some(path) = files
        .keys()
        .find(|path| path.as_str() != "manifest.json" && !used.contains_key(*path))
    {
        return Err(TraceError::Unreferenced(path.clone()));
    }
    Ok(records)
}

fn decode_records(
    bytes: &[u8],
    limits: TraceLimits,
    schema_version: u32,
) -> Result<Vec<RecordedEvent>, TraceError> {
    if bytes.is_empty() || !bytes.ends_with(b"\n") {
        return Err(TraceError::Framing);
    }
    let capacity = NonZeroUsize::new(limits.max_events).ok_or(TraceError::Limit {
        kind: LimitKind::Events,
        limit: 0,
    })?;
    let maximum = NonZeroUsize::new(limits.max_record_bytes).ok_or(TraceError::Limit {
        kind: LimitKind::RecordBytes,
        limit: 0,
    })?;
    let mut replay = ReplayJournal::for_schema(capacity, schema_version)
        .map_err(|_| TraceError::UnsupportedEventSchema(schema_version))?;
    let mut records = Vec::new();
    let mut streams = BTreeMap::<ExecutionId, (u64, bool)>::new();
    for (index, line) in bytes[..bytes.len() - 1].split(|b| *b == b'\n').enumerate() {
        limit(
            index.saturating_add(1),
            limits.max_events,
            LimitKind::Events,
        )?;
        limit(line.len(), limits.max_record_bytes, LimitKind::RecordBytes)?;
        if line.is_empty() {
            return Err(TraceError::Framing);
        }
        let outcome = replay
            .insert_json(line, maximum)
            .map_err(|error| TraceError::Journal {
                line: index + 1,
                error,
            })?;
        let record: RecordedEvent = serde_json::from_slice(line)?;
        if outcome != ReplayOutcome::Inserted {
            return Err(TraceError::Sequence(record.identity));
        }
        let (next, finished) = streams.entry(record.identity.execution_id).or_default();
        let sequence = record.identity.event_sequence.0;
        let terminal = matches!(record.event, ExecutionEvent::RunFinished { .. });
        if *finished {
            return Err(TraceError::Sequence(record.identity));
        }
        if let ExecutionEvent::RunFinished { failure, .. } = &record.event {
            let mut gaps = Vec::new();
            if let Some(failure) = failure {
                journal_gaps(failure, &mut gaps);
            }
            if sequence == *next {
                if !gaps.is_empty() {
                    return Err(TraceError::Sequence(record.identity));
                }
            } else if sequence < *next || gaps.len() != 1 || {
                let gap = gaps[0];
                gap.first_rejected.execution_id != record.identity.execution_id
                    || gap.last_rejected.execution_id != record.identity.execution_id
                    || gap.first_rejected.event_sequence.0 != *next
                    || gap.last_rejected.event_sequence.0 != sequence - 1
                    || gap.rejected_events != sequence - *next
            } {
                return Err(TraceError::Sequence(record.identity));
            }
        } else if sequence != *next
            || (*next == 0) != matches!(record.event, ExecutionEvent::RunStarted { .. })
        {
            return Err(TraceError::Sequence(record.identity));
        }
        // A terminal at zero has no start or explicit loss interval.
        if terminal && sequence == 0 {
            return Err(TraceError::Sequence(record.identity));
        }
        *next = sequence
            .checked_add(1)
            .ok_or(TraceError::Sequence(record.identity))?;
        *finished = terminal;
        records.push(record);
    }
    if let Some((execution, _)) = streams.iter().find(|(_, (_, finished))| !finished) {
        return Err(TraceError::IncompleteExecution(*execution));
    }
    Ok(records)
}

fn journal_gaps<'a>(
    failure: &'a ExecutionFailure,
    gaps: &mut Vec<&'a webtest_observation::JournalOverflow>,
) {
    match failure {
        ExecutionFailure::JournalOverflow(gap) => gaps.push(gap),
        ExecutionFailure::Multiple { primary, secondary } => {
            journal_gaps(primary, gaps);
            for failure in secondary {
                journal_gaps(failure, gaps);
            }
        }
        _ => {}
    }
}
