use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};

use tempfile::TempDir;
use tokio::{
    process::{Child, Command as ProcessCommand},
    time::{Instant, sleep, timeout},
};
use webtest_browser::BrowserError;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub(crate) struct ChromeProcess {
    child: Child,
    profile: Option<TempDir>,
}

impl ChromeProcess {
    pub(crate) async fn launch(
        executable: &Path,
        headless: bool,
    ) -> Result<(Self, String), BrowserError> {
        let profile =
            tempfile::tempdir().map_err(|error| BrowserError::Launch(error.to_string()))?;
        let mut command = ProcessCommand::new(executable);
        let mut child = command
            .args(chrome_arguments(profile.path(), headless))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| BrowserError::Launch(error.to_string()))?;

        let port_file = profile.path().join("DevToolsActivePort");
        let contents = timeout(STARTUP_TIMEOUT, async {
            loop {
                match tokio::fs::read_to_string(&port_file).await {
                    Ok(contents) if contents.lines().take(2).count() == 2 => break Ok(contents),
                    Ok(_) => {
                        if let Some(status) = child
                            .try_wait()
                            .map_err(|error| BrowserError::Launch(error.to_string()))?
                        {
                            break Err(BrowserError::BrowserCrashed {
                                status: format!("exited before CDP became available ({status})"),
                            });
                        }
                        sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if let Some(status) = child
                            .try_wait()
                            .map_err(|error| BrowserError::Launch(error.to_string()))?
                        {
                            break Err(BrowserError::BrowserCrashed {
                                status: format!("exited before CDP became available ({status})"),
                            });
                        }
                        sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) => break Err(BrowserError::Launch(error.to_string())),
                }
            }
        })
        .await
        .map_err(|_| BrowserError::Launch("timed out waiting for Chrome to start".into()))??;

        let websocket_url = websocket_url(&contents)?;
        Ok((
            Self {
                child,
                profile: Some(profile),
            },
            websocket_url,
        ))
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), BrowserError> {
        let process_result = match timeout(SHUTDOWN_GRACE, self.child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(BrowserError::Launch(format!(
                "could not reap Chrome: {error}"
            ))),
            Err(_) => {
                self.child.start_kill().map_err(|error| {
                    BrowserError::Launch(format!("could not terminate Chrome: {error}"))
                })?;
                timeout(SHUTDOWN_GRACE, self.child.wait())
                    .await
                    .map_err(|_| BrowserError::Launch("timed out while reaping Chrome".into()))?
                    .map_err(|error| {
                        BrowserError::Launch(format!("could not reap Chrome: {error}"))
                    })?;
                Ok(())
            }
        };
        let profile_result = self.cleanup_profile().await;
        process_result.and(profile_result)
    }

    async fn cleanup_profile(&mut self) -> Result<(), BrowserError> {
        let Some(profile) = self.profile.take() else {
            return Ok(());
        };
        let path = profile.keep();
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match tokio::fs::remove_dir_all(&path).await {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) if Instant::now() >= deadline => {
                    return Err(BrowserError::Launch(format!(
                        "could not remove temporary Chrome profile {}: {error}",
                        path.display()
                    )));
                }
                Err(_) => sleep(Duration::from_millis(25)).await,
            }
        }
    }
}

fn chrome_arguments(profile: &Path, headless: bool) -> Vec<OsString> {
    let mut arguments = Vec::with_capacity(12);
    if headless {
        arguments.push("--headless=new".into());
        arguments.push("--disable-gpu".into());
    }
    arguments.extend([
        "--remote-debugging-address=127.0.0.1".into(),
        "--remote-debugging-port=0".into(),
        format!("--user-data-dir={}", profile.display()).into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-dev-shm-usage".into(),
        "--disable-background-timer-throttling".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
        "--no-startup-window".into(),
    ]);
    arguments
}

fn websocket_url(contents: &str) -> Result<String, BrowserError> {
    let mut lines = contents.lines();
    let port = lines
        .next()
        .ok_or_else(|| BrowserError::Launch("DevToolsActivePort did not contain a port".into()))?;
    let path = lines.next().ok_or_else(|| {
        BrowserError::Launch("DevToolsActivePort did not contain a WebSocket path".into())
    })?;
    Ok(format!("ws://127.0.0.1:{port}{path}"))
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let Some(profile) = self.profile.take() else {
            return;
        };
        let path = profile.keep();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let deadline = Instant::now() + SHUTDOWN_GRACE;
                loop {
                    match tokio::fs::remove_dir_all(&path).await {
                        Ok(()) => break,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                        Err(_) if Instant::now() >= deadline => break,
                        Err(_) => sleep(Duration::from_millis(25)).await,
                    }
                }
            });
        } else {
            let _ = fs_err_remove_dir_all(&path);
        }
    }
}

fn fs_err_remove_dir_all(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
impl ChromeProcess {
    pub(crate) fn profile_path(&self) -> Option<&Path> {
        self.profile.as_ref().map(TempDir::path)
    }

    pub(crate) fn start_kill(&mut self) -> std::io::Result<()> {
        self.child.start_kill()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_arguments_preserve_isolation_security_and_headed_behavior() {
        let profile = Path::new("/tmp/webtest-owned-profile");
        let headless = chrome_arguments(profile, true);
        let headless = headless
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(headless.first().map(String::as_str), Some("--headless=new"));
        assert!(headless.contains(&"--remote-debugging-address=127.0.0.1".into()));
        assert!(headless.contains(&"--remote-debugging-port=0".into()));
        assert!(headless.contains(&"--user-data-dir=/tmp/webtest-owned-profile".into()));
        assert!(headless.contains(&"--no-startup-window".into()));
        assert!(!headless.iter().any(|argument| argument == "--no-sandbox"));

        let headed = chrome_arguments(profile, false);
        assert!(!headed.iter().any(|argument| argument == "--headless=new"));
    }

    #[test]
    fn active_port_contents_preserve_local_websocket_derivation_and_errors() {
        assert_eq!(
            websocket_url("9222\n/devtools/browser/abc\n").expect("WebSocket URL"),
            "ws://127.0.0.1:9222/devtools/browser/abc"
        );
        assert!(
            matches!(websocket_url(""), Err(BrowserError::Launch(message)) if message.contains("port"))
        );
        assert!(
            matches!(websocket_url("9222\n"), Err(BrowserError::Launch(message)) if message.contains("WebSocket path"))
        );
    }

    #[test]
    fn missing_profile_cleanup_is_idempotent() {
        let directory = tempfile::tempdir().expect("temporary parent");
        let missing = directory.path().join("already-removed");
        fs_err_remove_dir_all(&missing).expect("missing profile is already clean");
    }
}
