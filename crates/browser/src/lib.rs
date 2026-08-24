//! Protocol-independent browser semantics used by the test runtime.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
pub use webtest_feedback::{RepairHint, RepairHintKind, RepairReplacement};

pub const MAX_CANDIDATES: usize = 5;
pub const INSPECTION_SCHEMA_VERSION: u32 = 1;
pub const MAX_INSPECTION_ELEMENTS: usize = 2_000;
pub const MAX_LOCATOR_CANDIDATES_PER_ELEMENT: usize = 8;
pub const MAX_INSPECTION_TEXT_BYTES: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectionOptions {
    pub max_elements: usize,
    pub max_candidates_per_element: usize,
    pub max_text_bytes: usize,
    pub include_hidden: bool,
    pub redacted_query_parameters: Vec<String>,
    pub redacted_values: Vec<String>,
}

impl Default for InspectionOptions {
    fn default() -> Self {
        Self {
            max_elements: 500,
            max_candidates_per_element: 4,
            max_text_bytes: 256,
            include_hidden: false,
            redacted_query_parameters: vec!["token".into(), "code".into(), "key".into()],
            redacted_values: Vec::new(),
        }
    }
}

impl InspectionOptions {
    pub fn bounded(&self) -> Self {
        Self {
            max_elements: self.max_elements.clamp(1, MAX_INSPECTION_ELEMENTS),
            max_candidates_per_element: self
                .max_candidates_per_element
                .clamp(1, MAX_LOCATOR_CANDIDATES_PER_ELEMENT),
            max_text_bytes: self.max_text_bytes.clamp(1, MAX_INSPECTION_TEXT_BYTES),
            include_hidden: self.include_hidden,
            redacted_query_parameters: self.redacted_query_parameters.clone(),
            redacted_values: self.redacted_values.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageInspection {
    pub kind: String,
    pub inspection_schema_version: u32,
    pub snapshot_id: String,
    pub browser_version: String,
    pub page: PageSummary,
    pub elements: Vec<InspectableElement>,
    pub truncation: InspectionTruncation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSummary {
    pub url: String,
    pub title: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementStates {
    pub visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receives_pointer_input: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportedAction {
    Fill,
    Type,
    Press,
    Click,
    Check,
    Uncheck,
    Select,
    Hover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocatorCandidateKind {
    Label,
    Role,
    TestId,
    Id,
    Placeholder,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocatorCandidate {
    pub source: String,
    pub kind: LocatorCandidateKind,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectableElement {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(rename = "name", skip_serializing_if = "Option::is_none")]
    pub accessible_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dom_id: Option<String>,
    pub states: ElementStates,
    pub supported_actions: Vec<SupportedAction>,
    pub preferred_locator: LocatorCandidate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternate_locators: Vec<LocatorCandidate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionTruncation {
    pub elements_truncated: bool,
    pub omitted_elements: usize,
    pub candidates_truncated: bool,
    pub text_truncated: bool,
    pub options_truncated: bool,
}

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
    pub redacted_query_parameters: Vec<String>,
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
    #[error("evaluation of {expression:?} failed: {message}")]
    EvaluationFailed { expression: String, message: String },
    #[error("browser capability `{capability}` is unavailable")]
    UnsupportedCapability { capability: String },
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
            Self::EvaluationFailed { .. } => "evaluation_failed",
            Self::UnsupportedCapability { .. } => "unsupported_browser_capability",
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
                | Self::UnsupportedCapability { .. }
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
    async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError>;

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

    async fn inspect(
        &mut self,
        _options: &InspectionOptions,
    ) -> Result<PageInspection, BrowserError> {
        Err(BrowserError::UnsupportedCapability {
            capability: "semantic_inspection".into(),
        })
    }
}

pub fn locator_repair_hints(
    requested: &Locator,
    inspection: &PageInspection,
    limit: usize,
) -> Vec<RepairHint> {
    let (requested_kind, requested_role, requested_name) = match requested {
        Locator::Role { role, name } => {
            (Some(LocatorCandidateKind::Role), Some(role), name.as_ref())
        }
        Locator::Label(value) => (Some(LocatorCandidateKind::Label), None, Some(value)),
        Locator::Text(value) => (Some(LocatorCandidateKind::Text), None, Some(value)),
        Locator::Placeholder(value) => (Some(LocatorCandidateKind::Placeholder), None, Some(value)),
        Locator::TestId(value) => (Some(LocatorCandidateKind::TestId), None, Some(value)),
        Locator::Id(value) => (Some(LocatorCandidateKind::Id), None, Some(value)),
        Locator::Css(_) | Locator::XPath(_) => (None, None, None),
    };
    if requested_kind.is_none() {
        return Vec::new();
    }
    let requested_source = requested.to_string();
    let mut candidates = inspection
        .elements
        .iter()
        .flat_map(|element| {
            std::iter::once(&element.preferred_locator)
                .chain(element.alternate_locators.iter())
                .map(move |candidate| (element, candidate))
        })
        .filter(|(_, candidate)| candidate.source != requested_source)
        .map(|(element, candidate)| {
            let role_penalty =
                usize::from(requested_role.is_some_and(|role| element.role.as_ref() != Some(role)));
            let kind_penalty = usize::from(Some(candidate.kind) != requested_kind);
            let candidate_name = element
                .accessible_name
                .as_ref()
                .or(element.label.as_ref())
                .or(element.placeholder.as_ref());
            let distance = requested_name
                .zip(candidate_name)
                .map_or(usize::MAX / 4, |(left, right)| edit_distance(left, right));
            (
                role_penalty,
                kind_penalty,
                distance,
                candidate.source.clone(),
                candidate.reason.clone(),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup_by(|left, right| left.3 == right.3);
    candidates
        .into_iter()
        .take(limit.min(MAX_CANDIDATES))
        .map(|(role_penalty, _, _, source, candidate_reason)| {
            let reason = if role_penalty == 0 && requested_role.is_some() {
                "same accessible role with a nearby accessible name".to_owned()
            } else {
                candidate_reason
            };
            RepairHint::locator(source, reason)
        })
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right.iter().enumerate() {
            current.push(
                (previous[right_index + 1] + 1)
                    .min(current[right_index] + 1)
                    .min(previous[right_index] + usize::from(left_char != *right_char)),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button() -> InspectableElement {
        InspectableElement {
            role: Some("button".into()),
            accessible_name: Some("Sign in".into()),
            label: None,
            placeholder: None,
            test_id: Some("sign-in".into()),
            dom_id: None,
            states: ElementStates {
                visible: true,
                enabled: Some(true),
                receives_pointer_input: Some(true),
                ..ElementStates::default()
            },
            supported_actions: vec![SupportedAction::Click, SupportedAction::Hover],
            preferred_locator: LocatorCandidate {
                source: "role(\"button\", name: \"Sign in\")".into(),
                kind: LocatorCandidateKind::Role,
                reason: "unique accessible role and name".into(),
            },
            alternate_locators: vec![LocatorCandidate {
                source: "test_id(\"sign-in\")".into(),
                kind: LocatorCandidateKind::TestId,
                reason: "unique configured test ID".into(),
            }],
            options: Vec::new(),
        }
    }

    #[test]
    fn inspection_and_repair_serialization_is_versioned_and_semantic() {
        let inspection = PageInspection {
            kind: "inspection".into(),
            inspection_schema_version: INSPECTION_SCHEMA_VERSION,
            snapshot_id: "snapshot-1".into(),
            browser_version: "Chrome/1".into(),
            page: PageSummary {
                url: "http://example.test/login".into(),
                title: "Sign in".into(),
            },
            elements: vec![button()],
            truncation: InspectionTruncation::default(),
        };
        let hints = locator_repair_hints(
            &Locator::Role {
                role: "button".into(),
                name: Some("Log in".into()),
            },
            &inspection,
            MAX_CANDIDATES,
        );
        assert_eq!(hints.len(), 2);
        assert_eq!(
            hints[0].replacement,
            RepairReplacement::locator("role(\"button\", name: \"Sign in\")")
        );
        let value = serde_json::to_value(inspection).expect("inspection JSON");
        assert_eq!(value["inspection_schema_version"], 1);
        assert_eq!(value["elements"][0]["name"], "Sign in");
        assert!(value["elements"][0].get("backend_node_id").is_none());
    }

    #[test]
    fn configured_inspection_limits_are_hard_bounded() {
        let bounded = InspectionOptions {
            max_elements: usize::MAX,
            max_candidates_per_element: usize::MAX,
            max_text_bytes: usize::MAX,
            include_hidden: true,
            redacted_query_parameters: Vec::new(),
            redacted_values: Vec::new(),
        }
        .bounded();
        assert_eq!(bounded.max_elements, MAX_INSPECTION_ELEMENTS);
        assert_eq!(
            bounded.max_candidates_per_element,
            MAX_LOCATOR_CANDIDATES_PER_ELEMENT
        );
        assert_eq!(bounded.max_text_bytes, MAX_INSPECTION_TEXT_BYTES);
    }
}
