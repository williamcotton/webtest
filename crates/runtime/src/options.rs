use std::{path::PathBuf, time::Duration};

use webtest_browser::{BrowserContextOptions, InspectionOptions};
use webtest_provider::NativeProviderConfig;

#[derive(Clone, Debug)]
pub struct EvidenceOptions {
    pub screenshot_on_failure: bool,
    pub dom_snapshot_on_failure: bool,
    pub max_dom_bytes: usize,
    pub artifact_directory: PathBuf,
}

impl Default for EvidenceOptions {
    fn default() -> Self {
        Self {
            screenshot_on_failure: false,
            dom_snapshot_on_failure: false,
            max_dom_bytes: 1_048_576,
            artifact_directory: PathBuf::from(".webtest/artifacts"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunnerOptions {
    pub base_url: Option<String>,
    pub action_timeout: Duration,
    pub assertion_timeout: Duration,
    pub navigation_timeout: Duration,
    pub provider_call_timeout: Duration,
    pub test_timeout: Duration,
    pub browser_context: BrowserContextOptions,
    pub evidence: EvidenceOptions,
    pub project_root: PathBuf,
    pub redacted_json_fields: Vec<String>,
    pub provider_config: NativeProviderConfig,
    pub inspection: InspectionOptions,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            base_url: None,
            action_timeout: Duration::from_secs(5),
            assertion_timeout: Duration::from_secs(5),
            navigation_timeout: Duration::from_secs(30),
            provider_call_timeout: Duration::from_secs(60),
            test_timeout: Duration::from_secs(60),
            browser_context: BrowserContextOptions::default(),
            evidence: EvidenceOptions::default(),
            project_root: PathBuf::from("."),
            redacted_json_fields: vec![
                "password".into(),
                "token".into(),
                "secret".into(),
                "authorization".into(),
                "cookie".into(),
                "set-cookie".into(),
            ],
            provider_config: NativeProviderConfig::default(),
            inspection: InspectionOptions::default(),
        }
    }
}
