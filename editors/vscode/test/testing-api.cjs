const assert = require("node:assert/strict");
const { test } = require("node:test");
const Module = require("node:module");

const disposable = () => ({ dispose() {} });
class Collection extends Map {
    constructor(owner) { super(); this.owner = owner; }
    replace(items) { this.clear(); for (const item of items) { item.parent = this.owner; this.set(item.id, item); } }
}
function harness() {
    const profiles = new Map(), runs = [], requests = [], notifications = [];
    const uri = value => ({ fsPath: value.replace("file://", ""), toString: () => value });
    const controller = {
        items: new Collection(), dispose() {},
        createTestItem(id, label, uri) { const item = { id, label, uri }; item.children = new Collection(item); return item; },
        createRunProfile(name, kind, callback) { profiles.set(name, callback); return disposable(); },
        createTestRun() { const calls = []; const run = Object.fromEntries(["enqueued", "started", "passed", "failed", "errored", "skipped", "appendOutput", "end"].map(method => [method, (...args) => calls.push([method, ...args])])); runs.push(calls); return run; },
    };
    const started = [], terminated = [];
    let tracker, debugLaunch;
    const vscode = {
        Uri: { parse: uri, file: path => uri(`file://${path}`) },
        Range: class { constructor(...values) { this.values = values; } },
        Location: class { constructor(uri, range) { this.uri = uri; this.range = range; } },
        TestMessage: class { constructor(message) { this.message = message; } },
        TestTag: class { constructor(id) { this.id = id; } },
        CancellationTokenSource: class { constructor() { this.token = {}; } cancel() {} dispose() {} },
        TestRunProfileKind: { Run: 1, Debug: 2 },
        tests: { createTestController: () => controller },
        window: {},
        workspace: {
            workspaceFolders: [{ uri: uri("file:///workspace"), name: "Workspace" }],
            getWorkspaceFolder: () => ({ name: "Workspace" }), asRelativePath: value => value.fsPath,
            onDidChangeTextDocument: disposable, onDidOpenTextDocument: disposable, onDidCloseTextDocument: disposable,
            createFileSystemWatcher: () => ({ dispose() {}, onDidCreate: disposable, onDidChange: disposable, onDidDelete: disposable }),
        },
        debug: {
            registerDebugAdapterTrackerFactory: (_, factory) => { tracker = factory; return disposable(); },
            onDidStartDebugSession: callback => { started.push(callback); return disposable(); },
            onDidTerminateDebugSession: callback => { terminated.push(callback); return disposable(); },
            stopDebugging: async () => {},
            startDebugging: async (_, config) => { debugLaunch = config; const session = { configuration: config }; started.forEach(callback => callback(session)); tracker.createDebugAdapterTracker(session).onDidSendMessage({ event: "exited", body: { exitCode: 0 } }); terminated.forEach(callback => callback(session)); return true; },
        },
    };
    const discovery = { workspace_id: "workspace", workspace_revision: 7, truncated: false, modules: [{ id: "module", uri: "file:///workspace/tests.webtest", path: "tests.webtest", tests: ["first", "second"].map((id, index) => ({ id, label: "same label", static_status: "ready", range: { start: { line: index, character: 0 }, end: { line: index, character: 9 } }, variants: [{ id: `${id}-variant`, label: "default", runnable: true, debuggable: true, tags: [] }] })) }] };
    let summary = { tests: ["first", "second"].map(id => ({ variant_id: `${id}-variant`, outcome: "passed", duration_ms: 1, artifacts: [] })) };
    let onRun = () => {};
    const client = { sendRequest: async (method, params) => { requests.push([method, params]); if (method === "webtest/discoverTests") { return discovery; } onRun(); return summary; }, sendNotification: async (...args) => notifications.push(args) };
    const context = { subscriptions: [] };
    const original = Module._load;
    Module._load = function(name, ...args) { return name === "vscode" ? vscode : original.call(this, name, ...args); };
    try { delete require.cache[require.resolve("../out/testing.js")]; require("../out/testing.js").registerTesting(context, client); }
    finally { Module._load = original; }
    let cancel;
    const token = { isCancellationRequested: false, onCancellationRequested: callback => { cancel = callback; return disposable(); } };
    return { controller, profiles, requests, notifications, runs, token, setSummary: value => summary = value, onRun: callback => onRun = callback, cancel: () => { token.isCancellationRequested = true; cancel(); }, get debugLaunch() { return debugLaunch; }, dispose: () => context.subscriptions.forEach(value => value.dispose()) };
}

test("Testing API selects stable variants, deduplicates parents, honors exclusions, and routes Debug", async () => {
    const h = harness();
    try {
        await h.controller.refreshHandler();
        const root = h.controller.items.get("workspace"), module = root.children.get("module");
        const first = module.children.get("first"), second = module.children.get("second");
        const variant = first.children.get("first-variant");
        await h.profiles.get("Run")({ include: [root, variant], exclude: [second] }, h.token);
        const run = h.requests.find(([method]) => method === "webtest/runVariants")[1];
        assert.deepEqual(run.variants, ["first-variant"]);
        assert.equal(run.workspace_revision, 7);
        assert.ok(h.runs[0].some(([method, item]) => method === "passed" && item.id === "first-variant"));
        await h.profiles.get("Debug")({ include: [variant] }, h.token);
        assert.deepEqual(h.debugLaunch.variants, ["first-variant"]);
        assert.equal(h.debugLaunch.program, "/workspace/tests.webtest");
        assert.ok(h.runs[1].some(([method]) => method === "passed"));
    } finally { h.dispose(); }
});
test("Testing API retains primary failure messages and marks shared teardown errors only on affected variants", async () => {
    const h = harness();
    try {
        h.setSummary({ tests: [{ variant_id: "first-variant", outcome: "failed", message: "primary assertion", artifacts: [] }, { variant_id: "second-variant", outcome: "passed", artifacts: [] }], failures: [{ variants: ["first-variant"], message: "fixture teardown" }] });
        await h.profiles.get("Run")({}, h.token);
        const errors = h.runs[0].filter(([method]) => method === "errored");
        assert.equal(errors.length, 1);
        assert.equal(errors[0][1].id, "first-variant");
        assert.deepEqual(errors[0][2].map(message => message.message), ["primary assertion", "fixture teardown"]);
    } finally { h.dispose(); }
});
test("Testing API cancellation is correlated with the owned run request", async () => {
    const h = harness();
    try {
        h.onRun(h.cancel);
        await h.profiles.get("Run")({}, h.token);
        const run = h.requests.find(([method]) => method === "webtest/runVariants")[1];
        assert.deepEqual(h.notifications[0], ["webtest/cancelRun", { run_id: run.run_id }]);
        assert.ok(h.runs[0].some(([method]) => method === "end"));
    } finally { h.dispose(); }
});
