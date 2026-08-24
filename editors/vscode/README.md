# WebTest for Cursor and VS Code

This standard VSIX connects Cursor or VS Code to the native `webtest` language and debug adapters. It provides CST-backed semantic syntax highlighting, diagnostics, document formatting, test execution, and source breakpoints.

Install the Rust executable from the repository checkout:

```sh
cargo install --path crates/app --locked
```

Or build it in place for extension development:

```sh
cargo build
```

The extension discovers `target/debug/webtest` in an open workspace and Cargo-installed binaries in `CARGO_HOME/bin` or `~/.cargo/bin`. Otherwise, put `webtest` on `PATH` or configure `webtest.serverPath` with an absolute executable path. Debug sessions select configuration from the nearest `webtest.toml` above the test file, rather than assuming the editor workspace is the project root.

Package, install, and verify the current extension version in Cursor with:

```sh
npm run smoke:cursor
```

`npm run package` names the VSIX from the extension manifest. `npm run smoke` compiles, packages, opens that VSIX, and verifies its manifest, commands, semantic-token mapping, and debug-adapter entry points. `npm run smoke:cursor` additionally installs it into Cursor and verifies the manifest's current publisher, name, and version are registered. `npm run smoke:activation` launches an isolated official extension host and exercises activation, language diagnostics, semantic tokens, synchronized-buffer execution, and a zero-`launch.json` debug session; when headed Chrome is available, it also verifies the breakpoint stack frame.

Open a `.webtest` file and run **WebTest: Run Current File** from the Command Palette or the editor title button. Runtime failures are published against the source revision that was executed, so stale diagnostics disappear after an edit.

Set a breakpoint on an `open`, `click`, or `expect` statement, then run **WebTest: Debug Current File**. The default debugger launches one headed Chrome window and pauses before the marked operation without requiring a `launch.json`. The WebTest scope shows the current test, operation, source line, line number, and every binding evaluated so far. Records, lists, responses, headers, decoded JSON, and process results are expandable. Server-only values remain available, with secrets redacted, while stepping into the browser block. The debug session closes automatically after the final step.
