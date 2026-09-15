use super::*;
use std::time::Duration;
use webtest_model::{StepId, TestId};
use webtest_observation::{
    Artifact, ArtifactKind, Attachment, EventJournal, EventMetadata, EventSequence, EventTime,
    ExecutionEvent, ExecutionFailure, JournalOverflow, RunOutcomeKind,
};
use webtest_text::{SyntaxOrigin, TextRange};

const SOURCE: &str = "test \"é\" {}";
const EVIDENCE: &[u8] = b"redacted evidence\0\xff";

fn metadata(capture: bool) -> TraceMetadata {
    TraceMetadata {
        compiler_version: "0.1.0".into(),
        project_identity: [1; 32],
        configuration_fingerprint: [2; 32],
        browser_version: None,
        provider_versions: BTreeMap::from([("process".into(), "1".into())]),
        capture: CapturePolicy {
            sources: capture,
            attachments: capture,
        },
    }
}

fn finish(id: ExecutionId) -> ExecutionEvent {
    ExecutionEvent::RunFinished {
        execution_id: id,
        failure: None,
        cancellation: None,
        outcome: RunOutcomeKind::Completed,
        failure_class: None,
    }
}

fn fixture() -> Vec<RecordedEvent> {
    let a = ExecutionId::from_u128(1);
    let b = ExecutionId::from_u128(2);
    let mut journal = EventJournal::default();
    let time = EventTime {
        since_unix_epoch: Duration::from_secs(10),
        elapsed: Duration::ZERO,
    };
    journal.record(ExecutionEvent::RunStarted { execution_id: a }, time);
    journal.record(ExecutionEvent::RunStarted { execution_id: b }, time);
    let event = ExecutionEvent::AttachmentCreated {
        execution_id: a,
        test_id: TestId(0),
        step_id: StepId(3),
        attachment: Attachment {
            artifact: Artifact {
                kind: ArtifactKind::Evidence,
                path: "/private/never-open-this-path".into(),
            },
            byte_length: EVIDENCE.len() as u64,
            blake3: *blake3::hash(EVIDENCE).as_bytes(),
        },
    };
    let metadata = EventMetadata {
        source_revision: Some(SourceRevision::of(SOURCE)),
        origin: Some(SyntaxOrigin::new(
            FileId::new(0),
            TextRange::new(6.into(), 8.into()),
        )),
        ..event.metadata()
    };
    journal.record_with_metadata(event, time, metadata);
    // Wall time and execution order are independent; preserve the input order.
    let time = EventTime {
        since_unix_epoch: Duration::from_secs(1),
        elapsed: Duration::from_secs(1),
    };
    journal.record(finish(b), time);
    journal.record(finish(a), time);
    journal.into_records()
}

fn build(
    records: &[RecordedEvent],
    capture: bool,
    limits: TraceLimits,
) -> Result<Vec<u8>, TraceError> {
    let attachments = [AttachmentInput {
        event: records[2].identity,
        bytes: EVIDENCE,
    }];
    write(
        TraceInput {
            metadata: metadata(capture),
            records,
            sources: &[SourceInput {
                execution_id: ExecutionId::from_u128(1),
                file_id: FileId::new(0),
                revision: SourceRevision::of(SOURCE),
                project_path: "tests/unicode.webtest",
                text: capture.then_some(SOURCE),
            }],
            attachments: if capture { &attachments } else { &[] },
        },
        limits,
    )
}

fn members(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    archive::decode(bytes, TraceLimits::default()).unwrap()
}
fn pack(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    archive::encode(files, TraceLimits::default()).unwrap()
}
fn mutate_manifest(files: &mut BTreeMap<String, Vec<u8>>, mutate: impl FnOnce(&mut TraceManifest)) {
    let mut manifest = serde_json::from_slice(&files["manifest.json"]).unwrap();
    mutate(&mut manifest);
    files.insert(
        "manifest.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    );
}
fn replace_events(files: &mut BTreeMap<String, Vec<u8>>, bytes: Vec<u8>) {
    let descriptor = archive::descriptor("events.jsonl".into(), &bytes);
    files.insert("events.jsonl".into(), bytes);
    mutate_manifest(files, |m| m.events = descriptor);
}

#[test]
fn deterministic_roundtrip_preserves_native_facts_and_resolves_only_bundled_content() {
    let records = fixture();
    let bytes = build(&records, true, TraceLimits::default()).unwrap();
    assert_eq!(
        bytes,
        build(&records, true, TraceLimits::default()).unwrap()
    );
    let trace = read(&bytes, TraceLimits::default()).unwrap();
    assert_eq!(trace.records(), records);
    assert_eq!(
        trace.source(ExecutionId::from_u128(1), FileId::new(0)),
        Some(SOURCE)
    );
    assert_eq!(trace.attachment(records[2].identity), Some(EVIDENCE));
    assert!(trace.attachment(records[0].identity).is_none());
    assert_eq!(trace.manifest().executions.len(), 2);
    assert_eq!(trace.manifest().format_version, 1);
    assert_eq!(
        trace.manifest().event_schema_version,
        webtest_observation::EVENT_JOURNAL_SCHEMA_VERSION
    );
    let files = members(&bytes);
    assert_eq!(files.len(), 4);
    assert!(files.keys().all(|path| !path.starts_with('/')));
}

#[test]
fn capture_policy_explicitly_omits_bytes_without_inventing_or_rewriting_events() {
    let records = fixture();
    let trace = read(
        &build(&records, false, TraceLimits::default()).unwrap(),
        TraceLimits::default(),
    )
    .unwrap();
    assert_eq!(trace.records(), records);
    assert_eq!(
        trace.source(ExecutionId::from_u128(1), FileId::new(0)),
        None
    );
    assert_eq!(trace.attachment(records[2].identity), None);
    assert_eq!(trace.files.len(), 2);
    let mut files = trace.files;
    mutate_manifest(&mut files, |m| m.metadata.capture.sources = true);
    assert!(matches!(
        read(&pack(&files), TraceLimits::default()),
        Err(TraceError::CapturePolicy)
    ));
}

#[test]
fn version_checks_precede_unknown_future_payload_decoding() {
    let original = members(&build(&fixture(), false, TraceLimits::default()).unwrap());
    for version in [0, 2] {
        let mut files = original.clone();
        files.insert(
            "manifest.json".into(),
            format!("{{\"format_version\":{version},\"event_schema_version\":8,\"future\":true}}")
                .into_bytes(),
        );
        assert!(
            matches!(read(&pack(&files), TraceLimits::default()), Err(TraceError::UnsupportedFormat(v)) if v == version)
        );
    }
    let mut files = original.clone();
    mutate_manifest(&mut files, |m| m.event_schema_version += 1);
    assert!(matches!(
        read(&pack(&files), TraceLimits::default()),
        Err(TraceError::UnsupportedEventSchema(_))
    ));
    let mut files = original;
    let mut value: serde_json::Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
    value["unknown"] = true.into();
    files.insert("manifest.json".into(), serde_json::to_vec(&value).unwrap());
    assert!(matches!(
        read(&pack(&files), TraceLimits::default()),
        Err(TraceError::Json(_))
    ));
}

#[test]
fn every_contained_file_is_checksumming_required_and_unlisted_members_are_rejected() {
    let original = members(&build(&fixture(), true, TraceLimits::default()).unwrap());
    for path in original.keys().filter(|p| *p != "manifest.json") {
        let mut files = original.clone();
        files.get_mut(path).unwrap()[0] ^= 1;
        assert!(matches!(
            read(&pack(&files), TraceLimits::default()),
            Err(TraceError::Integrity(_))
        ));
        let mut files = original.clone();
        files.remove(path);
        assert!(matches!(
            read(&pack(&files), TraceLimits::default()),
            Err(TraceError::Missing(_))
        ));
    }
    let mut files = original;
    files.insert("unlisted".into(), vec![]);
    assert!(matches!(
        read(&pack(&files), TraceLimits::default()),
        Err(TraceError::Unreferenced(_))
    ));
}

#[test]
fn paths_links_duplicate_members_and_trailing_or_incomplete_frames_are_rejected() {
    let original = members(&build(&fixture(), false, TraceLimits::default()).unwrap());
    for path in [
        "../outside",
        "/absolute",
        "C:/drive",
        "a\\b",
        "a/../b",
        "a//b",
        "./a",
        "a/",
        "a\0b",
        "a.",
        "a ",
    ] {
        let mut files = original.clone();
        files.insert(path.into(), vec![]);
        assert!(
            matches!(
                read(&pack(&files), TraceLimits::default()),
                Err(TraceError::UnsafePath(_))
            ),
            "{path:?}"
        );
        let mut files = original.clone();
        mutate_manifest(&mut files, |m| m.sources[0].project_path = path.into());
        assert!(
            matches!(
                read(&pack(&files), TraceLimits::default()),
                Err(TraceError::UnsafePath(_))
            ),
            "{path:?}"
        );
    }
    let bytes = pack(&original);
    let entry_start = b"WEBTEST-TRACE\0\x01".len() + 4;
    let mut linked = bytes.clone();
    linked[entry_start] = 1;
    assert!(matches!(
        read(&linked, TraceLimits::default()),
        Err(TraceError::UnsupportedMember(_))
    ));
    let mut duplicate = bytes.clone();
    duplicate[b"WEBTEST-TRACE\0\x01".len()..entry_start].copy_from_slice(&4u32.to_le_bytes());
    duplicate.extend_from_slice(&bytes[entry_start..]);
    assert!(matches!(
        read(&duplicate, TraceLimits::default()),
        Err(TraceError::Duplicate(_))
    ));
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        read(&trailing, TraceLimits::default()),
        Err(TraceError::ArchiveFraming)
    ));
    for end in [0, 10, entry_start + 5, bytes.len() - 1] {
        assert!(read(&bytes[..end], TraceLimits::default()).is_err());
    }
}

#[test]
fn reader_and_writer_enforce_all_independent_budgets() {
    let records = fixture();
    let bytes = build(&records, true, TraceLimits::default()).unwrap();
    let files = members(&bytes);
    let maximum = files.values().map(Vec::len).max().unwrap();
    let total: usize = files.values().map(Vec::len).sum();
    let record_max = files["events.jsonl"]
        .split(|b| *b == b'\n')
        .map(<[u8]>::len)
        .max()
        .unwrap();
    let exact = TraceLimits {
        max_archive_bytes: bytes.len(),
        max_total_bytes: total,
        max_entry_bytes: maximum,
        max_manifest_bytes: files["manifest.json"].len(),
        max_record_bytes: record_max,
        max_entries: files.len(),
        max_events: records.len(),
    };
    assert_eq!(build(&records, true, exact).unwrap(), bytes);
    read(&bytes, exact).unwrap();
    for kind in [
        LimitKind::ArchiveBytes,
        LimitKind::TotalBytes,
        LimitKind::EntryBytes,
        LimitKind::ManifestBytes,
        LimitKind::RecordBytes,
        LimitKind::Entries,
        LimitKind::Events,
    ] {
        let mut limited = exact;
        match kind {
            LimitKind::ArchiveBytes => limited.max_archive_bytes -= 1,
            LimitKind::TotalBytes => limited.max_total_bytes -= 1,
            LimitKind::EntryBytes => limited.max_entry_bytes -= 1,
            LimitKind::ManifestBytes => limited.max_manifest_bytes -= 1,
            LimitKind::RecordBytes => limited.max_record_bytes -= 1,
            LimitKind::Entries => limited.max_entries -= 1,
            LimitKind::Events => limited.max_events -= 1,
        }
        assert!(
            matches!(read(&bytes, limited), Err(TraceError::Limit { kind: actual, .. }) if actual == kind),
            "read {kind:?}"
        );
        assert!(
            matches!(build(&records, true, limited), Err(TraceError::Limit { kind: actual, .. }) if actual == kind),
            "write {kind:?}"
        );
    }
}

#[test]
fn source_identity_utf8_ranges_and_attachment_event_identity_are_validated() {
    let original = fixture();
    for (start, end) in [(7, 8), (6, 100)] {
        let mut records = original.clone();
        records[2].metadata.origin.as_mut().unwrap().range =
            TextRange::new(start.into(), end.into());
        assert!(matches!(
            build(&records, true, TraceLimits::default()),
            Err(TraceError::Source(_))
        ));
    }
    let mut records = original.clone();
    records[2].metadata.source_revision = Some(SourceRevision::of("other"));
    assert!(matches!(
        build(&records, true, TraceLimits::default()),
        Err(TraceError::Source(_))
    ));
    let mut files = members(&build(&original, true, TraceLimits::default()).unwrap());
    mutate_manifest(&mut files, |m| {
        m.attachments[0].event = original[0].identity
    });
    assert!(matches!(
        read(&pack(&files), TraceLimits::default()),
        Err(TraceError::Attachment(_))
    ));
    let mut records = original;
    if let ExecutionEvent::AttachmentCreated { attachment, .. } = &mut records[2].event {
        attachment.byte_length += 1;
    }
    assert!(matches!(
        build(&records, true, TraceLimits::default()),
        Err(TraceError::Attachment(_))
    ));
}

#[test]
fn corrupted_replay_identity_versions_missing_terminals_and_reordering_are_rejected() {
    let original = fixture();
    let mut records = original.clone();
    records[2].identity.execution_id = ExecutionId::from_u128(3);
    assert!(matches!(
        build(&records, true, TraceLimits::default()),
        Err(TraceError::Journal { .. })
    ));
    let mut records = original.clone();
    records[2].schema_version += 1;
    assert!(matches!(
        build(&records, true, TraceLimits::default()),
        Err(TraceError::Journal { .. })
    ));
    let mut records = original.clone();
    records.pop();
    assert!(matches!(
        build(&records, true, TraceLimits::default()),
        Err(TraceError::IncompleteExecution(_))
    ));
    let mut records = original.clone();
    records.swap(2, 4);
    assert!(matches!(
        build(&records, false, TraceLimits::default()),
        Err(TraceError::Sequence(_))
    ));
    let mut files = members(&build(&original, false, TraceLimits::default()).unwrap());
    let mut bytes = files["events.jsonl"].clone();
    bytes.extend(serde_json::to_vec(&original[0]).unwrap());
    bytes.push(b'\n');
    replace_events(&mut files, bytes);
    assert!(matches!(
        read(&pack(&files), TraceLimits::default()),
        Err(TraceError::Sequence(_))
    ));
}

#[test]
fn authoritative_overflow_is_preserved_and_unexplained_gaps_are_rejected() {
    let id = ExecutionId::from_u128(4);
    let mut journal = EventJournal::default();
    let first = journal.omit(id);
    let last = journal.omit(id);
    // A one-record collector can lose even RunStarted, retaining only its terminal.
    let mut event = finish(id);
    if let ExecutionEvent::RunFinished {
        failure, outcome, ..
    } = &mut event
    {
        *outcome = RunOutcomeKind::Aborted;
        *failure = Some(Box::new(ExecutionFailure::JournalOverflow(
            JournalOverflow {
                capacity: 1,
                first_rejected: first,
                last_rejected: last,
                rejected_events: 2,
            },
        )));
    }
    journal.record(
        event,
        EventTime {
            since_unix_epoch: Duration::ZERO,
            elapsed: Duration::ZERO,
        },
    );
    let records = journal.into_records();
    let input = |records| TraceInput {
        metadata: metadata(false),
        records,
        sources: &[],
        attachments: &[],
    };
    let bytes = write(input(&records), TraceLimits::default()).unwrap();
    assert_eq!(
        read(&bytes, TraceLimits::default()).unwrap().records(),
        records
    );
    let mut records = records.clone();
    records[0].identity.event_sequence = EventSequence(3);
    assert!(matches!(
        write(input(&records), TraceLimits::default()),
        Err(TraceError::Sequence(_))
    ));
}

#[test]
fn version_one_container_framing_is_byte_stable() {
    let files = BTreeMap::from([("x".into(), vec![0xfe, 0xff])]);
    let bytes = pack(&files);
    assert_eq!(
        bytes,
        b"WEBTEST-TRACE\0\x01\x01\0\0\0\0\x01\0\0\0\x02\0\0\0\0\0\0\0x\xfe\xff"
    );
    assert_eq!(members(&bytes), files);
}

#[test]
fn source_ids_are_local_to_execution_and_repeated_content_is_deduplicated() {
    let records = fixture();
    let a = ExecutionId::from_u128(1);
    let b = ExecutionId::from_u128(2);
    let sources = [
        SourceInput {
            execution_id: b,
            file_id: FileId::new(0),
            revision: SourceRevision::of(SOURCE),
            project_path: "tests/second.webtest",
            text: Some(SOURCE),
        },
        SourceInput {
            execution_id: a,
            file_id: FileId::new(0),
            revision: SourceRevision::of(SOURCE),
            project_path: "tests/first.webtest",
            text: Some(SOURCE),
        },
    ];
    let export = |sources: &[SourceInput<'_>]| {
        write(
            TraceInput {
                metadata: TraceMetadata {
                    capture: CapturePolicy {
                        sources: true,
                        attachments: false,
                    },
                    ..metadata(false)
                },
                records: &records,
                sources,
                attachments: &[],
            },
            TraceLimits::default(),
        )
    };
    let bytes = export(&sources).unwrap();
    let trace = read(&bytes, TraceLimits::default()).unwrap();
    assert_eq!(trace.source(a, FileId::new(0)), Some(SOURCE));
    assert_eq!(trace.source(b, FileId::new(0)), Some(SOURCE));
    assert_eq!(trace.files.len(), 3);
    let mut sources = sources;
    sources.reverse();
    assert_eq!(
        export(&sources).unwrap(),
        bytes,
        "source input ordering is not artifact ordering"
    );
    sources[1].text = Some("different revision");
    sources[1].revision = SourceRevision::of("different revision");
    let trace = read(&export(&sources).unwrap(), TraceLimits::default()).unwrap();
    assert_eq!(trace.source(b, FileId::new(0)), Some("different revision"));
    sources[1].execution_id = a;
    assert!(matches!(export(&sources), Err(TraceError::Duplicate(_))));
}
