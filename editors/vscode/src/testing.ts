import * as vscode from "vscode";
import { randomUUID } from "node:crypto";
import { LanguageClient } from "vscode-languageclient/node";

type WireRange = { start: { line: number; character: number }; end: { line: number; character: number } };
type Variant = { id: string; label: string; runnable: boolean; debuggable: boolean; tags: string[] };
type Discovery = {
    workspace_id: string; workspace_revision: number; truncated: boolean;
    modules: { id: string; uri: string; path: string; tests: {
        id: string; label: string; range: WireRange; static_status: string; variants: Variant[];
    }[] }[];
};
type VariantData = Variant & { uri: vscode.Uri };
type RunSummary = { tests: {
    variant_id: string; outcome: string; duration_ms: number; message?: string;
    location?: { uri?: string; range?: WireRange };
    artifacts: { path: string }[];
}[]; failures?: { message: string; variants: string[] }[] };
const range = (value: WireRange): vscode.Range => new vscode.Range(value.start.line, value.start.character, value.end.line, value.end.character);

/** Rust owns discovery, identities, ranges, readiness, and selection semantics. */
export function registerTesting(context: vscode.ExtensionContext, client: LanguageClient): void {
    const controller = vscode.tests.createTestController("webtest", "WebTest");
    const items = new Map<string, vscode.TestItem>();
    let variants = new Map<string, VariantData>();
    let discovery: Discovery | undefined;
    let refreshGeneration = 0;
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;
    let refreshCancellation: vscode.CancellationTokenSource | undefined;
    let disposed = false;
    let lastProjectUri: vscode.Uri | undefined;
    const workspaceUri = (): vscode.Uri | undefined => {
        const document = vscode.window.activeTextEditor?.document;
        if (document?.languageId === "webtest" && document.uri.scheme === "file") {
            lastProjectUri = document.uri;
        }
        // Rust resolves the document's nearest project and provider schema. A
        // parent editor folder can contain examples with unrelated schemas.
        return lastProjectUri ?? vscode.workspace.workspaceFolders?.[0]?.uri;
    };
    const item = (id: string, label: string, uri?: vscode.Uri): vscode.TestItem => {
        let value = items.get(id);
        if (!value) { value = controller.createTestItem(id, label, uri); items.set(id, value); }
        value.label = label;
        return value;
    };
    const refresh = async (): Promise<void> => {
        const uri = workspaceUri();
        if (!uri || disposed) { return; }
        const generation = ++refreshGeneration;
        refreshCancellation?.cancel();
        refreshCancellation?.dispose();
        const cancellation = new vscode.CancellationTokenSource();
        refreshCancellation = cancellation;
        try {
            const next = await client.sendRequest<Discovery>("webtest/discoverTests", { uri: uri.toString() }, cancellation.token);
            if (generation !== refreshGeneration || disposed) { return; }
            if (next.truncated) { throw new Error("Test discovery exceeds the workspace query limit."); }
            const nextVariants = new Map<string, VariantData>();
            const alive = new Set<string>();
            const root = item(next.workspace_id, vscode.workspace.getWorkspaceFolder(uri)?.name ?? "Workspace", uri);
            alive.add(root.id);
            root.error = undefined;
            const modules = next.modules.map(module => {
                const moduleUri = vscode.Uri.parse(module.uri);
                const moduleItem = item(module.id, vscode.workspace.asRelativePath(moduleUri), moduleUri);
                alive.add(moduleItem.id);
                moduleItem.children.replace(module.tests.map(test => {
                    const testItem = item(test.id, test.label, moduleUri);
                    alive.add(testItem.id);
                    testItem.range = range(test.range);
                    testItem.error = test.static_status === "ready" ? undefined : "Fix static diagnostics before running this test.";
                    testItem.children.replace(test.variants.map(variant => {
                        const variantItem = item(variant.id, variant.label, moduleUri);
                        alive.add(variantItem.id);
                        variantItem.range = testItem.range;
                        variantItem.tags = variant.tags.map(tag => new vscode.TestTag(tag));
                        variantItem.error = variant.runnable ? undefined : testItem.error;
                        nextVariants.set(variant.id, { ...variant, uri: moduleUri });
                        return variantItem;
                    }));
                    return testItem;
                }));
                return moduleItem;
            });
            root.children.replace(modules);
            controller.items.replace([root]);
            for (const id of items.keys()) { if (!alive.has(id)) { items.delete(id); } }
            variants = nextVariants;
            discovery = next;
        } catch (error) {
            if (!cancellation.token.isCancellationRequested && generation === refreshGeneration && !disposed) {
                for (const root of controller.items) { root[1].error = String(error); }
            }
        }
    };
    const schedule = (): void => {
        if (refreshTimer) { clearTimeout(refreshTimer); }
        refreshTimer = setTimeout(() => { void refresh(); }, 200);
    };
    controller.resolveHandler = async () => { await refresh(); };
    controller.refreshHandler = refresh;
    const selected = (request: vscode.TestRunRequest): vscode.TestItem[] => {
        const excluded = new Set(request.exclude?.map(value => value.id));
        const found = new Map<string, vscode.TestItem>();
        const visit = (value: vscode.TestItem): void => {
            for (let ancestor: vscode.TestItem | undefined = value; ancestor; ancestor = ancestor.parent) { if (excluded.has(ancestor.id)) { return; } }
            const current = items.get(value.id);
            if (!current) { return; }
            if (variants.has(current.id)) { found.set(current.id, current); }
            else { current.children.forEach(visit); }
        };
        (request.include ?? [...controller.items].map(([, value]) => value)).forEach(visit);
        return [...found.values()];
    };
    const execute = async (request: vscode.TestRunRequest, token: vscode.CancellationToken): Promise<void> => {
        await refresh();
        const run = controller.createTestRun(request);
        const selection = selected(request);
        const id = randomUUID();
        const cancellation = token.onCancellationRequested(() => { void client.sendNotification("webtest/cancelRun", { run_id: id }); });
        try {
            if (!discovery || token.isCancellationRequested) { selection.forEach(value => run.skipped(value)); return; }
            const ready = selection.filter(value => {
                if (variants.get(value.id)?.runnable) { run.enqueued(value); return true; }
                run.errored(value, new vscode.TestMessage("The test has static errors.")); return false;
            });
            if (!ready.length) { return; }
            ready.forEach(value => run.started(value));
            const result = await client.sendRequest<RunSummary>("webtest/runVariants", {
                uri: variants.get(ready[0].id)!.uri.toString(), workspace_revision: discovery.workspace_revision,
                variants: ready.map(value => value.id), run_id: id,
            });
            const completed = new Set<string>();
            const messages = new Map<string, vscode.TestMessage[]>();
            for (const test of result.tests) {
                const value = ready.find(value => value.id === test.variant_id);
                if (!value) { continue; }
                completed.add(value.id);
                if (test.outcome === "passed") { run.passed(value, test.duration_ms); }
                else if (test.outcome === "cancelled") { run.skipped(value); }
                else {
                    const message = new vscode.TestMessage(test.message ?? test.outcome);
                    messages.set(value.id, [message]);
                    if (test.location?.uri && test.location.range) { message.location = new vscode.Location(vscode.Uri.parse(test.location.uri), range(test.location.range)); }
                    if (test.outcome === "failed" || test.outcome === "timed_out") { run.failed(value, message, test.duration_ms); }
                    else { run.errored(value, message, test.duration_ms); }
                }
                for (const artifact of test.artifacts) { run.appendOutput(`Evidence: ${vscode.Uri.file(artifact.path).toString()}\r\n`, undefined, value); }
            }
            if (result.failures?.length) {
                for (const failure of result.failures) {
                    for (const value of ready.filter(value => failure.variants.includes(value.id))) {
                        const combined = [...(messages.get(value.id) ?? []), new vscode.TestMessage(failure.message)];
                        messages.set(value.id, combined);
                        run.errored(value, combined);
                    }
                }
            }
            ready.filter(value => !completed.has(value.id)).forEach(value => run.errored(value, new vscode.TestMessage("The runner returned no result for this variant.")));
        } catch (error) {
            selection.forEach(value => token.isCancellationRequested ? run.skipped(value) : run.errored(value, new vscode.TestMessage(String(error))));
        } finally { cancellation.dispose(); run.end(); }
    };
    const debug = async (request: vscode.TestRunRequest, token: vscode.CancellationToken): Promise<void> => {
        await refresh();
        const run = controller.createTestRun(request);
        try {
            for (const value of selected(request)) {
                const variant = variants.get(value.id)!;
                if (token.isCancellationRequested) { run.skipped(value); continue; }
                if (!variant.debuggable) { run.errored(value, new vscode.TestMessage("The test has static errors.")); continue; }
                run.started(value);
                const runId = randomUUID();
                let session: vscode.DebugSession | undefined;
                let exitCode: number | undefined;
                let finish: () => void = () => {};
                const ended = new Promise<void>(resolve => { finish = resolve; });
                const tracker = vscode.debug.registerDebugAdapterTrackerFactory("webtest", { createDebugAdapterTracker: candidate => candidate.configuration.webtestRunId === runId ? {
                    onDidSendMessage: message => { if (message.event === "exited") { exitCode = message.body?.exitCode; } },
                } : undefined });
                const started = vscode.debug.onDidStartDebugSession(candidate => { if (candidate.configuration.webtestRunId === runId) { session = candidate; if (token.isCancellationRequested) { void vscode.debug.stopDebugging(candidate); } } });
                const terminated = vscode.debug.onDidTerminateDebugSession(candidate => { if (candidate.configuration.webtestRunId === runId) { finish(); } });
                const cancel = token.onCancellationRequested(() => { if (session) { void vscode.debug.stopDebugging(session); } });
                try {
                    const launched = await vscode.debug.startDebugging(vscode.workspace.getWorkspaceFolder(variant.uri), {
                        type: "webtest", request: "launch", name: `Debug ${value.parent?.label ?? value.label}`,
                        program: variant.uri.fsPath, variants: [variant.id], webtestRunId: runId,
                    });
                    if (!launched) { run.errored(value, new vscode.TestMessage("Debug launch was cancelled.")); continue; }
                    await ended;
                    if (token.isCancellationRequested) { run.skipped(value); }
                    else if (exitCode === 0) { run.passed(value); }
                    else { run.failed(value, new vscode.TestMessage(`Debug run exited with ${exitCode ?? "no result"}.`)); }
                } finally { tracker.dispose(); started.dispose(); terminated.dispose(); cancel.dispose(); }
            }
        } finally { run.end(); }
    };
    context.subscriptions.push(controller,
        controller.createRunProfile("Run", vscode.TestRunProfileKind.Run, execute, true),
        controller.createRunProfile("Debug", vscode.TestRunProfileKind.Debug, debug, true),
        vscode.window.onDidChangeActiveTextEditor(() => { schedule(); }),
        vscode.workspace.onDidChangeTextDocument(event => { if (event.document.languageId === "webtest") { schedule(); } }),
        vscode.workspace.onDidOpenTextDocument(document => { if (document.languageId === "webtest") { schedule(); } }),
        vscode.workspace.onDidCloseTextDocument(document => { if (document.languageId === "webtest") { schedule(); } }),
        { dispose: () => { disposed = true; if (refreshTimer) { clearTimeout(refreshTimer); } refreshCancellation?.cancel(); refreshCancellation?.dispose(); } });
    const watcher = vscode.workspace.createFileSystemWatcher("**/*.{webtest,toml,json}");
    context.subscriptions.push(watcher, watcher.onDidCreate(schedule), watcher.onDidChange(schedule), watcher.onDidDelete(schedule));
    void refresh();
}
