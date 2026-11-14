import { createEditor } from '../dist/index.js';
import { registerWebTest } from '../dist/monaco.js';
const editor = createEditor();
await editor.openWorkspace({ configuration: { root: '/workspace' }, sources: [] });
window.require.config({ paths: { vs: '../node_modules/monaco-editor/min/vs' } });
window.require(['vs/editor/editor.main'], () => {
    const adapter = registerWebTest(window.monaco, editor);
    const model = monaco.editor.createModel('test "hello 😀" { let value = 1 expect value == 1 }', 'webtest', monaco.Uri.file('/workspace/hello.webtest'));
    const view = monaco.editor.create(document.querySelector('#editor'), { model, automaticLayout: true });
    document.querySelector('#format').onclick = () => view.getAction('editor.action.formatDocument').run();
    document.querySelector('#compile').onclick = async () => {
        const response = await editor.query({ kind: 'compile', path: model.uri.path });
        document.querySelector('#result').textContent = JSON.stringify(response.result.value, null, 2);
    };
    window.addEventListener('pagehide', () => { view.dispose(); model.dispose(); adapter.dispose(); void editor.dispose(); }, { once: true });
});
