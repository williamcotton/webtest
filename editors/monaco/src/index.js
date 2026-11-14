/** A worker owns one workspace; create separate clients for independent workspaces. */
export class EditorError extends Error {
    constructor(code, message) { super(message); this.name = 'EditorError'; this.code = code; }
}
export function createEditor({ worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' }), wasmURL = new URL('./webtest_bg.wasm', import.meta.url), onNotification = () => {} } = {}) {
    let sequence = 0, revision = 0, disposed = false, terminalError;
    const pending = new Map();
    let readyResolve, readyReject;
    const ready = new Promise((resolve, reject) => { readyResolve = resolve; readyReject = reject; });
    void ready.catch(() => {});
    function fail(error) {
        terminalError = error;
        worker.terminate();
        readyReject(error);
        for (const entry of pending.values()) { entry.cleanup(); entry.reject(error); }
        pending.clear();
    }
    worker.addEventListener('error', () => fail(new EditorError('worker_crashed', 'Editor worker crashed; create a new editor and reopen supplied inputs.')));
    worker.addEventListener('messageerror', () => fail(new EditorError('invalid_response', 'Worker response could not be cloned.')));
    worker.addEventListener('message', ({ data }) => {
        if (data?.protocol !== 1) { fail(new EditorError('unsupported_protocol', 'Expected worker protocol 1')); return; }
        if (data.notification) {
            if (data.notification === 'ready') readyResolve();
            if (data.notification === 'fatal') fail(new EditorError(data.error.code, data.error.message));
            onNotification(data); return;
        }
        const entry = pending.get(data.id);
        if (!entry) return;
        pending.delete(data.id); entry.cleanup();
        revision = Math.max(revision, data.revision ?? 0);
        if (data.ok) entry.resolve(data);
        else entry.reject(new EditorError(data.error.code, data.error.message));
    });
    worker.postMessage({ protocol: 1, id: 0, method: 'initialize', params: { wasmURL: String(wasmURL) } });
    async function request(method, params, { signal, workspaceRevision } = {}) {
        if (disposed) throw new EditorError('disposed', 'Editor is disposed');
        await ready;
        if (terminalError) throw terminalError;
        if (signal?.aborted) throw new EditorError('cancelled', 'Request cancelled');
        const id = ++sequence;
        return new Promise((resolve, reject) => {
            const abort = () => {
                worker.postMessage({ protocol: 1, id: ++sequence, method: 'cancel', params: { id } });
                pending.delete(id); signal?.removeEventListener('abort', abort);
                reject(new EditorError('cancelled', 'Request cancelled'));
            };
            pending.set(id, { resolve, reject, cleanup: () => signal?.removeEventListener('abort', abort) });
            signal?.addEventListener('abort', abort, { once: true });
            try { worker.postMessage({ protocol: 1, id, method, params, ...(workspaceRevision === undefined ? {} : { workspaceRevision }) }); }
            catch (error) { pending.delete(id); signal?.removeEventListener('abort', abort); reject(error); }
        });
    }
    return {
        request, get revision() { return revision; },
        openWorkspace: inputs => request('openWorkspace', inputs),
        updateConfiguration: config => request('updateConfiguration', config),
        updateProviderSchema: text => request('updateProviderSchema', text),
        openDocument: source => request('openDocument', source),
        updateDocument: (path, edits, version) => request('updateDocument', { path, edits, version }),
        closeDocument: path => request('closeDocument', { path }),
        removeFile: path => request('removeFile', { path }),
        query: (query, options) => request('query', query, options),
        describe: params => request('describe', params ?? {}),
        async dispose() {
            if (disposed) return;
            disposed = true; worker.terminate();
            fail(new EditorError('disposed', 'Editor is disposed'));
        },
    };
}
