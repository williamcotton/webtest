# Milestone F — Workspace and Editor Intelligence

## 0. Status and dependencies

This specification expands Milestone F in [`future-functionality.md`](./future-functionality.md). It depends on typed HIR/provider schemas from [`milestone-c.md`](./milestone-c.md), bridge schema inputs from [`milestone-d.md`](./milestone-d.md), and stable plan/event/source identities from [`milestone-e.md`](./milestone-e.md).

Milestone F turns the one-file analysis cache into a dependency-tracked workspace and adds modules, reusable declarations, fixtures, and full protocol-neutral editor intelligence. The native LSP remains an adapter over Rust editor services.

## 1. Outcome

A multi-file project remains responsive while users import helpers/fixtures, navigate and rename symbols, complete typed provider calls and returned values, discover tests, and receive current static/runtime information:

```webtest
import { user_fixture } from "./fixtures/users.webtest"

test "user signs in" {
    let user = use user_fixture(email: "alice@example.com")

    browser {
        open "/login"
        fill label("Email") with user.email
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
```

Editing the fixture invalidates dependent tests and editor results, but unrelated modules reuse prior parse/HIR/type/plan queries.

## 2. Scope

Milestone F includes:

- a formal dependency-tracked incremental workspace database;
- project file/config/provider-schema inputs and immutable analysis snapshots;
- module declarations/imports and deterministic module resolution;
- declarative helper functions and reusable fixtures;
- explicit `test`, `file`, `worker`, and `suite` fixture lifetimes;
- document/workspace symbols, folding, selection, completion, signature help, hover, definition, references, rename, code actions, inlay hints, and richer semantic tokens;
- workspace test discovery and Cursor/VS Code test-explorer integration;
- incremental LSP document synchronization, request cancellation, and progress;
- cross-file source mapping for plan/runtime/DAP frames.

## 3. Non-goals

This milestone does not add an online package registry, remote imports, arbitrary build scripts, macro expansion, dynamic code loading during analysis, higher-order functions, unrestricted recursion, language-specific app introspection, collaborative editing, or the browser/Monaco package. Portable delivery is Milestone G.

## 4. Workspace model

### 4.1 Inputs

The analysis database receives explicit values for:

```text
WorkspaceId and canonical root
file set, FileId, canonical path/URI, document text/version
resolved webtest.toml and environment-profile metadata
provider schema manifests and hashes
module/source-root configuration
toolchain/compiler feature set
```

Queries never read the filesystem, environment, network, clock, or process state directly. Native project discovery and WASM/worker hosts populate inputs through adapters.

### 4.2 Required queries

At minimum:

```text
lex/parse and syntax diagnostics per file
module declaration/import extraction
module graph and cycle diagnostics
typed AST -> HIR lowering per declaration
export table and name resolution
type/capability/provider resolution
static diagnostics
test/fixture/helper indexes
TestPlan construction
symbols, folding, selection, semantic tokens
completion, signature, hover
definition, references, rename
code actions and inlay hints
test discovery
```

Use Salsa unless an alternative dependency-tracked design is documented and demonstrates equivalent invalidation, parallel snapshot, cycle, and memory behavior before implementation. Full-file reparsing is acceptable; recomputing unrelated files/features is not.

### 4.3 Snapshots and concurrency

Editor requests operate on immutable database snapshots. Applying a document/config/schema update creates a new revision without mutating in-flight request results. Cancellation stops expensive downstream work and prevents stale publication, but cached completed subqueries remain reusable where safe.

Long-lived identities use semantic keys plus `FileId`/origin, not pointer addresses. File deletion/rename invalidates affected modules and references predictably. Database memory has bounded retention for old revisions.

## 5. Modules and imports

### 5.1 Surface

Modules are files in configured source roots. Explicit imports use project-relative or importing-file-relative paths:

```webtest
import { user_fixture, email_for } from "./fixtures/users.webtest"
import { admin_fixture as admin } from "project/support/admin.webtest"
```

Version 1 has named imports and aliases only—no wildcard imports, implicit prelude beyond documented built-ins, remote URLs, or runtime-computed paths.

Files export declarations explicitly:

```webtest
export fn email_for(name: String) -> String = lower(name) + "@example.com"

export fixture user_fixture(email: String) -> { id: Int, email: String } scope test {
    setup server {
        let user = app.create_user(email: email)
        provide user
    }

    teardown server {
        app.delete_user(id: user.id)
    }
}
```

Surface details may be refined during grammar work, but explicit imports/exports, declared parameter/result types at module boundaries, fixture scope, setup/provide/teardown, and source mapping are normative.

### 5.2 Resolution

Resolution normalizes paths without escaping configured source roots, applies one documented extension rule, and assigns stable `ModuleId`s. Two paths resolving to the same canonical file are one module. Case-collision diagnostics are required on case-insensitive platforms.

Import cycles are errors in version 1 and report the shortest available cycle with related ranges. Missing modules, non-exported names, duplicate imports, alias collisions, and ambiguous source-root paths have distinct diagnostics.

The module graph is deterministic and independent of filesystem enumeration order.

## 6. Helper functions

Helper functions are declarative WebTest functions compiled through HIR/plan; they are not host-language callbacks. Parameters and return types are required on exported functions and may be inferred for private expression-local values.

Milestone F supports:

- pure expression-bodied helpers;
- typed helpers containing sequential WebTest statements with inferred/declared capability;
- calls to other resolved helpers/providers;
- lexical local bindings and return/provide value;
- deterministic specialization/lowering into plan scopes.

No higher-order values, closures crossing scopes, dynamic dispatch, or recursion are allowed initially. The call graph must be acyclic, allowing bounded plan construction and clear diagnostics. Capability checking prevents a server-effect helper from being called inside a browser-only region.

Definition, references, rename, hover, and signature help operate on stable `FunctionId`/parameter IDs.

## 7. Fixtures and lifetimes

### 7.1 Fixture model

A fixture declares parameters, result type, capability, dependencies, lifetime, setup, provided value, and teardown. It lowers to explicit resource/plan scopes from Milestone E; teardown is never hidden in an SDK callback.

Fixture dependencies form an acyclic graph. Acquisition follows dependency order; teardown runs in reverse successful-acquisition order exactly once.

### 7.2 Lifetimes

```text
test    one instance per test execution/attempt as specified by retry ownership
file    one instance shared by tests from a module file within an execution
worker  one instance per test scheduler worker
suite   one instance for the entire suite execution
```

The analyzer verifies that a longer-lived fixture does not depend on a shorter-lived fixture. A fixture result shared across tests/workers must be declared shareable and contain only values/resources whose provider schema supports that lifetime/concurrency. Browser pages/contexts cannot be suite/file shared by default.

Test-lifetime fixtures are the safe default. File/worker/suite lifetimes require explicit syntax and visible plan/event identity. Parallel tests coordinate acquisition exactly once and observe the same terminal acquisition result.

### 7.3 Failure behavior

An acquisition failure skips dependent bodies and still tears down already acquired dependencies. A teardown failure is retained with the primary test result using Milestone E aggregation. Retry behavior is explicit: by default a test-scoped fixture lives inside the retry attempt; longer-lived fixtures do not reacquire on every retry.

## 8. Symbols and navigation

The protocol-neutral editor layer exposes stable DTOs for:

```text
DocumentSymbol and WorkspaceSymbol
Location and LocationLink
Reference
RenameEdit / WorkspaceEdit
FoldingRange and SelectionRange
```

Symbols include modules/imports, tests, fixtures, functions, parameters, bindings, provider namespaces/functions, and record fields where identity exists. Definition on an imported alias can navigate to the import or target based on request semantics; implementation must be consistent and tested.

References are semantic, not text matches. Rename validates identifier syntax, export/import collisions, shadowing, and read-only/generated/provider symbols before returning edits. Edits carry document versions; adapters must not apply them to changed documents silently.

## 9. Completion, signature help, and hover

### 9.1 Completion

Completion is context-aware and returns replacement ranges, label/detail, kind, documentation, sort/filter text, optional additional edits, and semantic identity. Required contexts include:

- top-level/import declarations;
- statements valid in test/server/browser/control scopes;
- local/imported names;
- provider namespaces and schema functions;
- named call arguments excluding already supplied names;
- record fields and returned-value members;
- locator/action/assertion names and matcher states;
- types and fixture lifetime values.

Completion on malformed/half-typed code uses the recovered CST/HIR where available and never reparses an ad hoc substring.

### 9.2 Signature help

Signature help reports resolved overload/signature, active parameter, types, optional/default status, capability, and documentation. Provider signatures originate in the same schemas used by analysis; SDK or extension code does not recreate them.

### 9.3 Hover

Hover can show resolved type, declaration signature, capability, fixture lifetime, provider/schema source, and current-revision runtime evidence summary. Secret values and stale observations never appear.

## 10. Code actions, inlay hints, and tokens

Code actions are protocol-neutral edits tied to diagnostic codes. Initial actions include importing a uniquely resolvable symbol, adding a missing required named argument, replacing an unknown record field when one close match exists, qualifying a provider call, and applying canonical formatting. No action executes application code or mutates snapshots implicitly.

Inlay hints may show inferred local types, provider result types, fixture lifetimes, and parameter names where useful. They are configurable and derived from typed HIR.

Semantic tokens expand to declarations/references, types, parameters, properties, provider namespaces/functions, tests, fixtures, and deprecated/read-only modifiers. CST token classification remains the lexical foundation; semantic overlays come from resolved HIR. A TextMate grammar is not introduced as a second semantic implementation.

## 11. Test discovery and editor integration

Editor services expose a test tree with stable IDs:

```text
workspace
  -> module/file
      -> test declaration
          -> optional data/variant nodes later
```

Each test item includes label, URI/range, tags, static status, and runnable/debuggable identity. IDs derive from project/module/declaration semantic identity rather than display name alone.

The LSP may expose custom requests/notifications for discovery/run/debug routing until a suitable standard protocol exists. The Cursor/VS Code extension maps DTOs to the Testing API, run profiles, debug profiles, result messages, and trace links. It does not discover tests by scanning text.

Running an unsaved file uses synchronized editor content. Workspace/suite runs use a coherent database snapshot and record every source revision in the plan/execution.

## 12. Incremental LSP synchronization

Advertise incremental text synchronization. Apply LSP's UTF-16 range edits to the current versioned document and convert safely to internal UTF-8 byte ranges. Reject out-of-order versions, invalid ranges, or positions splitting Unicode scalar/UTF-16 boundaries without corrupting state; request/rely on the next full synchronization according to documented recovery behavior.

Handlers use cancellation tokens and immutable snapshots. Long workspace operations send progress notifications. Diagnostic publication, semantic-token deltas/full results, and runtime facts are discarded if their document/workspace revision is stale.

Protocol tests cover concurrent edits/requests, cancellation, shutdown, malformed positions, Unicode, provider-schema change, file create/delete/rename, and multi-root workspace behavior. If multi-root semantics are not implemented, reject them clearly rather than conflating roots.

## 13. Cross-file runtime and DAP mapping

Plans retain origin chains from expanded helper/fixture operations back to declaration and call sites. Runtime observations choose the actionable user range while preserving related locations for the declaration/provider schema.

DAP stack frames show test, helper, fixture setup/body/teardown, control scope, and operation frames across files. Breakpoints in helper/fixture declarations resolve to all executable plan instances for the selected tests. Source revisions are verified before pause; stale generated plan mappings are rejected.

## 14. Performance and quality budgets

Add a checked-in synthetic benchmark workspace with at least 1,000 files, 5,000 declarations, deep imports, provider schemas, and representative syntax errors. On documented CI hardware/profile:

- initial workspace diagnostics complete within an agreed baseline budget recorded in CI;
- a local function-body edit reparses that file and invalidates only semantic dependents;
- completion in an already analyzed file meets an interactive p95 budget;
- workspace symbol/reference queries are cancellable and bounded;
- repeated edits do not grow retained revisions/memory without bound.

Before implementation, record numeric budgets from the delivered baseline/prototype and make regressions visible. Correctness and invalidation assertions are mandatory even where wall-clock thresholds are platform-tolerant.

## 15. Architecture and crate responsibilities

- `syntax` remains the only lossless parser and adds module/import/export/helper/fixture AST views.
- `hir` owns stable declaration/reference IDs and semantic origin chains, not database implementation details.
- `analysis` owns workspace inputs, module graph, queries, name/type/capability resolution, indexes, and editor-ready semantic facts.
- `plan` owns expanded helper/fixture/resource scopes with cross-file origins.
- `runtime` consumes plans and fixture scopes without resolving imports or reading source.
- `editor` owns protocol-neutral feature DTOs and revision-aware orchestration.
- `lsp` converts positions/capabilities/messages only; it contains no completion, rename, or test-discovery semantics.
- `dap` maps shared frames/scopes/source chains only.
- `editors/vscode` maps test/editor DTOs to host APIs and contains no parser/index.
- Filesystem watchers/project discovery update database inputs through native adapters.

## 16. Delivery slices

1. Introduce the dependency-tracked database and port existing parse/HIR/diagnostic/plan queries with parity tests.
2. Add workspace file/config/schema inputs, immutable snapshots, cancellation, and memory retention policy.
3. Add module/import/export syntax, graph, name resolution, symbols, definition, and diagnostics.
4. Add helper functions/call graph and cross-file plan/source mapping.
5. Add fixtures/lifetimes/dependencies and explicit setup/provide/teardown planning.
6. Add completion/signature/hover, then references/rename/code actions/inlay hints/tokens.
7. Add incremental LSP sync, progress/cancellation, and protocol integration tests.
8. Add test discovery, Cursor Testing API, and cross-file DAP behavior.
9. Add benchmark workspace, invalidation assertions, performance dashboards, examples, and docs.

## 17. Testing requirements

Required coverage includes:

- module resolution across relative/project paths, Unicode/case/symlink behavior, missing imports, and cycles;
- stable symbol/binding/reference IDs across unrelated edits;
- query invalidation assertions for source, config, schema, file-set, and module changes;
- helper call-graph, capability, recursion, and cross-file origin tests;
- fixture dependency/lifetime/concurrency/acquire/teardown/retry tests;
- completion/signature/hover golden fixtures for valid and malformed source;
- definition/reference/rename collision and versioned-edit tests;
- code-action idempotence and semantic-token stability tests;
- incremental UTF-16 edit, cancellation, progress, stale-result, and workspace protocol tests;
- test-discovery stable-ID and synchronized-buffer run/debug tests;
- cross-file runtime observation and DAP breakpoint/frame tests;
- native/WASM query parity for portable inputs in preparation for Milestone G;
- benchmark/invalidation/memory regression tests.

## 18. Acceptance criteria

Milestone F is complete only when:

1. A representative multi-file project imports typed helpers/fixtures and compiles deterministic source-mapped plans.
2. Changing one file or `.webtest/app-schema.json` invalidates all and only semantic dependents under automated assertions.
3. Completion, hover, navigation, references, rename, actions, hints, symbols, and test discovery derive from shared Rust services and behave on half-typed Unicode source.
4. Fixture lifetimes acquire/share/teardown exactly as specified under parallel tests, retries, cancellation, and failure.
5. Incremental LSP sync never corrupts UTF-8 ranges or publishes stale diagnostics/tokens/runtime facts.
6. Cursor's test explorer discovers/runs/debugs tests without scanning source in TypeScript.
7. The benchmark workspace meets recorded responsiveness and bounded-memory gates.

The roadmap acceptance statement is thereby satisfied: multi-file projects remain responsive, provider schemas invalidate correctly, and every editor feature derives from shared Rust semantics.
