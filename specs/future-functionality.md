# WebTest — Post-Vertical-Slice Product Specification

## 0. Status and relationship to the initial specification

This document specifies the functionality to build after the initial vertical slice described in [`intitial-vertical-slice.md`](./intitial-vertical-slice.md). It is based on the repository as implemented, not only on the aspirations in the original specification.

The purpose of the next stages is to turn the proven architecture into a useful web-application testing product while preserving its defining constraints:

- one Rust lexer, parser, lossless CST, and syntax-to-HIR lowering path;
- typed AST nodes as Rowan CST views;
- protocol-neutral analysis and editor services;
- execution through a source-mapped `TestPlan`;
- browser semantics independent of CDP;
- structured runtime events and revision-bound observations;
- Tower LSP, Cursor/VS Code, Monaco, CLI, and terminal output as adapters;
- one native `webtest` executable.

This document does not redefine the completed slice. When it conflicts with an implementation detail that is already necessary to preserve the architecture, the architectural invariant wins and the implementation should be migrated deliberately.

---

## 1. Implemented baseline

The repository currently provides:

- a Rust 2024 Cargo workspace with separate text, syntax, HIR, analysis, formatting, planning, browser, runtime, observation, editor, LSP, WASM, and application crates;
- a lossless, error-tolerant Rowan CST preserving whitespace, line comments, malformed tokens, punctuation, and source ranges;
- typed CST wrappers for tests, browser blocks, `open`, `click`, and `id(...)` locators;
- HIR and `TestPlan` lowering with typed test/step IDs, BLAKE3 source revisions, and precise locator origins;
- syntax diagnostics and a CST-based formatter;
- a protocol-neutral browser API and a direct, hand-written CDP subset that launches local Chrome, navigates, evaluates JavaScript safely, and clicks by ID;
- sequential runtime execution, structured events, and process-local runtime observations;
- revision-safe editor diagnostics, formatting, running, and CST-backed semantic tokens;
- a Tower LSP server using full-document synchronization, formatting, diagnostics, semantic tokens, and `webtest.runFile`;
- a Cursor/VS Code VSIX that runs the synchronized editor buffer;
- `webtest check`, `webtest fmt`, `webtest test`, and `webtest lsp`;
- a WASM-compatible facade exposing diagnostics and formatting.

Known limitations are intentional: the language has no expressions, bindings, assertions, server domain, modules, or static type system; runtime is sequential; locator behavior is ID-only and not actionable; CDP bindings are hand-written; Chrome is not managed; observations do not cross processes; LSP sync is full-file; and the WASM API is not yet a complete editor service.

---

## 2. Target product model

The native executable should grow toward:

```text
webtest check [paths...]
webtest fmt [paths...] [--check|--stdout]
webtest test [paths...] [--filter PATTERN] [--tag TAG] [--jobs N]
webtest run <file> [--test NAME]
webtest build [paths...] [--emit plan.json]
webtest lsp
webtest repl
webtest trace <artifact>
webtest browser install|list|path|clean
```

`test` is the test-suite command. `run` executes an explicitly selected file or test and may favor interactive output. `build` performs analysis and emits a versioned, serializable `TestPlan` without executing it. All commands must call the same analysis, formatting, planning, and runtime APIs used by editor services.

A project-level `webtest.toml` should eventually define source roots, default timeouts, browser selection, tags, environment profiles, artifact paths, and managed-browser policy. Configuration inputs must participate in query invalidation and plan identity.

---

## 3. Language evolution

### 3.1 Syntax and expressions

Extend the existing grammar incrementally. Every addition must preserve lossless parsing and useful recovery for incomplete editor input.

Required expression forms:

```text
string, integer, boolean, null, duration
list and record literals
name references
member access
function calls
named arguments
unary and binary operators
```

Required statements:

```text
let binding
expression/action statement
assertion
server block
browser block
sequence, parallel, race, retry, and timeout blocks
```

Illustrative syntax:

```webtest
test "password reset" tags ["browser", "mail"] {
    server {
        let user = fixture.user(email: "me@example.test")
        let reset = http.post("/api/password-reset", json: {
            email: user.email,
        })
        expect reset.status == 202
    }

    browser {
        open "/forgot-password"
        fill label("Email") with user.email
        click role("button", name: "Send reset link")
        expect text("Check your email").visible
    }
}
```

New syntax kinds and AST wrappers must be added to `syntax`; downstream layers must never scan source text or construct an alternate parse tree.

### 3.2 Types and execution capabilities

Introduce a static type system in HIR/analysis with at least:

```text
String, Int, Bool, Null, Duration, Url, Json
List<T>, Option<T>, records
StatusCode, Headers, Request, Response<T>
Locator, Element
Browser, BrowserContext, Page
ProcessResult, FilePath
```

Operations also have an execution capability:

```text
Pure | Server | Browser | Test
```

Analysis must reject operations used in the wrong domain before planning. Values crossing between server and browser blocks must be serializable and explicitly transferable. Strings, numbers, booleans, URLs, and JSON-like records are transferable; pages, sockets, database connections, browser element handles, and file handles are not.

Diagnostics should explain both the expected type/capability and the actual one, point to the smallest useful source range, and offer fixes through protocol-neutral code actions where possible.

### 3.3 Bindings, scope, and identity

Bindings are lexically scoped. Names must resolve to stable semantic IDs rather than strings. Shadowing policy must be explicit and diagnosed consistently. HIR should move toward arena-backed IDs or another compact representation suitable for incremental queries.

Test identity must remain stable enough for filtering, observations, traces, and editor decorations. A test's display name is not its only identity; project, module path, declaration origin, and an internal ID should form the durable identity.

### 3.4 Modules, packages, and fixtures

Add a module graph only after single-file expressions and types are stable. Projects should support imports, reusable fixture declarations, helper functions, and project configuration without executing arbitrary code during analysis.

Fixture lifetimes should be explicit:

```text
test | file | worker | suite
```

Setup and teardown must be represented in the plan and event stream so cancellation and failure cannot silently skip cleanup.

---

## 4. Browser testing functionality

### 4.1 Locator model

Expand the shared semantic locator model to support:

```rust
Id(String)
Role { role, name }
Label(String)
Text(String)
Placeholder(String)
TestId(String)
Css(String)
XPath(String)
```

Role, label, text, and test-ID locators should be the preferred user-facing forms. CSS and XPath remain escape hatches. Locator resolution must return structured results distinguishing zero, one, and multiple matches and should retain candidate/evidence information for diagnostics.

### 4.2 Actions

Add, in order of value:

```text
open, click, fill, type, press
check, uncheck, select
hover, focus, blur
scroll, upload, download
new page/tab, close page
wait for URL, event, response, or locator state
```

Language-level actions remain independent of CDP request names. Browser traits may evolve into context/page/element abstractions, but CDP types must not escape `browser-cdp`.

### 4.3 Actionability and auto-waiting

Replace JavaScript `element.click()` with an actionability engine in the browser abstraction. A click should:

1. repeatedly resolve the locator until its timeout;
2. require exactly one attached element;
3. verify visibility, stability, enabled state, and pointer hit testing;
4. scroll into view;
5. calculate a valid click point;
6. dispatch physical input through CDP;
7. observe navigation or resulting page state.

Timeouts, retries, and polling intervals must be configurable and observable. Failures must say which actionability condition failed and attach available evidence.

### 4.4 Browser contexts and state

Support isolated contexts, pages, cookies, local/session storage, permissions, geolocation, viewport, user agent, locale, timezone, downloads, dialogs, console events, and network events. Tests should default to isolation. Reuse a browser process for performance without reusing test state unless explicitly requested.

### 4.5 Network control

After passive network events are reliable, add request routing, aborting, modification, and deterministic response fulfillment. Matchers should operate on structured URL, method, headers, and resource type rather than raw CDP events. Mocks and their invocation counts must appear in the plan, event stream, assertions, and trace.

---

## 5. Server-side execution domains

### 5.1 HTTP

Provide typed HTTP operations for methods, headers, query parameters, JSON/form bodies, authentication, cookies, redirects, timeouts, and response decoding. HTTP results must expose status, headers, body text/bytes, and typed JSON.

HTTP assertions should produce structured diffs rather than flattened messages. Sensitive headers and configured JSON fields must be redacted from logs and traces.

### 5.2 Processes

Provide process operations with explicit executable, arguments, environment, working directory, stdin, timeout, exit status, stdout, and stderr. Process execution is a native host capability, unavailable in WASM analysis. Cancellation must terminate the child process tree where the platform permits.

The runtime must never invoke a shell implicitly. Shell syntax should require an explicit shell operation and a clear security warning.

### 5.3 Files and fixtures

Add sandbox-aware file operations for temporary directories, fixture copying, and assertions over files. Prefer typed temporary resources that clean up automatically. Project-relative paths must resolve through workspace configuration, not the process's accidental working directory.

### 5.4 Databases and services

Database integrations are later adapters behind server-domain traits. Start with fixture/service interfaces and transferable DTOs; do not embed database-specific syntax into the core grammar until at least one reusable capability model is proven.

---

## 6. Assertions and diagnostics

Add `expect` with typed matchers for equality, inequality, containment, patterns, status codes, JSON structures, locator states, URLs, and eventually visual snapshots.

Snapshot storage must use deterministic project-relative names. Creating or updating snapshots requires an explicit CLI/editor action; ordinary test execution must never silently accept a new result.

Assertion failures must preserve:

- the expression and matcher source ranges;
- expected and actual typed values;
- a structured diff when applicable;
- relevant page/request/process context;
- source revision, execution, test, and step identity.

Avoid compiling assertions into opaque callbacks. They must remain visible in HIR, `TestPlan`, events, traces, and editor observations.

---

## 7. Structured execution and `TestPlan`

Grow `TestOperation` into a versioned IR containing:

```text
Browser, Http, Process, File, Fixture, Assertion
Sequence, Parallel, Race, Retry, Timeout
Setup, Teardown
```

Plans must be deterministic, serializable, and independent of syntax nodes. Every executable or controlling node carries a stable plan ID, source origin, capability requirements, timeout policy, and source revision.

The runtime scheduler must provide structured concurrency:

- child operations cannot outlive their parent scope;
- cancellation propagates predictably;
- `race` cancels losers and awaits cleanup;
- `retry` records each attempt and its evidence;
- `timeout` distinguishes timeout from cancellation and underlying failure;
- teardown executes once when its resource was acquired.

Parallel test execution should be added only after resource isolation and deterministic event ordering are defined. `--jobs` controls test-level concurrency; concurrency inside a test is expressed by the plan.

---

## 8. Runtime events, observations, and traces

### 8.1 Event schema

Extend the existing event stream with timestamps, parent operation IDs, attempt numbers, durations, captured output, attachments, and explicit cancellation/infrastructure events. The event schema should be serializable and versioned so terminal reporters, traces, editor observations, and future remote runners consume the same facts.

Reporters subscribe to events. The runner must not print directly.

### 8.2 Observation lifecycle

Add observation kinds for ambiguous locators, assertion mismatches, navigation failures, HTTP failures, console errors, network failures, observed values, timings, and retries.

The store must atomically replace observations for a completed file/revision execution. New runs clear prior current observations; successful reruns remove prior failures. No adapter may publish an observation when its `SourceRevision` differs from the current document revision.

### 8.3 CLI-to-LSP bridge

Implement local, versioned IPC so `webtest test` can publish observations to a running `webtest lsp` process. Use Unix sockets on Unix and named pipes on Windows. The LSP process advertises a workspace-scoped endpoint through a metadata file containing PID, workspace identity, endpoint, and protocol version.

Messages contain canonical path/URI, source revision, execution/test/step IDs, range, observation kind, and evidence references. The receiver validates workspace membership, protocol version, and revision before storing anything.

### 8.4 Trace artifacts

Add a portable trace container containing:

```text
manifest.json
events.jsonl
sources and revisions
attachments/
screenshots/
network/
console/
DOM or accessibility snapshots
```

`webtest trace` should open or serve an HTML viewer. Trace capture levels must be configurable to control size and sensitive data.

---

## 9. CDP and managed browsers

Replace the hand-written CDP subset incrementally with generated typed bindings from a pinned Chrome DevTools Protocol schema. Keep a generic command path for protocol evolution and debugging, but normal browser behavior should use generated request/response/event types.

The connection layer must handle target discovery, flattened sessions, events, request cancellation, disconnects, browser crashes, protocol timeouts, and graceful shutdown. Add bounded queues and prevent a slow observer from blocking CDP reads.

Introduce a `BrowserManager` and commands:

```text
webtest browser install
webtest browser list
webtest browser path
webtest browser clean
```

Each WebTest release should declare a tested Chrome for Testing version and verify archive checksums. Store browsers under a platform cache directory, not inside the executable. Resolution order should be explicit: CLI/config override, `WEBTEST_CHROME_PATH`, managed pinned browser, then supported system discovery.

Chrome/CDP remains the primary backend until browser semantics and actionability are stable. The `BrowserHost` boundary should permit later WebDriver BiDi or other backends for Firefox and WebKit without adding browser-specific constructs to the DSL. Cross-browser support must pass the same semantic browser conformance suite.

Never use the user's browser profile or add `--no-sandbox` by default.

---

## 10. Analysis and workspace model

The current database uses revision-keyed memoization suitable for one-file analysis. Before modules and large workspaces, move to a formal incremental query model, preferably Salsa, or document an equivalent dependency-tracked design.

Required inputs include file text, paths/URIs, configuration, environment profile metadata, and module/package graph. Required queries include parse, HIR, name resolution, types/effects, static diagnostics, plan construction, symbols, and editor features.

Full-file reparsing remains acceptable until profiling proves it problematic. Query reuse must prevent independent reparses by formatting, diagnostics, semantic tokens, planning, and editor requests.

---

## 11. Editor services and native LSP

Add protocol-neutral services in this order:

1. document symbols, folding ranges, and selection ranges;
2. completion and signature help;
3. hover with resolved types/capabilities;
4. definition, references, and rename;
5. code actions for syntax/type/runtime failures;
6. inlay hints and richer semantic tokens;
7. workspace symbols and test discovery.

Runtime-aware features may show match counts, observed values, timings, or candidate locators, but only for the current source revision.

Tower LSP remains an adapter. Add incremental synchronization, cancellation, progress reporting, pull diagnostics if useful, test discovery commands, and protocol-level integration tests. LSP handlers must not implement name resolution, completion logic, or runtime formatting.

The Cursor/VS Code extension remains TypeScript glue. It may provide commands, test-explorer UI, trace opening, and settings, but no parser or semantic model. Server and UI command identifiers must remain distinct to avoid command-registration collisions.

---

## 12. WASM, Monaco, and browser editor package

Expand `webtest-wasm` from diagnostics/formatting into stable DTO-based APIs:

```text
openDocument(uri, text)
updateDocument(uri, text)
closeDocument(uri)
diagnostics(uri)
format(uri)
semanticTokens(uri)
completion(uri, offset)
hover(uri, offset)
compileTestPlan(uri)
```

Run the WASM module in a Web Worker and communicate with Monaco through messages. Monaco should call editor services directly; it should not require an in-browser LSP server.

Publish `@webtest/editor` containing WASM, worker glue, TypeScript declarations, and a Monaco adapter. Native-only plan operations should still compile in WASM and be marked as requiring host capabilities; the browser build does not launch Chrome or native processes.

Parity tests must run the same syntax, diagnostics, formatting, semantic-token, and plan fixtures against native and WASM builds.

---

## 13. CLI, reporting, and developer experience

Add human, concise, JSON, JUnit, and event-stream reporters. Color must be disabled when output is not a terminal or when requested. Machine-readable output schemas must be versioned.

CLI diagnostics should show source snippets, labels, related ranges, and suggested fixes. Test output should distinguish assertion/test failure from browser/process infrastructure failure and internal bugs through stable exit codes.

Support directory discovery, ignore rules, test/tag filtering, fail-fast, deterministic seeds, default/configurable timeouts, and artifact directories. Watch mode may be added after incremental workspace invalidation is reliable.

`repl` is a late feature and must reuse the language and runtime rather than evaluating a second ad hoc command language.

---

## 14. Security and privacy

The product intentionally drives browsers, networks, files, and processes, but it must still:

- safely serialize values crossing into JavaScript or protocols;
- avoid implicit shell interpretation;
- redact configured secrets from logs, diagnostics, IPC, and traces;
- bind CDP and local IPC only to local/private endpoints;
- use isolated temporary browser profiles;
- validate downloaded browser checksums;
- bound captured body, DOM, console, and process output sizes;
- make destructive file/process operations explicit;
- document that arbitrary test projects should be treated as executable code.

WASM/editor analysis must not gain ambient filesystem or network access.

---

## 15. Testing and quality gates

Every language feature needs:

- lossless valid, invalid, and half-typed syntax fixtures;
- typed AST and exact-range tests;
- HIR/type/effect diagnostics tests;
- deterministic plan snapshots;
- fake-host runtime tests for success, failure, retry, timeout, and cancellation;
- editor revision-safety tests;
- LSP UTF-16 and protocol tests;
- native/WASM parity coverage where portable.

Use real Chrome tests for navigation, locators, actionability, input, contexts, events, and evidence. Serve fixture pages from random loopback ports and skip only when the environment truly lacks Chrome or socket capability. Add parser fuzzing and property tests for losslessness and non-panicking malformed input.

Required gates remain:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run compile
cargo check -p webtest-wasm --target wasm32-unknown-unknown
```

Release CI should additionally run cross-platform browser integration tests and package/install smoke tests for the VSIX and npm editor package.

---

## 16. Distribution

Ship precompiled `webtest` binaries for supported macOS, Linux, and Windows targets. Distribution may include release archives, shell/PowerShell installers, Homebrew, winget/Scoop, an optional npm launcher, and a container image.

The official CI container should contain `webtest`, the matching managed Chrome for Testing build, and required Linux libraries. Chromium itself should not be embedded in the Rust executable.

Serializable plans and versioned events may later support remote workers. A remote runner must negotiate plan/event protocol versions and host capabilities, preserve source revisions, and produce the same events as local execution; it must not introduce a second compiler.

Publish the Cursor/VS Code extension as a standard VSIX and, eventually, through compatible marketplaces. Keep server discovery configurable and do not bundle a platform-specific executable into one universal VSIX unless a deliberate multi-platform packaging design is adopted.

---

## 17. Delivery milestones

### Milestone A — Productize the proven slice

- project configuration and path discovery;
- managed Chrome for Testing;
- improved CLI diagnostics/reporters and stable exit codes;
- robust CDP disconnect/timeouts;
- protocol-level LSP tests and packaged extension smoke tests.

Acceptance: a new user can install WebTest and its browser, run the examples, and see current runtime diagnostics without manually locating Chrome.

### Milestone B — Useful browser testing

- semantic locators;
- fill, press, select, check, hover, and waits;
- actionability and auto-waiting;
- locator and page-state assertions;
- browser reuse with isolated contexts;
- screenshots and core evidence.

Acceptance: common form and navigation flows are reliable without manual sleeps or CSS selectors.

### Milestone C — Typed server/browser workflows

- expressions, bindings, records, and functions;
- static types and execution capabilities;
- HTTP, process, file, and fixture operations;
- transferable cross-domain values;
- typed assertions and structured diffs.

Acceptance: one test can prepare state through a server operation, use it in a browser, and receive static errors for invalid domain/type transfers.

### Milestone D — Structured execution and observability

- parallel/race/retry/timeout plan nodes;
- cancellation-safe resource lifecycles;
- expanded event and observation schemas;
- CLI-to-LSP observation IPC;
- trace artifact and viewer.

Acceptance: concurrent/retried tests remain deterministic, diagnosable, and source-mapped in terminal, trace, and editor.

### Milestone E — Workspace and editor intelligence

- incremental query database and module graph;
- completion, hover, navigation, rename, symbols, actions, and test discovery;
- incremental LSP synchronization;
- modules and reusable fixtures.

Acceptance: multi-file projects remain responsive and every editor feature derives from shared Rust semantics.

### Milestone F — Portable editor and distribution

- complete WASM editor-service facade and worker;
- Monaco adapter and `@webtest/editor`;
- native release automation, VSIX publishing, and CI container;
- native/WASM parity suite.

Acceptance: native, Cursor/VS Code, and Monaco experiences agree on syntax, formatting, diagnostics, semantic tokens, and compiled plans.

---

## 18. Definition of long-term success

WebTest is successful when a user can install one native product, author statically checked tests spanning server and browser domains, run them reliably with managed Chrome and structured concurrency, inspect rich traces, and receive current runtime facts directly in their editor.

The same Rust language implementation must power CLI, runtime planning, Cursor/VS Code, and Monaco. Adding functionality must deepen that shared implementation rather than creating adapter-specific parsers, semantics, or diagnostics.
