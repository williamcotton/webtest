# WebTest

WebTest is a statically analyzable language for typed web-system tests. One lossless parser feeds checking, formatting, plan compilation, execution, debugging, and editor diagnostics. Native HTTP, process, filesystem, language-neutral application bridge, and direct Chrome DevTools Protocol backends run behind shared typed contracts.

```webtest
test "created user can sign in" {
    server {
        let response = http.post("/api/test/users", json: {
            email: "alice@example.com",
        })
        expect response.status == 201
        let user: { id: Int, email: String } = response.json
    }

    browser {
        open "/login"
        fill label("Email") with user.email
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
```

## Install and use

Install the native executable from this checkout with Rust stable. Cargo places `webtest` in its bin directory (normally `~/.cargo/bin`), so it is available to Cursor/VS Code and from any project directory:

```sh
cargo install --path crates/app --locked
webtest --version
```

Bootstrap a new or existing application directory before authoring tests:

```sh
webtest init path/to/application
```

Initialization creates a non-overwriting, language-neutral starter project with
`webtest.toml`, `.webtest/app-schema.json`, `tests/example.webtest`, and the
WebTest agent skill. The generated `app.echo` test statically checks immediately;
configure `[app]` and implement the Protocol 1 echo operation before executing it.
The canonical skill lives at `.agents/skills/webtest`, with a relative
`.claude/skills/webtest` compatibility link. Its application-bridge section routes
agents through `webtest describe app`, `app.schema`, `app.protocol`,
`app.pseudocode`, and the generated `app.echo` operation instead of duplicating the
wire contract in prompt text.

For development, `cargo build` keeps the executable at `target/debug/webtest`. Install the tested Chrome for Testing release into WebTest's versioned cache after either setup. Ordinary test runs never download a browser.

```sh
webtest browser install
webtest browser path
webtest check examples/plain-html
webtest test examples/plain-html
webtest build examples/plain-html --emit plan.json
```

All file-oriented commands accept zero or more files/directories. With no paths, WebTest finds the nearest `webtest.toml` and deterministically discovers `.webtest` files under `project.test_roots`. Useful variants are:

```sh
target/debug/webtest check examples/plain-html/sign-in.webtest --reporter json
target/debug/webtest fmt examples/plain-html --check
target/debug/webtest test examples/plain-html/sign-in.webtest --headed
target/debug/webtest test examples/plain-html --reporter junit
target/debug/webtest browser list
target/debug/webtest browser clean
target/debug/webtest lsp
target/debug/webtest dap
```

Browser resolution is `--chrome-path`, `WEBTEST_CHROME_PATH`, `browser.path`, managed Chrome, then supported system locations. Set `WEBTEST_CACHE_DIR` to relocate the managed cache. Tests are headless by default; `--headed` shows Chrome.

The default human `test` reporter streams progress while it works: static checking, application and health-check readiness, per-file Chrome startup and shutdown, and each active test. Concise, JSON, events, and JUnit reporters remain free of human progress text.

Exit codes are stable: `0` success, `1` static/test/format failure, `2` CLI/config/input error, `3` browser/CDP/filesystem/reporter infrastructure failure, and `4` internal invariant failure. JSON and JSONL event output use `schema_version: 1`; machine durations are integer nanoseconds. JUnit distinguishes test failures from infrastructure errors.

## Project configuration

The initial `webtest.toml` schema is intentionally small:

```toml
[project]
name = "storefront"
test_roots = ["tests"]
exclude = ["tests/generated/**", "node_modules/**"]

[browser]
headless = true
channel = "managed" # or "system"
# path = "/absolute/path/to/chrome"

[server]
base_url = "http://127.0.0.1:3000"

[server.http]
follow_redirects = true
max_response_bytes = 8388608

[server.process]
allowed_working_roots = ["."]
max_output_bytes = 1048576

[server.fs]
read_roots = ["fixtures"]
write_root = ".webtest/tmp"

# Optional runner-owned application plus typed app.* bridge.
[app]
command = "node"
args = ["server.js"]
working_directory = "."

[app.environment]
WEBTEST = "1"

[app.health]
url = "http://127.0.0.1:3000/health"
timeout = "10s"

[server.app]
adapter = "bridge"
transport = "auto"
schema = ".webtest/app-schema.json"

[redaction]
headers = ["authorization", "cookie", "set-cookie"]
json_fields = ["password", "token", "secret"]

[timeouts]
browser_command = "10s"
navigation = "30s"
test = "60s"

[artifacts]
directory = ".webtest/artifacts"
```

Durations accept positive `ms`, `s`, or `m` values. Test roots, provider roots, and artifact paths must remain project-relative. Filesystem and process working roots are canonicalized before use. Excludes use slash-normalized glob syntax: `*` matches within one path component and `**` crosses directories. Hidden and symlinked discovery directories are not traversed. Unknown keys warn; malformed values and contradictory `browser.channel = "system"` plus `browser.path` are errors.

`webtest build --emit` writes a versioned plan only after successful static analysis. Emission refuses literal values in configured secret fields or schema-secret provider arguments; it never writes a redacted placeholder that would alter execution semantics.

## Examples

Each directory under `examples` is an independent WebTest project with its own configuration. For the plain HTML browser example, serve the fixture in one terminal:

```sh
cd examples/plain-html
python3 -m http.server 3000 --bind 127.0.0.1
```

Then run from the repository root:

```sh
target/debug/webtest test examples/plain-html/sign-in.webtest --headed
```

The server-backed example uses only the Python standard library. Start its application in one terminal:

```sh
python3 examples/simple-server/server.py
```

Then run its typed server-to-browser workflow:

```sh
target/debug/webtest test examples/simple-server/created-user.webtest --headed
```

The [application-bridge matrix](examples/application-bridge/README.md) runs the same typed
`app.create_user` flow against Node, Ruby, Go, Python, Elixir, Java, .NET, Rust, and PHP applications.
The checked manifest powers `describe`, `check`, LSP, and WASM services while those applications are
stopped. Build once, verify the matrix, then smoke any installed host runtime:

```sh
cargo build
python3 scripts/check-application-bridge-examples.py
python3 scripts/check-application-bridge-examples.py --example node --smoke
```

Protocol compatibility and SDK commands are documented in [protocol/protocol.md](protocol/protocol.md),
[sdks/node/README.md](sdks/node/README.md), and [sdks/ruby/README.md](sdks/ruby/README.md).

## VS Code / Cursor extension development

```sh
cd editors/vscode
npm install
npm run compile
npm run smoke
npm run smoke:cursor
npm run smoke:activation
```

Reload VS Code/Cursor after installing the VSIX, then open the repository and a `.webtest` file. The extension discovers `target/debug/webtest` in the workspace or a Cargo-installed `webtest` automatically, including in GUI-launched editor windows whose shell `PATH` omits `~/.cargo/bin`. For each opened file, the language server discovers the nearest `webtest.toml`, so project-supplied provider manifests drive diagnostics, completion, hover, and synchronized-buffer execution even when a parent repository is the editor workspace. Changes to the resolved `webtest.toml` or application manifest invalidate that cached semantic input and republish diagnostics without restarting the language server. The command **WebTest: Run Current File** asks the language server to run the currently synchronized buffer, including unsaved changes. Set `webtest.serverPath` explicitly when the executable lives elsewhere.

For interactive debugging, set a breakpoint on a provider call, assertion, or browser operation and choose **WebTest: Debug Current File** (or press F5 and select **Debug WebTest**). The adapter discovers the nearest `webtest.toml` from the selected test file even when a parent repository is the editor workspace. Debug sessions show one Chrome window by default, pause before the selected source-mapped plan step, and expose all already-evaluated bindings—including server-only HTTP responses and process results—with configured secrets redacted. Records, lists, headers, responses, decoded JSON, and process results expand into nested debugger variables. Bindings remain visible as execution moves into later browser steps, and the debug session closes when execution finishes. Continue or step from VS Code/Cursor's debug toolbar. No `launch.json` is required.

## Architecture

The architectural invariants are deliberate:

- one lexer and parser produce the canonical, lossless Rowan CST;
- typed AST nodes are CST views and HIR has one lowering path;
- the formatter consumes CST tokens so comments are retained;
- runtime executes a source-mapped `TestPlan`, never syntax nodes;
- browser behavior is protocol-neutral and CDP is only one backend;
- application integration uses one generated, authenticated bridge protocol and the shared provider path;
- runtime failures remain structured observations tagged with a BLAKE3 source revision;
- editor services contain no LSP types; Tower LSP and VS Code are adapters;
- stdout belongs exclusively to LSP while `webtest lsp` is running.

Run the complete quality suite with:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p webtest-wasm --target wasm32-unknown-unknown
cd editors/vscode && npm run smoke
cd editors/vscode && npm run smoke:activation
```
