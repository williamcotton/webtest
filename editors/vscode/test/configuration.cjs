const assert = require("node:assert/strict");
const { test } = require("node:test");
const Module = require("node:module");

const disposable = () => ({ dispose() {} });

async function harness(settings = {}) {
    const commands = new Map();
    const uri = path => ({ scheme: "file", fsPath: path, toString: () => `file://${path}` });
    const document = { languageId: "webtest", uri: uri("/project/other.webtest"), getText: () => 'test "other" {}' };
    const folder = { uri: uri("/project") };
    let provider, factory, launched, server;
    const vscode = {
        Uri: { file: uri },
        window: { activeTextEditor: { document } },
        workspace: {
            textDocuments: [document], workspaceFolders: [folder],
            getWorkspaceFolder: () => folder,
            getConfiguration: () => ({
                inspect: () => ({ workspaceValue: "/fake/webtest" }),
                get: (key, fallback) => settings[key] ?? fallback,
            }),
            createFileSystemWatcher: disposable,
        },
        commands: { registerCommand: (name, handler) => { commands.set(name, handler); return disposable(); } },
        debug: {
            registerDebugConfigurationProvider: (_type, value) => { provider = value; return disposable(); },
            registerDebugAdapterDescriptorFactory: (_type, value) => { factory = value; return disposable(); },
            startDebugging: async (_folder, value) => { launched = value; return true; },
        },
        DebugAdapterExecutable: class { constructor(command, args, options) { Object.assign(this, { command, args, options }); } },
    };
    class LanguageClient {
        constructor(_id, _name, options) { server = options; }
        onNotification() { return disposable(); }
        async start() {}
    }
    const original = Module._load;
    Module._load = function(name, ...args) {
        if (name === "vscode") { return vscode; }
        if (name === "vscode-languageclient/node") { return { LanguageClient }; }
        if (name === "./testing") { return { registerTesting() {} }; }
        return original.call(this, name, ...args);
    };
    try {
        for (const module of ["../out/extension.js", "../out/configuration.js"]) { delete require.cache[require.resolve(module)]; }
        const context = { globalStorageUri: uri("/storage"), subscriptions: [] };
        await require("../out/extension.js").activate(context);
    } finally { Module._load = original; }
    return { commands, provider, factory, folder, document, server, get launched() { return launched; } };
}

test("extension settings supply zero-config current-file and Testing view debug launches", async () => {
    const h = await harness({ "debug.stopOnEntry": true });
    const implicit = h.provider.resolveDebugConfiguration(h.folder, {});
    assert.equal(implicit.stopOnEntry, true);
    assert.equal(implicit.program, "${file}");
    await h.commands.get("webtest.debugCurrentFile")();
    assert.equal(h.launched.program, "/project/other.webtest");
    assert.equal(h.launched.stopOnEntry, true);
    // Testing view supplies a program and selected variant, without a launch file.
    const selected = { type: "webtest", request: "launch", name: "Debug case", program: "/project/cases.webtest", variants: ["selected"] };
    assert.equal(h.provider.resolveDebugConfiguration(h.folder, selected).stopOnEntry, true);
    assert.deepEqual(selected.variants, ["selected"]);
    assert.equal(h.provider.resolveDebugConfiguration(h.folder, { ...selected, stopOnEntry: false }).stopOnEntry, false);
});

test("language server and DAP inherit configured environment; launch overrides win", async () => {
    const before = process.env.PATH;
    const h = await harness({ environment: { PATH: "/custom/bin:${env:PATH}", WEBTEST_EXAMPLE: "configured" } });
    assert.equal(h.server.options.env.PATH, `/custom/bin:${before ?? ""}`);
    const adapter = h.factory.createDebugAdapterDescriptor({
        workspaceFolder: h.folder,
        configuration: { program: "/project/other.webtest", env: { WEBTEST_EXAMPLE: "launch" } },
    });
    assert.equal(adapter.options.env.PATH, h.server.options.env.PATH);
    assert.equal(adapter.options.env.WEBTEST_EXAMPLE, "launch");
    assert.equal(process.env.PATH, before, "configuration must not mutate the extension host environment");
});

test("unconfigured debug sessions retain breakpoint-based behavior", async () => {
    const h = await harness();
    assert.equal(h.provider.resolveDebugConfiguration(h.folder, {}).stopOnEntry, false);
    assert.equal(h.server.options.env, undefined);
});
