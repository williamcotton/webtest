# WebTest for Cursor and VS Code

This standard VSIX connects Cursor or VS Code to the native `webtest` language and debug adapters. It provides CST-backed semantic syntax highlighting, diagnostics, document formatting, test execution, and source breakpoints.

Build the Rust executable before using a development installation:

```sh
cargo build
```

When the repository is open as a workspace, the extension discovers `target/debug/webtest` automatically. Otherwise, put `webtest` on `PATH` or configure `webtest.serverPath` with an absolute executable path.

Install a local package in Cursor with:

```sh
npm run package
cursor --install-extension webtest-vscode-0.2.0.vsix --force
```

Open a `.webtest` file and run **WebTest: Run Current File** from the Command Palette or the editor title button. Runtime failures are published against the source revision that was executed, so stale diagnostics disappear after an edit.

Set a breakpoint on an `open`, `click`, or `expect` statement, then run **WebTest: Debug Current File**. The default debugger launches headed Chrome and pauses before the marked operation without requiring a `launch.json`. The WebTest scope shows the current test, operation, source line, and line number.
