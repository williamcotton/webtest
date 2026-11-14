import { test } from 'node:test';
import assert from 'node:assert/strict';
import { attachWorker } from '../src/worker-runtime.js';
import { createEditor } from '../src/index.js';
function host(initialize) {
    const output = []; let listener;
    attachWorker({ addEventListener: (_name, cb) => { listener = cb; }, postMessage: m => output.push(m) }, initialize);
    return { output, send: data => listener({ data }) };
}
const tick = () => new Promise(resolve => setTimeout(resolve, 20));
test('queue cancellation, protocol and bounded admission', async () => {
    const h = host(async () => ({ request: s => { const r = JSON.parse(s); return JSON.stringify({ protocol: 1, id: r.id, ok: true, revision: 1, result: r.params }); } }));
    await h.send({ protocol: 1, id: 0, method: 'initialize', params: {} });
    h.send({ protocol: 2, id: 1, method: 'query' });
    h.send({ protocol: 1, id: 2, method: 'query', params: {} });
    h.send({ protocol: 1, id: 3, method: 'cancel', params: { id: 2 } });
    await tick();
    assert.equal(h.output.find(r => r.id === 1).error.code, 'unsupported_protocol');
    assert.equal(h.output.find(r => r.id === 2).error.code, 'cancelled');
    const q = host(async () => new Promise(() => {}));
    for (let id = 1; id <= 257; id++) q.send({ protocol: 1, id, method: 'query', params: {} });
    assert.equal(q.output.at(-1).error.code, 'limit_exceeded');
});
test('fatal initialization is reported', async () => {
    const h = host(async () => { throw new Error('private stack'); });
    await h.send({ protocol: 1, id: 0, method: 'initialize', params: {} });
    assert.equal(h.output[0].notification, 'fatal');
    assert.equal(h.output[0].error.code, 'initialization_failed');
    assert.ok(!JSON.stringify(h.output).includes('private stack'));
});
test('client rejects pending work when worker crashes or disposes', async () => {
    const listeners = new Map();
    const worker = { addEventListener: (n, cb) => listeners.set(n, cb), postMessage: () => {}, terminate() {} };
    const editor = createEditor({ worker, wasmURL: 'https://host/editor.wasm' });
    listeners.get('message')({ data: { protocol: 1, notification: 'ready' } });
    const pending = editor.query({ kind: 'discovery', limit: 10 });
    await Promise.resolve();
    listeners.get('error')({});
    await assert.rejects(pending, { code: 'worker_crashed' });
    await editor.dispose();
    await assert.rejects(editor.query({ kind: 'discovery' }), { code: 'disposed' });
});
test('cyclic and bigint messages become typed protocol errors', async () => {
    const h = host(async () => ({ request() { throw new Error('unexpected'); } }));
    const data = { protocol: 1, id: 1, method: 'query', params: {} }; data.params.cycle = data;
    await h.send(data);
    assert.equal(h.output.at(-1).error.code, 'invalid_request');
    await h.send({ protocol: 1, id: 2, method: 'query', params: 1n });
    assert.equal(h.output.at(-1).error.code, 'invalid_request');
});
test('applies every version and coalesces diagnostic/test notifications', async () => {
    const applied = [];
    const h = host(async () => ({ request: text => {
        const r = JSON.parse(text); applied.push(r);
        return JSON.stringify({ protocol: 1, id: r.id, ok: true, revision: applied.length, result: r.method === 'query' ? { value: [], document_version: 2 } : {} });
    } }));
    await h.send({ protocol: 1, id: 0, method: 'initialize', params: {} });
    h.send({ protocol: 1, id: 1, method: 'openWorkspace', params: { sources: [{ path: '/a' }] } });
    h.send({ protocol: 1, id: 2, method: 'updateDocument', params: { path: '/a', version: 2, edits: [] } });
    await tick();
    assert.deepEqual(applied.filter(r => r.method !== 'query').map(r => r.id), [1, 2]);
    assert.equal(h.output.filter(r => r.notification === 'diagnostics').length, 1);
    assert.equal(h.output.filter(r => r.notification === 'testsChanged').length, 1);
});
test('future requests fail after fatal worker loss instead of hanging', async () => {
    const listeners = new Map();
    const worker = { addEventListener: (n, cb) => listeners.set(n, cb), postMessage() {}, terminate() {} };
    const editor = createEditor({ worker, wasmURL: 'https://host/editor.wasm' });
    listeners.get('message')({ data: { protocol: 1, notification: 'ready' } });
    listeners.get('error')({});
    await assert.rejects(editor.query({ kind: 'discovery' }), { code: 'worker_crashed' });
});
