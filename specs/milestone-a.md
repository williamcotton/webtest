# Milestone A — Productize the Proven Slice

**Status: implemented.** 

## 0. Status and relationship to the roadmap

This specification expands Milestone A in [`future-functionality.md`](./future-functionality.md). It begins from the delivered lossless parser, source-mapped plan/runtime, direct CDP backend, runtime diagnostics, headed mode, Tower LSP, DAP, and Cursor/VS Code extension.

Milestone A changes packaging, configuration, discovery, reporting, and operational reliability. It must not add a second parser, browser runtime, diagnostic engine, or editor semantic model.

## 1. Outcome

A new user can install `webtest`, install or select a supported Chrome, open a project, and run its tests without knowing repository internals or manually locating binaries. Failures have stable exit codes, useful source snippets, and actionable infrastructure messages. LSP, DAP, and the packaged VSIX are covered by protocol-level smoke tests.

The reference journey is:

```text
install webtest
    |
    v
webtest browser install
    |
    v
webtest check
    |
    v
webtest test
    |
    +--> human diagnostics in a terminal
    +--> JSON/JUnit in CI
    +--> current diagnostics and breakpoints in Cursor
```

## 2. Scope

Milestone A includes:

- project-root, configuration, and `.webtest` file discovery;
- a deliberately small first `webtest.toml` schema;
- managed Chrome for Testing installation and resolution;
- robust CDP timeouts, disconnect handling, and shutdown;
- path-oriented `check`, `fmt`, and `test` commands;
- human, concise, JSON, JUnit, and event-stream reporting foundations;
- stable failure classes and process exit codes;
- cross-platform LSP/DAP protocol tests;
- VSIX packaging and install/activation smoke tests.

## 3. Non-goals

This milestone does not add new DSL syntax, locators, browser actions, typed expressions, server providers, application lifecycle, concurrency, modules, traces, or full WASM editor services. It does not add Playwright or WebDriver. It does not silently download Chrome during an ordinary test run.

## 4. Project discovery and configuration

### 4.1 Root selection

For each explicit input, walk from the file or directory toward the filesystem root and select the nearest `webtest.toml`. With no explicit path, start at the current directory. The directory containing the selected configuration is the project root.

Milestone A does not merge nested configurations. If one command resolves inputs to different project roots, fail with a configuration diagnostic and require separate invocations. Canonicalize paths for identity while preserving a user-facing relative path for output.

If no configuration exists, use the current directory as an implicit project with conservative defaults. Explicit files still work outside a configured project.

### 4.2 Initial configuration schema

```toml
[project]
name = "storefront"
test_roots = ["tests"]
exclude = ["target/**", "node_modules/**"]

[browser]
headless = true
channel = "managed"
# path = "/absolute/path/to/chrome"

[timeouts]
browser_command = "10s"
navigation = "30s"
test = "60s"

[artifacts]
directory = ".webtest/artifacts"
```

Unknown keys produce warnings in this milestone so newer configuration remains forward-compatible. Invalid types, paths escaping the project where prohibited, malformed durations, and contradictory browser settings are errors.

Configuration precedence is:

```text
CLI flag
    > supported WEBTEST_* environment variable
    > webtest.toml
    > product default
```

The resolved configuration is an immutable typed value passed into analysis/runtime composition. Configuration diagnostics use source ranges in `webtest.toml` when the TOML parser provides them; otherwise they identify the key path precisely.

### 4.3 Test discovery

`check`, `fmt`, and `test` accept zero or more files/directories. Zero inputs use `project.test_roots`, falling back to the project root. Directory traversal:

- includes files ending in `.webtest`;
- honors configured excludes and hidden-directory defaults;
- does not follow symlinked directories by default;
- detects canonical-path duplicates;
- sorts by normalized project-relative path for deterministic output;
- reports unreadable explicit inputs as errors rather than silently skipping them.

No ignore-file dialect is introduced until its compatibility rules are specified. Configuration patterns use one documented, cross-platform glob syntax.

## 5. CLI contract

Milestone A supports:

```text
webtest check [paths...] [--reporter human|concise|json]
webtest fmt [paths...] [--check]
webtest test [paths...] [--headed] [--reporter human|concise|json|junit|events]
webtest browser install [--version VERSION]
webtest browser list
webtest browser path
webtest browser clean [--version VERSION]
webtest lsp
webtest dap [--headless]
```

`fmt` rewrites files by default. `fmt --check` reports files that differ without modifying them. Multiple tests remain sequential; `--jobs` belongs to Milestone E.

### 5.1 Exit codes

Exit behavior is part of the public contract:

```text
0  requested operation completed successfully
1  static diagnostic, formatting mismatch, or test/assertion failure
2  CLI usage, configuration, or input-discovery error
3  infrastructure failure: Chrome, CDP, filesystem host, or reporter sink
4  internal invariant failure
```

For a run containing multiple outcomes, the highest-severity class wins in the order `internal > infrastructure > usage/configuration > test/static > success`. Machine reporters include the per-file/per-test class and final exit class.

## 6. Diagnostics and reporters

The runtime continues to emit structured events and observations; it never prints. Reporter adapters consume those facts.

### 6.1 Human reporter

Human diagnostics show the project-relative path, one-based line/column, error code, severity, message, source line, and the smallest useful underline. Runtime locator errors underline the locator. Color is enabled only on a compatible terminal and can be disabled with `NO_COLOR` or a CLI setting.

### 6.2 Concise reporter

The concise form emits one stable line per diagnostic/test result and a deterministic summary. It is suitable for logs but is not promised as a machine protocol.

### 6.3 Machine reporters

JSON and event-stream output use versioned envelopes and UTF-8. JSON produces one complete result document; events produce one JSON object per line. JUnit produces deterministic test case names and distinguishes failure from infrastructure error. Paths are project-relative where possible, durations use integer nanoseconds in machine formats, and source revisions remain explicit.

No reporter may include ANSI escapes, secrets, Chrome profile paths, or unbounded CDP payloads in machine output.

## 7. Managed Chrome

### 7.1 Browser manager

Introduce a Chrome-specific `BrowserManager` behind a narrow API. A dedicated native crate such as `crates/browser-manager` may own release metadata, downloads, cache layout, locking, checksums, and executable resolution. `browser-cdp` continues to own launch and protocol semantics; `app` composes both.

Resolution order is:

```text
--chrome-path
WEBTEST_CHROME_PATH
webtest.toml browser.path
installed managed Chrome for Testing version
supported system Chrome discovery
```

The resolved executable and provenance are available to diagnostics and `webtest browser path`.

### 7.2 Installation

Each WebTest release declares a tested Chrome for Testing version per supported platform. Installation:

1. resolves the platform/architecture artifact from pinned metadata;
2. downloads into a temporary file;
3. verifies the published cryptographic checksum;
4. extracts into a versioned temporary directory;
5. verifies the expected executable;
6. atomically renames into the cache;
7. records version, platform, checksum, and installation time.

Use a lock to prevent concurrent corruption. An interrupted install must leave the previous installation usable. `clean` removes only validated manager-owned version directories and never follows symlinks outside the cache. Ordinary `webtest test` reports the exact install command when no browser is available; it does not download implicitly.

## 8. CDP reliability

Every outbound CDP command has a deadline and correlation entry. Completion, protocol error, timeout, disconnect, browser exit, or cancellation removes that entry exactly once. A disconnect fails all pending calls promptly with structured infrastructure errors.

The connection layer must:

- bind remote debugging to loopback and use a fresh temporary profile;
- bound event and response queues;
- keep the socket reader independent from slow event consumers;
- distinguish command timeout, navigation timeout, disconnect, crash, and malformed protocol response;
- reject responses for unknown sessions/IDs without panicking;
- gracefully request shutdown, then terminate and reap Chrome after a bounded grace period;
- remove temporary profiles on normal exit and best-effort after failure;
- send logs only through `tracing` to stderr.

Never reuse a personal profile or add `--no-sandbox` by default.

## 9. Architecture and crate responsibilities

- `text`, `syntax`, `hir`, `plan`, and `format` remain semantically unchanged.
- A project/config component owns root resolution, typed configuration, and deterministic file discovery. Analysis receives resolved inputs; it does not read ambient configuration itself.
- `browser-manager` (or an equivalently isolated module) owns distribution/cache behavior, not CDP messages.
- `browser-cdp` owns process launch, connection lifecycle, target sessions, and structured protocol failures.
- `runtime` continues to emit results/events without terminal formatting.
- `app` parses CLI flags, selects reporters, applies precedence, and maps structured outcomes to exit codes.
- `lsp` and `dap` remain framed-protocol adapters with stdout reserved exclusively for protocol messages.
- `editors/vscode` contains discovery and UI glue only.

## 10. Delivery slices

1. Add typed project configuration and deterministic file discovery with tests.
2. Generalize CLI commands from one file to discovered input sets and add stable exit classes.
3. Extract reporter-neutral run summaries and implement human/concise reporters.
4. Add versioned JSON, events, and JUnit reporters with golden fixtures.
5. Implement managed Chrome metadata, cache, install/list/path/clean, and resolution precedence.
6. Harden CDP deadlines, disconnect fan-out, bounded queues, and child cleanup.
7. Add black-box LSP/DAP protocol harnesses and VSIX package/install smoke coverage.
8. Update examples, README, configuration reference, and release packaging.

Each slice must preserve a runnable workspace and use the existing `TestPlan`/`Runner` path.

## 11. Testing requirements

Required automated coverage includes:

- root/config precedence, invalid TOML, Unicode paths, symlinks, excludes, duplicates, and deterministic ordering;
- reporter golden tests for Unicode ranges, static errors, runtime failures, infrastructure errors, and successful reruns;
- exit-code tests for single and mixed outcomes;
- browser-manager tests against a local fake artifact server, including checksum mismatch, interruption, locking, and safe cleanup;
- fake-CDP tests for timeouts, out-of-order replies, disconnects, malformed replies, bounded event pressure, and shutdown;
- real-Chrome navigation/execution on random loopback ports;
- LSP initialize/open/change/diagnostics/format/semantic-token/run/shutdown framing tests;
- DAP initialize/launch/setBreakpoints/configurationDone/continue/disconnect framing tests;
- packaged VSIX manifest, activation, command, semantic-token, and debug-adapter smoke tests.

Tests requiring network downloads use local fixtures. Real Chrome tests may skip only when Chrome or loopback sockets are genuinely unavailable.

## 12. Acceptance criteria

Milestone A is complete only when:

1. A clean supported machine can install `webtest`, run `webtest browser install`, and resolve the managed executable.
2. `webtest check` and `webtest test` discover configured tests deterministically without explicit file arguments.
3. Human output contains precise source snippets; JSON/JUnit/event output validates against versioned fixtures.
4. Static/test failures and Chrome/CDP infrastructure failures produce distinct documented exit codes.
5. A forced CDP disconnect fails promptly, reaps Chrome, and leaves no current runtime diagnostic after a later successful rerun.
6. The packaged VSIX activates against the release binary, shows semantic diagnostics, runs a synchronized buffer, and starts a headed breakpoint session without `launch.json`.
7. Formatting, Clippy, workspace tests, WASM check, extension compilation, and package smoke tests pass on supported CI platforms.

The roadmap acceptance statement is thereby satisfied: a new user can install WebTest and its browser, run the examples, and see current runtime diagnostics without manually locating Chrome.
