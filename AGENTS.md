# Repository Guidelines

## Product and Architectural Intent

WebTest is a statically analyzable language platform for web-system tests, not merely a browser automation wrapper or an editor extension. The native product is one Rust executable, `webtest`, while portable language services are exposed through `webtest-wasm`. The initial architecture is specified in `specs/intitial-vertical-slice.md`; `specs/future-functionality.md` is the roadmap. Treat milestone-era feature lists as historical limits, not current facts. Confirm current behavior in code before relying on either spec.

The durable architecture is:

```text
source -> lexer -> lossless Rowan CST -> typed AST -> HIR
       -> analysis -> source-mapped TestPlan -> runtime -> browser abstraction
       -> structured events/observations -> editor services -> LSP/Cursor
```

There is one Rust lexer, parser, CST, syntax-to-HIR lowering path, formatter, plan lowering path, runtime, and editor-service implementation. Never add an adapter-specific parser, formatter, semantic model, locator evaluator, or diagnostic engine. TypeScript is host glue only.

## Current Implemented Slice

The current DSL supports `test` declarations containing `browser` blocks, `open`, `click`, `id(...)` and `text(...)` locators, and `expect <locator>.visible`. Plans distinguish browser operations from assertions. Chrome can run headlessly or with `--headed`; text visibility failures, missing locators, and ambiguous locators remain structured and source-mapped.

The executable currently provides path-oriented `check`, `fmt`, and `test`, managed-browser commands, `lsp`, and `dap`. Project discovery and typed `webtest.toml` configuration live in `project`; managed Chrome distribution is separate from CDP in `browser-manager`. CLI reporters provide human, concise, versioned JSON/events, and JUnit output with stable exit classes. The Tower LSP supplies full-document synchronization, diagnostics, formatting, semantic tokens, and synchronized-buffer execution. Syntax highlighting comes from CST-backed semantic tokens; the extension only maps token categories to theme scopes. The Cursor/VS Code extension supplies language registration, run/debug commands, breakpoint contribution, and zero-config DAP launch. `webtest dap` uses the same `TestPlan` and `Runner` as normal execution, pausing through `RunControl` before a source-mapped step. The WASM facade currently exposes diagnostics and formatting; it is not yet a complete Monaco service.

Do not claim unimplemented roadmap features—such as bindings, types/effects, modules, actionability, retries, parallelism, server/HTTP operations, traces, or CLI-to-LSP IPC—already exist. Add them incrementally without weakening the shared architecture.

## Crate Ownership and Dependency Direction

- `crates/text`: `FileId`, BLAKE3 `SourceRevision`, `DocumentVersion`, `SyntaxOrigin`, and Rowan ranges. All long-lived source mappings use these primitives.
- `crates/syntax`: the only lexer, error-tolerant parser, lossless CST, syntax kinds, string decoding, and typed Rowan AST wrappers. It preserves whitespace, comments, punctuation, malformed tokens, and exact source text.
- `crates/hir`: semantic constructs and typed IDs lowered only from typed AST views. It must not contain editor, runtime, or CDP concerns.
- `crates/analysis`: file/path inputs and revision-keyed memoization of parse, HIR, static diagnostics, and plans. It is currently a single-process cache, not yet a Salsa-style workspace database.
- `crates/format`: the one canonical formatter. It consumes CST tokens so trivia survives; CLI and editor formatting must call it.
- `crates/plan`: runtime-facing, syntax-independent `TestPlan`, deterministic `StepId`s, browser/assertion operations, locators, source revision, and precise origins.
- `crates/browser`: protocol-neutral `BrowserHost`, `BrowserSession`, `Page`, `Locator`, and structured `BrowserError` semantics.
- `crates/project`: nearest-root selection, typed `webtest.toml`, configuration warnings/errors, and deterministic path discovery. Analysis never reads ambient project configuration.
- `crates/browser-manager`: pinned Chrome for Testing metadata, verified atomic installation, owned cache cleanup, and managed executable resolution. It contains no CDP semantics.
- `crates/browser-cdp`: system Chrome discovery/launch, temporary profiles, bounded/deadlined WebSocket command correlation, target sessions, navigation, locator resolution, clicking, visibility evaluation, and child reaping. CDP JSON types never escape this crate.
- `crates/observation`: execution IDs/events and revision-bound runtime observations stored by `(FileId, SourceRevision)`.
- `crates/runtime`: sequential plan execution, plan-to-browser conversion, structured results/events, observation recording, and the pre-step `RunControl` hook used by DAP. It does not parse source or print terminal output.
- `crates/editor`: protocol-neutral document state, diagnostic composition, formatting, semantic tokens, and run orchestration. It returns internal DTOs, never LSP or VS Code types.
- `crates/lsp`: thin Tower adapter, document synchronization, UTF-8 byte-range to UTF-16 LSP conversion, command routing, and diagnostic/token publication.
- `crates/dap`: stdio DAP framing, launch/breakpoint state, source-to-step mapping, stack/scopes/variables, and pause/continue/step control. The app injects a `BrowserHost`; DAP does not own CDP semantics.
- `crates/wasm`: stable portable DTO facade over shared analysis/formatting. Native filesystem, process, socket, and Chrome capabilities do not belong here.
- `crates/app`: Clap CLI, configuration precedence, reporters/exit classes, filesystem/terminal presentation, tracing setup, and composition of LSP/DAP/runtime with managed Chrome and `ChromeHost` into the single executable.
- `editors/vscode`: Cursor/VS Code manifest and TypeScript adapter. It locates/spawns `webtest lsp` and `webtest dap`; it contains no language intelligence.
- `examples`: manual HTTP fixture and passing/failing `.webtest` programs. Automated tests should not depend on port 3000.

Dependencies point from adapters toward reusable cores. In particular, forbid `syntax -> analysis/runtime/lsp`, `hir -> lsp/browser-cdp`, `analysis -> lsp/CDP/VS Code`, `editor -> LSP types`, `browser -> browser-cdp`, and `runtime -> CLI formatting`. `app` is the composition root and may depend on all native components.

## Source, Revision, and Error Invariants

Internal ranges are UTF-8 byte offsets. Preserve the smallest useful `SyntaxOrigin` at every lowering stage; locator/assertion failures should underline the locator rather than an entire block. LSP converts to zero-based UTF-16 positions, while DAP exposes one-based source positions. Add Unicode tests whenever boundary conversion changes.

Every executable plan carries the source revision from which it was built. Every editor-visible runtime observation carries that revision plus file, test, step, and range identity. `EditorService` may publish an observation only when its revision equals the current document revision. Starting a new run clears prior observations for the file; a successful rerun must remove old runtime diagnostics.

Keep these failure classes distinct:

- malformed DSL or invalid semantics: static diagnostic, no execution;
- missing/ambiguous/invisible element or assertion mismatch: test failure and structured observation;
- Chrome launch, disconnect, navigation transport, or CDP protocol failure: infrastructure error;
- violated internal invariant: bug.

Do not flatten errors into strings below presentation adapters. CLI, editor/LSP, and DAP may render the same structured fact differently.

## How to Implement a Language or Runtime Feature

Make changes vertically through the existing path rather than special-casing the consumer:

1. Add tokens/kinds and error-tolerant grammar in `syntax`; update the manual Rowan raw-kind mapping and ensure recovery always makes progress.
2. Add typed AST wrappers/accessors over CST nodes. Never scan source text downstream or construct a second tree.
3. Lower into HIR with semantic values, typed identity, and precise origins.
4. Add analysis diagnostics or queries when the feature has static rules.
5. Lower into an explicit, deterministic `TestPlan` operation. Assertions and control flow must remain visible in the IR, not opaque callbacks.
6. Extend protocol-neutral browser/runtime traits and structured errors where execution semantics require it; implement CDP mechanics only in `browser-cdp`.
7. Convert source-relevant failures into revision-bound observations, then compose them in `editor`.
8. Expose the capability through LSP, DAP, WASM, CLI, or the extension only as thin conversion/routing work.
9. Update examples and the relevant spec/status section when the product contract or roadmap changes; do not silently rewrite the initial milestone's historical scope.

For syntax work, test valid, invalid, and half-typed input and assert `parse.syntax().text().to_string() == source`. For new operations, test exact AST/HIR/plan ranges and deterministic step ordering. Prefer fake `BrowserHost`/`Page` implementations for runtime and editor tests; reserve real Chrome for backend semantics and end-to-end proof.

## Browser, Protocol, and Security Boundaries

Never interpolate user strings unsafely into evaluated JavaScript; JSON-serialize them or pass protocol arguments. Keep Chrome remote debugging on `127.0.0.1`, use a random port and a fresh temporary profile, kill the child on drop, and never reuse a personal browser profile. Do not add `--no-sandbox` by default. Bound waits and turn disconnects/protocol failures into typed errors. Do not introduce Playwright as a second runtime backend or bypass the `browser` traits from language/runtime code.

Both `webtest lsp` and `webtest dap` own stdout for framed protocol messages. All logging goes to stderr via `tracing`; never use stdout debugging in either mode. Keep server command `webtest.runFile` distinct from extension-local commands such as `webtest.runCurrentFile` and `webtest.debugCurrentFile` to avoid duplicate VS Code command registration. Do not add a TextMate grammar as a second source of language semantics; any future declarative lexical fallback must remain subordinate to shared Rust analysis.

## Build, Test, and Development Commands

- `cargo build`: build the workspace and `target/debug/webtest`.
- `cargo test --workspace`: run unit, Chrome integration, and doc tests.
- `cargo fmt --all -- --check`: verify Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: require a warning-free workspace.
- `target/debug/webtest check examples/minimal/passing.webtest`: run static checks.
- `target/debug/webtest fmt examples/minimal/passing.webtest`: rewrite with the shared formatter.
- `target/debug/webtest test examples/minimal/passing.webtest`: execute in headless Chrome.
- `target/debug/webtest test --headed examples/minimal/passing.webtest`: execute with visible Chrome.
- `target/debug/webtest lsp`: run Tower LSP over stdio; do not type into this process manually.
- `target/debug/webtest dap`: run headed DAP over stdio; normally launched by the extension.
- `cd editors/vscode && npm install && npm run compile`: install and type-check extension glue.
- `cd editors/vscode && npm run package`: produce the versioned VSIX; install with `cursor --install-extension <file>.vsix --force`.
- `cargo check -p webtest-wasm --target wasm32-unknown-unknown`: verify the portable target when that Rust target is installed.

Set `WEBTEST_CHROME_PATH` or pass `--chrome-path` when discovery fails. For manual examples, serve `examples/index.html` on port 3000. Chrome tests must instead bind `127.0.0.1:0`, use the assigned port, and gracefully skip only when Chrome or loopback sockets are genuinely unavailable.

## Coding and Test Conventions

Use Rust 2024 and `rustfmt` defaults (four spaces). Follow `snake_case` for functions/modules, `PascalCase` for types, and typed identifiers such as `FileId`, `TestId`, `StepId`, and `ExecutionId`. Avoid `unwrap`/`expect` in runtime paths; tests may use them to state invariants. Prefer narrow APIs, typed errors, explicit ownership, and `tracing` spans. Do not add global mutable state except narrowly scoped atomics used for identity generation.

Keep focused unit tests beside code in `#[cfg(test)]` modules. Required coverage includes lossless CST/recovery, string decoding, source ranges, revision invalidation, deterministic plans, structured failures, observation clearing, UTF-16 conversion, DAP framing/breakpoint control, and native/WASM parity where portable. Never edit generated `editors/vscode/out` or `node_modules`; edit `src/extension.ts` or `package.json`, then compile/package.

## Commits and Pull Requests

Use short, lowercase, imperative commit subjects consistent with history, such as `add text expectation` or `add dap debugger`. Keep commits focused by architectural slice. Pull requests should describe affected layers and dependency changes, list exact verification commands, link issues/spec sections, and include screenshots or captured diagnostic/debug output for Cursor/VS Code behavior.
