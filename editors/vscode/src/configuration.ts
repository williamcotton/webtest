import * as vscode from "vscode";

/** Host environment only; this does not interpret WebTest source or projects. */
export function configuredEnvironment(): Record<string, string> | undefined {
    const overrides = vscode.workspace.getConfiguration("webtest")
        .get<Record<string, string>>("environment", {});
    if (!Object.keys(overrides).length) { return undefined; }
    return Object.fromEntries(Object.entries(overrides).map(([key, value]) => [
        key,
        value.replace(/\$\{env:([^}]+)\}/g, (_match, name: string) => process.env[name] ?? ""),
    ]));
}

export function debugStopOnEntry(resource?: vscode.Uri): boolean {
    return vscode.workspace.getConfiguration("webtest", resource)
        .get<boolean>("debug.stopOnEntry", false);
}
