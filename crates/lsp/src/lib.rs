//! Thin Tower-based LSP adapter over protocol-independent editor services.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tower_lsp_server::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::{Error, Result},
    ls_types::*,
};
use webtest_analysis::{Diagnostic as CoreDiagnostic, DiagnosticSeverity, DiagnosticSource};
use webtest_browser::BrowserHost;
use webtest_editor::{
    EditorService, SemanticToken as CoreSemanticToken, SemanticTokenKind as CoreSemanticTokenKind,
};
use webtest_text::{FileId, TextRange, TextSize};

#[derive(Clone)]
struct Document {
    file: FileId,
    uri: Uri,
    version: i32,
}

#[derive(Default)]
struct DocumentStore {
    documents: Mutex<HashMap<String, Document>>,
}

impl DocumentStore {
    fn insert(&self, document: Document) {
        self.lock()
            .insert(document.uri.as_str().to_owned(), document);
    }

    fn get(&self, uri: &Uri) -> Option<Document> {
        self.lock().get(uri.as_str()).cloned()
    }

    fn remove(&self, uri: &Uri) -> Option<Document> {
        self.lock().remove(uri.as_str())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Document>> {
        self.documents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct Backend {
    client: Client,
    editor: Arc<EditorService>,
    documents: Arc<DocumentStore>,
    browser: Arc<dyn BrowserHost>,
}

impl Backend {
    fn new(client: Client, browser: Arc<dyn BrowserHost>, editor: Arc<EditorService>) -> Self {
        Self {
            client,
            editor,
            documents: Arc::new(DocumentStore::default()),
            browser,
        }
    }

    async fn publish(&self, uri: &Uri) {
        let Some(document) = self.documents.get(uri) else {
            return;
        };
        let source = match self.editor.source(document.file) {
            Ok(source) => source,
            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await;
                return;
            }
        };
        let diagnostics = match self.editor.diagnostics(document.file) {
            Ok(diagnostics) => diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic_to_lsp(&source, diagnostic))
                .collect(),
            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await;
                Vec::new()
            }
        };
        self.client
            .publish_diagnostics(document.uri, diagnostics, Some(document.version))
            .await;
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "webtest".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                position_encoding: Some(PositionEncodingKind::UTF16),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensOptions {
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                        legend: SemanticTokensLegend {
                            token_types: vec![
                                SemanticTokenType::KEYWORD,
                                SemanticTokenType::STRING,
                                SemanticTokenType::COMMENT,
                                SemanticTokenType::FUNCTION,
                                SemanticTokenType::VARIABLE,
                                SemanticTokenType::PROPERTY,
                                SemanticTokenType::TYPE,
                            ],
                            token_modifiers: Vec::new(),
                        },
                        range: None,
                        full: Some(SemanticTokensFullOptions::Bool(true)),
                    }
                    .into(),
                ),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["webtest.runFile".into()],
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "WebTest language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        let file = self
            .editor
            .open_document(document.uri.as_str(), document.text);
        self.documents.insert(Document {
            file,
            uri: document.uri.clone(),
            version: document.version,
        });
        self.publish(&document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(mut document) = self.documents.get(&uri) else {
            return;
        };
        if let Some(change) = params.content_changes.into_iter().last() {
            self.editor.update_document(document.file, change.text);
        }
        document.version = params.text_document.version;
        self.documents.insert(document);
        self.publish(&uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Some(document) = self.documents.remove(&params.text_document.uri) {
            self.editor.close_document(document.file);
            self.client
                .publish_diagnostics(document.uri, Vec::new(), None)
                .await;
        }
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let source = self
            .editor
            .source(document.file)
            .map_err(|error| Error::invalid_params(error.to_string()))?;
        let formatted = self
            .editor
            .format(document.file)
            .map_err(|error| Error::invalid_params(error.to_string()))?;
        let full_range = text_range_to_lsp(
            &source,
            TextRange::new(TextSize::new(0), TextSize::from(source.len() as u32)),
        );
        Ok(Some(vec![TextEdit::new(full_range, formatted)]))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let source = self
            .editor
            .source(document.file)
            .map_err(|error| Error::invalid_params(error.to_string()))?;
        let tokens = self
            .editor
            .semantic_tokens(document.file)
            .map_err(|error| Error::invalid_params(error.to_string()))?;
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: semantic_tokens_to_lsp(&source, &tokens),
        })))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let Some(document) = self
            .documents
            .get(&params.text_document_position_params.text_document.uri)
        else {
            return Ok(None);
        };
        let source = self
            .editor
            .source(document.file)
            .map_err(|error| Error::invalid_params(error.to_string()))?;
        let offset = position_to_offset(&source, params.text_document_position_params.position);
        let hover = self
            .editor
            .hover(document.file, TextSize::from(offset as u32))
            .map_err(|error| Error::invalid_params(error.to_string()))?;
        Ok(hover.map(|hover| Hover {
            contents: HoverContents::Scalar(MarkedString::String(hover.contents)),
            range: Some(text_range_to_lsp(&source, hover.range)),
        }))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        if params.command != "webtest.runFile" {
            return Err(Error::invalid_request());
        }
        let uri_text = params
            .arguments
            .first()
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::invalid_params("webtest.runFile expects a document URI"))?;
        let uri: Uri = uri_text
            .parse()
            .map_err(|_| Error::invalid_params("invalid document URI"))?;
        let document = self
            .documents
            .get(&uri)
            .ok_or_else(|| Error::invalid_params("document is not open"))?;

        if let Err(error) = self
            .editor
            .run_file(document.file, self.browser.as_ref())
            .await
        {
            self.client
                .show_message(MessageType::ERROR, error.to_string())
                .await;
        }
        self.publish(&uri).await;
        Ok(None)
    }
}

pub async fn serve(browser: Arc<dyn BrowserHost>) {
    serve_with_editor(browser, Arc::new(EditorService::new())).await;
}

pub async fn serve_with_editor(browser: Arc<dyn BrowserHost>, editor: Arc<EditorService>) {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend::new(client, browser, editor));
    Server::new(stdin, stdout, socket).serve(service).await;
}

pub fn text_range_to_lsp(text: &str, range: TextRange) -> Range {
    Range::new(
        offset_to_position(text, u32::from(range.start()) as usize),
        offset_to_position(text, u32::from(range.end()) as usize),
    )
}

fn offset_to_position(text: &str, requested_offset: usize) -> Position {
    let mut offset = requested_offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let mut line = 0u32;
    let mut character = 0u32;
    for value in text[..offset].chars() {
        if value == '\n' {
            line += 1;
            character = 0;
        } else {
            character += value.len_utf16() as u32;
        }
    }
    Position::new(line, character)
}

fn position_to_offset(text: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut utf16_column = 0u32;
    for (offset, character) in text.char_indices() {
        if line == position.line {
            let width = character.len_utf16() as u32;
            if utf16_column + width > position.character {
                return offset;
            }
            if utf16_column == position.character {
                return offset;
            }
            utf16_column += width;
        }
        if character == '\n' {
            if line == position.line {
                return offset;
            }
            line += 1;
            utf16_column = 0;
        }
    }
    text.len()
}

fn diagnostic_to_lsp(text: &str, diagnostic: CoreDiagnostic) -> Diagnostic {
    let data = serde_json::json!({
        "diagnostic_schema_version": webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        "repair_hint_schema_version": webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        "byte_range": {
            "start": u32::from(diagnostic.range.start()),
            "end": u32::from(diagnostic.range.end()),
        },
        "semantic_details": diagnostic.semantic_details,
        "repair_hints": diagnostic.repair_hints,
        "reference_queries": diagnostic.reference_queries,
    });
    Diagnostic {
        range: text_range_to_lsp(text, diagnostic.range),
        severity: Some(match diagnostic.severity {
            DiagnosticSeverity::Error => tower_lsp_server::ls_types::DiagnosticSeverity::ERROR,
            DiagnosticSeverity::Warning => tower_lsp_server::ls_types::DiagnosticSeverity::WARNING,
            DiagnosticSeverity::Information => {
                tower_lsp_server::ls_types::DiagnosticSeverity::INFORMATION
            }
            DiagnosticSeverity::Hint => tower_lsp_server::ls_types::DiagnosticSeverity::HINT,
        }),
        code: Some(NumberOrString::String(diagnostic.code.into())),
        source: Some(
            match diagnostic.source {
                DiagnosticSource::Syntax => "webtest.syntax",
                DiagnosticSource::Semantic => "webtest.semantic",
                DiagnosticSource::Runtime => "webtest.runtime",
            }
            .into(),
        ),
        message: diagnostic.message,
        data: Some(data),
        ..Default::default()
    }
}

fn semantic_tokens_to_lsp(text: &str, tokens: &[CoreSemanticToken]) -> Vec<SemanticToken> {
    let mut result = Vec::new();
    let mut previous_line = 0u32;
    let mut previous_start = 0u32;

    for token in tokens {
        let start = u32::from(token.range.start()) as usize;
        let end = u32::from(token.range.end()) as usize;
        let mut segment_start = start.min(text.len());
        let token_end = end.min(text.len());

        while segment_start < token_end {
            let newline = text[segment_start..token_end]
                .find('\n')
                .map(|offset| segment_start + offset);
            let segment_end = newline.unwrap_or(token_end);
            if segment_end > segment_start {
                let start_position = offset_to_position(text, segment_start);
                let end_position = offset_to_position(text, segment_end);
                let delta_line = start_position.line - previous_line;
                let delta_start = if delta_line == 0 {
                    start_position.character - previous_start
                } else {
                    start_position.character
                };
                result.push(SemanticToken {
                    delta_line,
                    delta_start,
                    length: end_position.character - start_position.character,
                    token_type: match token.kind {
                        CoreSemanticTokenKind::Keyword => 0,
                        CoreSemanticTokenKind::String => 1,
                        CoreSemanticTokenKind::Comment => 2,
                        CoreSemanticTokenKind::Function => 3,
                        CoreSemanticTokenKind::Variable => 4,
                        CoreSemanticTokenKind::Property => 5,
                        CoreSemanticTokenKind::Type => 6,
                    },
                    token_modifiers_bitset: 0,
                });
                previous_line = start_position.line;
                previous_start = start_position.character;
            }
            segment_start = newline.map_or(token_end, |offset| offset + 1);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_byte_offsets_to_utf16_positions() {
        let text = "a😀\nβ";
        let beta = text.find('β').expect("beta offset");
        let range = TextRange::new(
            TextSize::from(beta as u32),
            TextSize::from(text.len() as u32),
        );
        assert_eq!(
            text_range_to_lsp(text, range),
            Range::new(Position::new(1, 0), Position::new(1, 1))
        );
        assert_eq!(offset_to_position(text, "a😀".len()), Position::new(0, 3));
    }

    #[test]
    fn lsp_ranges_are_utf16_while_machine_data_preserves_canonical_bytes() {
        let text = "😀 emial";
        let start = text.find("emial").expect("member");
        let range = TextRange::new(
            TextSize::from(start as u32),
            TextSize::from((start + "emial".len()) as u32),
        );
        let diagnostic = diagnostic_to_lsp(
            text,
            CoreDiagnostic {
                range,
                severity: DiagnosticSeverity::Error,
                code: "semantic.unknown_member",
                message: "unknown member".into(),
                source: DiagnosticSource::Semantic,
                semantic_details: Some(serde_json::json!({"requested": "emial"})),
                repair_hints: vec![webtest_feedback::RepairHint::text(
                    webtest_feedback::RepairHintKind::MemberCandidate,
                    "email",
                )],
                reference_queries: vec!["type.Record".into()],
            },
        );
        assert_eq!(diagnostic.range.start, Position::new(0, 3));
        let data = diagnostic.data.expect("machine diagnostic data");
        assert_eq!(data["repair_hint_schema_version"], 1);
        assert_eq!(data["byte_range"]["start"], start as u32);
        assert_eq!(data["byte_range"]["end"], (start + 5) as u32);
        assert_eq!(data["repair_hints"][0]["replacement"], "email");
        assert_eq!(data["reference_queries"][0], "type.Record");
    }

    #[test]
    fn encodes_cst_semantic_tokens_with_utf16_deltas() {
        let text = "test \"😀\"\n// note\nid(\"x\")";
        let ranges = [
            ("test", CoreSemanticTokenKind::Keyword),
            ("\"😀\"", CoreSemanticTokenKind::String),
            ("// note", CoreSemanticTokenKind::Comment),
            ("id", CoreSemanticTokenKind::Function),
        ];
        let tokens: Vec<_> = ranges
            .into_iter()
            .map(|(needle, kind)| {
                let start = text.find(needle).expect("token offset");
                CoreSemanticToken {
                    range: TextRange::new(
                        TextSize::from(start as u32),
                        TextSize::from((start + needle.len()) as u32),
                    ),
                    kind,
                }
            })
            .collect();
        let encoded = semantic_tokens_to_lsp(text, &tokens);
        assert_eq!(encoded.len(), 4);
        assert_eq!(encoded[0].delta_line, 0);
        assert_eq!(encoded[0].delta_start, 0);
        assert_eq!(encoded[1].length, 4);
        assert_eq!(encoded[2].delta_line, 1);
        assert_eq!(encoded[3].delta_line, 1);
        assert_eq!(encoded[3].token_type, 3);
    }
}
