import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import {
    defaultVsixPath,
    packageManifest,
} from "./package-info.mjs";

const require = createRequire(import.meta.url);
const yauzl = require("yauzl");
const vsix = path.resolve(process.argv[2] ?? defaultVsixPath);
assert.ok(fs.statSync(vsix).isFile(), `missing VSIX: ${vsix}`);

const files = await readZip(vsix);
for (const required of [
    "extension/package.json",
    "extension/out/extension.js",
    "extension/language-configuration.json",
    "extension/LICENSE.txt",
]) {
    assert.ok(files.has(required), `${required} is missing from the VSIX`);
}

const manifest = JSON.parse(files.get("extension/package.json").toString("utf8"));
assert.equal(manifest.name, packageManifest.name);
assert.equal(manifest.publisher, packageManifest.publisher);
assert.equal(manifest.version, packageManifest.version);
assert.equal(manifest.main, "./out/extension.js");
assert.ok(manifest.activationEvents.includes("onLanguage:webtest"));
assert.ok(manifest.activationEvents.includes("onDebug"));
assert.deepEqual(
    manifest.contributes.commands.map(({ command }) => command).sort(),
    ["webtest.debugCurrentFile", "webtest.runCurrentFile"],
    "only extension-owned commands should be registered; webtest.runFile belongs to LSP",
);
assert.ok(manifest.contributes.semanticTokenScopes[0].scopes.keyword.length > 0);
assert.equal(manifest.contributes.debuggers[0].type, "webtest");
assert.equal(manifest.contributes.debuggers[0].initialConfigurations[0].headed, true);

const extension = files.get("extension/out/extension.js").toString("utf8");
for (const marker of [
    "webtest.runCurrentFile",
    "webtest.debugCurrentFile",
    "registerDebugAdapterDescriptorFactory",
    'args: ["lsp"]',
    'const args = ["dap"]',
    'args.push("--project", program)',
    'path.join(os.homedir(), ".cargo")',
    "sourceText",
]) {
    assert.ok(extension.includes(marker), `compiled extension is missing ${marker}`);
}

console.log(`VSIX smoke passed: ${path.basename(vsix)} (${files.size} files)`);

function readZip(filename) {
    return new Promise((resolve, reject) => {
        yauzl.open(filename, { lazyEntries: true }, (openError, zip) => {
            if (openError) {
                reject(openError);
                return;
            }
            const files = new Map();
            zip.on("error", reject);
            zip.on("end", () => resolve(files));
            zip.on("entry", (entry) => {
                if (/\/$/.test(entry.fileName)) {
                    zip.readEntry();
                    return;
                }
                zip.openReadStream(entry, (streamError, stream) => {
                    if (streamError) {
                        reject(streamError);
                        return;
                    }
                    const chunks = [];
                    stream.on("data", (chunk) => chunks.push(chunk));
                    stream.on("error", reject);
                    stream.on("end", () => {
                        files.set(entry.fileName, Buffer.concat(chunks));
                        zip.readEntry();
                    });
                });
            });
            zip.readEntry();
        });
    });
}
