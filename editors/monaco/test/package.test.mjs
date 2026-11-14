import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { build } from 'esbuild';
test('packed ESM exports, declarations and host-controlled bundled assets', async () => {
    const temp = await mkdtemp(join(tmpdir(), 'webtest-package-'));
    try {
        const packageRoot = new URL('../', import.meta.url);
        const packed = JSON.parse(execFileSync('npm', ['pack', '--json', '--ignore-scripts', '--pack-destination', temp], { cwd: packageRoot, encoding: 'utf8' }))[0];
        const files = packed.files.map(f => f.path);
        for (const file of ['dist/index.js', 'dist/index.d.ts', 'dist/monaco.js', 'dist/monaco.d.ts', 'dist/worker.js', 'dist/webtest.js', 'dist/webtest_bg.wasm', 'LICENSE', 'THIRD_PARTY_NOTICES']) assert.ok(files.includes(file), file);
        assert.ok(!files.some(f => /node_modules|test\/|src\/|\.env/.test(f)));
        await writeFile(join(temp, 'package.json'), '{"type":"module","private":true}');
        execFileSync('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund', join(temp, packed.filename)], { cwd: temp, stdio: 'pipe' });
        await symlink(new URL('../node_modules/monaco-editor', import.meta.url), join(temp, 'node_modules/monaco-editor'), 'dir');
        await writeFile(join(temp, 'consumer.mts'), `import { createEditor, type Editor } from '@webtest/editor';\nimport { registerWebTest } from '@webtest/editor/monaco';\nconst editor: Editor = createEditor();\nvoid editor.openWorkspace({configuration:{root:'/workspace'},sources:[]});\nvoid registerWebTest;\n`);
        execFileSync(new URL('../node_modules/.bin/tsc', import.meta.url).pathname, ['--noEmit', '--strict', '--target', 'ES2022', '--module', 'NodeNext', '--moduleResolution', 'NodeNext', join(temp, 'consumer.mts')], { cwd: temp, stdio: 'pipe' });
        await build({ entryPoints: [join(temp, 'consumer.mts')], outfile: join(temp, 'bundle.js'), bundle: true, format: 'esm', platform: 'browser' });
        const bundled = await readFile(join(temp, 'bundle.js'), 'utf8');
        assert.ok(bundled.includes('worker.js'));
        assert.ok(bundled.includes('webtest_bg.wasm'));
        assert.ok(!bundled.includes('eval('));
        // Browser integration loads this bundled core beside the original worker/assets.
        await build({ entryPoints: [new URL('../dist/worker.js', import.meta.url).pathname], outfile: new URL('./bundled-worker.js', import.meta.url).pathname, bundle: true, format: 'esm', platform: 'browser' });
        await build({ entryPoints: [new URL('../dist/index.js', import.meta.url).pathname], outfile: new URL('./bundled-index.js', import.meta.url).pathname, bundle: true, format: 'esm', platform: 'browser' });
    } finally { await rm(temp, { recursive: true, force: true }); }
});
