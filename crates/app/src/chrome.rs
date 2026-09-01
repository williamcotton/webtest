use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use webtest_browser::{
    BrowserContext, BrowserContextOptions, BrowserError, BrowserHost, BrowserSession, Page,
};
use webtest_browser_cdp::{ChromeHost, find_system_chrome};
use webtest_browser_manager::{BrowserManager, BrowserManagerError};
use webtest_project::Project;

use crate::{error::AppError, test_progress::HumanTestProgress};

#[derive(Clone)]
pub(crate) struct LazyChromeHost {
    shared: Arc<LazyChromeConfiguration>,
    display_path: String,
}

struct LazyChromeConfiguration {
    project: Project,
    explicit: Option<PathBuf>,
    headed: bool,
    command_timeout: Duration,
    navigation_timeout: Duration,
    resolved: Mutex<Option<PathBuf>>,
    progress: Option<Arc<HumanTestProgress>>,
}

impl LazyChromeHost {
    pub(crate) fn new(
        project: Project,
        explicit: Option<PathBuf>,
        headed: bool,
        progress: Option<Arc<HumanTestProgress>>,
    ) -> Self {
        let command_timeout = project.config.timeouts.browser_command;
        let navigation_timeout = project.config.timeouts.navigation;
        Self {
            shared: Arc::new(LazyChromeConfiguration {
                project,
                explicit,
                headed,
                command_timeout,
                navigation_timeout,
                resolved: Mutex::new(None),
                progress,
            }),
            display_path: String::new(),
        }
    }

    pub(crate) fn for_file(&self, display_path: impl Into<String>) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            display_path: display_path.into(),
        }
    }

    fn executable(&self) -> Result<PathBuf, BrowserError> {
        if let Some(path) = self
            .shared
            .resolved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return Ok(path);
        }
        let resolved = resolve_chrome(&self.shared.project, self.shared.explicit.clone())
            .map_err(|error| BrowserError::Launch(error.message))?;
        *self
            .shared
            .resolved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(resolved.path.clone());
        Ok(resolved.path)
    }
}

#[async_trait]
impl BrowserHost for LazyChromeHost {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
        let progress = BrowserStartProgress::new(
            self.shared.progress.clone(),
            &self.display_path,
            self.shared.headed,
        );
        let executable = match self.executable() {
            Ok(executable) => executable,
            Err(error) => {
                progress.finish(false);
                return Err(error);
            }
        };
        let host = ChromeHost::new(Some(executable))
            .with_headed(self.shared.headed)
            .with_timeouts(self.shared.command_timeout, self.shared.navigation_timeout);
        match host.start().await {
            Ok(session) => {
                progress.finish(true);
                Ok(Box::new(LazyChromeSession {
                    session,
                    progress: self.shared.progress.clone(),
                    close_announced: false,
                }))
            }
            Err(error) => {
                progress.finish(false);
                Err(error)
            }
        }
    }
}

struct BrowserStartProgress {
    progress: Option<Arc<HumanTestProgress>>,
}

impl BrowserStartProgress {
    fn new(progress: Option<Arc<HumanTestProgress>>, display_path: &str, headed: bool) -> Self {
        if let Some(progress) = &progress {
            let _ = progress.starting_browser(display_path, headed);
        }
        Self { progress }
    }

    fn finish(mut self, succeeded: bool) {
        if let Some(progress) = self.progress.take() {
            let _ = progress.browser_started(succeeded);
        }
    }
}

impl Drop for BrowserStartProgress {
    fn drop(&mut self) {
        if let Some(progress) = self.progress.take() {
            let _ = progress.browser_started(false);
        }
    }
}

struct LazyChromeSession {
    session: Box<dyn BrowserSession>,
    progress: Option<Arc<HumanTestProgress>>,
    close_announced: bool,
}

#[async_trait]
impl BrowserSession for LazyChromeSession {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        self.session.new_page().await
    }

    async fn new_context(
        &mut self,
        options: &BrowserContextOptions,
    ) -> Result<Box<dyn BrowserContext>, BrowserError> {
        self.session.new_context(options).await
    }

    async fn close(&mut self) -> Result<(), BrowserError> {
        if !self.close_announced {
            self.close_announced = true;
            if let Some(progress) = &self.progress {
                let _ = progress.stopping_browser();
            }
        }
        let result = self.session.close().await;
        if let Some(progress) = &self.progress {
            let _ = progress.browser_stopped(result.is_ok());
        }
        result
    }
}

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

    #[test]
    fn lazy_host_caches_a_successfully_resolved_executable() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let executable = directory.path().join("chrome");
        std::fs::write(&executable, b"fixture").expect("browser fixture");
        let project = Project {
            root: directory.path().to_path_buf(),
            config_path: None,
            config: webtest_project::ProjectConfig::default(),
            warnings: Vec::new(),
            files: Vec::new(),
        };
        let host = LazyChromeHost::new(project, Some(executable.clone()), false, None);
        let first = host.executable().expect("first resolution");
        std::fs::remove_file(executable).expect("remove fixture after resolution");
        assert_eq!(host.executable().expect("cached resolution"), first);
    }
}
