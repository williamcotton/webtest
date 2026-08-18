# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 Cargo workspace. Core layers live under `crates/`: `syntax` owns the single lossless Rowan parser and CST, `hir` and `plan` perform lowering, `analysis` provides revision-keyed queries, and `runtime` executes plans. Browser semantics are defined in `browser`; the direct Chrome DevTools Protocol implementation is isolated in `browser-cdp`. Protocol-neutral editor behavior lives in `editor`, with Tower LSP and WASM adapters in `lsp` and `wasm`. The `app` crate builds the `webtest` executable. VS Code glue is in `editors/vscode`, while manual fixtures and `.webtest` programs are in `examples/`.

Keep unit tests beside their implementation in `#[cfg(test)]` modules. Tests needing Chrome should use loopback servers on random ports and gracefully skip when Chrome or socket access is unavailable.

## Build, Test, and Development Commands

- `cargo build`: build the complete workspace and `target/debug/webtest`.
- `cargo test --workspace`: run all unit, integration, and documentation tests.
- `cargo fmt --all -- --check`: verify Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: enforce lint-clean code.
- `target/debug/webtest check examples/minimal/passing.webtest`: check DSL source.
- `target/debug/webtest test examples/minimal/passing.webtest`: run through headless Chrome.
- `cd editors/vscode && npm install && npm run compile`: build the VS Code adapter.

Set `WEBTEST_CHROME_PATH` when Chrome is not discovered automatically.

## Coding Style & Naming Conventions

Use `rustfmt` defaults (four-space indentation) and idiomatic Rust naming: `snake_case` functions/modules, `PascalCase` types, and descriptive typed IDs such as `FileId` or `StepId`. Avoid `unwrap`/`expect` in runtime paths. Preserve dependency direction: core crates must not depend on LSP, VS Code, or CDP details. Never introduce a second parser or formatter.

## Testing Guidelines

Add focused tests for lossless CST round-trips, precise source ranges, revision safety, structured runtime failures, and UTF-16 LSP conversion. Verify successful reruns remove previous runtime diagnostics. Run formatting, Clippy, and the full workspace suite before submitting.

## Commit & Pull Request Guidelines

History uses short, lowercase, imperative summaries such as `add spec` and `implement vertical slice`. Keep commits similarly focused. Pull requests should describe architectural impact, list verification commands, link relevant issues, and include screenshots or diagnostic output for VS Code-facing changes.

## Security & Runtime Boundaries

Serialize user values before JavaScript evaluation. Use temporary Chrome profiles, bind debugging locally, never use a personal browser profile, and keep LSP stdout free of logs.
