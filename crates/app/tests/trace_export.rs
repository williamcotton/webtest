use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use webtest_observation::ExecutionEvent;
use webtest_trace::{TraceLimits, VerifiedTrace};

fn fixture(config: &str, source: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("tests")).unwrap();
    fs::write(directory.path().join("webtest.toml"), config).unwrap();
    fs::write(directory.path().join("tests/run.webtest"), source).unwrap();
    directory
}

fn run(directory: &Path, reporter: &str, jobs: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_webtest"))
        .current_dir(directory)
        .args(["test", "--reporter", reporter, "--jobs", jobs])
        .output()
        .unwrap()
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn trace(report: &Value, index: usize) -> VerifiedTrace {
    let entry = &report["traces"][index];
    assert_eq!(entry["outcome"], "published", "{report}");
    let bytes = fs::read(entry["path"].as_str().unwrap()).unwrap();
    let trace = webtest_trace::read(&bytes, TraceLimits::default()).unwrap();
    assert_eq!(
        trace.manifest().executions[0].to_string(),
        entry["execution_id"].as_str().unwrap()
    );
    trace
}

#[test]
fn export_is_opt_in_retention_covers_the_invocation_and_static_files_get_no_trace() {
    let source = "test \"pass\" { expect 1 == 1 }";
    for (mode, failing, count) in [
        ("off", false, 0),
        ("on", false, 1),
        ("retain-on-failure", false, 0),
        ("retain-on-failure", true, 1),
    ] {
        let directory = fixture(&format!("[trace]\nmode = '{mode}'\n"), source);
        if failing {
            fs::write(directory.path().join("tests/bad.webtest"), "test").unwrap();
        }
        let output = run(directory.path(), "json", "2");
        assert_eq!(output.status.code(), Some(i32::from(failing)));
        let report = report(&output);
        assert_eq!(report["traces"].as_array().map_or(0, Vec::len), count);
        if count == 0 {
            assert!(!directory.path().join(".webtest/artifacts/traces").exists());
        } else {
            let trace = trace(&report, 0);
            assert!(trace.manifest().sources[0].file.is_none());
            assert!(!trace.manifest().metadata.capture.sources);
            assert_eq!(report["traces"][0]["file"], "tests/run.webtest");
        }
    }
}

#[test]
fn concurrent_file_traces_preserve_sources_failures_and_unique_nonoverwriting_paths() {
    let source =
        "test \"branches\" { parallel { server { expect 1 == 2 } server { expect 3 == 4 } } }";
    let directory = fixture("[trace]\nmode = 'on'\nsources = true\n", source);
    fs::write(
        directory.path().join("tests/z.webtest"),
        "test \"pass\" { expect 1 == 1 }",
    )
    .unwrap();
    let first = run(directory.path(), "json", "5");
    assert_eq!(first.status.code(), Some(1));
    let first = report(&first);
    assert_eq!(first["schema_version"], 14);
    assert_eq!(first["traces"].as_array().unwrap().len(), 2);
    let captured = trace(&first, 0);
    let mapping = &captured.manifest().sources[0];
    assert_eq!(
        captured.source(mapping.execution_id, mapping.file_id),
        Some(source)
    );
    assert_eq!(
        captured
            .records()
            .iter()
            .filter(|record| matches!(record.event, ExecutionEvent::StepFailed { .. }))
            .count(),
        2
    );
    let second = report(&run(directory.path(), "json", "5"));
    assert_ne!(first["traces"][0]["path"], second["traces"][0]["path"]);
    assert_ne!(
        first["traces"][0]["execution_id"],
        first["traces"][1]["execution_id"]
    );
    assert_eq!(
        fs::read_dir(directory.path().join(".webtest/artifacts/traces"))
            .unwrap()
            .count(),
        4
    );
    trace(&first, 0);
    trace(&second, 1);
}

#[test]
fn source_budget_truncates_content_but_small_mandatory_budget_is_an_explicit_export_failure() {
    let source = format!(
        "// {}\ntest \"pass\" {{ expect 1 == 1 }}",
        "x".repeat(200_000)
    );
    let directory = fixture(
        "[trace]\nmode = 'on'\nsources = true\nmax_bytes = 50000\n",
        &source,
    );
    let output = run(directory.path(), "json", "1");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report = report(&output);
    let trace = trace(&report, 0);
    assert!(trace.manifest().sources[0].file.is_none());
    assert_eq!(
        trace
            .manifest()
            .capture_report
            .as_ref()
            .unwrap()
            .events
            .len(),
        1
    );
    assert!(
        trace
            .records()
            .iter()
            .any(|record| matches!(record.event, ExecutionEvent::RunFinished { .. }))
    );
    fs::write(
        directory.path().join("webtest.toml"),
        "[trace]\nmode = 'on'\nmax_bytes = 1\n",
    )
    .unwrap();
    for reporter in ["json", "events", "junit", "human", "concise"] {
        let output = run(directory.path(), reporter, "1");
        assert_eq!(output.status.code(), Some(3));
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains(if reporter == "events" {
                "trace_export"
            } else if reporter == "json" {
                "\"traces\""
            } else {
                "trace.export"
            }),
            "{text}"
        );
        if reporter == "json" {
            let report: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(report["summary"]["passed"], 1);
            assert_eq!(report["summary"]["infrastructure_errors"], 1);
            assert_eq!(report["traces"][0]["outcome"], "failed");
            assert!(report["traces"][0].get("path").is_none());
        }
    }
    assert_eq!(
        fs::read_dir(directory.path().join(".webtest/artifacts/traces"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn journal_overflow_remains_readable_in_a_retained_trace() {
    let directory = fixture(
        "[trace]\nmode = 'retain-on-failure'\n[journal]\nmax_events = 1\n",
        "test \"overflow\" { expect 1 == 1 }",
    );
    let output = run(directory.path(), "json", "2");
    assert_eq!(output.status.code(), Some(3));
    let report = report(&output);
    let trace = trace(&report, 0);
    assert_eq!(trace.records().len(), 1);
    assert!(matches!(
        trace.records()[0].event,
        ExecutionEvent::RunFinished {
            failure: Some(_),
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn source_capture_uses_the_analyzed_snapshot_after_provider_mutation() {
    let source = r#"test "snapshot" { server { process.run("/bin/sh", args: ["-c", "printf changed > tests/run.webtest"]) } }"#;
    let directory = fixture("[trace]\nmode = 'on'\nsources = true\n", source);
    let output = run(directory.path(), "json", "1");
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(directory.path().join("tests/run.webtest")).unwrap(),
        "changed"
    );
    let trace = trace(&report(&output), 0);
    let mapping = &trace.manifest().sources[0];
    assert_eq!(
        trace.source(mapping.execution_id, mapping.file_id),
        Some(source)
    );
}

#[cfg(unix)]
#[test]
fn publication_refuses_symlink_storage_without_writing_outside_the_project() {
    let directory = fixture("[trace]\nmode = 'on'\n", "test \"pass\" { expect 1 == 1 }");
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), directory.path().join(".webtest")).unwrap();
    let output = run(directory.path(), "json", "1");
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    assert_eq!(report(&output)["summary"]["passed"], 1);
}

#[test]
fn chrome_failure_exports_acknowledged_evidence_and_static_viewer_opens_it() {
    if webtest_browser_cdp::ChromeHost::default()
        .locate()
        .is_none()
    {
        return;
    }
    let directory = fixture(
        "[trace]\nmode = 'retain-on-failure'\n[timeouts]\nassertion = '100ms'\n",
        "test \"missing\" { browser { expect text(\"missing\").visible } }",
    );
    let output = run(directory.path(), "json", "1");
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report = report(&output);
    let trace = trace(&report, 0);
    assert!(!trace.manifest().attachments.is_empty());
    for reference in &trace.manifest().attachments {
        assert!(trace.attachment(reference.event).is_some());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_webtest"))
        .current_dir(directory.path())
        .args([
            "trace",
            report["traces"][0]["path"].as_str().unwrap(),
            "--output",
            "viewer.html",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(directory.path().join("viewer.html"))
            .unwrap()
            .contains("Download captured bytes")
    );
}

#[cfg(unix)]
#[test]
fn application_resources_are_joined_before_the_invocation_trace_terminal() {
    let directory = fixture(
        "[app]\ncommand = 'sh'\nargs = ['-c', 'exec sleep 60']\n[trace]\nmode = 'on'\n",
        "test \"pass\" { expect 1 == 1 }",
    );
    let output = run(directory.path(), "json", "1");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report(&output);
    assert_eq!(report["traces"].as_array().unwrap().len(), 2, "{report}");
    let application = trace(&report, 0);
    assert!(application.manifest().sources.is_empty());
    let records = application.records();
    assert_eq!(
        serde_json::to_value(records).unwrap(),
        report["application_events"]
    );
    let history: Vec<_> = records
        .iter()
        .filter_map(|record| match &record.event {
            ExecutionEvent::Resource { scope, event, .. } => {
                assert!(scope.is_none());
                assert!(event.resource.owner_scope_id.is_none());
                Some(event.kind)
            }
            _ => None,
        })
        .collect();
    use webtest_observation::ResourceEventKind as K;
    assert_eq!(
        history,
        [K::AcquireStarted, K::Ready, K::ReleaseStarted, K::Released]
    );
    assert!(matches!(
        records.last().unwrap().event,
        ExecutionEvent::RunFinished { .. }
    ));
}

#[cfg(unix)]
#[test]
fn failed_bridge_startup_exports_terminal_process_and_endpoint_cleanup() {
    let directory = fixture(
        "[app]\ncommand = 'sh'\nargs = ['-c', 'exec sleep 60']\n[server.app]\nadapter = 'bridge'\ntransport = 'tcp'\nschema = 'app-schema.json'\nstartup_timeout = '20ms'\n[trace]\nmode = 'retain-on-failure'\n",
        "test \"pass\" { expect 1 == 1 }",
    );
    fs::write(
        directory.path().join("app-schema.json"),
        include_str!("../../../protocol/examples/app-schema.json"),
    )
    .unwrap();
    let output = run(directory.path(), "json", "1");
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report = report(&output);
    assert_eq!(report["traces"].as_array().unwrap().len(), 1, "{report}");
    let captured = trace(&report, 0);
    let resources: Vec<_> = captured
        .records()
        .iter()
        .filter_map(|record| match &record.event {
            ExecutionEvent::Resource { event, .. } => Some(event),
            _ => None,
        })
        .collect();
    use webtest_observation::{ResourceEventKind as K, ResourceKind};
    for kind in [
        ResourceKind::ApplicationProcess,
        ResourceKind::BridgeEndpoint,
    ] {
        let history: Vec<_> = resources
            .iter()
            .filter(|event| event.resource.resource_kind == kind)
            .map(|event| event.kind)
            .collect();
        assert_eq!(
            history,
            [
                K::AcquireStarted,
                K::AcquireFailed,
                K::ReleaseStarted,
                K::Released
            ]
        );
    }
    assert!(matches!(
        captured.records().last().unwrap().event,
        ExecutionEvent::RunFinished {
            failure: Some(_),
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn worker_application_terminal_facts_share_one_invocation_journal() {
    let directory = fixture(
        "[app]\ncommand = 'sh'\nargs = ['-c', 'exec sleep 60']\n",
        "test \"a\" { expect 1 == 1 } test \"b\" { expect 2 == 2 } test \"c\" { expect 3 == 3 }",
    );
    let output = run(directory.path(), "json", "3");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report = report(&output);
    let events = report["application_events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|record| record["kind"] == "resource"
                && record["payload"]["event"]["kind"] == "released")
            .count(),
        3,
        "{report}"
    );
}

#[test]
fn imported_helper_and_fixture_sources_are_captured_and_failures_use_their_own_text() {
    let root_source = "import { verify, seed } from \"../support/helpers\"\ntest \"helper\" { verify() }\ntest \"fixture\" { let value = use seed() expect value == 7 }\n";
    let library = "export fn verify() -> Null {\n expect 1 == 2\n}\nexport fixture seed() -> Int scope suite shareable { setup { provide 7 } teardown { expect 2 == 3 } }\n";
    let directory = fixture(
        "[project]\ntest_roots = ['tests']\nsource_roots = ['tests', 'support']\n[trace]\nmode = 'on'\nsources = true\n",
        root_source,
    );
    fs::create_dir(directory.path().join("support")).unwrap();
    fs::write(directory.path().join("support/helpers.webtest"), library).unwrap();
    let output = run(directory.path(), "json", "2");
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value = report(&output);
    let failure = &value["files"][0]["tests"][0]["failure"];
    assert_eq!(failure["source"]["path"], "support/helpers.webtest");
    assert!(
        failure["span"]["source_line"]
            .as_str()
            .unwrap()
            .contains("expect 1 == 2")
    );
    let captured = trace(&value, 0);
    assert_eq!(captured.manifest().sources.len(), 2);
    assert!(
        captured
            .manifest()
            .sources
            .iter()
            .any(|source| captured.source(source.execution_id, source.file_id) == Some(library))
    );
    assert!(
        captured.manifest().sources.iter().any(|source| captured
            .source(source.execution_id, source.file_id)
            == Some(root_source))
    );
    assert!(
        captured
            .records()
            .iter()
            .any(|record| record.metadata.execution_context.fixture.is_some())
    );
}
