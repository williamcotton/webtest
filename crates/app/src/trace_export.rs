//! Native publication of completed journals. No report-to-event reconstruction.
use crate::{error::AppError, project_context::normalized_path};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use webtest_observation::ExecutionEvent;
use webtest_project::{Project, TraceMode};
use webtest_text::SourceRevision;
use webtest_trace::{
    AttachmentInput, CapturePolicy, SourceInput, TraceInput, TraceLimits, TraceMetadata,
};

pub(crate) fn retain(mode: TraceMode, failed: bool) -> bool {
    match mode {
        TraceMode::Off => false,
        TraceMode::On => true,
        TraceMode::RetainOnFailure => failed,
    }
}

pub(crate) fn metadata(
    project: &Project,
    providers: &webtest_provider::ProviderRegistry,
) -> Result<TraceMetadata, AppError> {
    let config = serde_json::to_string(&project.config).map_err(AppError::internal)?;
    Ok(TraceMetadata {
        compiler_version: env!("CARGO_PKG_VERSION").into(),
        project_identity: *SourceRevision::of(&normalized_path(&project.root)).as_bytes(),
        configuration_fingerprint: *SourceRevision::of(&format!("webtest-inputs/v1/{config}"))
            .as_bytes(),
        browser_version: None,
        provider_versions: providers
            .schemas()
            .map(|schema| (schema.name.0.clone(), format!("schema:{}", schema.hash())))
            .collect(),
        capture: CapturePolicy {
            sources: project.config.trace.sources,
            attachments: project.config.trace.attachments,
        },
    })
}

pub(crate) fn export(
    project: &Project,
    metadata: TraceMetadata,
    source: &str,
    plan: &webtest_plan::TestPlan,
    project_path: &str,
    result: &webtest_runtime::RunResult,
) -> Result<PathBuf, AppError> {
    export_records(
        project,
        metadata,
        Some((source, plan, project_path)),
        result,
    )
}

pub(crate) fn export_application(
    project: &Project,
    metadata: TraceMetadata,
    result: &webtest_runtime::RunResult,
) -> Result<PathBuf, AppError> {
    export_records(project, metadata, None, result)
}

fn export_records(
    project: &Project,
    metadata: TraceMetadata,
    source: Option<(&str, &webtest_plan::TestPlan, &str)>,
    result: &webtest_runtime::RunResult,
) -> Result<PathBuf, AppError> {
    let execution_id = result.execution_id.ok_or_else(|| {
        AppError::infrastructure("trace export requires an allocated execution identity")
    })?;
    // Pre-admission identity failures intentionally have no synthetic trace.
    if !result
        .journal
        .iter()
        .any(|record| matches!(record.event, ExecutionEvent::RunFinished { .. }))
    {
        return Err(AppError::infrastructure(
            "trace export requires a terminal journal",
        ));
    }
    let limits = TraceLimits {
        max_archive_bytes: project.config.trace.max_bytes.get(),
        max_total_bytes: project.config.trace.max_bytes.get(),
        ..Default::default()
    };
    let artifact_directory = project.root.join(&project.config.artifacts.directory);
    let mut captures = BTreeMap::new();
    let mut references = Vec::new();
    let mut retained_bytes = 0usize;
    if metadata.capture.attachments {
        for record in &result.journal {
            if let ExecutionEvent::AttachmentCreated { attachment, .. } = &record.event {
                if attachment.artifact.path.parent() != Some(artifact_directory.as_path()) {
                    return Err(AppError::infrastructure(
                        "attachment is outside the runtime artifact directory",
                    ));
                }
                let key = attachment.blake3;
                if let std::collections::btree_map::Entry::Vacant(entry) = captures.entry(key) {
                    // Independent staging bound: never allocate unbounded evidence before
                    // the portable writer chooses whole-member budget omissions.
                    let maximum = TraceLimits::default()
                        .max_total_bytes
                        .saturating_sub(retained_bytes);
                    let bytes = read_attachment(&attachment.artifact.path, maximum, &project.root)?;
                    if bytes.len() as u64 != attachment.byte_length
                        || *blake3::hash(&bytes).as_bytes() != key
                    {
                        return Err(AppError::infrastructure(
                            "acknowledged attachment content changed before trace export",
                        ));
                    }
                    retained_bytes += bytes.len();
                    entry.insert(bytes);
                }
                references.push((record.identity, key));
            }
        }
    }
    let attachments: Vec<_> = references
        .iter()
        .map(|(event, key)| AttachmentInput {
            event: *event,
            bytes: &captures[key],
        })
        .collect();
    let include_sources = metadata.capture.sources;
    let bytes = webtest_trace::write_budgeted(
        TraceInput {
            metadata,
            records: &result.journal,
            sources: &source
                .map(|(source, plan, project_path)| SourceInput {
                    execution_id,
                    file_id: plan.file,
                    revision: plan.source_revision,
                    project_path,
                    text: include_sources.then_some(source),
                })
                .into_iter()
                .collect::<Vec<_>>(),
            attachments: &attachments,
        },
        limits,
    )
    .map_err(AppError::infrastructure)?;
    let directory = create_storage(project)?;
    let path = directory.join(format!("{execution_id}.webtest-trace"));
    let mut temp = tempfile::NamedTempFile::new_in(&directory).map_err(AppError::infrastructure)?;
    temp.write_all(&bytes).map_err(AppError::infrastructure)?;
    temp.as_file()
        .sync_all()
        .map_err(AppError::infrastructure)?;
    temp.persist_noclobber(&path)
        .map_err(AppError::infrastructure)?;
    Ok(path)
}

fn create_storage(project: &Project) -> Result<PathBuf, AppError> {
    let mut directory = project
        .root
        .canonicalize()
        .map_err(AppError::infrastructure)?;
    for component in project
        .config
        .artifacts
        .directory
        .join("traces")
        .components()
    {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => directory.push(name),
            _ => {
                return Err(AppError::infrastructure(
                    "trace storage must be project-relative",
                ));
            }
        }
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(AppError::infrastructure(error)),
        }
        if !std::fs::symlink_metadata(&directory)
            .map_err(AppError::infrastructure)?
            .is_dir()
        {
            return Err(AppError::infrastructure(
                "trace storage must use real directories",
            ));
        }
    }
    Ok(directory)
}

fn confined_directory(path: &Path, project: &Path) -> Result<PathBuf, AppError> {
    let directory = path.canonicalize().map_err(AppError::infrastructure)?;
    let project = project.canonicalize().map_err(AppError::infrastructure)?;
    if !directory.starts_with(project) {
        return Err(AppError::infrastructure(
            "trace storage must remain inside the project",
        ));
    }
    Ok(directory)
}

fn read_attachment(path: &Path, maximum: usize, project: &Path) -> Result<Vec<u8>, AppError> {
    let directory = confined_directory(
        path.parent()
            .ok_or_else(|| AppError::infrastructure("attachment has no parent directory"))?,
        project,
    )?;
    let path = directory.join(
        path.file_name()
            .ok_or_else(|| AppError::infrastructure("attachment has no file name"))?,
    );
    if !std::fs::symlink_metadata(&path)
        .map_err(AppError::infrastructure)?
        .is_file()
    {
        return Err(AppError::infrastructure(
            "trace attachment must be a regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path).map_err(AppError::infrastructure)?;
    let metadata = file.metadata().map_err(AppError::infrastructure)?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(AppError::infrastructure(
            "attachment exceeds the regular-file staging bound",
        ));
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(AppError::infrastructure)?;
    if bytes.len() > maximum {
        return Err(AppError::infrastructure(
            "attachment exceeds the staging bound",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_staging_is_bounded_and_rejects_links_and_nonfiles() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("evidence");
        std::fs::write(&file, b"bytes").unwrap();
        assert_eq!(
            read_attachment(&file, 5, directory.path()).unwrap(),
            b"bytes"
        );
        assert!(read_attachment(&file, 4, directory.path()).is_err());
        assert!(read_attachment(directory.path(), 100, directory.path()).is_err());
        #[cfg(unix)]
        {
            let link = directory.path().join("link");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(read_attachment(&link, 100, directory.path()).is_err());
        }
    }
}
