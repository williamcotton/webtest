import * as vscode from "vscode";
import * as fs from "node:fs";
import * as path from "node:path";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from "vscode-languageclient/node";

import { installRelease, managedBinary } from "./releases";
import { registerTesting } from "./testing";
import { configuredEnvironment, debugStopOnEntry } from "./configuration";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    context.subscriptions.push(vscode.commands.registerCommand("webtest.installServer", async () => {
        const version = await vscode.window.showInputBox({ prompt: "Exact WebTest release version (x.y.z)", value: "0.1.0", validateInput: value => /^\d+\.\d+\.\d+$/.test(value) ? undefined : "Use an exact x.y.z version" });
        if (!version) return;
        const consent = await vscode.window.showInformationMessage(`Download and verify WebTest ${version} from williamcotton/webtest into extension storage?`, { modal: true }, "Install");
        if (consent !== "Install") return;
        try {
            await vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title: "Installing verified WebTest server" }, () => installRelease(context.globalStorageUri.fsPath, version));
            const action = await vscode.window.showInformationMessage("WebTest installed. Reload to use it. Previous installations remain available in extension storage.", "Reload");
            if (action) await vscode.commands.executeCommand("workbench.action.reloadWindow");
        } catch (error) { void vscode.window.showErrorMessage(`WebTest installation failed: ${String(error)}`); }
    }));
    const serverPath = await resolveServerPath(context.globalStorageUri.fsPath);

    const debugProvider: vscode.DebugConfigurationProvider = {
        provideDebugConfigurations: folder => [defaultDebugConfiguration(folder?.uri)],
        resolveDebugConfiguration: (_folder, configuration) => {
            const editor = vscode.window.activeTextEditor;
            const resource = typeof configuration.program === "string" && path.isAbsolute(configuration.program)
                ? vscode.Uri.file(configuration.program) : editor?.document.uri ?? _folder?.uri;
            if (!configuration.type && !configuration.request && !configuration.name) {
                configuration = defaultDebugConfiguration(resource);
            }
            configuration.type ??= "webtest";
            configuration.request ??= "launch";
            configuration.name ??= "Debug WebTest";
            configuration.program ??=
                editor?.document.languageId === "webtest"
                    ? editor.document.uri.fsPath
                    : "${file}";
            configuration.headed ??= true;
            configuration.stopOnEntry ??= debugStopOnEntry(resource);
            return configuration;
        },
        resolveDebugConfigurationWithSubstitutedVariables: (folder, configuration) => {
            const workspaceFolder = folder ?? vscode.workspace.getWorkspaceFolder(vscode.Uri.file(String(configuration.program)));
            configuration.sources = Object.fromEntries(vscode.workspace.textDocuments
                .filter((document) => document.languageId === "webtest" && document.uri.scheme === "file"
                    && vscode.workspace.getWorkspaceFolder(document.uri)?.uri.toString() === workspaceFolder?.uri.toString())
                .map((document) => [document.uri.fsPath, document.getText()]));
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
            const program = resolveDebugProgram(session);
            if (program) {
                args.push("--project", program);
            }
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
                cwd: program
                    ? path.dirname(program)
                    : session.workspaceFolder?.uri.fsPath,
                env: { ...configuredEnvironment(), ...session.configuration.env },
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
        options: { env: configuredEnvironment() },
    };
    const projectConfigurationWatcher = vscode.workspace.createFileSystemWatcher(
        "**/webtest.toml",
    );
    const projectJsonWatcher = vscode.workspace.createFileSystemWatcher("**/*.json");
    const sourceWatcher = vscode.workspace.createFileSystemWatcher("**/*.webtest");
    context.subscriptions.push(projectConfigurationWatcher, projectJsonWatcher, sourceWatcher);
    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "webtest" }],
        initializationOptions: { projectFileEvents: true },
        synchronize: {
            fileEvents: [projectConfigurationWatcher, projectJsonWatcher, sourceWatcher],
        },
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
                    ...defaultDebugConfiguration(editor.document.uri),
                    program: editor.document.uri.fsPath,
                    sourceText: editor.document.getText(),
                },
            );
        }),
    );

    try {
        context.subscriptions.push(client.onNotification("webtest/resyncRequired", (params: { uri: string }) => {
            const document = vscode.workspace.textDocuments.find(candidate => candidate.uri.toString() === params.uri);
            if (document && client) {
                void client.sendNotification("textDocument/didChange", {
                    textDocument: { uri: params.uri, version: document.version },
                    contentChanges: [{ text: document.getText() }],
                });
            }
        }));
        await client.start();
        registerTesting(context, client);
    } catch (error) {
        void vscode.window.showErrorMessage(
            `Could not start the WebTest language server at ${serverPath}. ` +
                "Build webtest or configure webtest.serverPath.",
        );
        return;
    }
}

function defaultDebugConfiguration(resource?: vscode.Uri): vscode.DebugConfiguration {
    return {
        type: "webtest",
        request: "launch",
        name: "Debug WebTest",
        program: "${file}",
        headed: true,
        stopOnEntry: debugStopOnEntry(resource),
    };
}

export async function deactivate(): Promise<void> {
    await client?.stop();
}

async function resolveServerPath(storage: string): Promise<string> {
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
        if (await isExecutable(candidate)) {
            return candidate;
        }
    }

    for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
        if (!directory) continue;
        const candidate = path.join(directory, executable);
        if (await isExecutable(candidate)) return candidate;
    }
    return (await managedBinary(storage)) ?? "webtest";
}

function resolveDebugProgram(session: vscode.DebugSession): string | undefined {
    const program = session.configuration.program;
    if (typeof program !== "string" || !program.trim() || program.includes("${")) {
        return undefined;
    }
    if (path.isAbsolute(program)) {
        return path.normalize(program);
    }
    const base = session.workspaceFolder?.uri.fsPath ?? process.cwd();
    return path.resolve(base, program);
}

async function isExecutable(candidate: string): Promise<boolean> {
    try {
        await fs.promises.access(candidate, fs.constants.X_OK);
        return true;
    } catch {
        return false;
    }
}
