const position = p => ({ line: p.lineNumber - 1, character: p.column - 1 });
const webRange = r => ({ start: { line: r.startLineNumber - 1, character: r.startColumn - 1 }, end: { line: r.endLineNumber - 1, character: r.endColumn - 1 } });
const range = r => ({ startLineNumber: r.start.line + 1, startColumn: r.start.character + 1, endLineNumber: r.end.line + 1, endColumn: r.end.character + 1 });
const tokenTypes = ['keyword', 'string', 'comment', 'function', 'variable', 'property', 'type', 'parameter', 'namespace', 'fixture', 'test'];
export function registerWebTest(monaco, editor, { ownsModel = () => true, onError = console.error } = {}) {
    const disposables = [], models = new Map(), documentChains = new Map();
    let disposed = false, refreshTimer, generation = 0;
    const pathOf = model => model.uri.path;
    function enqueue(state, operation) {
        state.pending = state.pending.then(operation).catch(onError);
        documentChains.set(state.path, state.pending);
        return state.pending;
    }
    async function query(model, kind, extra = {}, token) {
        if (!models.has(model)) return null;
        await Promise.all([...models.values()].map(s => s.pending));
        const currentGeneration = generation;
        const version = model.getVersionId(), controller = new AbortController();
        const listener = token?.onCancellationRequested(() => controller.abort());
        if (token?.isCancellationRequested) controller.abort();
        try {
            const response = await editor.query({ kind, path: pathOf(model), ...extra }, { signal: controller.signal });
            if (disposed || currentGeneration !== generation || response.revision !== editor.revision || model.isDisposed() || model.getVersionId() !== version || response.result.document_version !== version) return null;
            return response.result.value;
        } finally { listener?.dispose(); }
    }
    function refresh() {
        generation++;
        clearTimeout(refreshTimer);
        refreshTimer = setTimeout(async () => {
            try {
                await Promise.all([...models.values()].map(s => s.pending));
                for (const model of models.keys()) {
                    const diagnostics = await query(model, 'diagnostics');
                    if (diagnostics) monaco.editor.setModelMarkers(model, 'webtest', diagnostics.map(d => ({ ...range(d.range), message: d.message, code: d.code, source: 'WebTest', severity: ({ error: 8, warning: 4, information: 2, hint: 1 })[d.severity] })));
                }
            } catch (error) { if (!disposed) onError(error); }
        }, 30);
    }
    function attach(model) {
        if (model.getLanguageId() !== 'webtest' || !ownsModel(model) || models.has(model)) return;
        const path = pathOf(model);
        const source = { path, uri: model.uri.toString(), text: model.getValue(), version: model.getVersionId() };
        const state = { path, pending: documentChains.get(path) ?? Promise.resolve(), listeners: [] };
        models.set(model, state);
        enqueue(state, () => editor.openDocument(source));
        state.listeners.push(model.onDidChangeContent(event => {
            const edits = [...event.changes].sort((a, b) => b.rangeOffset - a.rangeOffset).map(c => ({ range: webRange(c.range), text: c.text }));
            enqueue(state, () => editor.updateDocument(pathOf(model), edits, event.versionId)); refresh();
        }));
        state.listeners.push(model.onWillDispose(() => detach(model))); 
        refresh();
    }
    function detach(model) {
        const state = models.get(model);
        if (!state) return;
        models.delete(model); state.listeners.forEach(d => d.dispose());
        if (!model.isDisposed()) monaco.editor.setModelMarkers(model, 'webtest', []);
        enqueue(state, () => editor.closeDocument(state.path)); refresh();
    }
    const uri = path => [...models.keys()].find(model => pathOf(model) === path)?.uri ?? monaco.Uri.from({ scheme: 'file', path });
    const edits = edit => ({ edits: edit.documents.flatMap(d => d.edits.map(e => ({ resource: uri(d.path), versionId: d.version ?? undefined, textEdit: { range: range(e.origin.range), text: e.replacement } }))) });
    monaco.languages.register({ id: 'webtest', extensions: ['.webtest'], aliases: ['WebTest'] });
    disposables.push(monaco.languages.setLanguageConfiguration('webtest', { comments: { lineComment: '//' }, brackets: [['{', '}'], ['[', ']'], ['(', ')']] }));
    const register = (name, provider) => disposables.push(monaco.languages[name]('webtest', provider));
    const format = async (model, _options, token) => { const text = await query(model, 'format', {}, token); return text == null ? [] : [{ range: model.getFullModelRange(), text }]; };
    register('registerDocumentFormattingEditProvider', { provideDocumentFormattingEdits: format });
    register('registerDocumentRangeFormattingEditProvider', { provideDocumentRangeFormattingEdits: (m, _r, o, t) => format(m, o, t) });
    register('registerCompletionItemProvider', { triggerCharacters: ['.', ':'], async provideCompletionItems(m, p, _c, t) {
        const result = await query(m, 'completions', { position: position(p) }, t);
        return { suggestions: (result ?? []).map(c => ({ label: c.label, detail: c.detail, documentation: c.documentation, insertText: c.insert_text, sortText: c.sort_text, filterText: c.filter_text, kind: ({ function: 1, variable: 4, keyword: 17, namespace: 8, module: 8, type: 5, fixture: 5, parameter: 4, property: 9 })[c.kind], range: c.replacement_range?.range ? range(c.replacement_range.range) : { startLineNumber: p.lineNumber, endLineNumber: p.lineNumber, startColumn: m.getWordUntilPosition(p).startColumn, endColumn: p.column }, additionalTextEdits: c.additional_edits.map(e => ({ range: range(e.origin.range), text: e.replacement })) })) };
    } });
    register('registerSignatureHelpProvider', { signatureHelpTriggerCharacters: ['(', ','], async provideSignatureHelp(m, p, t) {
        const s = await query(m, 'signature', { position: position(p) }, t);
        return s && { value: { signatures: [{ label: s.label, documentation: s.documentation, parameters: s.parameters.map(p => ({ label: p.label, documentation: p.documentation })) }], activeSignature: 0, activeParameter: s.active_parameter }, dispose() {} };
    } });
    register('registerHoverProvider', { async provideHover(m, p, t) { const h = await query(m, 'hover', { position: position(p) }, t); return h && { range: range(h.range), contents: [{ value: h.contents }] }; } });
    register('registerDocumentSymbolProvider', { async provideDocumentSymbols(m, t) {
        return (await query(m, 'document_symbols', {}, t) ?? []).filter(s => s.location).map(s => ({ name: s.name, detail: s.documentation, kind: ({ function: 11, test: 11, fixture: 11, binding: 12, parameter: 12, type: 4, import: 1 })[s.kind] ?? 12, tags: [], range: range(s.full_origin?.range ?? s.location.origin.range), selectionRange: range(s.location.origin.range) }));
    } });
    register('registerDefinitionProvider', { async provideDefinition(m, p, t) { const d = await query(m, 'definition', { position: position(p) }, t); return d && [{ originSelectionRange: range(d.origin.range), uri: uri(d.target.path), range: range(d.target_range.range), targetSelectionRange: range(d.target.origin.range) }]; } });
    register('registerReferenceProvider', { async provideReferences(m, p, c, t) { const refs = await query(m, 'references', { position: position(p), include_declaration: c.includeDeclaration, limit: 10000 }, t); return refs?.items.map(r => ({ uri: uri(r.location.path), range: range(r.location.origin.range) })) ?? []; } });
    register('registerRenameProvider', { async provideRenameEdits(m, p, name, t) { const edit = await query(m, 'rename', { position: position(p), name }, t); return edit && edits(edit); } });
    register('registerCodeActionProvider', { async provideCodeActions(m, r, _c, t) { const a = await query(m, 'code_actions', { range: webRange(r) }, t); return { actions: a?.items.map(a => ({ title: a.title, kind: 'quickfix', edit: edits(a.edit) })) ?? [], dispose() {} }; } });
    register('registerInlayHintsProvider', { async provideInlayHints(m, r, t) { const h = await query(m, 'inlay_hints', {}, t); return { hints: h?.items.filter(h => h.position.line + 1 >= r.startLineNumber && h.position.line + 1 <= r.endLineNumber).map(h => ({ position: { lineNumber: h.position.line + 1, column: h.position.character + 1 }, label: h.label, kind: 1 })) ?? [], dispose() {} }; } });
    register('registerFoldingRangeProvider', { async provideFoldingRanges(m, _c, t) { return (await query(m, 'folding', {}, t) ?? []).map(f => ({ start: f.range.range.start.line + 1, end: f.range.range.end.line + 1 })); } });
    register('registerSelectionRangeProvider', { async provideSelectionRanges(m, positions, t) { return Promise.all(positions.map(async p => { let item = await query(m, 'selection', { position: position(p) }, t); const chain = []; while (item) { chain.push({ range: range(item.range.range) }); item = item.parent; } return chain; })); } });
    register('registerDocumentSemanticTokensProvider', {
        getLegend: () => ({ tokenTypes, tokenModifiers: ['declaration', 'readonly', 'deprecated'] }), releaseDocumentSemanticTokens() {},
        async provideDocumentSemanticTokens(m, _last, t) {
            const tokens = await query(m, 'semantic_tokens', {}, t), data = []; let lastLine = 0, lastCharacter = 0;
            for (const token of tokens ?? []) for (let line = token.range.start.line; line <= token.range.end.line; line++) {
                const start = line === token.range.start.line ? token.range.start.character : 0;
                const end = line === token.range.end.line ? token.range.end.character : m.getLineLength(line + 1);
                if (end <= start) continue;
                data.push(line - lastLine, line === lastLine ? start - lastCharacter : start, end - start, tokenTypes.indexOf(token.kind), (token.modifiers.declaration ? 1 : 0) | (token.modifiers.readonly ? 2 : 0) | (token.modifiers.deprecated ? 4 : 0)); lastLine = line; lastCharacter = start;
            }
            return { data: new Uint32Array(data) };
        },
    });
    disposables.push(monaco.editor.onDidCreateModel(attach), monaco.editor.onDidChangeModelLanguage(({ model }) => { detach(model); attach(model); }));
    monaco.editor.getModels().forEach(attach);
    return { refresh, dispose() { disposed = true; clearTimeout(refreshTimer); disposables.forEach(d => d.dispose()); for (const [m, s] of models) { s.listeners.forEach(d => d.dispose()); if (!m.isDisposed()) monaco.editor.setModelMarkers(m, 'webtest', []); enqueue(s, () => editor.closeDocument(pathOf(m))); } models.clear(); } };
}
