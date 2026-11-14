import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import * as path from 'node:path';
import { verify } from 'sigstore';

export const repository = 'williamcotton/webtest';
export const releaseOrigin = `https://github.com/${repository}/releases/download/`;
export interface Artifact { name: string; target: string; kind: 'binary' | 'archive'; sha256: string; bytes: number }
export interface Manifest { schema: 1; version: string; artifacts: Artifact[]; compatibility: { plan_format: number; worker_protocol: number; wasm_editor_api: number; event_schema: number; app_bridge_protocol: number[] } }
export function platformTarget(platform = process.platform, arch = process.arch): string {
    const target = new Map([
        ['darwin-x64', 'x86_64-apple-darwin'], ['darwin-arm64', 'aarch64-apple-darwin'],
        ['linux-x64', 'x86_64-unknown-linux-gnu'], ['linux-arm64', 'aarch64-unknown-linux-gnu'],
        ['win32-x64', 'x86_64-pc-windows-msvc'],
    ]).get(`${platform}-${arch}`);
    if (!target) throw new Error(`Unsupported platform ${platform}/${arch}. Build webtest from source and configure webtest.serverPath.`);
    return target;
}
export async function download(url: string, limit: number): Promise<Buffer> {
    if (!url.startsWith(releaseOrigin)) throw new Error('Release URL is outside the trusted repository');
    const response = await fetch(url, { signal: AbortSignal.timeout(120_000) });
    if (!response.ok || new URL(response.url).protocol !== 'https:' || !response.body) throw new Error(`Release download failed: ${response.status}`);
    const chunks: Buffer[] = []; let bytes = 0;
    try {
        for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) {
            bytes += chunk.length;
            if (bytes > limit) throw new Error('Release download exceeds byte limit');
            chunks.push(Buffer.from(chunk));
        }
    } catch (error) { await response.body.cancel().catch(() => {}); throw error; }
    return Buffer.concat(chunks);
}
export function parseManifest(bytes: Buffer, version: string): Manifest {
    const manifest = JSON.parse(bytes.toString('utf8')) as Manifest;
    if (!/^\d+\.\d+\.\d+$/.test(version) || manifest.schema !== 1 || manifest.version !== version || !Array.isArray(manifest.artifacts) || manifest.artifacts.length > 32) throw new Error('Unsupported release manifest');
    const c = manifest.compatibility;
    if (!c || c.plan_format !== 10 || c.event_schema !== 14 || c.worker_protocol !== 1 || c.wasm_editor_api !== 1 || !Array.isArray(c.app_bridge_protocol) || !c.app_bridge_protocol.includes(1)) throw new Error('Release is incompatible with this extension');
    const names = new Set<string>();
    for (const artifact of manifest.artifacts) {
        if (!/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,180}$/.test(artifact.name) || names.has(artifact.name) || !/^[a-f0-9]{64}$/.test(artifact.sha256) || !Number.isSafeInteger(artifact.bytes) || artifact.bytes <= 0 || artifact.bytes > 256 * 1024 * 1024) throw new Error('Invalid release artifact');
        names.add(artifact.name);
    }
    return manifest;
}
export function verifyBinary(binary: Buffer, artifact: Artifact): void {
    if (binary.length !== artifact.bytes || createHash('sha256').update(binary).digest('hex') !== artifact.sha256) throw new Error('Release checksum mismatch');
}
export async function installRelease(storage: string, version: string): Promise<string> {
    if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('An exact x.y.z release version is required');
    const base = `${releaseOrigin}v${version}/`;
    const bytes = await download(base + 'manifest.json', 1024 * 1024);
    const bundle = JSON.parse((await download(base + 'manifest.sigstore.json', 1024 * 1024)).toString('utf8'));
    await verify(bundle, bytes, {
        certificateIssuer: 'https://token.actions.githubusercontent.com',
        certificateIdentityURI: `https://github.com/${repository}/.github/workflows/release.yml@refs/tags/v${version}`,
        tlogThreshold: 1,
    });
    const manifest = parseManifest(bytes, version), target = platformTarget();
    const artifact = manifest.artifacts.find(a => a.kind === 'binary' && a.target === target);
    if (!artifact) throw new Error('Release has no supported platform binary');
    const binary = await download(base + artifact.name, artifact.bytes);
    verifyBinary(binary, artifact);
    await fs.mkdir(storage, { recursive: true });
    const directory = await fs.mkdtemp(path.join(storage, '.install-'));
    const executable = process.platform === 'win32' ? 'webtest.exe' : 'webtest';
    const candidate = path.join(directory, executable);
    // Raw binaries avoid all archive extraction, symlink, and traversal authority.
    try {
        await fs.writeFile(candidate, binary, { flag: 'wx', mode: 0o755 });
        await fs.writeFile(path.join(directory, 'receipt.json'), JSON.stringify({ version, target, artifact }), { flag: 'wx' });
        const final = path.join(storage, `${version}-${target}-${artifact.sha256.slice(0,16)}`);
        try { await fs.rename(directory, final); }
        catch (error) { // A verified immutable installation may already exist.
            const existing = await fs.readFile(path.join(final, executable)).catch(() => undefined);
            if (!existing) throw error;
            verifyBinary(existing, artifact);
            await fs.rm(directory, { recursive: true, force: true });
        }
        // Pointer activation preserves the previous installation for rollback.
        const pointer = path.join(storage, `.active-${Date.now()}-${process.pid}.json`);
        await fs.writeFile(pointer, JSON.stringify({ directory: path.basename(final) }), { flag: 'wx' });
        await fs.rename(pointer, path.join(storage, 'active.json'));
        return path.join(final, executable);
    } catch (error) { await fs.rm(directory, { recursive: true, force: true }); throw error; }
}
export async function managedBinary(storage: string): Promise<string | undefined> {
    try {
        const active = JSON.parse(await fs.readFile(path.join(storage, 'active.json'), 'utf8'));
        if (typeof active.directory !== 'string' || !/^[a-zA-Z0-9._-]+$/.test(active.directory)) return undefined;
        const directory = path.join(storage, active.directory);
        const receipt = JSON.parse(await fs.readFile(path.join(directory, 'receipt.json'), 'utf8'));
        if (receipt.target !== platformTarget()) return undefined;
        const executable = path.join(directory, process.platform === 'win32' ? 'webtest.exe' : 'webtest');
        verifyBinary(await fs.readFile(executable), receipt.artifact);
        return executable;
    } catch { return undefined; }
}
