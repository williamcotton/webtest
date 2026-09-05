# Repository Guidelines

## Product and Architectural Intent

WebTest is a statically analyzable language platform for web-system tests, not merely a browser automation wrapper or an editor extension. The native product is one Rust executable, `webtest`, while portable language services are exposed through `webtest-wasm`. The initial architecture is specified in `specs/intitial-vertical-slice.md`; `specs/future-functionality.md` is the roadmap. Treat staged feature lists as historical limits, not current facts. Confirm current behavior in code before relying on either spec.

The durable architecture is:

```text
source -> lexer -> lossless Rowan CST -> typed AST -> HIR
       -> analysis -> source-mapped TestPlan -> runtime -> browser abstraction
       -> structured events/observations -> editor services -> LSP/Cursor
```

There is one Rust lexer, parser, CST, syntax-to-HIR lowering path, formatter, plan lowering path, runtime, and editor-service implementation. Never add an adapter-specific parser, formatter, semantic model, locator evaluator, or diagnostic engine. TypeScript is host glue only.

Each boundary may translate representations, but ownership must flow in one direction: source → semantics → execution plan → execution → observations. No downstream layer may reconstruct or reinterpret an upstream representation.

## Current Implemented Slice

The current DSL supports sequential test flows with `server` and `browser` capability blocks, local typed `let` bindings, primitive/list/record expressions, provider calls, JSON-to-record decoding, typed value assertions, browser actions, and the full locator/state surface. Static analysis resolves bindings to IDs, checks provider schemas and capabilities, and rejects non-transferable server-to-browser values. Plans distinguish pure evaluation, provider calls, browser operations, and assertions. Chrome can run headlessly or with `--headed`; value/decode failures and browser failures remain structured and source-mapped.

The executable currently provides non-overwriting `init` scaffolding, path-oriented `check`, `fmt`, `test`, lifecycle-complete project-mode `inspect`, and deterministic `build --emit`, managed-browser commands, `lsp`, and `dap`. Relative or omitted inspection targets start and stop configured `[app]`; absolute HTTP(S) targets remain standalone. `init` creates a minimal typed application-bridge manifest and smoke test, installs the canonical WebTest agent skill, and links agent-specific discovery locations without adding another language implementation. Built-in HTTP, direct-process, and sandboxed filesystem providers plus the project-configured `app` bridge share one typed registry contract. The versioned application bridge has offline manifests, live schema verification, authenticated local IPC/TCP/stdio transports, command/HTTP compatibility adapters, runner-owned lifecycle, Node and Ruby SDKs, a shared conformance suite, and nine host-language web-server examples. Project discovery and typed `webtest.toml` configuration live in `project`; managed Chrome distribution is separate from CDP in `browser-manager`. CLI reporters provide human, concise, versioned JSON/events, and JUnit output with stable exit classes. The Tower LSP supplies full-document synchronization, static and revision-safe runtime diagnostics, formatting, semantic tokens, type/capability hover, provider completion/signature help, synchronized-buffer execution, and watched invalidation of `webtest.toml` and offline application manifests. Syntax highlighting comes from CST-backed semantic tokens; the extension only maps token categories to theme scopes. The Cursor/VS Code extension supplies language registration, run/debug commands, breakpoint contribution, and zero-config DAP launch. `webtest dap` uses the same `TestPlan` and `Runner` as normal execution, pausing through `RunControl` before source-mapped provider, assertion, and browser steps and showing evaluated redacted transferable bindings. The WASM facade exposes diagnostics, formatting, portable plan compilation, description, completion, signature, and hover services with offline app manifests and required host-capability metadata; it does not execute native providers and is not yet a complete Monaco service.

Do not claim unimplemented roadmap features—such as modules, user-defined functions, actionability, retries, parallelism, traces, or CLI-to-LSP IPC—already exist. Add them incrementally without weakening the shared architecture.

## Crate Ownership and Dependency Direction

- `crates/text`: `FileId`, BLAKE3 `SourceRevision`, `DocumentVersion`, `SyntaxOrigin`, and Rowan ranges. All long-lived source mappings use these primitives.
- `crates/model`: protocol-neutral `TestId`, `StepId`, `BindingId`, unary/binary operators, `Type`, `RecordField`, `Value` and compound values, JSON/value conversion, and `Capability`. It depends only on the standard library, Serde, and Serde JSON and contains no compiler, provider, runtime, browser, editor, or project behavior.
- `crates/syntax`: the only lexer, error-tolerant parser, lossless CST, syntax kinds, string decoding, and typed Rowan AST wrappers. It preserves whitespace, comments, punctuation, malformed tokens, and exact source text.
- `crates/hir`: source-semantic constructs lowered only from typed AST views. It uses shared IDs and operators from `model` and must not contain editor, runtime, or CDP concerns.
- `crates/provider`: provider/operation/parameter schemas, calls, results, errors, `ServerProvider`, and `ProviderRegistry`, using types, values, and capabilities from `model`. Its native feature owns the built-in HTTP, direct-process, and sandboxed-filesystem providers. It never parses the DSL or depends on browser, editor, or adapter crates.
- `crates/feedback`: versioned, protocol-neutral source-range, semantic-detail, and repair-hint DTOs shared by analysis, browser, runtime, observations, and adapters. It contains no rendering or transport policy.
- `crates/analysis`: explicit in-memory source and provider-schema inputs plus revision-keyed memoization of parsing, HIR, static diagnostics, and plans. It also owns shared description/search, completion, signature-help, hover, and documentation facts. It is a single-process cache, not yet a Salsa-style workspace database, and never reads ambient project configuration or performs runtime/browser work.
- `crates/app-bridge`: Protocol 1 manifest/schema/hash validation, normalized loading for author-edited offline schemas, and generated wire DTOs/codecs. Its native feature adds bounded framing, authenticated transports, lifecycle, compatibility adapters, and the shared `AppProvider`; portable schema and wire support remain available without native features.
- `crates/format`: the one canonical formatter. It consumes CST tokens so trivia survives; CLI and editor formatting must call it.
- `crates/plan`: runtime-facing, syntax-independent, serializable `TestPlan` and plan envelopes, deterministic typed IDs, pure evaluation/provider/browser/assertion operations, locators, values, source revision, and precise origins. It has no production dependency on syntax.
- `crates/browser`: protocol-neutral host/session/context/page traits, browser actions, locators and states, waits, inspection/evidence DTOs, and structured errors and repair semantics.
- `crates/project`: nearest-root selection, deterministic path discovery, and typed `webtest.toml` configuration for project, browser, timeouts, artifacts, evidence, server/app lifecycle, redaction, inspection, and description settings. It owns configuration warnings/errors; analysis never reads ambient project configuration.
- `crates/browser-manager`: pinned Chrome for Testing metadata, verified atomic installation, owned cache cleanup, and managed executable resolution. It contains no CDP semantics.
- `crates/browser-cdp`: system Chrome discovery/launch, temporary profiles, bounded/deadlined WebSocket command correlation, target sessions, navigation, locator/action/state execution, page inspection and evidence capture, and child reaping. CDP JSON types never escape this crate.
- `crates/observation`: execution IDs/events and revision-bound runtime observations stored by `(FileId, SourceRevision)`, including the closed `RuntimeFailureCode` vocabulary, structured failures, value diffs, page/candidate evidence, and repair hints.
- `crates/runtime`: sequential plan execution, provider dispatch, plan-to-browser conversion, evidence/artifact capture, structured results/events/failures/diffs/repair hints, observation recording, and the pre-step `RunControl` hook used by DAP. It does not parse source or print terminal output.
- `crates/editor`: protocol-neutral document state, provider-schema reconfiguration, diagnostic composition, formatting, semantic tokens, hover/completion/signature services, and run orchestration. It returns internal DTOs, never LSP, DAP, or VS Code types.
- `crates/lsp`: thin Tower adapter, document synchronization, project-input watcher registration/routing, UTF-8 byte-range to UTF-16 LSP conversion, command routing, and diagnostic/token publication.
- `crates/dap`: stdio DAP framing, launch/breakpoint state, source-to-step mapping, stack/scopes/variables, and pause/continue/step control. The app injects a `BrowserHost`; DAP does not own CDP semantics.
- `crates/wasm`: stable portable DTO facade over shared diagnostics, formatting, plan compilation, description/search, completion, signature-help, and hover services, with offline application manifests and required host-capability metadata. Native filesystem, process, socket, provider execution, and Chrome capabilities do not belong here.
- `crates/app`: the composition root and sole native executable: Clap commands including `init`, configuration precedence, reporters/exit classes, filesystem/terminal presentation, tracing setup, and composition of LSP, DAP, runtime, providers, managed Chrome, and `ChromeHost`. It owns bootstrap scaffold installation, including the canonical skill and agent-tool links.
- `editors/vscode`: Cursor/VS Code manifest and TypeScript adapter. It locates/spawns `webtest lsp` and `webtest dap` and forwards project configuration/JSON file events; it contains no language intelligence.
- `examples`: self-contained manual fixture projects and passing/failing `.webtest` programs. Automated tests should not depend on their fixed ports.
- `protocol` and `sdks`: normative application-bridge schemas/state semantics, generated projections, black-box conformance, and thin host-language SDKs. They never parse the DSL or construct plans.

Production dependencies point from adapters toward reusable cores. Keep `text`, `model`, `feedback`, `provider`, and `project` independent of their consumers; forbid `model -> any WebTest crate`, `syntax -> hir/analysis/plan/runtime/editor/adapters`, `hir -> analysis/plan/runtime/editor/adapters/browser-cdp`, `plan -> syntax/hir/provider/analysis/runtime/browser/adapters`, `analysis -> project/runtime/browser/browser-cdp/editor/adapters`, `browser -> browser-cdp/plan`, `runtime -> syntax/hir/analysis/project/editor/app`, `observation -> hir`, any runtime dependency on CLI/report formatting, and `editor -> LSP/DAP types or browser-cdp`. Test-only dev-dependencies may cross a boundary for vertical or black-box verification, but do not authorize the same production coupling. `app` is the composition root and may depend on all native components.

## Source, Revision, and Error Invariants

Internal ranges are UTF-8 byte offsets. Preserve the smallest useful `SyntaxOrigin` at every lowering stage; locator/assertion failures should underline the locator rather than an entire block. LSP converts to zero-based UTF-16 positions, while DAP exposes one-based source positions. Add Unicode tests whenever boundary conversion changes.

Every executable plan carries the source revision from which it was built. Every editor-visible runtime observation carries that revision plus file, test, step, and range identity. `EditorService` may publish an observation only when its revision equals the current document revision. Starting a new run clears prior observations for the file; a successful rerun must remove old runtime diagnostics.

Provider schemas are explicit semantic inputs even when source text is unchanged. Replacing a project provider schema must invalidate cached diagnostics, types, completions, signatures, hovers, and plans and clear runtime observations built against the prior schema. The native LSP watches `webtest.toml` and JSON project inputs, filters events to each cached project's resolved configuration and application-manifest paths, reconfigures the existing editor service, and republishes open-document diagnostics without requiring a server or developer-window restart. Author-edited offline manifests derive their in-memory identity from canonical `functions`, so a stale declared `schema_hash` is tolerated. A structurally invalid or half-written replacement must evict the stale project-provider schema and surface an error without terminating the LSP; never keep accepting operations from the last valid manifest.

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
9. Update examples and the relevant spec/status section when the product contract or roadmap changes; do not silently rewrite an archived specification's historical scope.

For syntax work, test valid, invalid, and half-typed input and assert `parse.syntax().text().to_string() == source`. For new operations, test exact AST/HIR/plan ranges and deterministic step ordering. Prefer fake `BrowserHost`/`Page` implementations for runtime and editor tests; reserve real Chrome for backend semantics and end-to-end proof.

## Keep `webtest describe` Current

Treat the author-facing description surface as a required part of every public language, locator, assertion, browser operation, type, capability, or provider feature. A feature is not complete when the parser/runtime accepts it but `webtest describe` cannot discover and explain it accurately. Add or update its canonical query ID, category/index membership, useful aliases and search terms, syntax forms, typed parameters/result, legal contexts, required capabilities, effects, failure modes, constraints, availability, related topics, and canonical examples as applicable. Do not use `describe` as a duplicate CLI/configuration manual; CLI flags remain documented by `webtest <command> --help`.

Keep description mechanics and content factored along the existing boundary: DTOs, query routing, bounds, sanitization, and deterministic search belong in `crates/analysis/src/description.rs`; installed language/provider catalog entries belong in `crates/analysis/src/description/catalog.rs`; focused Protocol 1 topics belong in `crates/analysis/src/description/app_bridge.rs`; configured `app.*` operations must continue to project from the shared validated `ProviderRegistry` rather than a CLI-only registry. Derive facts from the same syntax, analysis, plan, browser, runtime, and provider contracts that implement the feature. Never advertise roadmap behavior, invent project-provider examples, flatten typed failures into vague prose, or make a claim solely because a specification once proposed it.

Extend description tests with the feature. Maintain completeness and uniqueness checks for public IDs and the prohibition on unimplemented roadmap constructs; test exact lookup plus relevant category, alias, and search paths; prove described types/capabilities/contexts and failures agree with analysis and plan/runtime behavior; parse and statically analyze every installed canonical example through its declared enclosing context; and preserve native/WASM parity for portable description inputs. When bootstrap or authoring guidance changes, update `.agents/skills/webtest/SKILL.md`, its initializer parity assertions, current examples, and the relevant implementation-status documentation in the same change.

## Browser, Protocol, and Security Boundaries

Never interpolate user strings unsafely into evaluated JavaScript; JSON-serialize them or pass protocol arguments. Keep Chrome remote debugging on `127.0.0.1`, use a random port and a fresh temporary profile, kill the child on drop, and never reuse a personal browser profile. Do not add `--no-sandbox` by default. Bound waits and turn disconnects/protocol failures into typed errors. Do not introduce Playwright as a second runtime backend or bypass the `browser` traits from language/runtime code.

Both `webtest lsp` and `webtest dap` own stdout for framed protocol messages. All logging goes to stderr via `tracing`; never use stdout debugging in either mode. Keep server command `webtest.runFile` distinct from extension-local commands such as `webtest.runCurrentFile` and `webtest.debugCurrentFile` to avoid duplicate VS Code command registration. Do not add a TextMate grammar as a second source of language semantics; any future declarative lexical fallback must remain subordinate to shared Rust analysis.

## Build, Test, and Development Commands

- `cargo build`: build the workspace and `target/debug/webtest`.
- `cargo test --workspace`: run unit, Chrome integration, and doc tests.
- `cargo fmt --all -- --check`: verify Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: require a warning-free workspace.
- `target/debug/webtest check examples/plain-html/sign-in.webtest`: run static checks.
- `target/debug/webtest fmt examples/plain-html/sign-in.webtest`: rewrite with the shared formatter.
- `target/debug/webtest test examples/plain-html/sign-in.webtest`: execute in headless Chrome.
- `target/debug/webtest test --headed examples/plain-html/sign-in.webtest`: execute with visible Chrome.
- `target/debug/webtest lsp`: run Tower LSP over stdio; do not type into this process manually.
- `target/debug/webtest dap`: run headed DAP over stdio; normally launched by the extension.
- `cd editors/vscode && npm install && npm run compile`: install and type-check extension glue.
- `cd editors/vscode && npm run package`: produce the versioned VSIX; install with `cursor --install-extension <file>.vsix --force`.
- `cargo check -p webtest-wasm --target wasm32-unknown-unknown`: verify the portable target when that Rust target is installed.

Set `WEBTEST_CHROME_PATH` or pass `--chrome-path` when discovery fails. Manual example projects document their fixture commands and fixed ports in `examples/README.md`; Chrome tests must instead bind `127.0.0.1:0`, use the assigned port, and gracefully skip only when Chrome or loopback sockets are genuinely unavailable.

## Coding and Test Conventions

Use Rust 2024 and `rustfmt` defaults (four spaces). Follow `snake_case` for functions/modules, `PascalCase` for types, and typed identifiers such as `FileId`, `TestId`, `StepId`, and `ExecutionId`. Avoid `unwrap`/`expect` in runtime paths; tests may use them to state invariants. Prefer narrow APIs, typed errors, explicit ownership, and `tracing` spans. Do not add global mutable state except narrowly scoped atomics used for identity generation.

Keep focused unit tests beside code in `#[cfg(test)]` modules. Required coverage includes lossless CST/recovery, string decoding, source ranges, revision invalidation, deterministic plans, structured failures, observation clearing, UTF-16 conversion, DAP framing/breakpoint control, and native/WASM parity where portable. Never edit generated `editors/vscode/out` or `node_modules`; edit `src/extension.ts` or `package.json`, then compile/package.

## Commits and Pull Requests

Use short, lowercase, imperative commit subjects consistent with history, such as `add text expectation` or `add dap debugger`. Keep commits focused by architectural slice. Pull requests should describe affected layers and dependency changes, list exact verification commands, link issues/spec sections, and include screenshots or captured diagnostic/debug output for Cursor/VS Code behavior.
