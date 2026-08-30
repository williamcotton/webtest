use std::path::{Path, PathBuf};

use webtest_browser::PageEvidence;
use webtest_hir::{StepId, TestId};
use webtest_observation::ExecutionId;

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

pub(crate) fn write_artifacts(
    directory: &Path,
    execution_id: ExecutionId,
    test_id: TestId,
    step_id: StepId,
    evidence: &mut PageEvidence,
) -> Vec<Artifact> {
    if evidence.screenshot_png.is_none()
        && evidence.dom_snapshot.is_none()
        && evidence.current_url.is_none()
        && evidence.capture_failures.is_empty()
    {
        return Vec::new();
    }
    if let Err(error) = std::fs::create_dir_all(directory) {
        evidence
            .capture_failures
            .push(format!("artifact directory: {error}"));
        return Vec::new();
    }
    let stem = format!(
        "test-{}-step-{}-execution-{}",
        test_id.0, step_id.0, execution_id.0
    );
    let mut artifacts = Vec::new();
    if let Some(png) = evidence.screenshot_png.clone() {
        write_artifact(
            directory.join(format!("{stem}.png")),
            &png,
            ArtifactKind::Screenshot,
            evidence,
            &mut artifacts,
        );
    }
    if let Some(dom) = evidence.dom_snapshot.clone() {
        write_artifact(
            directory.join(format!("{stem}.dom.html")),
            dom.as_bytes(),
            ArtifactKind::DomSnapshot,
            evidence,
            &mut artifacts,
        );
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
    write_artifact(
        directory.join(format!("{stem}.evidence.txt")),
        summary.as_bytes(),
        ArtifactKind::Evidence,
        evidence,
        &mut artifacts,
    );
    artifacts
}

fn write_artifact(
    path: PathBuf,
    contents: &[u8],
    kind: ArtifactKind,
    evidence: &mut PageEvidence,
    artifacts: &mut Vec<Artifact>,
) {
    match std::fs::write(&path, contents) {
        Ok(()) => artifacts.push(Artifact { kind, path }),
        Err(error) => evidence
            .capture_failures
            .push(format!("write {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use webtest_hir::{StepId, TestId};
    use webtest_observation::ExecutionId;

    use super::*;

    #[test]
    fn empty_evidence_does_not_create_an_artifact_directory() {
        let root = tempfile::tempdir().expect("temporary root");
        let directory = root.path().join("artifacts");
        let artifacts = write_artifacts(
            &directory,
            ExecutionId::next(),
            TestId(1),
            StepId(2),
            &mut PageEvidence::default(),
        );
        assert!(artifacts.is_empty());
        assert!(!directory.exists());
    }

    #[test]
    fn artifacts_use_deterministic_names_kinds_and_contents() {
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
            &mut evidence,
        );
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
            std::fs::read(root.path().join(format!("{stem}.png"))).expect("screenshot"),
            [1, 2, 3]
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join(format!("{stem}.dom.html"))).expect("DOM"),
            "<main>Example</main>"
        );
        let summary = std::fs::read_to_string(root.path().join(format!("{stem}.evidence.txt")))
            .expect("summary");
        assert!(summary.contains("url: https://example.test/"));
        assert!(summary.contains("title: Example"));
    }

    #[test]
    fn directory_failure_is_secondary_evidence() {
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
            &mut evidence,
        );
        assert!(artifacts.is_empty());
        assert_eq!(evidence.capture_failures.len(), 1);
        assert!(evidence.capture_failures[0].starts_with("artifact directory:"));
    }
}
