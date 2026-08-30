use std::{path::PathBuf, time::Duration};

use async_trait::async_trait;
use tracing::instrument;
use webtest_browser::{BrowserError, BrowserHost, BrowserSession};

use crate::{
    connection::CdpConnection, discovery::find_system_chrome, process::ChromeProcess,
    session::CdpBrowserSession,
};

const LOAD_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct ChromeHost {
    executable: Option<PathBuf>,
    headless: bool,
    command_timeout: Duration,
    navigation_timeout: Duration,
}

impl ChromeHost {
    pub fn new(executable: Option<PathBuf>) -> Self {
        Self {
            executable,
            headless: true,
            command_timeout: COMMAND_TIMEOUT,
            navigation_timeout: LOAD_TIMEOUT,
        }
    }

    pub fn with_headed(mut self, headed: bool) -> Self {
        self.headless = !headed;
        self
    }

    pub fn with_timeouts(mut self, command: Duration, navigation: Duration) -> Self {
        self.command_timeout = command;
        self.navigation_timeout = navigation;
        self
    }

    pub fn locate(&self) -> Option<PathBuf> {
        self.executable
            .clone()
            .or_else(|| std::env::var_os("WEBTEST_CHROME_PATH").map(PathBuf::from))
            .or_else(find_system_chrome)
    }
}

#[async_trait]
impl BrowserHost for ChromeHost {
    #[instrument(skip_all)]
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
        let executable = self.locate().ok_or_else(|| {
            BrowserError::Launch(
                "Chrome was not found; set WEBTEST_CHROME_PATH or pass --chrome-path".into(),
            )
        })?;
        let (process, websocket_url) = ChromeProcess::launch(&executable, self.headless).await?;
        let connection = CdpConnection::connect(&websocket_url, self.command_timeout).await?;
        Ok(Box::new(CdpBrowserSession::new(
            process,
            connection,
            self.navigation_timeout,
        )))
    }
}

impl Default for ChromeHost {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
impl ChromeHost {
    pub(crate) fn test_configuration(&self) -> (bool, Duration, Duration) {
        (self.headless, self.command_timeout, self.navigation_timeout)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn assert_host_traits<T: Clone + std::fmt::Debug + Send + Sync>() {}

    #[test]
    fn root_facade_preserves_configuration_and_auto_traits() {
        assert_host_traits::<crate::ChromeHost>();
        let explicit = PathBuf::from("/explicit/chrome");
        let host = crate::ChromeHost::new(Some(explicit.clone()))
            .with_timeouts(Duration::from_millis(7), Duration::from_millis(11));
        assert_eq!(host.locate(), Some(explicit));
        assert_eq!(
            host.test_configuration(),
            (true, Duration::from_millis(7), Duration::from_millis(11))
        );
        assert!(!host.with_headed(true).test_configuration().0);
        assert!(
            crate::ChromeHost::default()
                .with_headed(false)
                .test_configuration()
                .0
        );
    }

    #[test]
    fn defaults_remain_headless_with_distinct_command_and_navigation_timeouts() {
        assert_eq!(
            crate::ChromeHost::default().test_configuration(),
            (true, Duration::from_secs(10), Duration::from_secs(15))
        );
    }
}
