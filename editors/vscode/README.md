# WebTest for Cursor and VS Code

This standard VSIX connects Cursor or VS Code to the native `webtest` language and debug adapters. It provides CST-backed semantic syntax highlighting, diagnostics, document formatting, test execution, and source breakpoints.

Build the Rust executable before using a development installation:

```sh
cargo build
```

When the repository is open as a workspace, the extension discovers `target/debug/webtest` automatically. Otherwise, put `webtest` on `PATH` or configure `webtest.serverPath` with an absolute executable path.

Package, install, and verify the current extension version in Cursor with:

```sh
npm run smoke:cursor
```

`npm run package` names the VSIX from the extension manifest. `npm run smoke` compiles, packages, opens that VSIX, and verifies its manifest, commands, semantic-token mapping, and debug-adapter entry points. `npm run smoke:cursor` additionally installs it into Cursor and verifies the manifest's current publisher, name, and version are registered. `npm run smoke:activation` launches an isolated official extension host and exercises activation, language diagnostics, semantic tokens, synchronized-buffer execution, and a zero-`launch.json` debug session; when headed Chrome is available, it also verifies the breakpoint stack frame.

Open a `.webtest` file and run **WebTest: Run Current File** from the Command Palette or the editor title button. Runtime failures are published against the source revision that was executed, so stale diagnostics disappear after an edit.

Set a breakpoint on an `open`, `click`, or `expect` statement, then run **WebTest: Debug Current File**. The default debugger launches headed Chrome and pauses before the marked operation without requiring a `launch.json`. The WebTest scope shows the current test, operation, source line, and line number.
