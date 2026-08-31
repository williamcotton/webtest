//! Protocol-independent editor services used by both LSP and future WASM hosts.

use std::sync::{Arc, Mutex, RwLock};

use thiserror::Error;
use webtest_analysis::{
    AnalysisDatabase, AnalysisError, Completion, Diagnostic, DiagnosticSeverity, DiagnosticSource,
    Signature,
};
use webtest_browser::{BrowserError, BrowserHost, Locator};
use webtest_observation::{ObservationStore, RuntimeObservationKind};
use webtest_provider::ProviderRegistry;
use webtest_runtime::{RunResult, Runner, RunnerOptions};
use webtest_syntax::SyntaxKind;
use webtest_text::{FileId, TextRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticTokenKind {
    Keyword,
    String,
    Comment,
    Function,
    Variable,
    Property,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub range: TextRange,
    pub kind: SemanticTokenKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    pub range: TextRange,
    pub contents: String,
}

#[derive(Debug, Error)]
pub enum EditorError {
    #[error(transparent)]
    Analysis(#[from] AnalysisError),
    #[error(transparent)]
    Browser(#[from] BrowserError),
    #[error("the file has static errors and cannot be executed")]
    StaticErrors,
}

pub struct EditorService {
    database: RwLock<AnalysisDatabase>,
    observations: Arc<ObservationStore>,
    runtime: RwLock<RuntimeConfiguration>,
    configuration: Mutex<()>,
}

struct RuntimeConfiguration {
    runner_options: RunnerOptions,
    providers: ProviderRegistry,
}

impl Default for EditorService {
    fn default() -> Self {
        Self::with_runner_options(RunnerOptions::default())
    }
}

impl EditorService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_runner_options(runner_options: RunnerOptions) -> Self {
        let providers = ProviderRegistry::built_in(runner_options.provider_config.clone());
        Self::with_provider_registry(runner_options, providers)
    }

    pub fn with_provider_registry(
        runner_options: RunnerOptions,
        providers: ProviderRegistry,
    ) -> Self {
        Self {
            database: RwLock::new(AnalysisDatabase::with_provider_registry(providers.clone())),
            observations: Arc::new(ObservationStore::default()),
            runtime: RwLock::new(RuntimeConfiguration {
                runner_options,
                providers,
            }),
            configuration: Mutex::new(()),
        }
    }

    pub fn reconfigure(&self, runner_options: RunnerOptions, providers: ProviderRegistry) {
        let _configuration = self.lock_configuration();
        self.write_database()
            .set_provider_registry(providers.clone());
        *self.write_runtime() = RuntimeConfiguration {
            runner_options,
            providers,
        };
        self.observations.clear();
    }

    pub fn observations(&self) -> Arc<ObservationStore> {
        Arc::clone(&self.observations)
    }

    pub fn open_document(&self, path: impl Into<String>, text: impl Into<String>) -> FileId {
        self.write_database().open_file(path, text)
    }

    pub fn update_document(&self, file: FileId, text: impl Into<String>) {
        self.write_database().set_file_text(file, text);
    }

    pub fn close_document(&self, file: FileId) {
        self.write_database().close_file(file);
        self.observations.clear_for_file(file);
    }

    pub fn file_for_path(&self, path: &str) -> Option<FileId> {
        self.read_database().file_for_path(path)
    }

    pub fn source(&self, file: FileId) -> Result<Arc<str>, EditorError> {
        Ok(self.read_database().source(file)?)
    }

    pub fn diagnostics(&self, file: FileId) -> Result<Vec<Diagnostic>, EditorError> {
        let (mut diagnostics, revision) = {
            let mut database = self.write_database();
            (
                database.diagnostics(file)?.as_ref().clone(),
                database.source_revision(file)?,
            )
        };
        diagnostics.extend(
            self.observations
                .observations_for(file, revision)
                .into_iter()
                .map(|observation| match observation.kind {
                    RuntimeObservationKind::BrowserFailure {
                        code,
                        message,
                        locator,
                        page_url,
                        candidates,
                        actionability,
                        artifacts,
                        elapsed_ms,
                        repair_hints,
                        ..
                    } => Diagnostic {
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: runtime_diagnostic_code(&code),
                        message: friendly_runtime_message(&code, locator.as_ref(), &message),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.as_ref().map(ToString::to_string),
                            "page_url": page_url,
                            "candidates": candidates,
                            "actionability": actionability,
                            "artifacts": artifacts,
                            "elapsed_ms": elapsed_ms,
                        })),
                        repair_hints,
                        reference_queries: locator
                            .as_ref()
                            .map(locator_reference_query)
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    },
                    RuntimeObservationKind::ValueFailure {
                        code,
                        message,
                        path,
                        expected,
                        actual,
                        diff: _,
                    } => {
                        let mut details = message;
                        if let Some(path) = &path {
                            details.push_str(&format!(" Path: {path}."));
                        }
                        if let (Some(expected), Some(actual)) = (&expected, &actual) {
                            details.push_str(&format!(" Expected {expected}; got {actual}."));
                        }
                        Diagnostic {
                            range: observation.range,
                            severity: DiagnosticSeverity::Error,
                            code: value_runtime_diagnostic_code(&code),
                            message: details,
                            source: DiagnosticSource::Runtime,
                            semantic_details: Some(serde_json::json!({
                                "path": path,
                                "expected": expected,
                                "actual": actual,
                            })),
                            repair_hints: Vec::new(),
                            reference_queries: vec!["assertion.value".into()],
                        }
                    }
                    RuntimeObservationKind::LocatorNotFound { locator, .. } => Diagnostic {
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: "runtime.locator_not_found",
                        message: format!(
                            "No element with {} was found during the last test run.",
                            locator_description(&locator)
                        ),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.to_string(),
                        })),
                        repair_hints: Vec::new(),
                        reference_queries: vec![locator_reference_query(&locator).into()],
                    },
                    RuntimeObservationKind::LocatorAmbiguous {
                        locator, matches, ..
                    } => Diagnostic {
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: "runtime.locator_ambiguous",
                        message: format!(
                            "The locator {locator} matched {matches} elements during the last test run."
                        ),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.to_string(),
                            "matches": matches,
                        })),
                        repair_hints: Vec::new(),
                        reference_queries: vec![locator_reference_query(&locator).into()],
                    },
                    RuntimeObservationKind::LocatorNotVisible { locator, .. } => Diagnostic {
                        range: observation.range,
                        severity: DiagnosticSeverity::Error,
                        code: "runtime.locator_not_visible",
                        message: format!(
                            "The element with {} was not visible during the last test run.",
                            locator_description(&locator)
                        ),
                        source: DiagnosticSource::Runtime,
                        semantic_details: Some(serde_json::json!({
                            "requested_locator": locator.to_string(),
                            "visible": false,
                        })),
                        repair_hints: Vec::new(),
                        reference_queries: vec![locator_reference_query(&locator).into()],
                    },
                }),
        );
        Ok(diagnostics)
    }

    pub fn format(&self, file: FileId) -> Result<String, EditorError> {
        let parse = self.write_database().parse(file)?;
        Ok(webtest_format::format_file(&parse))
    }

    pub fn hover(
        &self,
        file: FileId,
        offset: webtest_text::TextSize,
    ) -> Result<Option<Hover>, EditorError> {
        let mut database = self.write_database();
        if let Some(documentation) = database.documentation_at(file, offset)? {
            return Ok(Some(Hover {
                range: documentation.range,
                contents: documentation.contents,
            }));
        }
        Ok(database.type_at(file, offset)?.map(|fact| Hover {
            range: fact.range,
            contents: format!("{} ({})", fact.ty, fact.capability),
        }))
    }

    pub fn completions(
        &self,
        file: FileId,
        offset: webtest_text::TextSize,
    ) -> Result<Vec<Completion>, EditorError> {
        Ok(self.write_database().completions(file, offset)?)
    }

    pub fn signature_help(
        &self,
        file: FileId,
        offset: webtest_text::TextSize,
    ) -> Result<Option<Signature>, EditorError> {
        Ok(self.write_database().signature_help(file, offset)?)
    }

    pub fn semantic_tokens(&self, file: FileId) -> Result<Vec<SemanticToken>, EditorError> {
        let parse = self.write_database().parse(file)?;
        Ok(parse
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter_map(|token| {
                let parent = token.parent().map(|node| node.kind());
                let kind = match token.kind() {
                    SyntaxKind::TestKw
                    | SyntaxKind::BrowserKw
                    | SyntaxKind::ServerKw
                    | SyntaxKind::LetKw
                    | SyntaxKind::TrueKw
                    | SyntaxKind::FalseKw
                    | SyntaxKind::NullKw
                    | SyntaxKind::ContainsKw
                    | SyntaxKind::MatchesKw
                    | SyntaxKind::OpenKw
                    | SyntaxKind::ClickKw
                    | SyntaxKind::FillKw
                    | SyntaxKind::TypeKw
                    | SyntaxKind::PressKw
                    | SyntaxKind::KeyKw
                    | SyntaxKind::WithKw
                    | SyntaxKind::CheckKw
                    | SyntaxKind::UncheckKw
                    | SyntaxKind::SelectKw
                    | SyntaxKind::OptionKw
                    | SyntaxKind::HoverKw
                    | SyntaxKind::WaitKw
                    | SyntaxKind::ExpectKw
                    | SyntaxKind::WithinKw
                    | SyntaxKind::VisibleKw
                    | SyntaxKind::HiddenKw
                    | SyntaxKind::AttachedKw
                    | SyntaxKind::DetachedKw
                    | SyntaxKind::EnabledKw
                    | SyntaxKind::DisabledKw
                    | SyntaxKind::CheckedKw
                    | SyntaxKind::UncheckedKw => SemanticTokenKind::Keyword,
                    SyntaxKind::IdKw
                    | SyntaxKind::RoleKw
                    | SyntaxKind::LabelKw
                    | SyntaxKind::TextKw
                    | SyntaxKind::PlaceholderKw
                    | SyntaxKind::TestIdKw
                    | SyntaxKind::CssKw
                    | SyntaxKind::XPathKw
                    | SyntaxKind::UrlKw => SemanticTokenKind::Function,
                    SyntaxKind::String => SemanticTokenKind::String,
                    SyntaxKind::LineComment => SemanticTokenKind::Comment,
                    SyntaxKind::Ident
                        if matches!(parent, Some(SyntaxKind::LetStmt | SyntaxKind::NameExpr)) =>
                    {
                        SemanticTokenKind::Variable
                    }
                    SyntaxKind::Ident if parent == Some(SyntaxKind::MemberExpr) => {
                        SemanticTokenKind::Property
                    }
                    SyntaxKind::Ident
                        if matches!(
                            parent,
                            Some(SyntaxKind::NamedType | SyntaxKind::GenericType)
                        ) =>
                    {
                        SemanticTokenKind::Type
                    }
                    _ => return None,
                };
                Some(SemanticToken {
                    range: token.text_range(),
                    kind,
                })
            })
            .collect())
    }

    pub async fn run_file(
        &self,
        file: FileId,
        browser: &dyn BrowserHost,
    ) -> Result<RunResult, EditorError> {
        let (plan, runner_options, providers) = {
            let _configuration = self.lock_configuration();
            let mut database = self.write_database();
            if database
                .diagnostics(file)?
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            {
                return Err(EditorError::StaticErrors);
            }
            let plan = database.test_plan(file)?;
            drop(database);
            let runtime = self.read_runtime();
            (
                plan,
                runtime.runner_options.clone(),
                runtime.providers.clone(),
            )
        };
        let runner = Runner::new(Arc::clone(&self.observations))
            .with_options(runner_options)
            .with_provider_registry(providers);
        Ok(runner.run(&plan, browser).await)
    }

    fn read_database(&self) -> std::sync::RwLockReadGuard<'_, AnalysisDatabase> {
        self.database
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_database(&self) -> std::sync::RwLockWriteGuard<'_, AnalysisDatabase> {
        self.database
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_runtime(&self) -> std::sync::RwLockReadGuard<'_, RuntimeConfiguration> {
        self.runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_runtime(&self) -> std::sync::RwLockWriteGuard<'_, RuntimeConfiguration> {
        self.runtime
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_configuration(&self) -> std::sync::MutexGuard<'_, ()> {
        self.configuration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn locator_description(locator: &Locator) -> String {
    match locator {
        Locator::Id(value) => format!("id {value:?}"),
        Locator::Text(value) => format!("text {value:?}"),
        _ => locator.to_string(),
    }
}

fn locator_reference_query(locator: &Locator) -> &'static str {
    match locator {
        Locator::Id(_) => "locator.id",
        Locator::Role { .. } => "locator.role",
        Locator::Label(_) => "locator.label",
        Locator::Text(_) => "locator.text",
        Locator::Placeholder(_) => "locator.placeholder",
        Locator::TestId(_) => "locator.test_id",
        Locator::Css(_) => "locator.css",
        Locator::XPath(_) => "locator.xpath",
    }
}

fn friendly_runtime_message(code: &str, locator: Option<&Locator>, fallback: &str) -> String {
    match (code, locator) {
        ("locator_not_found", Some(locator)) => format!(
            "No element with {} was found during the last test run.",
            locator_description(locator)
        ),
        ("element_not_visible", Some(locator)) => format!(
            "The element with {} was not visible during the last test run.",
            locator_description(locator)
        ),
        _ => fallback.into(),
    }
}

fn runtime_diagnostic_code(code: &str) -> &'static str {
    match code {
        "locator_not_found" => "runtime.locator_not_found",
        "locator_ambiguous" => "runtime.locator_ambiguous",
        "locator_invalid" => "runtime.locator_invalid",
        "element_detached" => "runtime.element_detached",
        "element_not_visible" => "runtime.locator_not_visible",
        "element_unstable" => "runtime.element_unstable",
        "element_disabled" => "runtime.element_disabled",
        "element_obscured" => "runtime.element_obscured",
        "element_not_editable" => "runtime.element_not_editable",
        "option_not_found" => "runtime.option_not_found",
        "option_ambiguous" => "runtime.option_ambiguous",
        "invalid_key" => "runtime.invalid_key",
        "action_timeout" => "runtime.action_timeout",
        "url_mismatch" => "runtime.url_mismatch",
        _ => "runtime.assertion_failed",
    }
}

fn value_runtime_diagnostic_code(code: &str) -> &'static str {
    match code {
        "json_decode_failed" => "runtime.json_decode_failed",
        "assertion_failed" => "runtime.assertion_failed",
        "provider_invalid_argument" => "runtime.provider_invalid_argument",
        "path_escape" => "runtime.path_escape",
        "division_by_zero" => "runtime.division_by_zero",
        "response_decode_failed" => "runtime.response_decode_failed",
        "internal_error" => "runtime.internal_error",
        _ => "runtime.provider_failure",
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use webtest_browser::{BrowserSession, Page};
    use webtest_browser_cdp::ChromeHost;
    use webtest_provider::{
        Capability, OperationName, OperationSchema, ProviderName, ProviderSchema, Type,
    };
    use webtest_runtime::RunOutcome;

    use super::*;

    struct FakeHost(bool);
    struct FakeSession(bool);
    struct FakePage(bool);
    struct DisconnectHost;

    fn app_registry(functions: &[&str]) -> ProviderRegistry {
        let mut providers = ProviderRegistry::built_in_schemas();
        providers.register_schema(ProviderSchema {
            name: ProviderName("app".into()),
            operations: functions
                .iter()
                .map(|name| {
                    (
                        (*name).into(),
                        OperationSchema {
                            name: OperationName((*name).into()),
                            parameters: Vec::new(),
                            result: Type::String,
                            capability: Capability::Server,
                            documentation: format!("Call {name}."),
                            retry_safe: false,
                        },
                    )
                })
                .collect(),
            schema_identity: Some(functions.join(",")),
        });
        providers
    }

    #[test]
    fn provider_reconfiguration_invalidates_cached_analysis() {
        let editor = EditorService::with_provider_registry(
            RunnerOptions::default(),
            app_registry(&["existing"]),
        );
        let source = "test \"schema\" { server { let value = app.new_function() } }";
        let file = editor.open_document("file:///schema.webtest", source);
        assert!(
            editor
                .diagnostics(file)
                .expect("initial diagnostics")
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.unknown_provider_operation")
        );

        editor.reconfigure(
            RunnerOptions::default(),
            app_registry(&["existing", "new_function"]),
        );
        assert!(
            editor
                .diagnostics(file)
                .expect("reconfigured diagnostics")
                .is_empty()
        );
    }

    #[async_trait]
    impl BrowserHost for FakeHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Ok(Box::new(FakeSession(self.0)))
        }
    }

    #[async_trait]
    impl BrowserHost for DisconnectHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Err(BrowserError::BrowserDisconnected)
        }
    }

    #[async_trait]
    impl BrowserSession for FakeSession {
        async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError> {
            Ok(Box::new(FakePage(self.0)))
        }
    }

    #[async_trait]
    impl Page for FakePage {
        async fn open(&mut self, _url: &str) -> Result<(), BrowserError> {
            Ok(())
        }

        async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError> {
            if self.0 {
                Ok(())
            } else {
                Err(BrowserError::LocatorNotFound {
                    locator: locator.clone(),
                })
            }
        }

        async fn expect_visible(&mut self, locator: &Locator) -> Result<(), BrowserError> {
            if self.0 {
                Ok(())
            } else {
                Err(BrowserError::LocatorNotFound {
                    locator: locator.clone(),
                })
            }
        }

        async fn evaluate(&mut self, expression: &str) -> Result<(), BrowserError> {
            if self.0 {
                Ok(())
            } else {
                Err(BrowserError::EvaluationFailed {
                    expression: expression.into(),
                    message: "".into(),
                })
            }
        }
    }

    #[tokio::test]
    async fn runtime_diagnostics_are_exact_revision_safe_and_clear_on_success() {
        let editor = EditorService::new();
        let failing_source =
            "test \"x\" { browser { open \"about:blank\" click id(\"missing\") } }";
        let file = editor.open_document("file:///test.webtest", failing_source);
        editor
            .run_file(file, &FakeHost(false))
            .await
            .expect("failed test is a valid run");
        let diagnostics = editor.diagnostics(file).expect("diagnostics");
        let runtime = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
            .expect("runtime diagnostic");
        let start = u32::from(runtime.range.start()) as usize;
        let end = u32::from(runtime.range.end()) as usize;
        assert_eq!(&failing_source[start..end], "id(\"missing\")");

        let passing_source = failing_source.replace("missing", "submit");
        editor.update_document(file, &passing_source);
        assert!(
            editor
                .diagnostics(file)
                .expect("new diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );

        editor
            .run_file(file, &FakeHost(true))
            .await
            .expect("passing run");
        assert!(
            editor
                .diagnostics(file)
                .expect("passing diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );
    }

    #[tokio::test]
    async fn infrastructure_rerun_clears_previous_runtime_diagnostic() {
        let editor = EditorService::new();
        let source = "test \"x\" { browser { click id(\"missing\") } }";
        let file = editor.open_document("file:///disconnect.webtest", source);
        editor
            .run_file(file, &FakeHost(false))
            .await
            .expect("failed assertion run");
        assert!(
            editor
                .diagnostics(file)
                .expect("failure diagnostics")
                .iter()
                .any(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
        );

        let result = editor
            .run_file(file, &DisconnectHost)
            .await
            .expect("forced disconnect is an aborted runtime result");
        assert!(matches!(
            result.outcome,
            RunOutcome::Aborted {
                failure: webtest_runtime::RunError::Browser(BrowserError::BrowserDisconnected)
            }
        ));
        assert!(
            editor
                .diagnostics(file)
                .expect("disconnect diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );

        editor
            .run_file(file, &FakeHost(true))
            .await
            .expect("successful rerun");
        assert!(
            editor
                .diagnostics(file)
                .expect("successful diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );
    }

    #[tokio::test]
    async fn missing_visible_text_produces_a_precise_runtime_diagnostic() {
        let editor = EditorService::new();
        let source = "test \"x\" { browser { expect text(\"submitted\").visible } }";
        let file = editor.open_document("file:///expect.webtest", source);
        editor
            .run_file(file, &FakeHost(false))
            .await
            .expect("failed expectation is a valid run");

        let runtime = editor
            .diagnostics(file)
            .expect("diagnostics")
            .into_iter()
            .find(|diagnostic| diagnostic.source == DiagnosticSource::Runtime)
            .expect("runtime diagnostic");
        let start = u32::from(runtime.range.start()) as usize;
        let end = u32::from(runtime.range.end()) as usize;
        assert_eq!(&source[start..end], "text(\"submitted\")");
        assert_eq!(runtime.code, "runtime.locator_not_found");
        assert_eq!(
            runtime.message,
            "No element with text \"submitted\" was found during the last test run."
        );
    }

    #[test]
    fn semantic_tokens_are_views_over_cst_tokens() {
        let editor = EditorService::new();
        let source = "test \"x\" { // note\n browser { open \"about:blank\" click id(\"submit\") expect text(\"submitted\").visible } }";
        let file = editor.open_document("file:///tokens.webtest", source);
        let tokens = editor.semantic_tokens(file).expect("semantic tokens");
        let rendered: Vec<_> = tokens
            .iter()
            .map(|token| {
                let start = u32::from(token.range.start()) as usize;
                let end = u32::from(token.range.end()) as usize;
                (&source[start..end], token.kind)
            })
            .collect();
        assert!(rendered.contains(&("test", SemanticTokenKind::Keyword)));
        assert!(rendered.contains(&("// note", SemanticTokenKind::Comment)));
        assert!(rendered.contains(&("id", SemanticTokenKind::Function)));
        assert!(rendered.contains(&("expect", SemanticTokenKind::Keyword)));
        assert!(rendered.contains(&("text", SemanticTokenKind::Function)));
        assert!(rendered.contains(&("visible", SemanticTokenKind::Keyword)));
        assert!(rendered.contains(&("\"submit\"", SemanticTokenKind::String)));
    }

    #[test]
    fn hover_uses_shared_static_type_facts() {
        let editor = EditorService::new();
        let source = "test \"x\" { server { let response = http.get(\"/user\") expect response.status == 200 } }";
        let file = editor.open_document("file:///hover.webtest", source);
        let offset = source.find("status").expect("status") + 1;
        let hover = editor
            .hover(file, webtest_text::TextSize::from(offset as u32))
            .expect("hover")
            .expect("type fact");
        assert!(hover.contents.contains("StatusCode"), "{}", hover.contents);
    }

    #[tokio::test]
    async fn typed_json_decode_failure_is_published_on_the_constrained_expression() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let (mut stream, _) = listener.accept().await.expect("fixture request");
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await;
            let body = r#"{"id":"wrong","email":"alice@example.test"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("fixture response");
        });

        let source = format!(
            r#"test "decode" {{
    server {{
        let response = http.get("http://{address}/user")
        let user: {{ id: Int, email: String }} = response.json
    }}
}}
"#,
        );
        let editor = EditorService::new();
        let file = editor.open_document("file:///decode.webtest", &source);
        let run = editor
            .run_file(file, &FakeHost(true))
            .await
            .expect("decode failure is a completed test run");
        assert_eq!(run.failed(), 1);
        let diagnostic = editor
            .diagnostics(file)
            .expect("diagnostics")
            .into_iter()
            .find(|diagnostic| diagnostic.code == "runtime.json_decode_failed")
            .expect("runtime decode diagnostic");
        let start = u32::from(diagnostic.range.start()) as usize;
        let end = u32::from(diagnostic.range.end()) as usize;
        assert_eq!(&source[start..end], "response.json");
        assert!(
            diagnostic.message.contains("$.id"),
            "{}",
            diagnostic.message
        );
        assert!(diagnostic.message.contains("Int"), "{}", diagnostic.message);
    }

    #[tokio::test]
    async fn real_browser_vertical_slice_when_chrome_is_available() {
        let browser = ChromeHost::default();
        if browser.locate().is_none() {
            return;
        }
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};

                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).await;
                let body = "<!doctype html><html><body><button id=\"submit\" onclick=\"const result=document.createElement('div');result.textContent='submitted';document.body.append(result)\">Submit</button></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let editor = EditorService::new();
        let failing = format!(
            "test \"missing\" {{ browser {{ open \"http://{address}\" click id(\"missing\") expect text(\"submitted\").visible }} }}"
        );
        let file = editor.open_document("file:///vertical.webtest", &failing);
        let failed = editor
            .run_file(file, &browser)
            .await
            .expect("run failing test");
        assert_eq!(failed.failed(), 1);
        assert!(
            editor
                .diagnostics(file)
                .expect("runtime diagnostics")
                .iter()
                .any(|diagnostic| diagnostic.code == "runtime.locator_not_found")
        );

        let passing = failing.replace("id(\"missing\")", "id(\"submit\")");
        editor.update_document(file, passing);
        let passed = editor
            .run_file(file, &browser)
            .await
            .expect("run passing test");
        assert_eq!(passed.passed(), 1);
        assert!(
            editor
                .diagnostics(file)
                .expect("passing diagnostics")
                .iter()
                .all(|diagnostic| diagnostic.source != DiagnosticSource::Runtime)
        );
    }
}
