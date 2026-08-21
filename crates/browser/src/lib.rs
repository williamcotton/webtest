//! Protocol-independent browser semantics used by the test runtime.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_CANDIDATES: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Locator {
    Id(String),
    Role { role: String, name: Option<String> },
    Label(String),
    Text(String),
    Placeholder(String),
    TestId(String),
    Css(String),
    XPath(String),
}

impl std::fmt::Display for Locator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(value) => write!(f, "id({value:?})"),
            Self::Role {
                role,
                name: Some(name),
            } => write!(f, "role({role:?}, name: {name:?})"),
            Self::Role { role, name: None } => write!(f, "role({role:?})"),
            Self::Label(value) => write!(f, "label({value:?})"),
            Self::Text(value) => write!(f, "text({value:?})"),
            Self::Placeholder(value) => write!(f, "placeholder({value:?})"),
            Self::TestId(value) => write!(f, "test_id({value:?})"),
            Self::Css(value) => write!(f, "css({value:?})"),
            Self::XPath(value) => write!(f, "xpath({value:?})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LocatorState {
    Visible,
    Hidden,
    Attached,
    Detached,
    Enabled,
    Disabled,
    Checked,
    Unchecked,
}

impl std::fmt::Display for LocatorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Visible => "visible",
                Self::Hidden => "hidden",
                Self::Attached => "attached",
                Self::Detached => "detached",
                Self::Enabled => "enabled",
                Self::Disabled => "disabled",
                Self::Checked => "checked",
                Self::Unchecked => "unchecked",
            }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Click { locator: Locator },
    Fill { locator: Locator, value: String },
    Type { locator: Locator, value: String },
    Press { locator: Locator, key: String },
    Check { locator: Locator, checked: bool },
    Select { locator: Locator, option: String },
    Hover { locator: Locator },
}

impl Action {
    pub fn locator(&self) -> &Locator {
        match self {
            Self::Click { locator }
            | Self::Fill { locator, .. }
            | Self::Type { locator, .. }
            | Self::Press { locator, .. }
            | Self::Check { locator, .. }
            | Self::Select { locator, .. }
            | Self::Hover { locator } => locator,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserContextOptions {
    pub viewport: Viewport,
    pub test_id_attribute: String,
}

impl Default for BrowserContextOptions {
    fn default() -> Self {
        Self {
            viewport: Viewport::default(),
            test_id_attribute: "data-testid".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateEvidence {
    pub tag: String,
    pub id: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvidenceRequest {
    pub locator: Option<Locator>,
    pub include_screenshot: bool,
    pub include_dom: bool,
    pub max_dom_bytes: usize,
    pub redactions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageEvidence {
    pub screenshot_png: Option<Vec<u8>>,
    pub current_url: Option<String>,
    pub title: Option<String>,
    pub candidates: Vec<CandidateEvidence>,
    pub actionability: Vec<String>,
    pub dom_snapshot: Option<String>,
    pub console_errors: Vec<String>,
    pub capture_failures: Vec<String>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum BrowserError {
    #[error("no element matching {locator} was found")]
    LocatorNotFound { locator: Locator },
    #[error("locator {locator} matched {matches} elements")]
    LocatorAmbiguous { locator: Locator, matches: usize },
    #[error("locator {locator} is invalid: {message}")]
    LocatorInvalid { locator: Locator, message: String },
    #[error("element matching {locator} is detached")]
    ElementDetached { locator: Locator },
    #[error("element matching {locator} is not visible")]
    LocatorNotVisible { locator: Locator },
    #[error("element matching {locator} did not become stable")]
    ElementUnstable { locator: Locator },
    #[error("element matching {locator} is disabled")]
    ElementDisabled { locator: Locator },
    #[error("element matching {locator} is obscured")]
    ElementObscured { locator: Locator },
    #[error("element matching {locator} is not editable")]
    ElementNotEditable { locator: Locator },
    #[error("option {option:?} was not found for {locator}")]
    OptionNotFound { locator: Locator, option: String },
    #[error("option {option:?} is ambiguous for {locator} ({matches} matches)")]
    OptionAmbiguous {
        locator: Locator,
        option: String,
        matches: usize,
    },
    #[error("invalid key chord {key:?}")]
    InvalidKey { key: String },
    #[error("action on {locator} timed out after {timeout_ms}ms")]
    ActionTimeout { locator: Locator, timeout_ms: u64 },
    #[error("expected {locator} to be {expected}, but its final state was {actual}")]
    AssertionFailed {
        locator: Locator,
        expected: LocatorState,
        actual: String,
    },
    #[error("expected URL {expected:?}, got {actual:?}")]
    UrlMismatch { expected: String, actual: String },
    #[error("navigation to {url} failed: {reason}")]
    NavigationFailed { url: String, reason: String },
    #[error("navigation to {url} timed out after {timeout_ms}ms")]
    NavigationTimeout { url: String, timeout_ms: u64 },
    #[error("CDP method {method} timed out after {timeout_ms}ms")]
    CommandTimeout { method: String, timeout_ms: u64 },
    #[error("the browser disconnected")]
    BrowserDisconnected,
    #[error("the browser crashed: {status}")]
    BrowserCrashed { status: String },
    #[error("Chrome sent a malformed CDP message: {message}")]
    MalformedProtocol { message: String },
    #[error("CDP method {method} failed: {message}")]
    Protocol { method: String, message: String },
    #[error("could not launch the browser: {0}")]
    Launch(String),
    #[error("Evaluation of '{expression}' failed: ")]
    EvaluationFailed { expression: String, message: String }
}

impl BrowserError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::LocatorNotFound { .. } => "locator_not_found",
            Self::LocatorAmbiguous { .. } => "locator_ambiguous",
            Self::LocatorInvalid { .. } => "locator_invalid",
            Self::ElementDetached { .. } => "element_detached",
            Self::LocatorNotVisible { .. } => "element_not_visible",
            Self::ElementUnstable { .. } => "element_unstable",
            Self::ElementDisabled { .. } => "element_disabled",
            Self::ElementObscured { .. } => "element_obscured",
            Self::ElementNotEditable { .. } => "element_not_editable",
            Self::OptionNotFound { .. } => "option_not_found",
            Self::OptionAmbiguous { .. } => "option_ambiguous",
            Self::InvalidKey { .. } => "invalid_key",
            Self::ActionTimeout { .. } => "action_timeout",
            Self::AssertionFailed { .. } => "assertion_failed",
            Self::UrlMismatch { .. } => "url_mismatch",
            Self::NavigationFailed { .. } => "navigation_failed",
            Self::NavigationTimeout { .. } => "navigation_timeout",
            Self::CommandTimeout { .. } => "browser_command_timeout",
            Self::BrowserDisconnected => "browser_disconnected",
            Self::BrowserCrashed { .. } => "browser_crashed",
            Self::MalformedProtocol { .. } => "browser_malformed_protocol",
            Self::Protocol { .. } => "browser_protocol",
            Self::Launch(_) => "browser_launch",
            Self::EvaluationFailed { .. } => "evaluation_failed"
        }
    }

    pub fn is_infrastructure(&self) -> bool {
        matches!(
            self,
            Self::NavigationFailed { .. }
                | Self::NavigationTimeout { .. }
                | Self::CommandTimeout { .. }
                | Self::BrowserDisconnected
                | Self::BrowserCrashed { .. }
                | Self::MalformedProtocol { .. }
                | Self::Protocol { .. }
                | Self::Launch(_)
        )
    }
}

#[async_trait]
pub trait BrowserHost: Send + Sync {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError>;
}

#[async_trait]
pub trait BrowserSession: Send {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError>;

    async fn new_context(
        &mut self,
        _options: &BrowserContextOptions,
    ) -> Result<Box<dyn BrowserContext>, BrowserError> {
        Ok(Box::new(SinglePageContext {
            page: Some(self.new_page().await?),
        }))
    }

    async fn close(&mut self) -> Result<(), BrowserError> {
        Ok(())
    }
}

#[async_trait]
pub trait BrowserContext: Send {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError>;
    async fn close(&mut self) -> Result<(), BrowserError> {
        Ok(())
    }
}

struct SinglePageContext {
    page: Option<Box<dyn Page>>,
}

#[async_trait]
impl BrowserContext for SinglePageContext {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
        self.page.take().ok_or_else(|| BrowserError::Protocol {
            method: "BrowserContext.new_page".into(),
            message: "legacy context supports one page".into(),
        })
    }
}

#[async_trait]
pub trait Page: Send {
    async fn open(&mut self, url: &str) -> Result<(), BrowserError>;
    async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError>;
    async fn expect_visible(&mut self, locator: &Locator) -> Result<(), BrowserError>;
    async fn evaluate_expression(&mut self, expression: &str) -> Result<(), BrowserError>;

    async fn perform(&mut self, action: &Action, _timeout: Duration) -> Result<(), BrowserError> {
        match action {
            Action::Click { locator } => self.click(locator).await,
            _ => Err(BrowserError::Protocol {
                method: "Page.perform".into(),
                message: "action is not implemented by this browser backend".into(),
            }),
        }
    }

    async fn wait_for_locator(
        &mut self,
        locator: &Locator,
        state: LocatorState,
        _timeout: Duration,
    ) -> Result<(), BrowserError> {
        if state == LocatorState::Visible {
            self.expect_visible(locator).await
        } else {
            Err(BrowserError::Protocol {
                method: "Page.wait_for_locator".into(),
                message: format!("state {state} is not implemented by this browser backend"),
            })
        }
    }

    async fn wait_for_url(
        &mut self,
        expected: &str,
        _timeout: Duration,
    ) -> Result<(), BrowserError> {
        let actual = self.current_url().await?;
        if actual == expected {
            Ok(())
        } else {
            Err(BrowserError::UrlMismatch {
                expected: expected.into(),
                actual,
            })
        }
    }

    async fn current_url(&mut self) -> Result<String, BrowserError> {
        Err(BrowserError::Protocol {
            method: "Page.current_url".into(),
            message: "not implemented by this browser backend".into(),
        })
    }

    async fn capture_evidence(&mut self, _request: &EvidenceRequest) -> PageEvidence {
        PageEvidence::default()
    }
}
