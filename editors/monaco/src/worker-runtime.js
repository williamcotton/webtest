// Transport only. The Rust session validates every request and owns all semantics.
export function attachWorker(scope, initialize) {
    const queue = [], cancelled = new Set(), ids = new Set(), documents = new Set();
    const mutations = new Set(['openWorkspace', 'openDocument', 'updateDocument', 'removeFile', 'updateProviderSchema', 'updateConfiguration']);
    let session, draining = false, initialized = false, lastRevision = 0;
    const send = value => scope.postMessage(value);
    const failure = (id, code, message) => send({ protocol: 1, id, ok: false, error: { code, message } });
    async function drain() {
        if (draining || !session) return;
        draining = true;
        while (queue.length) {
            const request = queue.shift();
            // A macrotask boundary allows cancel/change messages between bounded requests.
            await new Promise(resolve => setTimeout(resolve, 0));
            ids.delete(request.id);
            if (cancelled.delete(request.id)) { failure(request.id, 'cancelled', 'Request cancelled'); continue; }
            try {
                const response = JSON.parse(session.request(JSON.stringify(request)));
                send(response);
                if (mutations.has(request.method) && (response.ok || (request.method === 'updateProviderSchema' && response.revision > lastRevision))) {
                    if (request.method === 'openWorkspace') { documents.clear(); request.params.sources.forEach(source => documents.add(source.path)); }
                    if (request.method === 'openDocument') documents.add(request.params.path);
                    if (request.method === 'removeFile') documents.delete(request.params.path);
                    send({ protocol: 1, notification: 'workspaceChanged', revision: response.revision });
                    // Coalesce publication across already queued changes, while applying every version.
                    if (!queue.some(item => mutations.has(item.method))) {
                        for (const path of documents) {
                            const diagnostics = JSON.parse(session.request(JSON.stringify({ protocol: 1, id: 0, method: 'query', params: { kind: 'diagnostics', path } })));
                            if (diagnostics.ok) send({ protocol: 1, notification: 'diagnostics', revision: response.revision, path, ...diagnostics.result });
                        }
                        const tests = JSON.parse(session.request(JSON.stringify({ protocol: 1, id: 0, method: 'query', params: { kind: 'discovery', limit: 10000 } })));
                        if (tests.ok) send({ protocol: 1, notification: 'testsChanged', revision: response.revision, value: tests.result.value });
                    }
                }
                lastRevision = response.revision ?? lastRevision;
            } catch {
                failure(request.id, 'internal_error', 'Editor failed at request ' + request.id);
                send({ protocol: 1, notification: 'fatal', error: { code: 'internal_error', message: 'Recreate the editor worker and reopen supplied inputs.' } });
                session = undefined; queue.length = 0; ids.clear(); break;
            }
        }
        draining = false;
    }
    scope.addEventListener('message', async ({ data }) => {
        if (!data || data.protocol !== 1) { failure(data?.id ?? null, 'unsupported_protocol', 'Expected worker protocol 1'); return; }
        if (!Number.isSafeInteger(data.id) || data.id < 0 || typeof data.method !== 'string') { failure(data.id ?? null, 'invalid_request', 'Invalid request envelope'); return; }
        if (data.method === 'initialize') {
            if (initialized) { failure(data.id, 'already_initialized', 'Worker already initialized'); return; }
            initialized = true;
            try { session = await initialize(data.params); send({ protocol: 1, notification: 'ready', api_version: 1 }); void drain(); }
            catch { send({ protocol: 1, notification: 'fatal', error: { code: 'initialization_failed', message: 'Unable to initialize supplied WASM asset' } }); }
            return;
        }
        if (data.method === 'cancel') {
            if (ids.has(data.params?.id)) cancelled.add(data.params.id);
            return;
        }
        if (queue.length >= 256) { failure(data.id, 'limit_exceeded', 'Worker queue limit exceeded'); return; }
        if (ids.has(data.id)) { failure(data.id, 'invalid_request', 'Duplicate queued request ID'); return; }
        // Reject large messages before retaining them; Rust applies per-field limits too.
        let encoded;
        try { encoded = JSON.stringify(data); }
        catch { failure(data.id, 'invalid_request', 'Message must be a plain JSON DTO'); return; }
        if (encoded.length > 80 * 1024 * 1024) { failure(data.id, 'limit_exceeded', 'Message limit exceeded'); return; }
        queue.push(data); ids.add(data.id); void drain();
    });
}
