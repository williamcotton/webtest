const { test } = require('node:test');
const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const { platformTarget, parseManifest, verifyBinary, managedBinary } = require('../out/releases');
const data = Buffer.from('binary');
const artifact = { name: 'webtest-0.1.0-aarch64-apple-darwin.bin', target: 'aarch64-apple-darwin', kind: 'binary', bytes: data.length, sha256: createHash('sha256').update(data).digest('hex') };
const manifest = () => ({ schema: 1, version: '0.1.0', compatibility: { plan_format: 10, event_schema: 14, worker_protocol: 1, wasm_editor_api: 1, app_bridge_protocol: [1] }, artifacts: [{ ...artifact }] });
test('tier 1 platform mapping and unsupported platform errors', () => {
    assert.equal(platformTarget('darwin','arm64'), 'aarch64-apple-darwin');
    assert.equal(platformTarget('linux','x64'), 'x86_64-unknown-linux-gnu');
    assert.equal(platformTarget('win32','x64'), 'x86_64-pc-windows-msvc');
    assert.throws(() => platformTarget('win32','arm64'), /Unsupported/);
});
test('compatibility, traversal, duplicate and checksum rejection', () => {
    const parse = value => parseManifest(Buffer.from(JSON.stringify(value)), '0.1.0');
    assert.deepEqual(parse(manifest()).artifacts[0], artifact);
    for (const name of ['../webtest','/webtest','a/b','a\\b','..','']) {
        const m = manifest(); m.artifacts[0].name = name;
        if (name !== '..') assert.throws(() => parse(m), /Invalid/);
    }
    const incompatible = manifest(); incompatible.compatibility.plan_format = 9;
    assert.throws(() => parse(incompatible), /incompatible/);
    const duplicate = manifest(); duplicate.artifacts.push(artifact);
    assert.throws(() => parse(duplicate), /Invalid/);
    verifyBinary(data, artifact);
    assert.throws(() => verifyBinary(Buffer.from('tamper'), artifact), /checksum/);
});
test('uninstalled or corrupt managed state is ignored', async () => {
    assert.equal(await managedBinary('/path-that-does-not-exist-webtest'), undefined);
});
