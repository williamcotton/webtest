import init, { EditorSession } from './webtest.js';
import { attachWorker } from './worker-runtime.js';
attachWorker(self, async ({ wasmURL }) => {
    // This is the sole asset fetch. Sources/configuration/schemas are supplied by the host.
    await init({ module_or_path: wasmURL });
    return new EditorSession();
});
