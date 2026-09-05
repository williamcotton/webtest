//! Structured execution events and revision-safe source observations.

mod scope;
pub use scope::{ExecutionContext, ScopeCancellation, ScopeEvent, ScopeOutcome};

use std::{
    collections::BTreeMap,
    collections::HashMap,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use webtest_browser::{CandidateEvidence, Locator, PageSummary};
use webtest_feedback::{FailureClass, RepairHint};
use webtest_model::{StepId, TestId};
use webtest_text::{FileId, SourceRevision, TextRange};

/// Stable machine identity for every implemented runtime failure.
///
/// The enum is intentionally payload-free: structured browser, provider, cleanup, and runtime
/// errors remain authoritative. Its serialized form is the existing short code spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimeFailureCode {
    LocatorNotFound,
    LocatorAmbiguous,
    LocatorInvalid,
    ElementDetached,
    LocatorNotVisible,
    ElementUnstable,
    ElementDisabled,
    ElementObscured,
    ElementNotEditable,
    OptionNotFound,
    OptionAmbiguous,
    InvalidKey,
    ActionTimeout,
    AssertionFailed,
    UrlMismatch,
    NavigationFailed,
    NavigationTimeout,
    BrowserCommandTimeout,
    BrowserDisconnected,
    BrowserCrashed,
    BrowserMalformedProtocol,
    BrowserProtocol,
    BrowserLaunch,
    EvaluationFailed,
    UnsupportedBrowserCapability,
    ProviderNotRegistered,
    ProviderUnknownOperation,
    ProviderInvalidArgument,
    HttpTransport,
    ResponseTooLarge,
    ProcessSpawn,
    ProcessTimeout,
    ProcessOutputTooLarge,
    Filesystem,
    PathEscape,
    ProviderUnavailable,
    AppBridgeHandshake,
    AppBridgeProtocol,
    AppBridgeTransport,
    AppBridgeProcess,
    AppSchemaDrift,
    AppBridgeValidation,
    AppBridgeTimeout,
    AppProviderFailure,
    TestTimeout,
    JsonDecodeFailed,
    ResponseDecodeFailed,
    DivisionByZero,
    IntegerOverflow,
    InternalError,
    CleanupBrowserContextFailed,
    CleanupBrowserSessionFailed,
    CleanupTemporaryDirectoryFailed,
}

impl RuntimeFailureCode {
    pub const fn short_code(self) -> &'static str {
        match self {
            Self::LocatorNotFound => "locator_not_found",
            Self::LocatorAmbiguous => "locator_ambiguous",
            Self::LocatorInvalid => "locator_invalid",
            Self::ElementDetached => "element_detached",
            Self::LocatorNotVisible => "element_not_visible",
            Self::ElementUnstable => "element_unstable",
            Self::ElementDisabled => "element_disabled",
            Self::ElementObscured => "element_obscured",
            Self::ElementNotEditable => "element_not_editable",
            Self::OptionNotFound => "option_not_found",
            Self::OptionAmbiguous => "option_ambiguous",
            Self::InvalidKey => "invalid_key",
            Self::ActionTimeout => "action_timeout",
            Self::AssertionFailed => "assertion_failed",
            Self::UrlMismatch => "url_mismatch",
            Self::NavigationFailed => "navigation_failed",
            Self::NavigationTimeout => "navigation_timeout",
            Self::BrowserCommandTimeout => "browser_command_timeout",
            Self::BrowserDisconnected => "browser_disconnected",
            Self::BrowserCrashed => "browser_crashed",
            Self::BrowserMalformedProtocol => "browser_malformed_protocol",
            Self::BrowserProtocol => "browser_protocol",
            Self::BrowserLaunch => "browser_launch",
            Self::EvaluationFailed => "evaluation_failed",
            Self::UnsupportedBrowserCapability => "unsupported_browser_capability",
            Self::ProviderNotRegistered => "provider_not_registered",
            Self::ProviderUnknownOperation => "provider_unknown_operation",
            Self::ProviderInvalidArgument => "provider_invalid_argument",
            Self::HttpTransport => "http_transport",
            Self::ResponseTooLarge => "response_too_large",
            Self::ProcessSpawn => "process_spawn",
            Self::ProcessTimeout => "process_timeout",
            Self::ProcessOutputTooLarge => "process_output_too_large",
            Self::Filesystem => "filesystem",
            Self::PathEscape => "path_escape",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::AppBridgeHandshake => "app_bridge_handshake",
            Self::AppBridgeProtocol => "app_bridge_protocol",
            Self::AppBridgeTransport => "app_bridge_transport",
            Self::AppBridgeProcess => "app_bridge_process",
            Self::AppSchemaDrift => "app_schema_drift",
            Self::AppBridgeValidation => "app_bridge_validation",
            Self::AppBridgeTimeout => "app_bridge_timeout",
            Self::AppProviderFailure => "app_provider_failure",
            Self::TestTimeout => "test_timeout",
            Self::JsonDecodeFailed => "json_decode_failed",
            Self::ResponseDecodeFailed => "response_decode_failed",
            Self::DivisionByZero => "division_by_zero",
            Self::IntegerOverflow => "integer_overflow",
            Self::InternalError => "internal_error",
            Self::CleanupBrowserContextFailed => "cleanup_browser_context_failed",
            Self::CleanupBrowserSessionFailed => "cleanup_browser_session_failed",
            Self::CleanupTemporaryDirectoryFailed => "cleanup_temporary_directory_failed",
        }
    }

    pub const fn diagnostic_code(self) -> &'static str {
        match self {
            Self::LocatorNotVisible => "runtime.locator_not_visible",
            Self::LocatorNotFound => "runtime.locator_not_found",
            Self::LocatorAmbiguous => "runtime.locator_ambiguous",
            Self::LocatorInvalid => "runtime.locator_invalid",
            Self::ElementDetached => "runtime.element_detached",
            Self::ElementUnstable => "runtime.element_unstable",
            Self::ElementDisabled => "runtime.element_disabled",
            Self::ElementObscured => "runtime.element_obscured",
            Self::ElementNotEditable => "runtime.element_not_editable",
            Self::OptionNotFound => "runtime.option_not_found",
            Self::OptionAmbiguous => "runtime.option_ambiguous",
            Self::InvalidKey => "runtime.invalid_key",
            Self::ActionTimeout => "runtime.action_timeout",
            Self::AssertionFailed => "runtime.assertion_failed",
            Self::UrlMismatch => "runtime.url_mismatch",
            Self::NavigationFailed => "runtime.navigation_failed",
            Self::NavigationTimeout => "runtime.navigation_timeout",
            Self::BrowserCommandTimeout => "runtime.browser_command_timeout",
            Self::BrowserDisconnected => "runtime.browser_disconnected",
            Self::BrowserCrashed => "runtime.browser_crashed",
            Self::BrowserMalformedProtocol => "runtime.browser_malformed_protocol",
            Self::BrowserProtocol => "runtime.browser_protocol",
            Self::BrowserLaunch => "runtime.browser_launch",
            Self::EvaluationFailed => "runtime.evaluation_failed",
            Self::UnsupportedBrowserCapability => "runtime.unsupported_browser_capability",
            Self::ProviderNotRegistered => "runtime.provider_not_registered",
            Self::ProviderUnknownOperation => "runtime.provider_unknown_operation",
            Self::ProviderInvalidArgument => "runtime.provider_invalid_argument",
            Self::HttpTransport => "runtime.http_transport",
            Self::ResponseTooLarge => "runtime.response_too_large",
            Self::ProcessSpawn => "runtime.process_spawn",
            Self::ProcessTimeout => "runtime.process_timeout",
            Self::ProcessOutputTooLarge => "runtime.process_output_too_large",
            Self::Filesystem => "runtime.filesystem",
            Self::PathEscape => "runtime.path_escape",
            Self::ProviderUnavailable => "runtime.provider_unavailable",
            Self::AppBridgeHandshake => "runtime.app_bridge_handshake",
            Self::AppBridgeProtocol => "runtime.app_bridge_protocol",
            Self::AppBridgeTransport => "runtime.app_bridge_transport",
            Self::AppBridgeProcess => "runtime.app_bridge_process",
            Self::AppSchemaDrift => "runtime.app_schema_drift",
            Self::AppBridgeValidation => "runtime.app_bridge_validation",
            Self::AppBridgeTimeout => "runtime.app_bridge_timeout",
            Self::AppProviderFailure => "runtime.app_provider_failure",
            Self::TestTimeout => "runtime.test_timeout",
            Self::JsonDecodeFailed => "runtime.json_decode_failed",
            Self::ResponseDecodeFailed => "runtime.response_decode_failed",
            Self::DivisionByZero => "runtime.division_by_zero",
            Self::IntegerOverflow => "runtime.integer_overflow",
            Self::InternalError => "runtime.internal_error",
            Self::CleanupBrowserContextFailed => "runtime.cleanup_browser_context_failed",
            Self::CleanupBrowserSessionFailed => "runtime.cleanup_browser_session_failed",
            Self::CleanupTemporaryDirectoryFailed => "runtime.cleanup_temporary_directory_failed",
        }
    }

    pub fn from_short_code(code: &str) -> Option<Self> {
        match code {
            "locator_not_found" => Some(Self::LocatorNotFound),
            "locator_ambiguous" => Some(Self::LocatorAmbiguous),
            "locator_invalid" => Some(Self::LocatorInvalid),
            "element_detached" => Some(Self::ElementDetached),
            "element_not_visible" => Some(Self::LocatorNotVisible),
            "element_unstable" => Some(Self::ElementUnstable),
            "element_disabled" => Some(Self::ElementDisabled),
            "element_obscured" => Some(Self::ElementObscured),
            "element_not_editable" => Some(Self::ElementNotEditable),
            "option_not_found" => Some(Self::OptionNotFound),
            "option_ambiguous" => Some(Self::OptionAmbiguous),
            "invalid_key" => Some(Self::InvalidKey),
            "action_timeout" => Some(Self::ActionTimeout),
            "assertion_failed" => Some(Self::AssertionFailed),
            "url_mismatch" => Some(Self::UrlMismatch),
            "navigation_failed" => Some(Self::NavigationFailed),
            "navigation_timeout" => Some(Self::NavigationTimeout),
            "browser_command_timeout" => Some(Self::BrowserCommandTimeout),
            "browser_disconnected" => Some(Self::BrowserDisconnected),
            "browser_crashed" => Some(Self::BrowserCrashed),
            "browser_malformed_protocol" => Some(Self::BrowserMalformedProtocol),
            "browser_protocol" => Some(Self::BrowserProtocol),
            "browser_launch" => Some(Self::BrowserLaunch),
            "evaluation_failed" => Some(Self::EvaluationFailed),
            "unsupported_browser_capability" => Some(Self::UnsupportedBrowserCapability),
            "provider_not_registered" => Some(Self::ProviderNotRegistered),
            "provider_unknown_operation" => Some(Self::ProviderUnknownOperation),
            "provider_invalid_argument" => Some(Self::ProviderInvalidArgument),
            "http_transport" => Some(Self::HttpTransport),
            "response_too_large" => Some(Self::ResponseTooLarge),
            "process_spawn" => Some(Self::ProcessSpawn),
            "process_timeout" => Some(Self::ProcessTimeout),
            "process_output_too_large" => Some(Self::ProcessOutputTooLarge),
            "filesystem" => Some(Self::Filesystem),
            "path_escape" => Some(Self::PathEscape),
            "provider_unavailable" => Some(Self::ProviderUnavailable),
            "app_bridge_handshake" => Some(Self::AppBridgeHandshake),
            "app_bridge_protocol" => Some(Self::AppBridgeProtocol),
            "app_bridge_transport" => Some(Self::AppBridgeTransport),
            "app_bridge_process" => Some(Self::AppBridgeProcess),
            "app_schema_drift" => Some(Self::AppSchemaDrift),
            "app_bridge_validation" => Some(Self::AppBridgeValidation),
            "app_bridge_timeout" => Some(Self::AppBridgeTimeout),
            "app_provider_failure" => Some(Self::AppProviderFailure),
            "test_timeout" => Some(Self::TestTimeout),
            "json_decode_failed" => Some(Self::JsonDecodeFailed),
            "response_decode_failed" => Some(Self::ResponseDecodeFailed),
            "division_by_zero" => Some(Self::DivisionByZero),
            "integer_overflow" => Some(Self::IntegerOverflow),
            "internal_error" => Some(Self::InternalError),
            "cleanup_browser_context_failed" => Some(Self::CleanupBrowserContextFailed),
            "cleanup_browser_session_failed" => Some(Self::CleanupBrowserSessionFailed),
            "cleanup_temporary_directory_failed" => Some(Self::CleanupTemporaryDirectoryFailed),
            _ => None,
        }
    }

    pub const fn default_reference_queries(self) -> &'static [&'static str] {
        match self {
            Self::AppBridgeHandshake
            | Self::AppBridgeProtocol
            | Self::AppBridgeTransport
            | Self::AppBridgeProcess
            | Self::AppSchemaDrift
            | Self::AppBridgeValidation
            | Self::AppBridgeTimeout => &["app.diagnostics", "runtime.configuration"],
            Self::AssertionFailed
            | Self::JsonDecodeFailed
            | Self::ResponseDecodeFailed
            | Self::DivisionByZero
            | Self::IntegerOverflow => &["assertion.value"],
            _ => &[],
        }
    }
}

impl From<&webtest_browser::BrowserError> for RuntimeFailureCode {
    fn from(error: &webtest_browser::BrowserError) -> Self {
        match error {
            webtest_browser::BrowserError::LocatorNotFound { .. } => Self::LocatorNotFound,
            webtest_browser::BrowserError::LocatorAmbiguous { .. } => Self::LocatorAmbiguous,
            webtest_browser::BrowserError::LocatorInvalid { .. } => Self::LocatorInvalid,
            webtest_browser::BrowserError::ElementDetached { .. } => Self::ElementDetached,
            webtest_browser::BrowserError::LocatorNotVisible { .. } => Self::LocatorNotVisible,
            webtest_browser::BrowserError::ElementUnstable { .. } => Self::ElementUnstable,
            webtest_browser::BrowserError::ElementDisabled { .. } => Self::ElementDisabled,
            webtest_browser::BrowserError::ElementObscured { .. } => Self::ElementObscured,
            webtest_browser::BrowserError::ElementNotEditable { .. } => Self::ElementNotEditable,
            webtest_browser::BrowserError::OptionNotFound { .. } => Self::OptionNotFound,
            webtest_browser::BrowserError::OptionAmbiguous { .. } => Self::OptionAmbiguous,
            webtest_browser::BrowserError::InvalidKey { .. } => Self::InvalidKey,
            webtest_browser::BrowserError::ActionTimeout { .. } => Self::ActionTimeout,
            webtest_browser::BrowserError::AssertionFailed { .. } => Self::AssertionFailed,
            webtest_browser::BrowserError::UrlMismatch { .. } => Self::UrlMismatch,
            webtest_browser::BrowserError::NavigationFailed { .. } => Self::NavigationFailed,
            webtest_browser::BrowserError::NavigationTimeout { .. } => Self::NavigationTimeout,
            webtest_browser::BrowserError::CommandTimeout { .. } => Self::BrowserCommandTimeout,
            webtest_browser::BrowserError::BrowserDisconnected => Self::BrowserDisconnected,
            webtest_browser::BrowserError::BrowserCrashed { .. } => Self::BrowserCrashed,
            webtest_browser::BrowserError::MalformedProtocol { .. } => {
                Self::BrowserMalformedProtocol
            }
            webtest_browser::BrowserError::Protocol { .. } => Self::BrowserProtocol,
            webtest_browser::BrowserError::Launch(_) => Self::BrowserLaunch,
            webtest_browser::BrowserError::EvaluationFailed { .. } => Self::EvaluationFailed,
            webtest_browser::BrowserError::UnsupportedCapability { .. } => {
                Self::UnsupportedBrowserCapability
            }
        }
    }
}

impl From<&webtest_provider::ProviderError> for RuntimeFailureCode {
    fn from(error: &webtest_provider::ProviderError) -> Self {
        match error {
            webtest_provider::ProviderError::NotRegistered { .. } => Self::ProviderNotRegistered,
            webtest_provider::ProviderError::UnknownOperation { .. } => {
                Self::ProviderUnknownOperation
            }
            webtest_provider::ProviderError::InvalidArgument { .. } => {
                Self::ProviderInvalidArgument
            }
            webtest_provider::ProviderError::HttpTransport { .. } => Self::HttpTransport,
            webtest_provider::ProviderError::ResponseTooLarge { .. } => Self::ResponseTooLarge,
            webtest_provider::ProviderError::ProcessSpawn { .. } => Self::ProcessSpawn,
            webtest_provider::ProviderError::ProcessTimeout { .. } => Self::ProcessTimeout,
            webtest_provider::ProviderError::ProcessOutputTooLarge { .. } => {
                Self::ProcessOutputTooLarge
            }
            webtest_provider::ProviderError::Filesystem { .. } => Self::Filesystem,
            webtest_provider::ProviderError::PathEscape { .. } => Self::PathEscape,
            webtest_provider::ProviderError::Unavailable => Self::ProviderUnavailable,
            webtest_provider::ProviderError::BridgeHandshake { .. } => Self::AppBridgeHandshake,
            webtest_provider::ProviderError::BridgeProtocol { .. } => Self::AppBridgeProtocol,
            webtest_provider::ProviderError::BridgeTransport { .. } => Self::AppBridgeTransport,
            webtest_provider::ProviderError::BridgeProcess { .. } => Self::AppBridgeProcess,
            webtest_provider::ProviderError::BridgeSchemaDrift { .. } => Self::AppSchemaDrift,
            webtest_provider::ProviderError::BridgeValidation { .. } => Self::AppBridgeValidation,
            webtest_provider::ProviderError::BridgeTimeout { .. } => Self::AppBridgeTimeout,
            webtest_provider::ProviderError::Application { .. } => Self::AppProviderFailure,
        }
    }
}

impl serde::Serialize for RuntimeFailureCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.short_code())
    }
}

impl<'de> serde::Deserialize<'de> for RuntimeFailureCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let code = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_short_code(&code).ok_or_else(|| serde::de::Error::unknown_variant(&code, &[]))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueDiff {
    Scalar {
        expected: Option<String>,
        actual: String,
    },
    String {
        common_prefix_chars: usize,
        expected_segment: String,
        actual_segment: String,
    },
    List {
        expected_len: usize,
        actual_len: usize,
        differing_indices: Vec<usize>,
    },
    Record {
        missing_fields: Vec<String>,
        unexpected_fields: Vec<String>,
        mismatched_fields: Vec<String>,
    },
    Contains {
        expected_item: String,
        actual: String,
    },
}

static NEXT_EXECUTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExecutionId(pub u64);

impl ExecutionId {
    pub fn next() -> Self {
        Self(NEXT_EXECUTION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    Requested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    RunCancelled,
    RunAborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcomeKind {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcomeKind {
    Completed,
    Cancelled,
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CleanupResource {
    BrowserContext,
    BrowserSession,
    TemporaryDirectory { path: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupIoErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidInput,
    InvalidData,
    TimedOut,
    Interrupted,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    ReadOnlyFilesystem,
    OutOfMemory,
    Other,
}

impl From<std::io::ErrorKind> for CleanupIoErrorKind {
    fn from(kind: std::io::ErrorKind) -> Self {
        match kind {
            std::io::ErrorKind::NotFound => Self::NotFound,
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            std::io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            std::io::ErrorKind::InvalidInput => Self::InvalidInput,
            std::io::ErrorKind::InvalidData => Self::InvalidData,
            std::io::ErrorKind::TimedOut => Self::TimedOut,
            std::io::ErrorKind::Interrupted => Self::Interrupted,
            std::io::ErrorKind::NotADirectory => Self::NotADirectory,
            std::io::ErrorKind::IsADirectory => Self::IsADirectory,
            std::io::ErrorKind::DirectoryNotEmpty => Self::DirectoryNotEmpty,
            std::io::ErrorKind::ReadOnlyFilesystem => Self::ReadOnlyFilesystem,
            std::io::ErrorKind::OutOfMemory => Self::OutOfMemory,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupIoFailure {
    pub kind: CleanupIoErrorKind,
    pub raw_os_error: Option<i32>,
    pub message: String,
}

impl From<std::io::Error> for CleanupIoFailure {
    fn from(error: std::io::Error) -> Self {
        Self {
            kind: error.kind().into(),
            raw_os_error: error.raw_os_error(),
            message: bounded_cleanup_message(error.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupCause {
    Browser(webtest_browser::BrowserError),
    Io(CleanupIoFailure),
    Internal { message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupFailure {
    pub resource: CleanupResource,
    pub cause: CleanupCause,
}

impl CleanupFailure {
    pub const fn failure_class(&self) -> FailureClass {
        match self.cause {
            CleanupCause::Browser(_) | CleanupCause::Io(_) => FailureClass::Infrastructure,
            CleanupCause::Internal { .. } => FailureClass::Internal,
        }
    }

    pub const fn code(&self) -> RuntimeFailureCode {
        match self.resource {
            CleanupResource::BrowserContext => RuntimeFailureCode::CleanupBrowserContextFailed,
            CleanupResource::BrowserSession => RuntimeFailureCode::CleanupBrowserSessionFailed,
            CleanupResource::TemporaryDirectory { .. } => {
                RuntimeFailureCode::CleanupTemporaryDirectoryFailed
            }
        }
    }

    pub fn message(&self) -> String {
        let resource = match &self.resource {
            CleanupResource::BrowserContext => "browser context".into(),
            CleanupResource::BrowserSession => "browser session".into(),
            CleanupResource::TemporaryDirectory { path } => {
                format!("temporary directory `{}`", path.display())
            }
        };
        let cause = match &self.cause {
            CleanupCause::Browser(error) => error.to_string(),
            CleanupCause::Io(error) => error.message.clone(),
            CleanupCause::Internal { message } => message.clone(),
        };
        bounded_cleanup_message(format!("failed to clean up {resource}: {cause}"))
    }
}

fn bounded_cleanup_message(message: String) -> String {
    const MAX_CHARS: usize = 1_024;
    if message.chars().count() <= MAX_CHARS {
        return message;
    }
    let mut bounded = message.chars().take(MAX_CHARS - 1).collect::<String>();
    bounded.push('…');
    bounded
}

#[cfg(test)]
mod cleanup_tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn cleanup_failures_keep_typed_resources_causes_codes_and_bounds() {
        let path = PathBuf::from("owned");
        let failure = CleanupFailure {
            resource: CleanupResource::TemporaryDirectory { path: path.clone() },
            cause: CleanupCause::Io(
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x".repeat(2_000)).into(),
            ),
        };
        assert_eq!(failure.failure_class(), FailureClass::Infrastructure);
        assert_eq!(
            failure.code(),
            RuntimeFailureCode::CleanupTemporaryDirectoryFailed
        );
        assert!(failure.message().chars().count() <= 1_024);
        assert_eq!(
            serde_json::to_value(&failure.resource).expect("serialize resource"),
            serde_json::json!({"kind": "temporary_directory", "path": path})
        );
    }

    #[test]
    fn internal_cleanup_causes_remain_internal() {
        let failure = CleanupFailure {
            resource: CleanupResource::BrowserContext,
            cause: CleanupCause::Internal {
                message: "ownership invariant".into(),
            },
        };
        assert_eq!(failure.failure_class(), FailureClass::Internal);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeFailure {
    TestTimeout {
        timeout_ms: u64,
        active_step: Option<StepId>,
    },
    Browser(webtest_browser::BrowserError),
    Provider(webtest_provider::ProviderError),
    Assertion {
        message: String,
        diff: ValueDiff,
    },
    Decode {
        message: String,
    },
    Evaluation {
        code: RuntimeFailureCode,
        message: String,
    },
    Internal {
        message: String,
    },
}

impl RuntimeFailure {
    pub fn code(&self) -> RuntimeFailureCode {
        match self {
            Self::TestTimeout { .. } => RuntimeFailureCode::TestTimeout,
            Self::Browser(error) => error.into(),
            Self::Provider(error) => error.into(),
            Self::Assertion { .. } => RuntimeFailureCode::AssertionFailed,
            Self::Decode { .. } => RuntimeFailureCode::JsonDecodeFailed,
            Self::Evaluation { code, .. } => *code,
            Self::Internal { .. } => RuntimeFailureCode::InternalError,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionEvent {
    Scope {
        execution_id: ExecutionId,
        event: ScopeEvent,
    },
    RunStarted {
        execution_id: ExecutionId,
    },
    TestStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        name: String,
    },
    StepStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
    },
    StepPassed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
    },
    ProviderCallStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        provider: String,
        operation: String,
        transport_kind: Option<String>,
        arguments: BTreeMap<String, String>,
    },
    ProviderCallFinished {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        provider: String,
        operation: String,
        elapsed_ms: u64,
        transport_kind: Option<String>,
        result: Option<String>,
    },
    ProviderCallFailed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        provider: String,
        operation: String,
        code: RuntimeFailureCode,
        message: String,
        failure_class: FailureClass,
        elapsed_ms: u64,
        transport_kind: Option<String>,
    },
    StepFailed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        failure_class: FailureClass,
        failure: RuntimeFailure,
        repair_hints: Vec<RepairHint>,
        page: Option<PageSummary>,
    },
    TestTimedOut {
        execution_id: ExecutionId,
        test_id: TestId,
        active_step: Option<StepId>,
        timeout_ms: u64,
    },
    CleanupFailed {
        execution_id: ExecutionId,
        test_id: Option<TestId>,
        resource: CleanupResource,
        failure_class: FailureClass,
        code: RuntimeFailureCode,
        message: String,
    },
    TestFinished {
        execution_id: ExecutionId,
        test_id: TestId,
        outcome: TestOutcomeKind,
        failure_class: Option<FailureClass>,
    },
    TestSkipped {
        execution_id: ExecutionId,
        test_id: TestId,
        name: String,
        reason: SkipReason,
        failure_class: Option<FailureClass>,
    },
    RunFinished {
        execution_id: ExecutionId,
        outcome: RunOutcomeKind,
        failure_class: Option<FailureClass>,
    },
}

impl ExecutionEvent {
    pub fn failure_code(&self) -> Option<RuntimeFailureCode> {
        match self {
            Self::ProviderCallFailed { code, .. } | Self::CleanupFailed { code, .. } => Some(*code),
            Self::StepFailed { failure, .. } => Some(failure.code()),
            Self::TestTimedOut { .. } => Some(RuntimeFailureCode::TestTimeout),
            Self::Scope { .. }
            | Self::RunStarted { .. }
            | Self::TestStarted { .. }
            | Self::StepStarted { .. }
            | Self::StepPassed { .. }
            | Self::ProviderCallStarted { .. }
            | Self::ProviderCallFinished { .. }
            | Self::TestFinished { .. }
            | Self::TestSkipped { .. }
            | Self::RunFinished { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeObservation {
    pub execution_id: ExecutionId,
    pub file: FileId,
    pub source_revision: SourceRevision,
    pub test_id: TestId,
    pub step_id: Option<StepId>,
    pub range: TextRange,
    pub kind: RuntimeObservationKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeObservationKind {
    TestTimeout {
        timeout_ms: u64,
        active_step: Option<StepId>,
    },
    BrowserFailure {
        code: RuntimeFailureCode,
        message: String,
        locator: Option<Locator>,
        page_url: Option<String>,
        candidates: Vec<CandidateEvidence>,
        actionability: Vec<String>,
        artifacts: Vec<String>,
        elapsed_ms: u64,
        repair_hints: Vec<RepairHint>,
    },
    ValueFailure {
        code: RuntimeFailureCode,
        message: String,
        path: Option<String>,
        expected: Option<String>,
        actual: Option<String>,
        diff: Option<ValueDiff>,
    },
    LocatorNotFound {
        locator: Locator,
        page_url: Option<String>,
    },
    LocatorAmbiguous {
        locator: Locator,
        matches: usize,
        page_url: Option<String>,
    },
    LocatorNotVisible {
        locator: Locator,
        page_url: Option<String>,
    },
}

impl RuntimeObservationKind {
    pub fn code(&self) -> RuntimeFailureCode {
        match self {
            Self::TestTimeout { .. } => RuntimeFailureCode::TestTimeout,
            Self::BrowserFailure { code, .. } | Self::ValueFailure { code, .. } => *code,
            Self::LocatorNotFound { .. } => RuntimeFailureCode::LocatorNotFound,
            Self::LocatorAmbiguous { .. } => RuntimeFailureCode::LocatorAmbiguous,
            Self::LocatorNotVisible { .. } => RuntimeFailureCode::LocatorNotVisible,
        }
    }
}

#[derive(Default)]
pub struct ObservationStore {
    observations: Mutex<HashMap<(FileId, SourceRevision), Vec<RuntimeObservation>>>,
    latest_executions: Mutex<HashMap<FileId, ExecutionId>>,
}

impl ObservationStore {
    pub fn clear(&self) {
        let mut latest = self
            .latest_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latest.clear();
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    pub fn clear_for_file(&self, file: FileId) {
        let mut latest = self
            .latest_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latest.remove(&file);
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        observations.retain(|(stored_file, _), _| *stored_file != file);
    }

    /// Starting a run invalidates earlier runs immediately, including in-flight publishers.
    pub fn begin_execution(&self, file: FileId, execution: ExecutionId) {
        let mut latest = self
            .latest_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latest.insert(file, execution);
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|(stored_file, _), _| *stored_file != file);
    }

    /// Commits one complete batch only if this is still the most recently started run.
    pub fn complete_execution(
        &self,
        file: FileId,
        revision: SourceRevision,
        execution: ExecutionId,
        values: Vec<RuntimeObservation>,
    ) -> bool {
        let latest = self
            .latest_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if latest.get(&file) != Some(&execution)
            || values.iter().any(|value| {
                value.file != file
                    || value.source_revision != revision
                    || value.execution_id != execution
            })
        {
            return false;
        }
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert((file, revision), values);
        true
    }

    pub fn clear_for_execution(&self, execution_id: ExecutionId) {
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for values in observations.values_mut() {
            values.retain(|observation| observation.execution_id != execution_id);
        }
    }

    pub fn replace_for_file_revision(
        &self,
        file: FileId,
        revision: SourceRevision,
        values: Vec<RuntimeObservation>,
    ) {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert((file, revision), values);
    }

    pub fn record(&self, observation: RuntimeObservation) {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry((observation.file, observation.source_revision))
            .or_default()
            .push(observation);
    }

    pub fn observations_for(
        &self,
        file: FileId,
        revision: SourceRevision,
    ) -> Vec<RuntimeObservation> {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(file, revision))
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use webtest_text::TextSize;

    use super::*;

    #[test]
    fn completed_batches_are_atomic_and_last_started_run_wins() {
        let store = ObservationStore::default();
        let file = FileId::new(1);
        let revision = SourceRevision::of("same source");
        let old = ExecutionId::next();
        let new = ExecutionId::next();
        let observation = |execution_id| RuntimeObservation {
            execution_id,
            file,
            source_revision: revision,
            test_id: TestId(0),
            step_id: None,
            range: TextRange::default(),
            kind: RuntimeObservationKind::TestTimeout {
                timeout_ms: 10,
                active_step: None,
            },
        };
        store.begin_execution(file, old);
        assert!(store.complete_execution(file, revision, old, vec![observation(old)]));
        assert_eq!(store.observations_for(file, revision).len(), 1);
        store.begin_execution(file, new);
        assert!(store.observations_for(file, revision).is_empty());
        assert!(!store.complete_execution(file, revision, old, vec![observation(old)]));
        assert!(!store.complete_execution(file, revision, new, vec![observation(old)]));
        assert!(store.complete_execution(file, revision, new, vec![]));
        assert!(store.observations_for(file, revision).is_empty());
        store.clear_for_file(file);
        assert!(!store.complete_execution(file, revision, new, vec![observation(new)]));
    }

    #[test]
    fn observations_are_partitioned_by_source_revision() {
        let store = ObservationStore::default();
        let file = FileId::new(1);
        let revision = SourceRevision::of("a");
        store.record(RuntimeObservation {
            execution_id: ExecutionId::next(),
            file,
            source_revision: revision,
            test_id: TestId(0),
            step_id: Some(StepId(0)),
            range: TextRange::empty(TextSize::new(0)),
            kind: RuntimeObservationKind::LocatorNotFound {
                locator: Locator::Id("missing".into()),
                page_url: None,
            },
        });
        assert_eq!(store.observations_for(file, revision).len(), 1);
        assert!(
            store
                .observations_for(file, SourceRevision::of("b"))
                .is_empty()
        );
        store.clear_for_file(file);
        assert!(store.observations_for(file, revision).is_empty());
    }

    #[test]
    fn events_failures_and_observations_derive_the_same_typed_identity() {
        let failure = RuntimeFailure::Evaluation {
            code: RuntimeFailureCode::IntegerOverflow,
            message: "overflow".into(),
        };
        assert_eq!(failure.code(), RuntimeFailureCode::IntegerOverflow);

        let event = ExecutionEvent::StepFailed {
            execution_id: ExecutionId(1),
            test_id: TestId(2),
            step_id: StepId(3),
            failure_class: FailureClass::Test,
            failure,
            repair_hints: Vec::new(),
            page: None,
        };
        assert_eq!(
            event.failure_code(),
            Some(RuntimeFailureCode::IntegerOverflow)
        );

        let observation = RuntimeObservationKind::ValueFailure {
            code: RuntimeFailureCode::IntegerOverflow,
            message: "overflow".into(),
            path: None,
            expected: None,
            actual: None,
            diff: None,
        };
        assert_eq!(observation.code(), RuntimeFailureCode::IntegerOverflow);
        assert_eq!(
            RuntimeObservationKind::LocatorNotVisible {
                locator: Locator::Id("target".into()),
                page_url: None,
            }
            .code(),
            RuntimeFailureCode::LocatorNotVisible
        );
    }
}

#[cfg(test)]
mod failure_code_tests {
    use webtest_browser::{BrowserError, Locator, LocatorState};
    use webtest_feedback::FailureClass;
    use webtest_provider::ProviderError;

    use super::RuntimeFailureCode;

    #[test]
    fn every_code_has_exact_short_diagnostic_and_closed_serde_spelling() {
        use RuntimeFailureCode as C;
        let cases = [
            (
                C::LocatorNotFound,
                "locator_not_found",
                "runtime.locator_not_found",
            ),
            (
                C::LocatorAmbiguous,
                "locator_ambiguous",
                "runtime.locator_ambiguous",
            ),
            (
                C::LocatorInvalid,
                "locator_invalid",
                "runtime.locator_invalid",
            ),
            (
                C::ElementDetached,
                "element_detached",
                "runtime.element_detached",
            ),
            (
                C::LocatorNotVisible,
                "element_not_visible",
                "runtime.locator_not_visible",
            ),
            (
                C::ElementUnstable,
                "element_unstable",
                "runtime.element_unstable",
            ),
            (
                C::ElementDisabled,
                "element_disabled",
                "runtime.element_disabled",
            ),
            (
                C::ElementObscured,
                "element_obscured",
                "runtime.element_obscured",
            ),
            (
                C::ElementNotEditable,
                "element_not_editable",
                "runtime.element_not_editable",
            ),
            (
                C::OptionNotFound,
                "option_not_found",
                "runtime.option_not_found",
            ),
            (
                C::OptionAmbiguous,
                "option_ambiguous",
                "runtime.option_ambiguous",
            ),
            (C::InvalidKey, "invalid_key", "runtime.invalid_key"),
            (C::ActionTimeout, "action_timeout", "runtime.action_timeout"),
            (
                C::AssertionFailed,
                "assertion_failed",
                "runtime.assertion_failed",
            ),
            (C::UrlMismatch, "url_mismatch", "runtime.url_mismatch"),
            (
                C::NavigationFailed,
                "navigation_failed",
                "runtime.navigation_failed",
            ),
            (
                C::NavigationTimeout,
                "navigation_timeout",
                "runtime.navigation_timeout",
            ),
            (
                C::BrowserCommandTimeout,
                "browser_command_timeout",
                "runtime.browser_command_timeout",
            ),
            (
                C::BrowserDisconnected,
                "browser_disconnected",
                "runtime.browser_disconnected",
            ),
            (
                C::BrowserCrashed,
                "browser_crashed",
                "runtime.browser_crashed",
            ),
            (
                C::BrowserMalformedProtocol,
                "browser_malformed_protocol",
                "runtime.browser_malformed_protocol",
            ),
            (
                C::BrowserProtocol,
                "browser_protocol",
                "runtime.browser_protocol",
            ),
            (C::BrowserLaunch, "browser_launch", "runtime.browser_launch"),
            (
                C::EvaluationFailed,
                "evaluation_failed",
                "runtime.evaluation_failed",
            ),
            (
                C::UnsupportedBrowserCapability,
                "unsupported_browser_capability",
                "runtime.unsupported_browser_capability",
            ),
            (
                C::ProviderNotRegistered,
                "provider_not_registered",
                "runtime.provider_not_registered",
            ),
            (
                C::ProviderUnknownOperation,
                "provider_unknown_operation",
                "runtime.provider_unknown_operation",
            ),
            (
                C::ProviderInvalidArgument,
                "provider_invalid_argument",
                "runtime.provider_invalid_argument",
            ),
            (C::HttpTransport, "http_transport", "runtime.http_transport"),
            (
                C::ResponseTooLarge,
                "response_too_large",
                "runtime.response_too_large",
            ),
            (C::ProcessSpawn, "process_spawn", "runtime.process_spawn"),
            (
                C::ProcessTimeout,
                "process_timeout",
                "runtime.process_timeout",
            ),
            (
                C::ProcessOutputTooLarge,
                "process_output_too_large",
                "runtime.process_output_too_large",
            ),
            (C::Filesystem, "filesystem", "runtime.filesystem"),
            (C::PathEscape, "path_escape", "runtime.path_escape"),
            (
                C::ProviderUnavailable,
                "provider_unavailable",
                "runtime.provider_unavailable",
            ),
            (
                C::AppBridgeHandshake,
                "app_bridge_handshake",
                "runtime.app_bridge_handshake",
            ),
            (
                C::AppBridgeProtocol,
                "app_bridge_protocol",
                "runtime.app_bridge_protocol",
            ),
            (
                C::AppBridgeTransport,
                "app_bridge_transport",
                "runtime.app_bridge_transport",
            ),
            (
                C::AppBridgeProcess,
                "app_bridge_process",
                "runtime.app_bridge_process",
            ),
            (
                C::AppSchemaDrift,
                "app_schema_drift",
                "runtime.app_schema_drift",
            ),
            (
                C::AppBridgeValidation,
                "app_bridge_validation",
                "runtime.app_bridge_validation",
            ),
            (
                C::AppBridgeTimeout,
                "app_bridge_timeout",
                "runtime.app_bridge_timeout",
            ),
            (
                C::AppProviderFailure,
                "app_provider_failure",
                "runtime.app_provider_failure",
            ),
            (C::TestTimeout, "test_timeout", "runtime.test_timeout"),
            (
                C::JsonDecodeFailed,
                "json_decode_failed",
                "runtime.json_decode_failed",
            ),
            (
                C::ResponseDecodeFailed,
                "response_decode_failed",
                "runtime.response_decode_failed",
            ),
            (
                C::DivisionByZero,
                "division_by_zero",
                "runtime.division_by_zero",
            ),
            (
                C::IntegerOverflow,
                "integer_overflow",
                "runtime.integer_overflow",
            ),
            (C::InternalError, "internal_error", "runtime.internal_error"),
            (
                C::CleanupBrowserContextFailed,
                "cleanup_browser_context_failed",
                "runtime.cleanup_browser_context_failed",
            ),
            (
                C::CleanupBrowserSessionFailed,
                "cleanup_browser_session_failed",
                "runtime.cleanup_browser_session_failed",
            ),
            (
                C::CleanupTemporaryDirectoryFailed,
                "cleanup_temporary_directory_failed",
                "runtime.cleanup_temporary_directory_failed",
            ),
        ];

        for (code, short, diagnostic) in cases {
            assert_eq!(code.short_code(), short);
            assert_eq!(code.diagnostic_code(), diagnostic);
            assert_eq!(RuntimeFailureCode::from_short_code(short), Some(code));
            assert_eq!(
                serde_json::to_value(code).unwrap(),
                serde_json::json!(short)
            );
            assert_eq!(
                serde_json::from_value::<RuntimeFailureCode>(serde_json::json!(short)).unwrap(),
                code
            );
        }
        assert!(
            serde_json::from_value::<RuntimeFailureCode>(serde_json::json!("external_code"))
                .is_err()
        );
    }

    #[test]
    fn every_browser_error_maps_exhaustively_without_classification_drift() {
        use RuntimeFailureCode as C;
        let locator = || Locator::Id("target".into());
        let cases = vec![
            (
                BrowserError::LocatorNotFound { locator: locator() },
                C::LocatorNotFound,
                FailureClass::Test,
            ),
            (
                BrowserError::LocatorAmbiguous {
                    locator: locator(),
                    matches: 2,
                },
                C::LocatorAmbiguous,
                FailureClass::Test,
            ),
            (
                BrowserError::LocatorInvalid {
                    locator: locator(),
                    message: "invalid".into(),
                },
                C::LocatorInvalid,
                FailureClass::Test,
            ),
            (
                BrowserError::ElementDetached { locator: locator() },
                C::ElementDetached,
                FailureClass::Test,
            ),
            (
                BrowserError::LocatorNotVisible { locator: locator() },
                C::LocatorNotVisible,
                FailureClass::Test,
            ),
            (
                BrowserError::ElementUnstable { locator: locator() },
                C::ElementUnstable,
                FailureClass::Test,
            ),
            (
                BrowserError::ElementDisabled { locator: locator() },
                C::ElementDisabled,
                FailureClass::Test,
            ),
            (
                BrowserError::ElementObscured { locator: locator() },
                C::ElementObscured,
                FailureClass::Test,
            ),
            (
                BrowserError::ElementNotEditable { locator: locator() },
                C::ElementNotEditable,
                FailureClass::Test,
            ),
            (
                BrowserError::OptionNotFound {
                    locator: locator(),
                    option: "x".into(),
                },
                C::OptionNotFound,
                FailureClass::Test,
            ),
            (
                BrowserError::OptionAmbiguous {
                    locator: locator(),
                    option: "x".into(),
                    matches: 2,
                },
                C::OptionAmbiguous,
                FailureClass::Test,
            ),
            (
                BrowserError::InvalidKey { key: "Bad".into() },
                C::InvalidKey,
                FailureClass::Test,
            ),
            (
                BrowserError::ActionTimeout {
                    locator: locator(),
                    timeout_ms: 10,
                },
                C::ActionTimeout,
                FailureClass::Test,
            ),
            (
                BrowserError::AssertionFailed {
                    locator: locator(),
                    expected: LocatorState::Visible,
                    actual: "hidden".into(),
                },
                C::AssertionFailed,
                FailureClass::Test,
            ),
            (
                BrowserError::UrlMismatch {
                    expected: "a".into(),
                    actual: "b".into(),
                },
                C::UrlMismatch,
                FailureClass::Test,
            ),
            (
                BrowserError::NavigationFailed {
                    url: "a".into(),
                    reason: "closed".into(),
                },
                C::NavigationFailed,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::NavigationTimeout {
                    url: "a".into(),
                    timeout_ms: 10,
                },
                C::NavigationTimeout,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::CommandTimeout {
                    method: "Page.enable".into(),
                    timeout_ms: 10,
                },
                C::BrowserCommandTimeout,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::BrowserDisconnected,
                C::BrowserDisconnected,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::BrowserCrashed { status: "1".into() },
                C::BrowserCrashed,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::MalformedProtocol {
                    message: "bad".into(),
                },
                C::BrowserMalformedProtocol,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::Protocol {
                    method: "Page.enable".into(),
                    message: "bad".into(),
                },
                C::BrowserProtocol,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::Launch("missing".into()),
                C::BrowserLaunch,
                FailureClass::Infrastructure,
            ),
            (
                BrowserError::EvaluationFailed {
                    expression: "x".into(),
                    message: "bad".into(),
                },
                C::EvaluationFailed,
                FailureClass::Test,
            ),
            (
                BrowserError::UnsupportedCapability {
                    capability: "browser".into(),
                },
                C::UnsupportedBrowserCapability,
                FailureClass::Infrastructure,
            ),
        ];
        for (error, code, class) in cases {
            assert_eq!(RuntimeFailureCode::from(&error), code);
            assert_eq!(code.short_code(), error.code());
            assert_eq!(
                if error.is_infrastructure() {
                    FailureClass::Infrastructure
                } else {
                    FailureClass::Test
                },
                class
            );
        }
    }

    #[test]
    fn every_provider_error_maps_exhaustively_and_dynamic_subcodes_stay_details() {
        use RuntimeFailureCode as C;
        let cases = vec![
            (
                ProviderError::NotRegistered {
                    provider: "x".into(),
                },
                C::ProviderNotRegistered,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::UnknownOperation {
                    provider: "x".into(),
                    operation: "y".into(),
                },
                C::ProviderUnknownOperation,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::InvalidArgument {
                    message: "bad".into(),
                },
                C::ProviderInvalidArgument,
                FailureClass::Test,
            ),
            (
                ProviderError::HttpTransport {
                    message: "bad".into(),
                },
                C::HttpTransport,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::ResponseTooLarge { limit: 1 },
                C::ResponseTooLarge,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::ProcessSpawn {
                    message: "bad".into(),
                },
                C::ProcessSpawn,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::ProcessTimeout {
                    timeout_ms: 1,
                    cleanup_succeeded: true,
                },
                C::ProcessTimeout,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::ProcessOutputTooLarge { limit: 1 },
                C::ProcessOutputTooLarge,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::Filesystem {
                    path: "x".into(),
                    message: "bad".into(),
                },
                C::Filesystem,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::PathEscape { path: "x".into() },
                C::PathEscape,
                FailureClass::Test,
            ),
            (
                ProviderError::Unavailable,
                C::ProviderUnavailable,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::BridgeHandshake {
                    code: "authentication_failed".into(),
                    message: "bad".into(),
                },
                C::AppBridgeHandshake,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::BridgeProtocol {
                    code: "unknown_response_id".into(),
                    message: "bad".into(),
                },
                C::AppBridgeProtocol,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::BridgeTransport {
                    message: "bad".into(),
                },
                C::AppBridgeTransport,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::BridgeProcess {
                    message: "bad".into(),
                },
                C::AppBridgeProcess,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::BridgeSchemaDrift {
                    expected: "a".into(),
                    live: "b".into(),
                },
                C::AppSchemaDrift,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::BridgeValidation {
                    path: "$.id".into(),
                    message: "bad".into(),
                },
                C::AppBridgeValidation,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::BridgeTimeout { timeout_ms: 1 },
                C::AppBridgeTimeout,
                FailureClass::Infrastructure,
            ),
            (
                ProviderError::Application {
                    code: "user_defined".into(),
                    message: "bad".into(),
                    retryable: false,
                    data: serde_json::json!({"secret": "redacted upstream"}),
                },
                C::AppProviderFailure,
                FailureClass::Test,
            ),
        ];
        for (error, code, class) in cases {
            assert_eq!(RuntimeFailureCode::from(&error), code);
            assert_eq!(code.short_code(), error.code());
            assert_eq!(
                if error.is_infrastructure() {
                    FailureClass::Infrastructure
                } else {
                    FailureClass::Test
                },
                class
            );
        }
        let alternate = ProviderError::Application {
            code: "another_dynamic_code".into(),
            message: "bad".into(),
            retryable: true,
            data: serde_json::Value::Null,
        };
        assert_eq!(RuntimeFailureCode::from(&alternate), C::AppProviderFailure);
        assert!(RuntimeFailureCode::from_short_code("another_dynamic_code").is_none());
    }
}
