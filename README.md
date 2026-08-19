# WebTest

WebTest is a small, statically analyzable language for browser tests. This repository contains the first complete vertical slice: one lossless parser feeds checking, formatting, execution, and editor diagnostics, while a direct Chrome DevTools Protocol backend runs the browser steps.

```webtest
test "submit button" {
    browser {
        open "http://127.0.0.1:3000"
        click id("submit")
        expect text("submitted").visible
    }
}
```

## Build and use

Rust stable and Chrome/Chromium are required. WebTest searches standard Chrome locations; use `WEBTEST_CHROME_PATH` or `--chrome-path` to override it.

```sh
cargo build
target/debug/webtest check examples/minimal/passing.webtest
target/debug/webtest fmt examples/minimal/passing.webtest
target/debug/webtest test examples/minimal/passing.webtest
target/debug/webtest test examples/minimal/passing.webtest --headed
target/debug/webtest lsp
target/debug/webtest dap
```

Tests run headlessly by default. Pass `--headed` to watch Chrome execute the test. The examples expect a site on port 3000. Automated browser tests instead start a fixture server on a random loopback port and skip gracefully when Chrome is unavailable.

## Cursor / VS Code extension development

```sh
cd editors/vscode
npm install
npm run compile
npm run package
cursor --install-extension webtest-vscode-0.2.0.vsix --force
```

Reload Cursor after installing the VSIX, then open the repository and a `.webtest` file. The extension discovers `target/debug/webtest` in the workspace automatically. The command **WebTest: Run Current File** asks the language server to run the currently synchronized buffer, including unsaved changes. Set `webtest.serverPath` explicitly when the executable lives elsewhere.

For interactive debugging, set a breakpoint on any `open`, `click`, or `expect` line and choose **WebTest: Debug Current File** (or press F5 and select **Debug WebTest**). Debug sessions show Chrome by default and pause immediately before the selected step, leaving the page available for inspection and Chrome DevTools. Continue or step from Cursor's debug toolbar. No `launch.json` is required.

## Architecture

The architectural invariants are deliberate:

- one lexer and parser produce the canonical, lossless Rowan CST;
- typed AST nodes are CST views and HIR has one lowering path;
- the formatter consumes CST tokens so comments are retained;
- runtime executes a source-mapped `TestPlan`, never syntax nodes;
- browser behavior is protocol-neutral and CDP is only one backend;
- runtime failures remain structured observations tagged with a BLAKE3 source revision;
- editor services contain no LSP types; Tower LSP and VS Code are adapters;
- stdout belongs exclusively to LSP while `webtest lsp` is running.

Run the complete quality suite with:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
