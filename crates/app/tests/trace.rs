use std::{collections::BTreeMap, fs, process::Command, time::Duration};
use webtest_observation::{
    EventJournal, EventTime, ExecutionEvent, ExecutionId, RunOutcomeKind, TestOutcomeKind,
};
use webtest_trace::{CapturePolicy, TraceInput, TraceLimits, TraceMetadata};

fn artifact() -> Vec<u8> {
    let execution_id = ExecutionId::from_u128(1);
    let mut journal = EventJournal::default();
    for event in [
        ExecutionEvent::RunStarted { execution_id },
        ExecutionEvent::TestStarted {
            execution_id,
            test_id: webtest_model::TestId(0),
            name: "<script>unsafe()</script>".into(),
        },
        ExecutionEvent::TestFinished {
            execution_id,
            test_id: webtest_model::TestId(0),
            outcome: TestOutcomeKind::Passed,
            failure_class: None,
        },
        ExecutionEvent::RunFinished {
            execution_id,
            outcome: RunOutcomeKind::Completed,
            failure_class: None,
            failure: None,
            cancellation: None,
        },
    ] {
        journal.record(
            event,
            EventTime {
                since_unix_epoch: Duration::ZERO,
                elapsed: Duration::ZERO,
            },
        );
    }
    webtest_trace::write(
        TraceInput {
            metadata: TraceMetadata {
                compiler_version: "fixture".into(),
                project_identity: [1; 32],
                configuration_fingerprint: [2; 32],
                browser_version: None,
                provider_versions: BTreeMap::new(),
                capture: CapturePolicy {
                    sources: false,
                    attachments: false,
                },
            },
            records: journal.records(),
            sources: &[],
            attachments: &[],
        },
        TraceLimits::default(),
    )
    .unwrap()
}

#[test]
fn static_viewer_validates_without_loading_project_configuration_and_does_not_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("webtest.toml"),
        "deliberately invalid TOML [",
    )
    .unwrap();
    let input = directory.path().join("run.webtest-trace");
    fs::write(&input, artifact()).unwrap();
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_webtest"))
            .current_dir(directory.path())
            .args(["trace", "run.webtest-trace", "--output", "viewer.html"])
            .output()
            .unwrap()
    };
    let result = invoke();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let html = fs::read_to_string(directory.path().join("viewer.html")).unwrap();
    assert!(html.contains("&lt;script&gt;unsafe()&lt;/script&gt;"));
    assert!(!html.contains("<script>"));
    assert!(html.contains("Execution record"));
    assert!(html.contains("Test finished · Passed"));
    let result = invoke();
    assert!(!result.status.success());
    assert_eq!(
        fs::read_to_string(directory.path().join("viewer.html")).unwrap(),
        html
    );
    assert_eq!(fs::read(&input).unwrap(), artifact());
}

#[test]
fn corrupt_or_oversized_archives_never_publish_html() {
    let directory = tempfile::tempdir().unwrap();
    for bytes in [b"invalid archive".to_vec(), artifact()[..25].to_vec()] {
        fs::write(directory.path().join("bad.webtest-trace"), bytes).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_webtest"))
            .current_dir(directory.path())
            .args(["trace", "bad.webtest-trace", "--output", "viewer.html"])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!directory.path().join("viewer.html").exists());
    }
    fs::File::create(directory.path().join("bad.webtest-trace"))
        .unwrap()
        .set_len(TraceLimits::default().max_archive_bytes as u64 + 1)
        .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_webtest"))
        .current_dir(directory.path())
        .args(["trace", "bad.webtest-trace", "--output", "viewer.html"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(!directory.path().join("viewer.html").exists());
}

#[cfg(unix)]
#[test]
fn trace_input_symlink_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("run.webtest-trace"), artifact()).unwrap();
    std::os::unix::fs::symlink(
        "run.webtest-trace",
        directory.path().join("link.webtest-trace"),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_webtest"))
        .current_dir(directory.path())
        .args(["trace", "link.webtest-trace", "--output", "viewer.html"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(!directory.path().join("viewer.html").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn chrome_renders_the_loopback_viewer_and_ctrl_c_reaps_the_server() {
    use tokio::io::AsyncBufReadExt;
    use webtest_browser::BrowserHost;
    let host = webtest_browser_cdp::ChromeHost::default();
    if host.locate().is_none() {
        eprintln!("skipping trace browser test: Chrome unavailable");
        return;
    }
    match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => drop(listener),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AddrNotAvailable
            ) =>
        {
            eprintln!("skipping trace browser test: loopback unavailable");
            return;
        }
        Err(error) => panic!("loopback: {error}"),
    }
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("run.webtest-trace"), artifact()).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_webtest"))
        .current_dir(directory.path())
        .args(["trace", "run.webtest-trace"])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let url = line
        .trim()
        .strip_prefix("trace viewer: ")
        .expect("viewer URL");
    assert!(url.starts_with("http://127.0.0.1:"));
    let mut session = host.start().await.unwrap();
    let mut context = session.new_context(&Default::default()).await.unwrap();
    let mut page = context.new_page().await.unwrap();
    page.open(url).await.unwrap();
    let result = page.evaluate(r#"
        if (document.title !== 'WebTest trace') throw new Error('viewer title');
        if (document.querySelectorAll('script, iframe, img').length) throw new Error('injected active content');
        if (!document.body.textContent.includes('<script>unsafe()</script>')) throw new Error('escaped test name missing');
        if (!document.querySelector('#timeline details').open) throw new Error('timeline collapsed');
        if (document.querySelectorAll('#timeline tbody tr').length !== 4) throw new Error('event loss');
        if (performance.getEntriesByType('resource').some(e => !e.name.startsWith(location.origin))) throw new Error('external request');
        document.querySelector('#timeline tbody details').open = true;
    "#).await;
    drop(page);
    context.close().await.unwrap();
    session.close().await.unwrap();
    let pid = child.id().unwrap();
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGINT) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    result.unwrap();
}
