import { createServer } from 'node:http';
import { readFile, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawn } from 'node:child_process';
import { corpus } from './corpus.mjs';
const root = fileURLToPath(new URL('../', import.meta.url)), repository = fileURLToPath(new URL('../../../', import.meta.url));
const binary = process.env.WEBTEST_BINARY ?? join(repository, 'target/debug/webtest');
const expected = execFileSync('cargo', ['run', '--locked', '-q', '-p', 'webtest-wasm', '--example', 'editor_session'], { cwd: repository, input: corpus().map(r => JSON.stringify(r)).join('\n') + '\n', encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 }).trim().split('\n').map(JSON.parse);
const server = createServer(async (req, res) => {
    try {
        if (req.url.startsWith('/test-result?')) { console.error(new URL(req.url, 'http://localhost').searchParams.get('error')); res.end('ok'); return; }
        if (req.url === '/native-parity.json') { res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify(expected)); return; }
        const file = resolve(root, '.' + new URL(req.url, 'http://localhost').pathname);
        if (!file.startsWith(root)) { res.writeHead(403).end(); return; }
        res.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; worker-src 'self' blob:; font-src 'self' data:; img-src 'self' data:");
        res.setHeader('Content-Type', ({ '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css' })[extname(file)] ?? 'application/octet-stream');
        res.end(await readFile(file));
    } catch { res.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const temp = await mkdtemp(join(tmpdir(), 'webtest-monaco-'));
try {
    execFileSync(binary, ['init', temp], { stdio: 'pipe' });
    await writeFile(join(temp, 'webtest.toml'), '[project]\ntest_roots = ["tests"]\n');
    const url = `http://127.0.0.1:${server.address().port}/test/browser.html`;
    await writeFile(join(temp, 'tests/example.webtest'), ["", "?bundled"].map(suffix => `test "real WASM worker and Monaco ${suffix}" { browser { open ${JSON.stringify(url + suffix)} expect text("Portable editor passed").visible } }`).join("\n"));
    execFileSync(binary, ['fmt', temp], { stdio: 'pipe' });
    execFileSync(binary, ['check', temp], { stdio: 'inherit' });
    const code = await new Promise(resolve => { const child = spawn(binary, ['test', temp, '--reporter', 'json'], { stdio: 'inherit' }); child.on('exit', resolve); });
    if (code !== 0) process.exitCode = 1;
} finally { await new Promise(resolve => server.close(resolve)); await rm(temp, { recursive: true, force: true }); }
