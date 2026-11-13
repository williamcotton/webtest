//! Thin Tower-based LSP adapter over protocol-independent editor services.

mod features;
#[cfg(unix)]
pub mod observation_ipc;
mod synchronization;

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tower_lsp_server::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::{Error, Result},
    ls_types::*,
};
use webtest_analysis::{
    CompletionKind as CoreCompletionKind, Diagnostic as CoreDiagnostic, DiagnosticSeverity,
    DiagnosticSource,
};
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
    synchronized: bool,
    editor: Arc<EditorService>,
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
        self.lock()
            .get(uri.as_str())
            .filter(|document| document.synchronized)
            .cloned()
    }

    fn remove(&self, uri: &Uri) -> Option<Document> {
        self.lock().remove(uri.as_str())
    }

    fn all(&self) -> Vec<Document> {
        self.lock().values().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Document>> {
        self.documents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct Backend {
    client: Client,
    default_editor: Arc<EditorService>,
    editor_for_path: Arc<DocumentEditorResolver>,
    documents: Arc<DocumentStore>,
    browser: Arc<dyn BrowserHost>,
    project_file_changed: Arc<ProjectFileChangeHandler>,
    supports_dynamic_file_watching: AtomicBool,
    queries: Arc<features::QueryRegistry>,
    publications: tokio::sync::Mutex<()>,
}

pub type DocumentEditorResolver =
    dyn Fn(&Path) -> std::result::Result<Arc<EditorService>, String> + Send + Sync;
pub type ProjectFileChangeHandler =
    dyn Fn(&Path, bool) -> std::result::Result<bool, String> + Send + Sync;

impl Backend {
    async fn resolve_synchronized_inputs(&self, uri: &Uri) {
        if let Some(path) = uri.to_file_path()
            && let Err(error) = (self.project_file_changed)(&path, true)
        {
            self.client
                .log_message(
                    MessageType::ERROR,
                    format!("could not resolve workspace imports: {error}"),
                )
                .await;
        }
    }
    async fn publish_workspace_documents(&self, uri: &Uri) {
        let documents = self.documents.all();
        let editor = documents
            .iter()
            .find(|document| document.uri == *uri)
            .map(|document| document.editor.clone());
        for document in documents {
            if editor
                .as_ref()
                .is_some_and(|editor| Arc::ptr_eq(editor, &document.editor))
            {
                self.publish(&document.uri).await;
            }
        }
    }
    fn new(
        client: Client,
        browser: Arc<dyn BrowserHost>,
        default_editor: Arc<EditorService>,
        editor_for_path: Arc<DocumentEditorResolver>,
        project_file_changed: Arc<ProjectFileChangeHandler>,
        queries: Arc<features::QueryRegistry>,
    ) -> Self {
        Self {
            client,
            default_editor,
            editor_for_path,
            documents: Arc::new(DocumentStore::default()),
            browser,
            project_file_changed,
            supports_dynamic_file_watching: AtomicBool::new(false),
            queries,
            publications: tokio::sync::Mutex::new(()),
        }
    }

    async fn publish(&self, uri: &Uri) {
        let Some(document) = self.documents.get(uri) else {
            return;
        };
        let file = document.file;
        let result = self
            .query(Some(&document), document.editor.clone(), move |snapshot| {
                let source = snapshot.source(file).map_err(features::core_error)?;
                let diagnostics = snapshot
                    .diagnostics(file)
                    .map_err(features::editor_error)?
                    .into_iter()
                    .map(|diagnostic| {
                        let related = diagnostic.related_locations.clone();
                        let mut wire = diagnostic_to_lsp(&source, diagnostic);
                        let locations = related
                            .into_iter()
                            .filter_map(|location| {
                                let source = snapshot.source(location.origin.file).ok()?;
                                if webtest_text::SourceRevision::of(&source) != location.revision {
                                    return None;
                                }
                                Some(DiagnosticRelatedInformation {
                                    location: Location::new(
                                        features::source_uri(
                                            snapshot,
                                            location.origin.file,
                                            &location.path,
                                        )
                                        .ok()?,
                                        text_range_to_lsp(&source, location.origin.range),
                                    ),
                                    message: "Related declaration or call site".into(),
                                })
                            })
                            .collect::<Vec<_>>();
                        wire.related_information = (!locations.is_empty()).then_some(locations);
                        wire
                    })
                    .collect::<Vec<_>>();
                Ok((
                    diagnostics,
                    snapshot.workspace_revision(),
                    snapshot.observation_revision(),
                ))
            })
            .await;
        match result {
            Ok((diagnostics, revision, observation_revision)) => {
                let _publication = self.publications.lock().await;
                if self
                    .documents
                    .get(uri)
                    .is_some_and(|current| current.version == document.version)
                    && document.editor.analysis_revision() == revision
                    && document.editor.observations().generation() == observation_revision
                {
                    self.client
                        .publish_diagnostics(uri.clone(), diagnostics, Some(document.version))
                        .await;
                }
            }
            Err(error)
                if matches!(
                    error.code,
                    tower_lsp_server::jsonrpc::ErrorCode::RequestCancelled
                        | tower_lsp_server::jsonrpc::ErrorCode::ContentModified
                ) => {}
            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await
            }
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if params
            .workspace_folders
            .as_ref()
            .is_some_and(|folders| folders.len() > 1)
        {
            return Err(Error::invalid_params(
                "WebTest currently supports one workspace root per language-server session",
            ));
        }

        let client_supplies_project_file_events = params
            .initialization_options
            .as_ref()
            .and_then(|options| options.get("projectFileEvents"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        self.supports_dynamic_file_watching.store(
            params
                .capabilities
                .workspace
                .and_then(|workspace| workspace.did_change_watched_files)
                .and_then(|watched| watched.dynamic_registration)
                .unwrap_or(false)
                && !client_supplies_project_file_events,
            Ordering::Relaxed,
        );
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "webtest".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                position_encoding: Some(PositionEncodingKind::UTF16),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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
                                SemanticTokenType::PARAMETER,
                                SemanticTokenType::NAMESPACE,
                                SemanticTokenType::new("fixture"),
                                SemanticTokenType::new("test"),
                            ],
                            token_modifiers: vec![
                                SemanticTokenModifier::DECLARATION,
                                SemanticTokenModifier::READONLY,
                                SemanticTokenModifier::DEPRECATED,
                            ],
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
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into(), ",".into()]),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".into(), ",".into()]),
                    retrigger_characters: Some(vec![",".into()]),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                ..Default::default()
            },
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.supports_dynamic_file_watching.load(Ordering::Relaxed) {
            let options = DidChangeWatchedFilesRegistrationOptions {
                watchers: vec![
                    FileSystemWatcher {
                        glob_pattern: GlobPattern::String("**/webtest.toml".into()),
                        kind: None,
                    },
                    FileSystemWatcher {
                        glob_pattern: GlobPattern::String("**/*.json".into()),
                        kind: None,
                    },
                    FileSystemWatcher {
                        glob_pattern: GlobPattern::String("**/*.webtest".into()),
                        kind: None,
                    },
                ],
            };
            let registration = Registration {
                id: "webtest-project-inputs".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: serde_json::to_value(options).ok(),
            };
            if let Err(error) = self.client.register_capability(vec![registration]).await {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("could not watch WebTest project inputs: {error}"),
                    )
                    .await;
            }
        }
        self.client
            .log_message(MessageType::INFO, "WebTest language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        self.finish_queries().await;
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        let editor = if document.uri.scheme().as_str() == "file" {
            match document.uri.to_file_path() {
                Some(path) => match (self.editor_for_path)(&path) {
                    Ok(editor) => editor,
                    Err(error) => {
                        self.client
                            .log_message(
                                MessageType::ERROR,
                                format!(
                                    "could not resolve WebTest project for {}: {error}",
                                    path.display()
                                ),
                            )
                            .await;
                        Arc::clone(&self.default_editor)
                    }
                },
                None => Arc::clone(&self.default_editor),
            }
        } else {
            Arc::clone(&self.default_editor)
        };
        let path = document
            .uri
            .to_file_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| document.uri.as_str().to_owned());
        let file = editor.open_document(path, document.text);
        if let Err(error) = editor.set_document_uri(file, document.uri.as_str().to_owned()) {
            self.client
                .log_message(MessageType::ERROR, error.to_string())
                .await;
            return;
        }
        if let Err(error) = editor.set_document_version(
            file,
            webtest_text::DocumentVersion(i64::from(document.version)),
        ) {
            self.client
                .log_message(MessageType::ERROR, error.to_string())
                .await;
            return;
        }
        self.documents.insert(Document {
            file,
            uri: document.uri.clone(),
            version: document.version,
            synchronized: true,
            editor,
        });
        self.resolve_synchronized_inputs(&document.uri).await;
        self.publish_workspace_documents(&document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let result = {
            let mut documents = self.documents.lock();
            let Some(document) = documents.get_mut(uri.as_str()) else {
                return;
            };
            let full = params
                .content_changes
                .first()
                .is_some_and(|change| change.range.is_none());
            let version = params.text_document.version;
            if version < document.version
                || (version == document.version && (document.synchronized || !full))
            {
                return;
            }
            let result = if !document.synchronized && !full {
                Err("document requires full synchronization".to_owned())
            } else {
                document
                    .editor
                    .source(document.file)
                    .map_err(|error| error.to_string())
                    .and_then(|source| {
                        synchronization::changes(&source, &params.content_changes)
                            .map_err(str::to_owned)
                    })
                    .and_then(|text| {
                        document
                            .editor
                            .update_document_versioned(
                                document.file,
                                text,
                                webtest_text::DocumentVersion(i64::from(version)),
                            )
                            .map_err(|error| error.to_string())
                    })
            };
            document.version = version;
            document.synchronized = result.is_ok();
            result
        };
        match result {
            Ok(()) => {
                self.resolve_synchronized_inputs(&uri).await;
                self.publish_workspace_documents(&uri).await;
            }
            Err(error) => {
                self.client
                    .publish_diagnostics(
                        uri.clone(),
                        Vec::new(),
                        Some(params.text_document.version),
                    )
                    .await;
                self.client.log_message(MessageType::WARNING,format!("WebTest rejected an incremental edit for {}: {error}; a full synchronization is required",uri.as_str())).await;
                self.client
                    .send_notification::<ResyncRequired>(ResyncParameters { uri })
                    .await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Some(document) = self.documents.remove(&params.text_document.uri) {
            document.editor.close_document(document.file);
            self.client
                .publish_diagnostics(document.uri, Vec::new(), None)
                .await;
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut reconfigured = false;
        for change in params.changes {
            if change.uri.scheme().as_str() != "file" {
                continue;
            }
            let Some(path) = change.uri.to_file_path() else {
                continue;
            };
            match (self.project_file_changed)(&path, false) {
                Ok(changed) => reconfigured |= changed,
                Err(error) => {
                    reconfigured = true;
                    self.client
                        .log_message(
                            MessageType::ERROR,
                            format!(
                                "could not reload WebTest project input {}: {error}",
                                path.display()
                            ),
                        )
                        .await;
                }
            }
        }
        if reconfigured {
            for document in self.documents.all() {
                self.resolve_synchronized_inputs(&document.uri).await;
                self.publish(&document.uri).await;
            }
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        self.definition_request(params).await
    }
    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        self.references_request(params).await
    }
    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        self.rename_request(params).await
    }
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.document_symbols_request(params).await
    }
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        self.workspace_symbols_request(params).await
    }
    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        self.folding_request(params).await
    }
    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        self.selection_request(params).await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        self.inlay_hints_request(params).await
    }
    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        self.code_actions_request(params).await
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(features::core_error)?;
            let formatted = snapshot.format(file).map_err(features::editor_error)?;
            let full_range = text_range_to_lsp(
                &source,
                TextRange::new(TextSize::new(0), TextSize::from(source.len() as u32)),
            );
            Ok(Some(vec![TextEdit::new(full_range, formatted)]))
        })
        .await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(features::core_error)?;
            let tokens = snapshot
                .semantic_tokens(file)
                .map_err(features::editor_error)?;
            Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: Some(snapshot.workspace_revision().to_string()),
                data: semantic_tokens_to_lsp(&source, &tokens),
            })))
        })
        .await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let Some(document) = self
            .documents
            .get(&params.text_document_position_params.text_document.uri)
        else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(features::core_error)?;
            let offset =
                synchronization::offset(&source, params.text_document_position_params.position)
                    .ok_or_else(|| Error::invalid_params("invalid UTF-16 position"))?;
            let hover = snapshot
                .hover(file, TextSize::from(offset as u32))
                .map_err(features::editor_error)?;
            Ok(hover.map(|hover| Hover {
                contents: HoverContents::Scalar(MarkedString::String(hover.contents)),
                range: Some(text_range_to_lsp(&source, hover.range)),
            }))
        })
        .await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let position = params.text_document_position;
        let Some(document) = self.documents.get(&position.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(features::core_error)?;
            let offset = synchronization::offset(&source, position.position)
                .ok_or_else(|| Error::invalid_params("invalid UTF-16 position"))?;
            let completions = snapshot
                .completions(file, TextSize::from(offset as u32))
                .map_err(features::core_error)?
                .into_iter()
                .map(|completion| CompletionItem {
                    label: completion.label,
                    kind: Some(match completion.kind {
                        CoreCompletionKind::Function => CompletionItemKind::FUNCTION,
                        CoreCompletionKind::Variable => CompletionItemKind::VARIABLE,
                        CoreCompletionKind::Keyword => CompletionItemKind::KEYWORD,
                        CoreCompletionKind::Namespace => CompletionItemKind::MODULE,
                        CoreCompletionKind::Module => CompletionItemKind::FILE,
                        CoreCompletionKind::Type => CompletionItemKind::TYPE_PARAMETER,
                        CoreCompletionKind::Fixture => CompletionItemKind::CONSTRUCTOR,
                        CoreCompletionKind::Parameter => CompletionItemKind::FIELD,
                        CoreCompletionKind::Property => CompletionItemKind::PROPERTY,
                    }),
                    detail: Some(completion.detail),
                    documentation: (!completion.documentation.is_empty())
                        .then_some(Documentation::String(completion.documentation)),
                    text_edit: completion.replacement_range.map(|range| {
                        CompletionTextEdit::Edit(TextEdit::new(
                            text_range_to_lsp(
                                &source,
                                TextRange::new(range.start.into(), range.end.into()),
                            ),
                            completion.insert_text.clone(),
                        ))
                    }),
                    additional_text_edits: (!completion.additional_edits.is_empty()).then(|| {
                        completion
                            .additional_edits
                            .into_iter()
                            .filter(|edit| edit.origin.file == file)
                            .map(|edit| {
                                TextEdit::new(
                                    text_range_to_lsp(&source, edit.origin.range),
                                    edit.replacement,
                                )
                            })
                            .collect()
                    }),
                    sort_text: Some(completion.sort_text),
                    filter_text: Some(completion.filter_text),
                    data: completion
                        .symbol
                        .map(|symbol| serde_json::json!({"symbol":symbol})),
                    insert_text: Some(completion.insert_text),
                    ..Default::default()
                })
                .collect();
            Ok(Some(CompletionResponse::Array(completions)))
        })
        .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let position = params.text_document_position_params;
        let Some(document) = self.documents.get(&position.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(features::core_error)?;
            let offset = synchronization::offset(&source, position.position)
                .ok_or_else(|| Error::invalid_params("invalid UTF-16 position"))?;
            let signature = snapshot
                .signature_help(file, TextSize::from(offset as u32))
                .map_err(features::core_error)?;
            Ok(signature.map(|signature| SignatureHelp {
                signatures: vec![SignatureInformation {
                    label: signature.label,
                    documentation: (!signature.documentation.is_empty())
                        .then_some(Documentation::String(signature.documentation)),
                    parameters: Some(
                        signature
                            .parameters
                            .into_iter()
                            .map(|parameter| ParameterInformation {
                                label: ParameterLabel::Simple(parameter.label),
                                documentation: (!parameter.documentation.is_empty())
                                    .then_some(Documentation::String(parameter.documentation)),
                            })
                            .collect(),
                    ),
                    active_parameter: Some(signature.active_parameter as u32),
                }],
                active_signature: Some(0),
                active_parameter: Some(signature.active_parameter as u32),
            }))
        })
        .await
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

        if let Err(error) = document
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
    let resolved_editor = Arc::clone(&editor);
    serve_with_document_editors(
        browser,
        editor,
        Arc::new(move |_| Ok(Arc::clone(&resolved_editor))),
    )
    .await;
}

pub async fn serve_with_document_editors(
    browser: Arc<dyn BrowserHost>,
    default_editor: Arc<EditorService>,
    editor_for_path: Arc<DocumentEditorResolver>,
) {
    serve_with_document_editors_and_project_changes(
        browser,
        default_editor,
        editor_for_path,
        Arc::new(|_, _| Ok(false)),
    )
    .await;
}

pub async fn serve_with_document_editors_and_project_changes(
    browser: Arc<dyn BrowserHost>,
    default_editor: Arc<EditorService>,
    editor_for_path: Arc<DocumentEditorResolver>,
    project_file_changed: Arc<ProjectFileChangeHandler>,
) {
    serve_with_observations(
        browser,
        default_editor,
        editor_for_path,
        project_file_changed,
        None,
    )
    .await;
}

pub async fn serve_with_observations(
    browser: Arc<dyn BrowserHost>,
    default_editor: Arc<EditorService>,
    editor_for_path: Arc<DocumentEditorResolver>,
    project_file_changed: Arc<ProjectFileChangeHandler>,
    workspace: Option<std::path::PathBuf>,
) {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (target, receive_target) = tokio::sync::oneshot::channel();
    let queries = Arc::new(features::QueryRegistry::default());
    let request_queries = queries.clone();
    let (service, socket) = LspService::build(|client| {
        let backend = Backend::new(
            client.clone(),
            browser,
            default_editor,
            editor_for_path,
            project_file_changed,
            request_queries,
        );
        let _ = target.send((client, backend.documents.clone()));
        backend
    })
    .custom_method("webtest/discoverTests", Backend::discover_tests)
    .custom_method("webtest/describeProject", Backend::describe_project)
    .custom_method("webtest/runVariants", Backend::run_variants)
    .custom_method("webtest/cancelRun", Backend::cancel_run)
    .custom_method("window/workDoneProgress/cancel", Backend::cancel_progress)
    .finish();
    let server = Server::new(stdin, stdout, socket).serve(service);
    #[cfg(unix)]
    {
        let endpoint = match workspace {
            Some(workspace) => match observation_ipc::Endpoint::bind(&workspace).await {
                Ok(endpoint) => Some(endpoint),
                Err(error) => {
                    tracing::warn!(%error, "observation publication unavailable");
                    None
                }
            },
            None => None,
        };
        let publication = async {
            if let (Some(endpoint), Ok((client, documents))) = (endpoint, receive_target.await) {
                endpoint.serve(client, documents).await;
            }
            std::future::pending::<()>().await;
        };
        tokio::select! { _ = server => {}, _ = publication => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = (workspace, receive_target);
        server.await;
    }
    queries.finish().await;
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
                        CoreSemanticTokenKind::Parameter => 7,
                        CoreSemanticTokenKind::Namespace => 8,
                        CoreSemanticTokenKind::Fixture => 9,
                        CoreSemanticTokenKind::Test => 10,
                    },
                    token_modifiers_bitset: u32::from(token.modifiers.declaration)
                        | (u32::from(token.modifiers.readonly) << 1)
                        | (u32::from(token.modifiers.deprecated) << 2),
                });
                previous_line = start_position.line;
                previous_start = start_position.character;
            }
            segment_start = newline.map_or(token_end, |offset| offset + 1);
        }
    }

    result
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ResyncParameters {
    uri: Uri,
}
enum ResyncRequired {}
impl tower_lsp_server::ls_types::notification::Notification for ResyncRequired {
    type Params = ResyncParameters;
    const METHOD: &'static str = "webtest/resyncRequired";
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
                related_locations: Vec::new(),
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
                    modifiers: Default::default(),
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
