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

    const debugProvider: vscode.DebugConfigurationProvider = {
        provideDebugConfigurations: () => [defaultDebugConfiguration()],
        resolveDebugConfiguration: (_folder, configuration) => {
            const editor = vscode.window.activeTextEditor;
            if (!configuration.type && !configuration.request && !configuration.name) {
                configuration = defaultDebugConfiguration();
            }
            configuration.type ??= "webtest";
            configuration.request ??= "launch";
            configuration.name ??= "Debug WebTest";
            configuration.program ??=
                editor?.document.languageId === "webtest"
                    ? editor.document.uri.fsPath
                    : "${file}";
            configuration.headed ??= true;
            configuration.stopOnEntry ??= false;
            return configuration;
        },
        resolveDebugConfigurationWithSubstitutedVariables: (_folder, configuration) => {
            if (typeof configuration.program === "string") {
                const program = path.resolve(configuration.program);
                const document = vscode.workspace.textDocuments.find(
                    (candidate) => path.resolve(candidate.uri.fsPath) === program,
                );
                if (document?.languageId === "webtest") {
                    configuration.sourceText = document.getText();
                }
            }
            return configuration;
        },
    };
    const debugAdapterFactory: vscode.DebugAdapterDescriptorFactory = {
        createDebugAdapterDescriptor: (session) => {
            const args = ["dap"];
            const configuredChrome = vscode.workspace
                .getConfiguration("webtest", session.workspaceFolder?.uri)
                .get<string>("chromePath", "")
                .trim();
            const chromePath =
                typeof session.configuration.chromePath === "string" &&
                session.configuration.chromePath.trim()
                    ? session.configuration.chromePath.trim()
                    : configuredChrome;
            if (chromePath) {
                args.push("--chrome-path", chromePath);
            }
            if (session.configuration.headed === false) {
                args.push("--headless");
            }
            return new vscode.DebugAdapterExecutable(serverPath, args, {
                cwd: session.workspaceFolder?.uri.fsPath,
            });
        },
    };

    context.subscriptions.push(
        vscode.debug.registerDebugConfigurationProvider("webtest", debugProvider),
        vscode.debug.registerDebugAdapterDescriptorFactory("webtest", debugAdapterFactory),
    );

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
        vscode.commands.registerCommand("webtest.debugCurrentFile", async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== "webtest") {
                void vscode.window.showWarningMessage(
                    "Open a WebTest file before debugging it.",
                );
                return;
            }
            await vscode.debug.startDebugging(
                vscode.workspace.getWorkspaceFolder(editor.document.uri),
                {
                    ...defaultDebugConfiguration(),
                    program: editor.document.uri.fsPath,
                    sourceText: editor.document.getText(),
                },
            );
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

function defaultDebugConfiguration(): vscode.DebugConfiguration {
    return {
        type: "webtest",
        request: "launch",
        name: "Debug WebTest",
        program: "${file}",
        headed: true,
        stopOnEntry: false,
    };
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
