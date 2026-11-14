// Vite-style asset imports. Other bundlers can supply equivalent self-hosted URLs.
import { createEditor } from '@webtest/editor';
import { registerWebTest } from '@webtest/editor/monaco';
import WebTestWorker from '@webtest/editor/worker?worker';
import wasmURL from '@webtest/editor/wasm?url';
export async function mount(monaco) {
    const editor = createEditor({ worker: new WebTestWorker(), wasmURL });
    await editor.openWorkspace({ configuration: { root: '/workspace' }, sources: [] });
    const adapter = registerWebTest(monaco, editor);
    return { editor, dispose: async () => { adapter.dispose(); await editor.dispose(); } };
}
// The worker import bundles its module graph; the WASM URL stays an explicit asset.
