const { createEditor } = await import(location.search.includes('bundled') ? '/test/bundled-index.js' : '/dist/index.js');
import { registerWebTest } from '/dist/monaco.js';
import { corpus } from '/test/corpus.mjs';
const assert = (value, message) => { if (!value) throw new Error(message); };
const wait = async predicate => { const end = Date.now() + 15000; while (!predicate()) { if (Date.now() > end) throw new Error('Timed out waiting for markers'); await new Promise(r => setTimeout(r, 20)); } };
let editor, adapter, view;
try {
    editor = createEditor({ worker: new Worker(location.search.includes('bundled') ? '/test/bundled-worker.js' : '/dist/worker.js', { type: 'module' }), wasmURL: '/dist/webtest_bg.wasm' });
    const expected = await (await fetch('/native-parity.json')).json();
    for (const [i, request] of corpus().entries()) {
        let actual;
        try { const r = await editor.request(request.method, request.params); actual = r; }
        catch (error) { assert(expected[i].ok === false && expected[i].error.code === error.code, `error parity ${i}: ${error}`); continue; }
        // IDs/revisions are transport-owned; exact semantic payloads must agree.
        assert(JSON.stringify(actual.result) === JSON.stringify(expected[i].result), `native/worker parity ${i}`);
    }
    await editor.openWorkspace({ configuration: { root: '/workspace', toolchain_identity: 'browser-test' }, sources: [] });
    await new Promise(resolve => { window.require.config({ paths: { vs: '/node_modules/monaco-editor/min/vs' } }); window.require(['vs/editor/editor.main'], resolve); });
    const monaco = window.monaco;
    const providers = {};
    const languageHost = new Proxy(monaco.languages, { get(target, name) {
        const value = target[name];
        if (typeof name === 'string' && name.startsWith('register') && typeof value === 'function') {
            return (language, provider) => { providers[name] = provider; return value.call(target, language, provider); };
        }
        return value;
    } });
    adapter = registerWebTest({ ...monaco, languages: languageHost }, editor, { onError: error => { document.querySelector('#details').textContent += String(error) + '\n'; } });
    const model = monaco.editor.createModel('test "😀" { let value = 1 expect value == 1 }', 'webtest', monaco.Uri.file('/workspace/editor.webtest'));
    model.setValue('test \"rapid\" {}');
    model.setValue('test \"😀\" { let value = 1 expect value == 1 }');
    view = monaco.editor.create(document.querySelector('#editor'), { model, automaticLayout: true });
    await view.getAction('editor.action.formatDocument').run();
    assert(model.getValue().includes('\n'), 'real Monaco formatter');
    const symbols = await providers.registerDocumentSymbolProvider.provideDocumentSymbols(model);
    assert(symbols.some(s => s.name.includes('😀')), 'Monaco symbols');
    const tokens = await providers.registerDocumentSemanticTokensProvider.provideDocumentSemanticTokens(model);
    assert(tokens.data.length > 0, 'Monaco semantic tokens');
    const folding = await providers.registerFoldingRangeProvider.provideFoldingRanges(model);
    assert(folding.length > 0, 'Monaco folding');
    const selection = await providers.registerSelectionRangeProvider.provideSelectionRanges(model, [{ lineNumber: 2, column: 10 }]);
    assert(selection[0].length > 0, 'Monaco selection');
    const rename = await providers.registerRenameProvider.provideRenameEdits(model, { lineNumber: 2, column: 10 }, 'renamed');
    assert(rename.edits.length > 0, 'Monaco rename edits');
    model.applyEdits(rename.edits.map(e => e.textEdit));
    assert(model.getValue().includes('renamed'), 'Monaco rename applied');
    await new Promise(resolve => setTimeout(resolve, 100));
    const hover = await providers.registerHoverProvider.provideHover(model, { lineNumber: 2, column: 10 });
    assert(hover?.contents.length > 0, 'Monaco hover');
    const hints = await providers.registerInlayHintsProvider.provideInlayHints(model, model.getFullModelRange());
    assert(hints.hints.length > 0, 'Monaco inlay hints');
    const actions = await providers.registerCodeActionProvider.provideCodeActions(model, model.getFullModelRange());
    assert(Array.isArray(actions.actions), 'Monaco actions');
    const plan = await editor.query({ kind: 'compile', path: model.uri.path });
    assert(plan.result.value.plan && plan.result.value.executable_on_wasm === false, 'plan capability boundary');
    model.setValue('test "bad😀" { let value = missing }');
    await wait(() => monaco.editor.getModelMarkers({ owner: 'webtest', resource: model.uri }).length > 0);
    model.setValue('test "fixed" { let value = 1 expect value == 1 }');
    await wait(() => monaco.editor.getModelMarkers({ owner: 'webtest', resource: model.uri }).length === 0);
    model.setValue('test \"completion\" { server { http. } }');
    await new Promise(resolve => setTimeout(resolve, 100));
    const completion = await providers.registerCompletionItemProvider.provideCompletionItems(model, { lineNumber: 1, column: model.getValue().indexOf('http.') + 6 });
    assert(completion.suggestions.length > 0, 'Monaco completion');
    model.dispose();
    const reopened = monaco.editor.createModel('test "reopened" {}', 'webtest', monaco.Uri.file('/workspace/editor.webtest'));
    const reopenedSymbols = await providers.registerDocumentSymbolProvider.provideDocumentSymbols(reopened);
    assert(reopenedSymbols.some(s => s.name.includes('reopened')), 'same-URI dispose/reopen ordering');
    monaco.editor.setModelLanguage(reopened, 'plaintext');
    monaco.editor.setModelLanguage(reopened, 'webtest');
    const resynchronized = await providers.registerDocumentSymbolProvider.provideDocumentSymbols(reopened);
    assert(resynchronized.length > 0, 'language lifecycle resynchronization');
    reopened.dispose();
    adapter.dispose(); adapter = undefined;
    document.querySelector('#result').textContent = 'Portable editor passed';
} catch (error) {
    document.querySelector('#result').textContent = 'Portable editor failed';
    document.querySelector('#details').textContent += error.stack ?? String(error);
    await fetch('/test-result?error=' + encodeURIComponent(error.stack ?? String(error)));
} finally { view?.dispose(); adapter?.dispose(); await editor?.dispose(); }
