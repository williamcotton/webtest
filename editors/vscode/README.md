# WebTest for Cursor and VS Code

This standard VSIX connects Cursor or VS Code to the native `webtest lsp` server. It provides CST-backed semantic syntax highlighting, diagnostics, document formatting, and the **WebTest: Run Current File** command.

Build the Rust executable before using a development installation:

```sh
cargo build
```

When the repository is open as a workspace, the extension discovers `target/debug/webtest` automatically. Otherwise, put `webtest` on `PATH` or configure `webtest.serverPath` with an absolute executable path.

Install a local package in Cursor with:

```sh
npm run package
cursor --install-extension webtest-vscode-0.1.1.vsix --force
```

Open a `.webtest` file and run **WebTest: Run Current File** from the Command Palette or the editor title button. Runtime failures are published against the source revision that was executed, so stale diagnostics disappear after an edit.
