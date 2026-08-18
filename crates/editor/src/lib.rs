//! Protocol-independent editor services used by both LSP and future WASM hosts.

use std::sync::{Arc, RwLock};

use thiserror::Error;
use webtest_analysis::{
    AnalysisDatabase, AnalysisError, Diagnostic, DiagnosticSeverity, DiagnosticSource,
};
use webtest_browser::{BrowserError, BrowserHost, Locator};
use webtest_observation::{ObservationStore, RuntimeObservationKind};
use webtest_runtime::{RunResult, Runner};
use webtest_syntax::SyntaxKind;
use webtest_text::{FileId, TextRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticTokenKind {
    Keyword,
    String,
    Comment,
    Function,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub range: TextRange,
    pub kind: SemanticTokenKind,
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

#[derive(Default)]
pub struct EditorService {
    database: RwLock<AnalysisDatabase>,
    observations: Arc<ObservationStore>,
}

impl EditorService {
    pub fn new() -> Self {
        Self::default()
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
                    RuntimeObservationKind::LocatorNotFound { locator, .. } => {
                        let Locator::Id(value) = locator;
                        Diagnostic {
                            range: observation.range,
                            severity: DiagnosticSeverity::Error,
                            code: "runtime.locator_not_found",
                            message: format!(
                                "No element with id {value:?} was found during the last test run."
                            ),
                            source: DiagnosticSource::Runtime,
                        }
                    }
                }),
        );
        Ok(diagnostics)
    }

    pub fn format(&self, file: FileId) -> Result<String, EditorError> {
        let parse = self.write_database().parse(file)?;
        Ok(webtest_format::format_file(&parse))
    }

    pub fn semantic_tokens(&self, file: FileId) -> Result<Vec<SemanticToken>, EditorError> {
        let parse = self.write_database().parse(file)?;
        Ok(parse
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter_map(|token| {
                let kind = match token.kind() {
                    SyntaxKind::TestKw
                    | SyntaxKind::BrowserKw
                    | SyntaxKind::OpenKw
                    | SyntaxKind::ClickKw => SemanticTokenKind::Keyword,
                    SyntaxKind::IdKw => SemanticTokenKind::Function,
                    SyntaxKind::String => SemanticTokenKind::String,
                    SyntaxKind::LineComment => SemanticTokenKind::Comment,
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
        let plan = {
            let mut database = self.write_database();
            if database
                .diagnostics(file)?
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            {
                return Err(EditorError::StaticErrors);
            }
            database.test_plan(file)?
        };
        let runner = Runner::new(Arc::clone(&self.observations));
        Ok(runner.run(&plan, browser).await?)
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
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use webtest_browser::{BrowserSession, Page};
    use webtest_browser_cdp::ChromeHost;

    use super::*;

    struct FakeHost(bool);
    struct FakeSession(bool);
    struct FakePage(bool);

    #[async_trait]
    impl BrowserHost for FakeHost {
        async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError> {
            Ok(Box::new(FakeSession(self.0)))
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

    #[test]
    fn semantic_tokens_are_views_over_cst_tokens() {
        let editor = EditorService::new();
        let source =
            "test \"x\" { // note\n browser { open \"about:blank\" click id(\"submit\") } }";
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
        assert!(rendered.contains(&("\"submit\"", SemanticTokenKind::String)));
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
                let body = "<!doctype html><html><body><button id=\"submit\">Submit</button></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let editor = EditorService::new();
        let failing = format!(
            "test \"missing\" {{ browser {{ open \"http://{address}\" click id(\"missing\") }} }}"
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
