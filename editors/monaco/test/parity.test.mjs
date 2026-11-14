import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import init, { EditorSession } from '../dist/webtest.js';
import { corpus } from './corpus.mjs';
test('native Rust and real WASM return identical portable DTOs', async () => {
    const requests = corpus();
    const expected = execFileSync('cargo', ['run', '--locked', '-q', '-p', 'webtest-wasm', '--example', 'editor_session'], { cwd: new URL('../../../', import.meta.url), input: requests.map(r => JSON.stringify(r)).join('\n') + '\n', encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 }).trim().split('\n').map(s => JSON.parse(s));
    await init({ module_or_path: readFileSync(new URL('../dist/webtest_bg.wasm', import.meta.url)) });
    const session = new EditorSession();
    try {
        requests.forEach((r, i) => assert.deepEqual(JSON.parse(session.request(JSON.stringify(r))), expected[i], `${r.method} ${r.params.kind ?? ''}`));
        assert.equal(expected[1].ok, true);
        assert.deepEqual(expected[1].result.value, []);
        assert.ok(expected[2].result.value.plan);
        assert.ok(expected.some(r => r.ok && Array.isArray(r.result?.value) && r.result.value.some(d => d.repair_hints?.length)));
    } finally { session.free(); }
});
