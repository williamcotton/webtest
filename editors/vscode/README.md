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

The extension discovers `target/debug/webtest` in an open workspace, then `webtest` on `PATH`, then a verified managed installation. Configure `webtest.serverPath` for an explicit executable override. Debug sessions select configuration from the nearest `webtest.toml` above the test file, rather than assuming the editor workspace is the project root.

Package, install, and verify the current extension version in Cursor with:

```sh
npm run smoke:cursor
```

`npm run package` names the VSIX from the extension manifest. `npm run smoke` compiles, packages, opens that VSIX, and verifies its manifest, commands, semantic-token mapping, and debug-adapter entry points. `npm run smoke:cursor` additionally installs it into Cursor and verifies the manifest's current publisher, name, and version are registered. `npm run smoke:activation` launches an isolated official extension host and exercises activation, language diagnostics, semantic tokens, synchronized-buffer execution, and a zero-`launch.json` debug session; when headed Chrome is available, it also verifies the breakpoint stack frame.

Open a `.webtest` file and run **WebTest: Run Current File** from the Command Palette or the editor title button. Runtime failures are published against the source revision that was executed, so stale diagnostics disappear after an edit.

Set a breakpoint on an `open`, `click`, or `expect` statement, then run **WebTest: Debug Current File**. The default debugger launches one headed Chrome window and pauses before the marked operation without requiring a `launch.json`. The WebTest scope shows the current test, operation, source line, line number, and every binding evaluated so far. Records, lists, responses, headers, decoded JSON, and process results are expandable. Server-only values remain available, with secrets redacted, while stepping into the browser block. The debug session closes automatically after the final step.


The Testing view discovers a workspace → module → test → default variant tree from shared Rust analysis. Run and Debug profiles route stable variant IDs, so duplicate labels remain independently selectable. Runs use synchronized buffers and one captured workspace snapshot; cancellation waits for runtime cleanup. Results retain source locations and evidence, including shared fixture teardown failures. Gutter runs print named variant outcomes, durations, and verdict reasons in Test Results; skipped and inconclusive verdicts use the editor’s skipped status. `npm run test:testing` checks selection, exclusions, debug routing, result messages, and cancellation with a mocked Testing API.

Workspace language services include import-path and auto-import completion, typed helper/fixture/provider completion and signatures, hover, definitions, references, rename, symbols, folding, selection ranges, code actions, inlay hints, and semantic tokens. Pure and statement helper bodies and fixture setup/teardown have source breakpoints and task frames. Debug restart retains the captured dependency sources.

Incremental edits validate UTF-16 boundaries and document versions. Invalid edits request a full buffer resynchronization through `webtest/resyncRequired`. Configuration/schema and source-file changes refresh explicit project inputs while preserving unsaved buffers, including newly added imports. Multiple workspace folders are rejected explicitly. Static project metadata is also available through `webtest describe project.declarations` and `webtest/describeProject`; it never runs the application.

## Verified release installation

Run **WebTest: Install/Update Server**, enter an exact release version, and confirm
the download. The extension verifies the Sigstore bundle against the exact
`williamcotton/webtest` release workflow tag identity and GitHub OIDC issuer, then
checks platform, protocol compatibility, size and SHA-256 before activating a raw
binary in global extension storage. It never extracts downloaded archives or
silently downloads on activation. Reload after installation. Existing immutable
version directories remain available for rollback; select a previous executable
with `webtest.serverPath` if needed.

Resolution order: explicit `webtest.serverPath`, workspace `target/debug/webtest`,
PATH, verified managed release. Unsupported platforms receive source-build/manual
installation guidance. A missing server leaves the Install/Update command usable.
No telemetry or document upload is added. Release documentation and support:
https://github.com/williamcotton/webtest/tree/main/release
