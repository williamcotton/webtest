import * as vscode from "vscode";
import * as fs from "node:fs";
import * as path from "node:path";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    const serverPath = await resolveServerPath();

    const serverOptions: ServerOptions = {
        command: serverPath,
        args: ["lsp"],
    };
    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "webtest" }],
    };

    client = new LanguageClient(
        "webtest",
        "WebTest Language Server",
        serverOptions,
        clientOptions,
    );

    context.subscriptions.push(
        vscode.commands.registerCommand("webtest.runCurrentFile", async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== "webtest") {
                void vscode.window.showWarningMessage("Open a WebTest file before running it.");
                return;
            }
            if (!client) {
                void vscode.window.showErrorMessage("The WebTest language server is not running.");
                return;
            }
            await client.sendRequest("workspace/executeCommand", {
                command: "webtest.runFile",
                arguments: [editor.document.uri.toString()],
            });
        }),
    );

    try {
        await client.start();
    } catch (error) {
        void vscode.window.showErrorMessage(
            `Could not start the WebTest language server at ${serverPath}. ` +
                "Build webtest or configure webtest.serverPath.",
        );
        throw error;
    }
}

export async function deactivate(): Promise<void> {
    await client?.stop();
}

async function resolveServerPath(): Promise<string> {
    const configuration = vscode.workspace.getConfiguration("webtest");
    const inspected = configuration.inspect<string>("serverPath");
    const configured =
        inspected?.workspaceFolderValue ??
        inspected?.workspaceValue ??
        inspected?.globalValue;
    if (configured?.trim()) {
        return configured;
    }

    const executable = process.platform === "win32" ? "webtest.exe" : "webtest";
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
        const candidate = path.join(folder.uri.fsPath, "target", "debug", executable);
        try {
            await fs.promises.access(candidate, fs.constants.X_OK);
            return candidate;
        } catch {
            // Continue to the next workspace, then fall back to PATH.
        }
    }

    return "webtest";
}
