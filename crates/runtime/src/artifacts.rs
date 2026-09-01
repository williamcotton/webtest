use std::{
    io,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use tokio::time::Instant;
use webtest_browser::PageEvidence;
use webtest_hir::{StepId, TestId};
use webtest_observation::ExecutionId;

const MAX_CAPTURE_FAILURE_CHARS: usize = 1_024;
const PERSISTENCE_DEADLINE_FAILURE: &str =
    "artifact persistence exceeded the remaining test budget";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Screenshot,
    DomSnapshot,
    Evidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: PathBuf,
}

#[async_trait]
trait ArtifactFilesystem: Sync {
    async fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    async fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
}

struct TokioArtifactFilesystem;

#[async_trait]
impl ArtifactFilesystem for TokioArtifactFilesystem {
    async fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        tokio::fs::write(path, contents).await
    }
}

pub(crate) async fn write_artifacts(
    directory: &Path,
    execution_id: ExecutionId,
    test_id: TestId,
    step_id: StepId,
    deadline: Instant,
    evidence: &mut PageEvidence,
) -> Vec<Artifact> {
    write_artifacts_with(
        &TokioArtifactFilesystem,
        directory,
        execution_id,
        test_id,
        step_id,
        deadline,
        evidence,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn write_artifacts_with(
    filesystem: &dyn ArtifactFilesystem,
    directory: &Path,
    execution_id: ExecutionId,
    test_id: TestId,
    step_id: StepId,
    deadline: Instant,
    evidence: &mut PageEvidence,
) -> Vec<Artifact> {
    if evidence.screenshot_png.is_none()
        && evidence.dom_snapshot.is_none()
        && evidence.current_url.is_none()
        && evidence.capture_failures.is_empty()
    {
        return Vec::new();
    }
    match await_io(deadline, filesystem.create_dir_all(directory)).await {
        IoAttempt::Completed(Ok(())) => {}
        IoAttempt::Completed(Err(error)) => {
            push_capture_failure(evidence, format!("artifact directory: {error}"));
            return Vec::new();
        }
        IoAttempt::DeadlineExceeded => {
            push_capture_failure(evidence, PERSISTENCE_DEADLINE_FAILURE.into());
            return Vec::new();
        }
    }
    let stem = format!(
        "test-{}-step-{}-execution-{}",
        test_id.0, step_id.0, execution_id.0
    );
    let mut artifacts = Vec::new();
    if let Some(png) = evidence.screenshot_png.clone()
        && !write_artifact(
            filesystem,
            directory.join(format!("{stem}.png")),
            &png,
            ArtifactKind::Screenshot,
            deadline,
            evidence,
            &mut artifacts,
        )
        .await
    {
        return artifacts;
    }
    if let Some(dom) = evidence.dom_snapshot.clone()
        && !write_artifact(
            filesystem,
            directory.join(format!("{stem}.dom.html")),
            dom.as_bytes(),
            ArtifactKind::DomSnapshot,
            deadline,
            evidence,
            &mut artifacts,
        )
        .await
    {
        return artifacts;
    }
    let summary = format!(
        "url: {}\ntitle: {}\nelapsed evidence candidates: {}\nactionability: {:?}\nconsole errors: {:?}\ncapture failures: {:?}\n",
        evidence.current_url.as_deref().unwrap_or("<unavailable>"),
        evidence.title.as_deref().unwrap_or("<unavailable>"),
        evidence.candidates.len(),
        evidence.actionability,
        evidence.console_errors,
        evidence.capture_failures,
    );
    let _ = write_artifact(
        filesystem,
        directory.join(format!("{stem}.evidence.txt")),
        summary.as_bytes(),
        ArtifactKind::Evidence,
        deadline,
        evidence,
        &mut artifacts,
    )
    .await;
    artifacts
}

enum IoAttempt<T> {
    Completed(io::Result<T>),
    DeadlineExceeded,
}

async fn await_io<T>(
    deadline: Instant,
    operation: impl Future<Output = io::Result<T>>,
) -> IoAttempt<T> {
    if Instant::now() >= deadline {
        return IoAttempt::DeadlineExceeded;
    }
    tokio::pin!(operation);
    tokio::select! {
        biased;
        _ = tokio::time::sleep_until(deadline) => IoAttempt::DeadlineExceeded,
        result = &mut operation => IoAttempt::Completed(result),
    }
}

#[allow(clippy::too_many_arguments)]
async fn write_artifact(
    filesystem: &dyn ArtifactFilesystem,
    path: PathBuf,
    contents: &[u8],
    kind: ArtifactKind,
    deadline: Instant,
    evidence: &mut PageEvidence,
    artifacts: &mut Vec<Artifact>,
) -> bool {
    match await_io(deadline, filesystem.write(&path, contents)).await {
        IoAttempt::Completed(Ok(())) => {
            artifacts.push(Artifact { kind, path });
            true
        }
        IoAttempt::Completed(Err(error)) => {
            push_capture_failure(evidence, format!("write {}: {error}", path.display()));
            true
        }
        IoAttempt::DeadlineExceeded => {
            push_capture_failure(evidence, PERSISTENCE_DEADLINE_FAILURE.into());
            false
        }
    }
}

fn push_capture_failure(evidence: &mut PageEvidence, message: String) {
    if message.chars().count() <= MAX_CAPTURE_FAILURE_CHARS {
        evidence.capture_failures.push(message);
        return;
    }
    let mut bounded = message
        .chars()
        .take(MAX_CAPTURE_FAILURE_CHARS - 1)
        .collect::<String>();
    bounded.push('…');
    evidence.capture_failures.push(bounded);
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use tokio::sync::Notify;
    use webtest_hir::{StepId, TestId};
    use webtest_observation::ExecutionId;

    use super::*;

    fn generous_deadline() -> Instant {
        Instant::now() + std::time::Duration::from_secs(30)
    }

    #[tokio::test]
    async fn empty_evidence_does_not_create_an_artifact_directory() {
        let root = tempfile::tempdir().expect("temporary root");
        let directory = root.path().join("artifacts");
        let artifacts = write_artifacts(
            &directory,
            ExecutionId::next(),
            TestId(1),
            StepId(2),
            generous_deadline(),
            &mut PageEvidence::default(),
        )
        .await;
        assert!(artifacts.is_empty());
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn artifacts_use_deterministic_names_kinds_contents_and_order() {
        let root = tempfile::tempdir().expect("temporary root");
        let execution_id = ExecutionId::next();
        let mut evidence = PageEvidence {
            screenshot_png: Some(vec![1, 2, 3]),
            current_url: Some("https://example.test/".into()),
            title: Some("Example".into()),
            dom_snapshot: Some("<main>Example</main>".into()),
            ..PageEvidence::default()
        };
        let artifacts = write_artifacts(
            root.path(),
            execution_id,
            TestId(4),
            StepId(7),
            generous_deadline(),
            &mut evidence,
        )
        .await;
        let stem = format!("test-4-step-7-execution-{}", execution_id.0);
        assert_eq!(
            artifacts
                .iter()
                .map(|artifact| (artifact.kind.clone(), artifact.path.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    ArtifactKind::Screenshot,
                    root.path().join(format!("{stem}.png"))
                ),
                (
                    ArtifactKind::DomSnapshot,
                    root.path().join(format!("{stem}.dom.html"))
                ),
                (
                    ArtifactKind::Evidence,
                    root.path().join(format!("{stem}.evidence.txt"))
                ),
            ]
        );
        assert_eq!(
            tokio::fs::read(root.path().join(format!("{stem}.png")))
                .await
                .expect("screenshot"),
            [1, 2, 3]
        );
        assert_eq!(
            tokio::fs::read_to_string(root.path().join(format!("{stem}.dom.html")))
                .await
                .expect("DOM"),
            "<main>Example</main>"
        );
        assert_eq!(
            tokio::fs::read_to_string(root.path().join(format!("{stem}.evidence.txt")))
                .await
                .expect("summary"),
            "url: https://example.test/\ntitle: Example\nelapsed evidence candidates: 0\nactionability: []\nconsole errors: []\ncapture failures: []\n"
        );
    }

    #[tokio::test]
    async fn directory_failure_is_secondary_evidence() {
        let root = tempfile::tempdir().expect("temporary root");
        let file = root.path().join("not-a-directory");
        std::fs::write(&file, b"file").expect("fixture");
        let mut evidence = PageEvidence {
            current_url: Some("https://example.test/".into()),
            ..PageEvidence::default()
        };
        let artifacts = write_artifacts(
            &file,
            ExecutionId::next(),
            TestId(1),
            StepId(2),
            generous_deadline(),
            &mut evidence,
        )
        .await;
        assert!(artifacts.is_empty());
        assert_eq!(evidence.capture_failures.len(), 1);
        assert!(evidence.capture_failures[0].starts_with("artifact directory:"));
    }

    #[tokio::test]
    async fn one_write_failure_does_not_publish_a_false_path_or_stop_later_writes() {
        let root = tempfile::tempdir().expect("temporary root");
        let execution_id = ExecutionId::next();
        let stem = format!("test-1-step-2-execution-{}", execution_id.0);
        std::fs::create_dir(root.path().join(format!("{stem}.png")))
            .expect("screenshot collision directory");
        let mut evidence = PageEvidence {
            screenshot_png: Some(vec![1, 2, 3]),
            dom_snapshot: Some("<main>saved</main>".into()),
            ..PageEvidence::default()
        };

        let artifacts = write_artifacts(
            root.path(),
            execution_id,
            TestId(1),
            StepId(2),
            generous_deadline(),
            &mut evidence,
        )
        .await;

        assert_eq!(
            artifacts
                .iter()
                .map(|artifact| artifact.kind.clone())
                .collect::<Vec<_>>(),
            [ArtifactKind::DomSnapshot, ArtifactKind::Evidence]
        );
        assert_eq!(evidence.capture_failures.len(), 1);
        assert!(
            artifacts
                .iter()
                .all(|artifact| !artifact.path.ends_with(format!("{stem}.png")))
        );
        assert!(root.path().join(format!("{stem}.dom.html")).is_file());
        assert!(root.path().join(format!("{stem}.evidence.txt")).is_file());
    }

    struct DelayedFilesystem {
        delayed: AtomicBool,
        write_started: Notify,
        release_write: Notify,
    }

    #[async_trait]
    impl ArtifactFilesystem for DelayedFilesystem {
        async fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
            Ok(())
        }

        async fn write(&self, _path: &Path, _contents: &[u8]) -> io::Result<()> {
            if !self.delayed.swap(true, Ordering::SeqCst) {
                self.write_started.notify_one();
                self.release_write.notified().await;
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn delayed_storage_yields_to_another_tokio_task() {
        let filesystem = Arc::new(DelayedFilesystem {
            delayed: AtomicBool::new(false),
            write_started: Notify::new(),
            release_write: Notify::new(),
        });
        let other_task_ran = Arc::new(AtomicBool::new(false));
        let mut evidence = PageEvidence {
            screenshot_png: Some(vec![1, 2, 3]),
            ..PageEvidence::default()
        };
        let persistence = write_artifacts_with(
            filesystem.as_ref(),
            Path::new("unused"),
            ExecutionId::next(),
            TestId(1),
            StepId(2),
            generous_deadline(),
            &mut evidence,
        );
        let coordinator = {
            let filesystem = Arc::clone(&filesystem);
            let other_task_ran = Arc::clone(&other_task_ran);
            async move {
                filesystem.write_started.notified().await;
                other_task_ran.store(true, Ordering::SeqCst);
                filesystem.release_write.notify_one();
            }
        };

        let (artifacts, ()) = tokio::join!(persistence, coordinator);

        assert!(other_task_ran.load(Ordering::SeqCst));
        assert_eq!(artifacts.len(), 2);
    }

    struct DeadlineFilesystem {
        writes: Mutex<Vec<PathBuf>>,
    }

    #[async_trait]
    impl ArtifactFilesystem for DeadlineFilesystem {
        async fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
            Ok(())
        }

        async fn write(&self, path: &Path, _contents: &[u8]) -> io::Result<()> {
            self.writes.lock().expect("writes").push(path.to_path_buf());
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            Ok(())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn persistence_deadline_stops_later_writes() {
        let filesystem = DeadlineFilesystem {
            writes: Mutex::new(Vec::new()),
        };
        let mut evidence = PageEvidence {
            screenshot_png: Some(vec![1, 2, 3]),
            dom_snapshot: Some("<main>later</main>".into()),
            ..PageEvidence::default()
        };

        let artifacts = write_artifacts_with(
            &filesystem,
            Path::new("unused"),
            ExecutionId::next(),
            TestId(1),
            StepId(2),
            Instant::now() + std::time::Duration::from_secs(1),
            &mut evidence,
        )
        .await;

        assert!(artifacts.is_empty());
        assert_eq!(filesystem.writes.lock().expect("writes").len(), 1);
        assert_eq!(evidence.capture_failures, [PERSISTENCE_DEADLINE_FAILURE]);
    }

    #[tokio::test]
    async fn persisted_redactions_and_io_errors_never_expose_evidence_contents() {
        const SECRET: &str = "private-secret";
        let root = tempfile::tempdir().expect("temporary root");
        let execution_id = ExecutionId::next();
        let mut evidence = PageEvidence {
            current_url: Some("https://example.test/?token=%5Bredacted%5D".into()),
            title: Some("[redacted]".into()),
            dom_snapshot: Some("<main>[redacted]</main>".into()),
            console_errors: vec!["authorization=[redacted]".into()],
            ..PageEvidence::default()
        };
        let artifacts = write_artifacts(
            root.path(),
            execution_id,
            TestId(1),
            StepId(2),
            generous_deadline(),
            &mut evidence,
        )
        .await;
        let mut persisted_text = String::new();
        for artifact in artifacts {
            let bytes = tokio::fs::read(&artifact.path).await.expect("artifact");
            persisted_text.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(persisted_text.contains("[redacted]"));
        assert!(!persisted_text.contains(SECRET));

        let collision = root.path().join("collision");
        std::fs::write(&collision, b"file").expect("collision fixture");
        let mut failing_evidence = PageEvidence {
            dom_snapshot: Some(SECRET.into()),
            ..PageEvidence::default()
        };
        let artifacts = write_artifacts(
            &collision,
            ExecutionId::next(),
            TestId(1),
            StepId(2),
            generous_deadline(),
            &mut failing_evidence,
        )
        .await;
        assert!(artifacts.is_empty());
        assert!(
            failing_evidence
                .capture_failures
                .iter()
                .all(|failure| !failure.contains(SECRET))
        );
    }

    #[test]
    fn capture_failure_messages_are_unicode_safe_and_bounded() {
        let mut evidence = PageEvidence::default();
        push_capture_failure(&mut evidence, "é".repeat(MAX_CAPTURE_FAILURE_CHARS + 10));

        assert_eq!(
            evidence.capture_failures[0].chars().count(),
            MAX_CAPTURE_FAILURE_CHARS
        );
        assert!(evidence.capture_failures[0].ends_with('…'));
    }
}
