import * as vscode from "vscode";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    const serverPath = vscode.workspace
        .getConfiguration("webtest")
        .get<string>("serverPath", "webtest");

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
        vscode.commands.registerCommand("webtest.runFile", async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== "webtest") {
                void vscode.window.showWarningMessage("Open a WebTest file before running it.");
                return;
            }
            await client?.sendRequest("workspace/executeCommand", {
                command: "webtest.runFile",
                arguments: [editor.document.uri.toString()],
            });
        }),
    );

    await client.start();
}

export async function deactivate(): Promise<void> {
    await client?.stop();
}

