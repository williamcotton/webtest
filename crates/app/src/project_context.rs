use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use webtest_project::{DiscoveredFile, Project};
use webtest_text::SourceRevision;

use crate::error::AppError;

pub(crate) fn project(paths: &[PathBuf]) -> Result<Project, AppError> {
    webtest_project::discover(paths).map_err(AppError::usage)
}

pub(crate) fn read_source(path: &Path) -> Result<String, AppError> {
    std::fs::read_to_string(path)
        .map_err(|error| AppError::usage(format!("could not read {}: {error}", path.display())))
}

pub(crate) fn display_path(file: &DiscoveredFile) -> String {
    normalized_path(&file.display_path)
}

pub(crate) fn normalized_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        path.into_owned()
    } else {
        path.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

pub(crate) fn revision_hex(revision: SourceRevision) -> String {
    revision
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_normalized_for_machine_output() {
        let path = Path::new("tests").join("nested").join("a.webtest");
        assert_eq!(normalized_path(&path), "tests/nested/a.webtest");
    }

    #[test]
    fn revisions_are_full_lowercase_hex() {
        let revision = revision_hex(SourceRevision::of("test"));
        assert_eq!(revision.len(), 64);
        assert!(
            revision
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
        assert_eq!(revision, revision.to_ascii_lowercase());
    }

    #[test]
    fn duration_conversion_saturates() {
        assert_eq!(nanos(Duration::from_nanos(17)), 17);
        assert_eq!(nanos(Duration::MAX), u64::MAX);
    }
}
