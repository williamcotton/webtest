//! Request lifetime and wire conversion for shared Rust workspace queries.
use super::*;
use tower_lsp_server::jsonrpc::ErrorCode;
use webtest_analysis::AnalysisCancellation;
use webtest_editor::EditorSnapshot as AnalysisSnapshot;

pub(super) struct QueryTask {
    progress: Option<ProgressToken>,
    cancellation: AnalysisCancellation,
    join: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
pub(super) struct QueryRegistry {
    tasks: Mutex<Vec<QueryTask>>,
    runs: Mutex<Vec<RunTask>>,
    run_controls: Arc<Mutex<HashMap<String, Arc<ExecutionCancellation>>>>,
    shutting_down: AtomicBool,
}
impl QueryRegistry {
    pub(super) async fn finish(&self) {
        self.shutting_down.store(true, Ordering::Release);
        for control in self
            .run_controls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            control.cancel();
        }
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()));
        for task in &tasks {
            task.cancellation.cancel();
        }
        for task in tasks {
            let _ = task.join.await;
        }
        let runs = std::mem::take(&mut *self.runs.lock().unwrap_or_else(|e| e.into_inner()));
        for run in &runs {
            run.control.cancel();
        }
        for run in runs {
            let _ = run.join.await;
        }
    }
}
struct CancelOnDrop(AnalysisCancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
pub(super) fn core_error(error: webtest_analysis::AnalysisError) -> Error {
    if matches!(error, webtest_analysis::AnalysisError::Cancelled) {
        Error::new(ErrorCode::RequestCancelled)
    } else {
        Error::invalid_params(error.to_string())
    }
}
pub(super) fn editor_error(error: webtest_editor::EditorError) -> Error {
    match error {
        webtest_editor::EditorError::Analysis(error) => core_error(error),
        error => Error::invalid_params(error.to_string()),
    }
}
pub(super) fn uri(path: &str) -> Result<Uri> {
    if let Ok(uri) = path.parse::<Uri>()
        && uri.scheme().as_str() == "file"
    {
        return Ok(uri);
    }
    Uri::from_file_path(Path::new(path))
        .ok_or_else(|| Error::invalid_params("source has no absolute file URI"))
}
pub(super) fn source_uri(snapshot: &AnalysisSnapshot, file: FileId, path: &str) -> Result<Uri> {
    if let Some(document_uri) = snapshot.document_uri(file) {
        document_uri
            .parse()
            .map_err(|_| Error::invalid_params("invalid synchronized document URI"))
    } else {
        uri(path)
    }
}
fn location(
    snapshot: &AnalysisSnapshot,
    value: &webtest_analysis::SourceLocation,
) -> Result<Location> {
    let source = snapshot.source(value.origin.file).map_err(core_error)?;
    Ok(Location::new(
        source_uri(snapshot, value.origin.file, &value.path)?,
        text_range_to_lsp(&source, value.origin.range),
    ))
}
fn symbol_kind(kind: webtest_analysis::SymbolKind) -> SymbolKind {
    use webtest_analysis::SymbolKind as K;
    match kind {
        K::Module => SymbolKind::MODULE,
        K::Import => SymbolKind::NAMESPACE,
        K::Test => SymbolKind::EVENT,
        K::Function | K::ProviderFunction => SymbolKind::FUNCTION,
        K::Fixture => SymbolKind::OBJECT,
        K::Parameter | K::Binding => SymbolKind::VARIABLE,
        K::RecordField => SymbolKind::PROPERTY,
        K::ProviderNamespace => SymbolKind::NAMESPACE,
        K::Type => SymbolKind::STRUCT,
    }
}
#[allow(deprecated)]
fn symbol_information(
    snapshot: &AnalysisSnapshot,
    symbol: webtest_analysis::Symbol,
) -> Result<Option<SymbolInformation>> {
    let Some(source) = symbol.location else {
        return Ok(None);
    };
    Ok(Some(SymbolInformation {
        name: symbol.name,
        kind: symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        location: location(snapshot, &source)?,
        container_name: None,
    }))
}
impl Backend {
    pub(super) async fn query<R: Send + 'static>(
        &self,
        document: Option<&Document>,
        editor: Arc<EditorService>,
        work: impl FnOnce(&mut AnalysisSnapshot) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        self.query_with_progress(document, editor, None, work).await
    }
    pub(super) async fn query_with_progress<R: Send + 'static>(
        &self,
        document: Option<&Document>,
        editor: Arc<EditorService>,
        progress: Option<(ProgressToken, String)>,
        work: impl FnOnce(&mut AnalysisSnapshot) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let snapshot = editor.snapshot();
        let revision = snapshot.workspace_revision();
        let cancellation = snapshot.cancellation();
        let _cancel = CancelOnDrop(cancellation.clone());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        {
            let mut pending = self.queries.tasks.lock().unwrap_or_else(|e| e.into_inner());
            pending.retain(|task| !task.join.is_finished());
            if self.queries.shutting_down.load(Ordering::Acquire) || pending.len() >= 256 {
                return Err(Error::new(ErrorCode::RequestCancelled));
            }
            let blocking = tokio::task::spawn_blocking(move || {
                let mut snapshot = snapshot;
                let result = work(&mut snapshot);
                drop(snapshot);
                result
            });
            let progress_token = progress.as_ref().map(|(token, _)| token.clone());
            let client = self.client.clone();
            let join = tokio::spawn(async move {
                if let Some((token, title)) = &progress {
                    client
                        .send_notification::<notification::Progress>(ProgressParams {
                            token: token.clone(),
                            value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                                WorkDoneProgressBegin {
                                    title: title.clone(),
                                    cancellable: Some(true),
                                    ..Default::default()
                                },
                            )),
                        })
                        .await;
                }
                let result = blocking
                    .await
                    .unwrap_or_else(|_| Err(Error::internal_error()));
                if let Some((token, _)) = progress {
                    client
                        .send_notification::<notification::Progress>(ProgressParams {
                            token,
                            value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                                WorkDoneProgressEnd::default(),
                            )),
                        })
                        .await;
                }
                let _ = sender.send(result);
            });
            pending.push(QueryTask {
                cancellation,
                join,
                progress: progress_token,
            });
        }
        let result = receiver.await.map_err(|_| Error::internal_error())?;
        if editor.analysis_revision() != revision
            || document.is_some_and(|document| {
                self.documents
                    .get(&document.uri)
                    .is_none_or(|current| current.version != document.version)
            })
        {
            return Err(Error::new(ErrorCode::ContentModified));
        }
        result
    }
    pub(super) async fn cancel_progress(&self, params: WorkDoneProgressCancelParams) {
        let tasks = self.queries.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for task in tasks
            .iter()
            .filter(|task| task.progress.as_ref() == Some(&params.token))
        {
            task.cancellation.cancel();
        }
    }
    pub(super) async fn finish_queries(&self) {
        self.queries.finish().await;
    }
    pub(super) async fn definition_request(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params;
        let Some(document) = self.documents.get(&position.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(core_error)?;
            let offset = synchronization::offset(&source, position.position)
                .ok_or_else(|| Error::invalid_params("invalid UTF-16 position"))?;
            let Some(link) = snapshot
                .definition(file, TextSize::from(offset as u32))
                .map_err(core_error)?
            else {
                return Ok(None);
            };
            Ok(Some(GotoDefinitionResponse::Scalar(location(
                snapshot,
                &link.target,
            )?)))
        })
        .await
    }
    pub(super) async fn references_request(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>> {
        let position = params.text_document_position;
        let Some(document) = self.documents.get(&position.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(core_error)?;
            let offset = synchronization::offset(&source, position.position)
                .ok_or_else(|| Error::invalid_params("invalid UTF-16 position"))?;
            let references = snapshot
                .references(
                    file,
                    TextSize::from(offset as u32),
                    params.context.include_declaration,
                    10_000,
                )
                .map_err(core_error)?;
            if references.truncated {
                return Err(Error::invalid_params(
                    "reference result exceeds the workspace query limit",
                ));
            }
            Ok(Some(
                references
                    .items
                    .iter()
                    .map(|reference| location(snapshot, &reference.location))
                    .collect::<Result<Vec<_>>>()?,
            ))
        })
        .await
    }
    pub(super) async fn rename_request(
        &self,
        params: RenameParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let position = params.text_document_position;
        let Some(document) = self.documents.get(&position.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(core_error)?;
            let offset = synchronization::offset(&source, position.position)
                .ok_or_else(|| Error::invalid_params("invalid UTF-16 position"))?;
            let edit = snapshot
                .rename(file, TextSize::from(offset as u32), &params.new_name)
                .map_err(|error| match error {
                    webtest_analysis::RenameError::Analysis(error) => core_error(error),
                    error => Error::invalid_params(error.to_string()),
                })?;
            let documents = edit
                .documents
                .into_iter()
                .map(|document| {
                    let source = snapshot.source(document.file).map_err(core_error)?;
                    Ok(TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: source_uri(snapshot, document.file, &document.path)?,
                            version: document
                                .version
                                .map(|version| {
                                    i32::try_from(version.0).map_err(|_| {
                                        Error::invalid_params("document version exceeds LSP range")
                                    })
                                })
                                .transpose()?,
                        },
                        edits: document
                            .edits
                            .into_iter()
                            .map(|edit| {
                                OneOf::Left(TextEdit::new(
                                    text_range_to_lsp(&source, edit.origin.range),
                                    edit.replacement,
                                ))
                            })
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Some(WorkspaceEdit {
                document_changes: Some(DocumentChanges::Edits(documents)),
                ..Default::default()
            }))
        })
        .await
    }
    pub(super) async fn document_symbols_request(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let symbols = snapshot.document_symbols(file).map_err(core_error)?;
            Ok(Some(DocumentSymbolResponse::Flat(
                symbols
                    .into_iter()
                    .filter(|symbol| symbol.kind != webtest_analysis::SymbolKind::Module)
                    .map(|symbol| symbol_information(snapshot, symbol))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect(),
            )))
        })
        .await
    }
    pub(super) async fn workspace_symbols_request(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let editor = self.documents.all().first().map_or_else(
            || self.default_editor.clone(),
            |document| document.editor.clone(),
        );
        self.query_with_progress(
            None,
            editor,
            params
                .work_done_progress_params
                .work_done_token
                .map(|token| (token, "Searching WebTest symbols".into())),
            move |snapshot| {
                let symbols = snapshot
                    .workspace_symbols(&params.query, 10_000)
                    .map_err(core_error)?;
                if symbols.truncated {
                    return Err(Error::invalid_params(
                        "symbol result exceeds the workspace query limit; narrow the query",
                    ));
                }
                Ok(Some(WorkspaceSymbolResponse::Flat(
                    symbols
                        .items
                        .into_iter()
                        .map(|symbol| symbol_information(snapshot, symbol))
                        .collect::<Result<Vec<_>>>()?
                        .into_iter()
                        .flatten()
                        .collect(),
                )))
            },
        )
        .await
    }
    pub(super) async fn inlay_hints_request(
        &self,
        params: InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(core_error)?;
            let hints = snapshot.inlay_hints(file).map_err(core_error)?;
            if hints.truncated {
                return Err(Error::invalid_params(
                    "inlay hints exceed the document query limit",
                ));
            }
            Ok(Some(
                hints
                    .items
                    .into_iter()
                    .filter_map(|hint| {
                        let position = text_range_to_lsp(
                            &source,
                            TextRange::empty(TextSize::from(hint.offset)),
                        )
                        .start;
                        (position >= params.range.start && position <= params.range.end).then_some(
                            InlayHint {
                                position,
                                label: InlayHintLabel::String(hint.label),
                                kind: Some(InlayHintKind::TYPE),
                                text_edits: None,
                                tooltip: None,
                                padding_left: Some(false),
                                padding_right: Some(false),
                                data: None,
                            },
                        )
                    })
                    .collect(),
            ))
        })
        .await
    }
    pub(super) async fn code_actions_request(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        if params.context.only.as_ref().is_some_and(|kinds| {
            !kinds
                .iter()
                .any(|kind| kind == &CodeActionKind::EMPTY || kind == &CodeActionKind::QUICKFIX)
        }) {
            return Ok(Some(Vec::new()));
        }
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(core_error)?;
            let start = synchronization::offset(&source, params.range.start)
                .ok_or_else(|| Error::invalid_params("invalid UTF-16 range"))?;
            let end = synchronization::offset(&source, params.range.end)
                .filter(|end| *end >= start)
                .ok_or_else(|| Error::invalid_params("invalid UTF-16 range"))?;
            let actions = snapshot
                .code_actions(
                    file,
                    TextRange::new((start as u32).into(), (end as u32).into()),
                )
                .map_err(core_error)?;
            if actions.truncated {
                return Err(Error::invalid_params(
                    "code actions exceed the document query limit",
                ));
            }
            let mut result = Vec::new();
            for action in actions.items {
                let mut documents = Vec::new();
                for document in action.edit.documents {
                    let source = snapshot.source(document.file).map_err(core_error)?;
                    documents.push(TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: source_uri(snapshot, document.file, &document.path)?,
                            version: document
                                .version
                                .map(|version| {
                                    i32::try_from(version.0).map_err(|_| {
                                        Error::invalid_params("document version exceeds LSP range")
                                    })
                                })
                                .transpose()?,
                        },
                        edits: document
                            .edits
                            .into_iter()
                            .map(|edit| {
                                OneOf::Left(TextEdit::new(
                                    text_range_to_lsp(&source, edit.origin.range),
                                    edit.replacement,
                                ))
                            })
                            .collect(),
                    });
                }
                result.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: action.title,
                    kind: Some(CodeActionKind::QUICKFIX),
                    edit: Some(WorkspaceEdit {
                        document_changes: Some(DocumentChanges::Edits(documents)),
                        ..Default::default()
                    }),
                    ..Default::default()
                }));
            }
            Ok(Some(result))
        })
        .await
    }
    pub(super) async fn folding_request(
        &self,
        params: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(core_error)?;
            Ok(Some(
                snapshot
                    .folding_ranges(file)
                    .map_err(core_error)?
                    .into_iter()
                    .map(|fold| {
                        let range = text_range_to_lsp(
                            &source,
                            TextRange::new(fold.range.start.into(), fold.range.end.into()),
                        );
                        FoldingRange {
                            start_line: range.start.line,
                            start_character: Some(range.start.character),
                            end_line: range.end.line,
                            end_character: Some(range.end.character),
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        }
                    })
                    .collect(),
            ))
        })
        .await
    }
    pub(super) async fn selection_request(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let file = document.file;
        self.query(Some(&document), document.editor.clone(), move |snapshot| {
            let source = snapshot.source(file).map_err(core_error)?;
            let mut results = Vec::new();
            for position in params.positions {
                let offset = synchronization::offset(&source, position)
                    .ok_or_else(|| Error::invalid_params("invalid UTF-16 position"))?;
                let selection = snapshot
                    .selection_range(file, TextSize::from(offset as u32))
                    .map_err(core_error)?;
                results.push(
                    selection
                        .map(|selection| selection_to_lsp(&source, selection))
                        .unwrap_or(SelectionRange {
                            range: Range::new(position, position),
                            parent: None,
                        }),
                );
            }
            Ok(Some(results))
        })
        .await
    }
}
fn selection_to_lsp(source: &str, selection: webtest_analysis::SelectionRange) -> SelectionRange {
    SelectionRange {
        range: text_range_to_lsp(
            source,
            TextRange::new(selection.range.start.into(), selection.range.end.into()),
        ),
        parent: selection
            .parent
            .map(|parent| Box::new(selection_to_lsp(source, *parent))),
    }
}

#[derive(Default, serde::Deserialize)]
pub(super) struct DiscoveryParams {
    uri: Option<Uri>,
    #[serde(flatten)]
    progress: WorkDoneProgressParams,
}
impl Backend {
    pub(super) async fn discover_tests(
        &self,
        params: DiscoveryParams,
    ) -> Result<serde_json::Value> {
        let editor = if let Some(uri) = params.uri {
            let path = uri
                .to_file_path()
                .ok_or_else(|| Error::invalid_params("discovery requires a file URI"))?;
            (self.editor_for_path)(&path).map_err(Error::invalid_params)?
        } else {
            self.documents.all().first().map_or_else(
                || self.default_editor.clone(),
                |document| document.editor.clone(),
            )
        };
        self.query_with_progress(
            None,
            editor,
            params
                .progress
                .work_done_token
                .map(|token| (token, "Discovering WebTest tests".into())),
            move |snapshot| {
                let discovery = snapshot.discover_tests(10_000).map_err(core_error)?;
                let mut value =
                    serde_json::to_value(&discovery).map_err(|_| Error::internal_error())?;
                if let Some(modules) = value["modules"].as_array_mut() {
                    for (module, wire) in discovery.modules.iter().zip(modules) {
                        wire["uri"] =
                            serde_json::to_value(source_uri(snapshot, module.file, &module.path)?)
                                .map_err(|_| Error::internal_error())?;
                        let source = snapshot.source(module.file).map_err(core_error)?;
                        if let Some(tests) = wire["tests"].as_array_mut() {
                            for (test, wire) in module.tests.iter().zip(tests) {
                                let range = serde_json::to_value(text_range_to_lsp(
                                    &source,
                                    test.origin.range,
                                ))
                                .map_err(|_| Error::internal_error())?;
                                wire["range"] = range.clone();
                                if let Some(variants) = wire["variants"].as_array_mut() {
                                    for variant in variants {
                                        variant["range"] = range.clone();
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(value)
            },
        )
        .await
    }
}

struct RunTask {
    id: String,
    control: Arc<ExecutionCancellation>,
    join: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
struct ExecutionCancellation(AtomicBool);
impl ExecutionCancellation {
    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}
#[async_trait::async_trait]
impl webtest_runtime::RunControl for ExecutionCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn should_capture_bindings(
        &self,
        _: &webtest_plan::PlannedTest,
        _: &webtest_plan::PlannedStep,
    ) -> bool {
        false
    }
    async fn before_step(&self, _: &webtest_plan::PlannedTest, _: &webtest_plan::PlannedStep) {}
}
struct CancelExecutionOnDrop {
    id: String,
    controls: Arc<Mutex<HashMap<String, Arc<ExecutionCancellation>>>>,
    control: Arc<ExecutionCancellation>,
}
impl Drop for CancelExecutionOnDrop {
    fn drop(&mut self) {
        self.control.cancel();
        self.controls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
#[derive(serde::Deserialize)]
pub(super) struct RunVariantsParams {
    uri: Uri,
    workspace_revision: u64,
    variants: Vec<webtest_model::TestVariantId>,
    run_id: String,
}
#[derive(serde::Deserialize)]
pub(super) struct CancelRunParams {
    run_id: String,
}
impl Backend {
    pub(super) async fn cancel_run(&self, params: CancelRunParams) {
        if let Some(control) = self
            .queries
            .run_controls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&params.run_id)
        {
            control.cancel();
        }
        for run in self
            .queries
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            if run.id == params.run_id {
                run.control.cancel();
            }
        }
    }
    pub(super) async fn run_variants(
        &self,
        params: RunVariantsParams,
    ) -> Result<serde_json::Value> {
        if params.run_id.is_empty() || params.run_id.len() > 128 || params.variants.len() > 10_000 {
            return Err(Error::invalid_params("invalid test run selection"));
        }
        let control = Arc::new(ExecutionCancellation::default());
        {
            let mut controls = self
                .queries
                .run_controls
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if self.queries.shutting_down.load(Ordering::Acquire)
                || controls.len() >= 16
                || controls.contains_key(&params.run_id)
            {
                return Err(Error::invalid_params(
                    "test run is already active or execution queue is full",
                ));
            }
            controls.insert(params.run_id.clone(), control.clone());
        }
        let _cancel = CancelExecutionOnDrop {
            id: params.run_id.clone(),
            controls: self.queries.run_controls.clone(),
            control: control.clone(),
        };
        let path = params
            .uri
            .to_file_path()
            .ok_or_else(|| Error::invalid_params("test run requires a file URI"))?;
        let editor = (self.editor_for_path)(&path).map_err(Error::invalid_params)?;
        let revision = params.workspace_revision;
        let variants = params.variants.into_iter().collect();
        let prepared = self
            .query(None, editor.clone(), move |snapshot| {
                snapshot
                    .prepare_variants(revision, &variants)
                    .map_err(editor_error)
            })
            .await?;
        if control.0.load(Ordering::Acquire) {
            return Err(Error::new(ErrorCode::RequestCancelled));
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        {
            let mut runs = self.queries.runs.lock().unwrap_or_else(|e| e.into_inner());
            runs.retain(|run| !run.join.is_finished());
            if self.queries.shutting_down.load(Ordering::Acquire)
                || runs.len() >= 16
                || runs.iter().any(|run| run.id == params.run_id)
            {
                return Err(Error::invalid_params(
                    "test run is already active or execution queue is full",
                ));
            }
            let browser = self.browser.clone();
            let execution_control = control.clone();
            let join = tokio::spawn(async move {
                let result = prepared
                    .execute(browser.as_ref(), Some(execution_control.as_ref()))
                    .await;
                let _ = sender.send(result);
            });
            runs.push(RunTask {
                id: params.run_id,
                control,
                join,
            });
        }
        let result = receiver.await.map_err(|_| Error::internal_error())?;
        for document in self.documents.all() {
            self.publish(&document.uri).await;
        }
        let mut value = serde_json::to_value(&result).map_err(|_| Error::internal_error())?;
        let editor = (self.editor_for_path)(&path).map_err(Error::invalid_params)?;
        let snapshot = editor.snapshot();
        if let Some(tests) = value["tests"].as_array_mut() {
            for (test, wire) in result.tests.iter().zip(tests) {
                if let Some(location) = &test.location
                    && let Ok(source) = snapshot.source(location.origin.file)
                    && webtest_text::SourceRevision::of(&source) == location.revision
                {
                    wire["location"]["uri"] = serde_json::to_value(source_uri(
                        &snapshot,
                        location.origin.file,
                        &location.path,
                    )?)
                    .map_err(|_| Error::internal_error())?;
                    wire["location"]["range"] =
                        serde_json::to_value(text_range_to_lsp(&source, location.origin.range))
                            .map_err(|_| Error::internal_error())?;
                }
            }
        }
        Ok(value)
    }
}

impl Backend {
    pub(super) async fn describe_project(
        &self,
        params: DiscoveryParams,
    ) -> Result<serde_json::Value> {
        let editor = if let Some(uri) = params.uri {
            let path = uri
                .to_file_path()
                .ok_or_else(|| Error::invalid_params("project description requires a file URI"))?;
            (self.editor_for_path)(&path).map_err(Error::invalid_params)?
        } else {
            self.documents.all().first().map_or_else(
                || self.default_editor.clone(),
                |document| document.editor.clone(),
            )
        };
        self.query_with_progress(
            None,
            editor,
            params
                .progress
                .work_done_token
                .map(|token| (token, "Describing WebTest declarations".into())),
            move |snapshot| {
                serde_json::to_value(snapshot.project_description(1000).map_err(core_error)?)
                    .map_err(|_| Error::internal_error())
            },
        )
        .await
    }
}
