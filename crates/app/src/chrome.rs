use std::{
    fmt,
    path::{Path, PathBuf},
};

use webtest_browser_cdp::find_system_chrome;
use webtest_browser_manager::{BrowserManager, BrowserManagerError};
use webtest_project::Project;

use crate::error::AppError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChromeProvenance {
    Cli,
    Environment,
    Configuration,
    Managed,
    System,
}

impl fmt::Display for ChromeProvenance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cli => "command line",
            Self::Environment => "WEBTEST_CHROME_PATH",
            Self::Configuration => "webtest.toml",
            Self::Managed => "managed Chrome for Testing",
            Self::System => "system discovery",
        })
    }
}

#[derive(Debug)]
pub(crate) struct ResolvedChrome {
    pub(crate) path: PathBuf,
    pub(crate) provenance: ChromeProvenance,
}

pub(crate) fn resolve_chrome(
    project: &Project,
    explicit: Option<PathBuf>,
) -> Result<ResolvedChrome, AppError> {
    if let Some((path, provenance)) = select_preconfigured_candidate(
        &project.root,
        explicit,
        std::env::var_os("WEBTEST_CHROME_PATH").map(PathBuf::from),
        project.config.browser.path.clone(),
    ) {
        return resolved_existing(path, provenance);
    }
    if project.config.browser.channel == webtest_project::BrowserChannel::Managed
        && let Some(installed) = BrowserManager::new()
            .map_err(AppError::infrastructure)?
            .current()
            .map_err(AppError::infrastructure)?
    {
        return Ok(ResolvedChrome {
            path: installed.executable,
            provenance: ChromeProvenance::Managed,
        });
    }
    if let Some(path) = find_system_chrome() {
        return Ok(ResolvedChrome {
            path,
            provenance: ChromeProvenance::System,
        });
    }
    Err(AppError::infrastructure(
        "Chrome was not found; run `webtest browser install`, set WEBTEST_CHROME_PATH, or configure browser.path",
    ))
}

fn select_preconfigured_candidate(
    root: &Path,
    explicit: Option<PathBuf>,
    environment: Option<PathBuf>,
    configuration: Option<PathBuf>,
) -> Option<(PathBuf, ChromeProvenance)> {
    explicit
        .map(|path| (path, ChromeProvenance::Cli))
        .or_else(|| environment.map(|path| (path, ChromeProvenance::Environment)))
        .or_else(|| {
            configuration.map(|path| {
                let path = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                (path, ChromeProvenance::Configuration)
            })
        })
}

fn resolved_existing(
    path: PathBuf,
    provenance: ChromeProvenance,
) -> Result<ResolvedChrome, AppError> {
    if !path.is_file() {
        return Err(AppError::infrastructure(format!(
            "Chrome selected from {provenance} does not exist at {}",
            path.display()
        )));
    }
    let path = std::fs::canonicalize(&path).map_err(|error| {
        AppError::infrastructure(format!(
            "could not resolve Chrome selected from {provenance} at {}: {error}",
            path.display()
        ))
    })?;
    Ok(ResolvedChrome { path, provenance })
}

pub(crate) fn browser_manager_error(error: BrowserManagerError) -> AppError {
    match error {
        BrowserManagerError::UnsupportedVersion { .. } => AppError::usage(error),
        _ => AppError::infrastructure(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_candidate_precedence_is_exact() {
        let root = Path::new("/project");
        let cases = [
            (
                Some(PathBuf::from("cli")),
                Some(PathBuf::from("environment")),
                Some(PathBuf::from("configuration")),
                PathBuf::from("cli"),
                ChromeProvenance::Cli,
            ),
            (
                None,
                Some(PathBuf::from("environment")),
                Some(PathBuf::from("configuration")),
                PathBuf::from("environment"),
                ChromeProvenance::Environment,
            ),
            (
                None,
                None,
                Some(PathBuf::from("configuration")),
                root.join("configuration"),
                ChromeProvenance::Configuration,
            ),
        ];
        for (explicit, environment, configuration, expected, provenance) in cases {
            assert_eq!(
                select_preconfigured_candidate(root, explicit, environment, configuration),
                Some((expected, provenance))
            );
        }
    }

    #[test]
    fn invalid_explicit_selection_is_strict() {
        let error = resolved_existing(PathBuf::from("definitely-missing"), ChromeProvenance::Cli)
            .expect_err("missing path must fail");
        assert_eq!(error.class, crate::report::ExitClass::Infrastructure);
        assert!(error.message.contains("command line"));
    }
}
