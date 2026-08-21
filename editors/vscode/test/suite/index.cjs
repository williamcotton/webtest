const assert = require("node:assert/strict");
const path = require("node:path");
const vscode = require("vscode");

async function run() {
    const extension = vscode.extensions.getExtension("webtest.webtest-vscode");
    assert.ok(extension, "development WebTest extension is installed");
    await extension.activate();
    assert.equal(extension.isActive, true);

    const commands = await vscode.commands.getCommands(true);
    assert.ok(commands.includes("webtest.runCurrentFile"));
    assert.ok(commands.includes("webtest.debugCurrentFile"));
    assert.ok(commands.includes("webtest.runFile"), "the language client registered the LSP command");

    const repository = vscode.workspace.workspaceFolders[0].uri.fsPath;
    const uri = vscode.Uri.file(path.join(repository, "editors/vscode/test/fixtures/activation.webtest"));
    const document = await vscode.workspace.openTextDocument(uri);
    const editor = await vscode.window.showTextDocument(document);
    const invalidSource =
        'test "extension activation" {\n    browser {\n        click id("broken"\n    }\n}\n';
    await replaceDocument(editor, document, invalidSource);
    await waitFor(
        () => vscode.languages.getDiagnostics(uri).some((diagnostic) => diagnostic.source === "webtest.syntax"),
        "syntax diagnostic",
    );

    const validSource =
        'test "extension activation" {\n' +
        '    browser {\n' +
        '        open "data:text/html,%3Cbutton%20id%3D%22submit%22%3ESubmit%3C%2Fbutton%3E"\n' +
        '        click id("submit")\n' +
        '    }\n' +
        '}\n';
    await replaceDocument(editor, document, validSource);
    await waitFor(() => vscode.languages.getDiagnostics(uri).length === 0, "cleared diagnostics");

    const semantic = await vscode.commands.executeCommand(
        "vscode.provideDocumentSemanticTokens",
        uri,
    );
    assert.ok(semantic && semantic.data && semantic.data.length > 0, "semantic tokens are available");
    await vscode.commands.executeCommand("webtest.runCurrentFile");

    const breakpoint = new vscode.SourceBreakpoint(
        new vscode.Location(uri, new vscode.Position(3, 8)),
    );
    vscode.debug.addBreakpoints([breakpoint]);
    const started = once(vscode.debug.onDidStartDebugSession, 10_000, "debug session start");
    await vscode.commands.executeCommand("webtest.debugCurrentFile");
    const session = await started;
    assert.equal(session.type, "webtest");
    assert.equal(session.configuration.headed, true);
    assert.equal(session.configuration.sourceText, validSource);

    if (process.env.WEBTEST_SMOKE_HEADED === "true") {
        const stack = await waitForRequest(
            session,
            "stackTrace",
            { threadId: 1 },
            (response) => response.stackFrames && response.stackFrames.length > 0,
            "headed breakpoint",
        );
        assert.equal(stack.stackFrames[0].line, 4);
        await session.customRequest("continue", { threadId: 1 });
    }
    await vscode.debug.stopDebugging(session);
    vscode.debug.removeBreakpoints([breakpoint]);
}

async function replaceDocument(editor, document, source) {
    const end = document.positionAt(document.getText().length);
    const applied = await editor.edit((builder) => {
        builder.replace(new vscode.Range(new vscode.Position(0, 0), end), source);
    });
    assert.equal(applied, true, "document edit was applied");
}

async function waitFor(predicate, description, timeout = 10_000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error(`timed out waiting for ${description}`);
}

async function waitForRequest(session, command, arguments, predicate, description) {
    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline) {
        try {
            const response = await session.customRequest(command, arguments);
            if (predicate(response)) return response;
        } catch {
            // The adapter may not have reached its first stopped state yet.
        }
        await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error(`timed out waiting for ${description}`);
}

function once(event, timeout, description) {
    return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
            subscription.dispose();
            reject(new Error(`timed out waiting for ${description}`));
        }, timeout);
        const subscription = event((value) => {
            clearTimeout(timer);
            subscription.dispose();
            resolve(value);
        });
    });
}

module.exports = { run };
