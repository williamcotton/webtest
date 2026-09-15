use crate::{
    archive::{json, limit},
    *,
};
use std::{borrow::Cow, collections::BTreeMap};

pub(crate) fn event_file(event: &CaptureEvent) -> &TraceFile {
    match event {
        CaptureEvent::Truncated { file } => file,
    }
}

pub(crate) fn encoded_size(files: &BTreeMap<String, impl AsRef<[u8]>>) -> usize {
    files.iter().fold(19usize, |size, (name, bytes)| {
        size.saturating_add(13)
            .saturating_add(name.len())
            .saturating_add(bytes.as_ref().len())
    })
}

/// Largest content first, lexical path as tie-breaker. Removing a shared member
/// removes every reference's bytes together and produces exactly one export fact.
/// Only descriptors are serialized during selection; capture bytes stay borrowed.
pub(crate) fn select(
    manifest: &mut TraceManifest,
    files: &mut BTreeMap<String, Cow<'_, [u8]>>,
    limits: TraceLimits,
) -> Result<(), TraceError> {
    let budget = CaptureBudget {
        max_archive_bytes: limits.max_archive_bytes as u64,
        max_total_bytes: limits.max_total_bytes as u64,
        max_entry_bytes: limits.max_entry_bytes as u64,
        max_entries: limits.max_entries as u64,
    };
    manifest.capture_report = Some(CaptureReport {
        budget,
        events: vec![],
    });
    let mut manifest_size = json(
        manifest,
        limits.max_manifest_bytes,
        LimitKind::ManifestBytes,
    )?
    .len();
    let mut total = files
        .values()
        .fold(manifest_size, |sum, bytes| sum.saturating_add(bytes.len()));
    let mut encoded = encoded_size(files)
        .saturating_add(13 + "manifest.json".len())
        .saturating_add(manifest_size);
    let mut entries = files.len().saturating_add(1);
    let mut content = BTreeMap::<String, (TraceFile, usize)>::new();
    for file in manifest
        .sources
        .iter()
        .filter_map(|s| s.file.as_ref())
        .chain(manifest.attachments.iter().filter_map(|a| a.file.as_ref()))
    {
        content
            .entry(file.path.clone())
            .or_insert((file.clone(), 0))
            .1 += 1;
    }
    let mut content: Vec<_> = content.into_values().collect();
    content.sort_by(|(a, _), (b, _)| {
        b.byte_length
            .cmp(&a.byte_length)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut events = Vec::new();
    for (file, references) in content {
        if file.byte_length <= limits.max_entry_bytes as u64
            && total <= limits.max_total_bytes
            && encoded <= limits.max_archive_bytes
            && entries <= limits.max_entries
            && manifest_size <= limits.max_entry_bytes
        {
            continue;
        }
        let event = CaptureEvent::Truncated { file: file.clone() };
        // JSON array commas and Option<File> -> null are the only shape changes.
        // Count exact serialized bytes so limits include the truncation facts too.
        let removed_metadata =
            (json(&file, limits.max_manifest_bytes, LimitKind::ManifestBytes)?.len() - 4)
                .saturating_mul(references);
        let added_metadata = json(&event, limits.max_manifest_bytes, LimitKind::ManifestBytes)?
            .len()
            + usize::from(!events.is_empty());
        manifest_size = manifest_size
            .checked_sub(removed_metadata)
            .and_then(|n| n.checked_add(added_metadata))
            .ok_or_else(|| TraceError::Capture("metadata size overflow".into()))?;
        let bytes = files
            .remove(&file.path)
            .ok_or_else(|| TraceError::Missing(file.path.clone()))?;
        total = total
            .checked_sub(bytes.len())
            .and_then(|n| n.checked_sub(removed_metadata))
            .and_then(|n| n.checked_add(added_metadata))
            .ok_or_else(|| TraceError::Capture("content size overflow".into()))?;
        encoded = encoded
            .checked_sub(bytes.len() + 13 + file.path.len())
            .and_then(|n| n.checked_sub(removed_metadata))
            .and_then(|n| n.checked_add(added_metadata))
            .ok_or_else(|| TraceError::Capture("archive size overflow".into()))?;
        entries -= 1;
        for source in &mut manifest.sources {
            if source.file.as_ref().is_some_and(|f| f.path == file.path) {
                source.file = None;
            }
        }
        for attachment in &mut manifest.attachments {
            if attachment
                .file
                .as_ref()
                .is_some_and(|f| f.path == file.path)
            {
                attachment.file = None;
            }
        }
        events.push(event);
    }
    manifest.capture_report = Some(CaptureReport { budget, events });
    limit(
        manifest_size,
        limits.max_manifest_bytes,
        LimitKind::ManifestBytes,
    )?;
    limit(manifest_size, limits.max_entry_bytes, LimitKind::EntryBytes)?;
    limit(total, limits.max_total_bytes, LimitKind::TotalBytes)?;
    limit(encoded, limits.max_archive_bytes, LimitKind::ArchiveBytes)?;
    limit(entries, limits.max_entries, LimitKind::Entries)?;
    Ok(())
}

pub(crate) fn omissions(
    manifest: &TraceManifest,
    files: &BTreeMap<String, impl AsRef<[u8]>>,
) -> Result<BTreeMap<String, TraceFile>, TraceError> {
    let mut omissions = BTreeMap::new();
    if let Some(report) = &manifest.capture_report {
        if manifest.format_version < 2 {
            return Err(TraceError::Capture(
                "version 1 cannot contain export facts".into(),
            ));
        }
        let budget = report.budget;
        let total = files.values().fold(0u64, |sum, bytes| {
            sum.saturating_add(bytes.as_ref().len() as u64)
        });
        if total > budget.max_total_bytes
            || encoded_size(files) as u64 > budget.max_archive_bytes
            || files.len() as u64 > budget.max_entries
            || files
                .values()
                .any(|bytes| bytes.as_ref().len() as u64 > budget.max_entry_bytes)
        {
            return Err(TraceError::Capture(
                "retained archive exceeds its declared capture budget".into(),
            ));
        }
        let mut previous: Option<&str> = None;
        for event in &report.events {
            let file = event_file(event);
            validation::safe_path(&file.path)?;
            if previous.is_some_and(|path| path >= file.path.as_str())
                || files.contains_key(&file.path)
            {
                return Err(TraceError::Capture(file.path.clone()));
            }
            previous = Some(&file.path);
            omissions.insert(file.path.clone(), file.clone());
        }
    }
    Ok(omissions)
}
