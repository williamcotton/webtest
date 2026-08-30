# Milestone D.6 — Analysis Query-Layer Decomposition

## 0. Status and dependencies

**Status: implemented (2026-08-30).**

This maintenance milestone follows the implemented application-bridge work in
[`milestone-d.md`](./milestone-d.md) and the application composition-root decomposition in
[`milestone-d-5.md`](./milestone-d-5.md). It prepares the existing single-file analysis
implementation for later workspace and editor work in [`milestone-f.md`](./milestone-f.md), but it
does not implement any Milestone F language or database capability.

The current `crates/analysis/src/lib.rs` is 2,436 lines. It combines:

- the crate's public facade and public analysis DTOs;
- source-file identity, source storage, path lookup, revisions, and the current per-file cache;
- provider-schema replacement and cache invalidation;
- parsing, syntax-diagnostic adaptation, HIR lowering, semantic compilation, and plan caching;
- type facts and the `type_at` query;
- completion, signature-help, and documentation/hover facts;
- provider-call validation and provider-schema-to-plan lowering;
- expression inference, type validation, binding resolution, and transferability checks;
- browser-operation and assertion lowering;
- semantic diagnostic construction, structured details, repair candidates, and description
  reference queries;
- deterministic test-plan and step construction;
- seven unit tests spanning several of those responsibilities.

Those responsibilities belong in the `analysis` crate, but they do not belong in one Rust module.
The already separated `description.rs`, `description/catalog.rs`, and
`description/app_bridge.rs` remain the canonical description implementation and are not the target
of this refactor.

At the time this plan was drafted, `cargo test -p webtest-analysis` passed all 14 analysis tests and
the crate's doc tests. That result is the starting baseline, not sufficient completion evidence:
several database, invalidation, editor-query, diagnostic-ordering, and plan-identity behaviors need
more direct characterization before code moves.

## 1. Outcome

`crates/analysis/src/lib.rs` becomes a small, stable public facade. It declares private modules and
re-exports the same root-level API that downstream crates use today. It contains no source storage,
query algorithms, semantic compiler, plan lowering, CST scanning, repair ranking, or unit-test
suite.

The intended top-level flow is:

```text
explicit source/provider inputs
        |
        v
database.rs
  |-- parse through the one syntax crate
  |-- collect syntax diagnostics
  |-- lower through the one HIR path
  |-- compile semantic facts and the TestPlan
  |-- cache one revision-coherent result
  |
  +-- editor_queries.rs projects completion/signature/documentation
  +-- description.rs projects the author-facing reference
        |
        v
root-level analysis API re-exported by lib.rs
        |
        +-- app / editor / LSP / DAP / WASM / app-bridge
```

Within semantic compilation:

```text
HIR file
  -> statement and scope compilation
       -> expression/provider inference
       -> type and capability validation
       -> browser/assertion lowering
  -> ordered diagnostics + type facts + deterministic TestPlan
```

The final `lib.rs` should normally remain below 100 lines. This is a design target, not permission
to hide the same implementation behind macros, `include!`, a generated file, or one replacement
module with thousands of lines.

The public crate path remains `webtest_analysis::*`. Moving a public type into a private internal
module must be paired with a root re-export so app, editor, LSP, DAP, WASM, and bridge consumers do
not change imports merely because internal ownership improved.

## 2. Research baseline

### 2.1 Current responsibility map

The current file divides approximately as follows:

| Lines | Current responsibility |
|---|---|
| `1–33` | module declaration, public description/feedback re-exports, and all imports |
| `34–109` | diagnostic, type-fact, completion, signature, documentation, and error DTOs |
| `111–134` | `SourceFile`, `CachedQueries`, and `AnalysisDatabase` storage |
| `135–276` | file lifecycle, source/revision, parse/HIR/diagnostic/type/plan queries, schema hashes |
| `277–496` | completion, signature-help, documentation, and description methods |
| `497–543` | revision-coherent parse → HIR → semantic compilation and cache population |
| `544–652` | CST token helpers, provider signatures, syntax references, duration diagnostics |
| `653–1913` | semantic compiler state, statement/expression/provider/browser/type checking, plan lowering |
| `1914–2193` | repair ranking, type helpers, reference routing, binding collection, browser helpers |
| `2194–2436` | mixed database, compiler, diagnostic, and syntax tests |

This inventory is descriptive of the current implementation. The line boundaries are not a future
API and should not be preserved after extraction.

### 2.2 Current public consumers

The compatibility surface is broader than the analysis crate's own tests:

| Consumer | Analysis surface used |
|---|---|
| `crates/app` | database construction, diagnostics, plans, description DTOs, resolved description configuration |
| `crates/editor` | database lifecycle, source/parse/diagnostics/plan, type facts, completion, signature, documentation, runtime diagnostic construction |
| `crates/lsp` | diagnostic severities/sources and completion-kind conversion through editor results |
| `crates/dap` | database construction, diagnostics, plans, and static-error gating |
| `crates/wasm` | database, diagnostics, portable compilation, completion, signature, hover, and description serialization |
| `crates/app-bridge` | project-provider schema analysis and structured repair-hint behavior in conformance tests |
| `description` tests | the normal database/compiler pipeline for every installed executable example |

The refactor therefore cannot be validated by `cargo test -p webtest-analysis` alone. Downstream
native and portable consumers are part of the compatibility boundary.

### 2.3 Current query model

The current database is an explicit-input, mutable, single-process per-file cache:

- `FileId`s are assigned monotonically when a new path is opened;
- reopening an existing path returns its existing `FileId` and updates its text through
  `set_file_text`;
- source revisions are BLAKE3-derived `SourceRevision` values from `webtest-text`;
- unchanged text does not evict the file's cached queries;
- changed text evicts only that file's cached query bundle;
- closing a file removes source, path lookup, and cached results;
- the cache bundle contains one mutually coherent parse, HIR file, diagnostic list, type-fact list,
  and plan for one source revision;
- replacing provider schemas always replaces the registry and clears all cached files only when
  the provider-name-to-schema-hash map changes;
- cache population parses, adapts syntax diagnostics, validates duration tokens, lowers HIR, then
  performs semantic compilation and plan construction once for the file revision;
- all fallible public queries use `AnalysisError::UnknownFile`; malformed source is represented by
  diagnostics rather than a query error.

Milestone F will replace or substantially extend this model with a dependency-tracked workspace.
D.6 must make that later replacement easier by isolating storage and orchestration in
`database.rs`; it must not partially implement snapshots, modules, Salsa, or cross-file queries now.

## 3. Architectural rules

### 3.1 Preserve the crate boundary

All moved behavior remains in `crates/analysis`. This milestone does not move analysis policy into
`editor`, `plan`, `syntax`, `hir`, `provider`, `app`, or an adapter merely to shorten a file.

The existing dependency rules remain mandatory:

- syntax is parsed only by `webtest-syntax`;
- HIR is lowered only through `webtest-hir` typed structures;
- analysis consumes explicit source text and `ProviderRegistry` schemas;
- plans remain syntax-independent `webtest-plan` DTOs;
- analysis does not read project configuration, the filesystem, environment variables, clocks,
  processes, sockets, browser state, runtime observations, or adapter protocol types;
- no reusable crate gains a production dependency back into analysis unless it already has one and
  the architectural guidelines authorize it.

No new crate is required. A cross-crate move is out of scope unless a separately reviewed change
proves the behavior belongs to that crate and preserves dependency direction.

### 3.2 Preserve one semantic pipeline

There remains exactly one path from a source revision and provider schema set to semantic results:

```text
webtest_syntax::parse
  -> syntax diagnostics
  -> webtest_hir::lower
  -> semantic compiler
  -> diagnostics + type facts + TestPlan
```

Completion, signature help, documentation, and description may inspect the cached CST and shared
provider schemas, but they must not create a second parser, lower a second semantic tree, infer
types independently, or construct adapter-specific plans.

The database owns query orchestration. The compiler owns semantic checking and plan construction.
Editor queries project facts from the cached CST/type facts/provider registry. These boundaries
must stay visible after extraction.

### 3.3 Keep the root API stable and internal visibility narrow

`lib.rs` is a facade, not an implementation owner. It should privately declare modules and publicly
re-export the same types and functions currently available at the crate root.

Do not make internal modules public to avoid fixing imports. New implementation symbols use the
narrowest visibility that works:

- `pub` only for the existing external contract;
- `pub(crate)` for collaboration between root analysis modules;
- `pub(super)` inside the semantic compiler family;
- private for leaf helpers and data fields wherever possible.

Splitting `impl Compiler` across child modules is acceptable. Cross-child methods should be
`pub(super)` only when another compiler responsibility genuinely calls them. Do not respond to Rust
privacy errors by making compiler state or helpers public at the crate boundary.

### 3.4 Organize by semantic responsibility

Modules own cohesive policies:

- `database` owns explicit inputs, file identity, revisions, caching, invalidation, and query
  orchestration;
- `diagnostic` owns the public diagnostic DTOs and shared structured-diagnostic/repair primitives;
- `facts` owns public data-only query result DTOs;
- `syntax_diagnostics` owns conversion of parser/CST facts into analysis diagnostics;
- `editor_queries` owns CST/type-fact/provider-schema projections for completion, signature help,
  and documentation;
- `compiler` owns semantic state and result assembly;
- compiler child modules own statements/scopes, expressions, provider calls, browser operations,
  and type-system rules;
- `description` remains the existing author-facing catalog/query implementation.

Do not add `helpers.rs`, `utils.rs`, `common.rs`, `semantics.rs`, or another vague catch-all. If a
function has two unrelated reasons to change, split the responsibility before moving it.

### 3.5 Preserve source, range, revision, and identity invariants

Internal ranges remain UTF-8 byte offsets. The smallest current `SyntaxOrigin` must survive every
move. This includes member-name diagnostics, provider/operation/argument diagnostics, locator
origins, browser value origins, assertion origins, and binding-name type facts.

Every `TestPlan` retains the exact `FileId` and `SourceRevision` used to create it. Test IDs continue
to originate in HIR. Step IDs continue to be deterministic and monotonically assigned across the
entire file in source compilation order; they are not reset at each test. Required host
capabilities remain deterministically sorted.

Do not replace typed IDs with indexes, paths, pointer identity, or display strings during the
refactor.

### 3.6 Preserve structured diagnostics

Do not flatten diagnostics into strings. Preserve:

- severity and source;
- stable diagnostic code;
- precise byte range;
- message;
- typed JSON semantic details;
- bounded structured repair hints and their byte ranges;
- canonical `reference_queries`;
- diagnostic ordering.

Syntax, semantic, and runtime remain distinct `DiagnosticSource` values even though this crate's
compiler emits only syntax and semantic diagnostics. `editor` currently constructs runtime
diagnostics using the public DTO; that external construction remains supported.

### 3.7 Refactor before redesign

Initial extraction should move coherent code with minimal reshaping. Once tests prove parity,
internal function signatures may be narrowed to make ownership explicit. Do not combine the
refactor with a new interning layer, arena, generic query trait, visitor framework, diagnostic
registry, type-class abstraction, dependency-injection container, or async database.

An abstraction introduced during D.6 must have a present concrete need. Future Milestone F
requirements alone are not enough to add unused infrastructure.

## 4. Scope

This milestone includes:

- characterization of the current public database and semantic-query behavior;
- root-level API compatibility tests or compile-time consumer coverage;
- extraction of public diagnostic and query-fact DTOs;
- extraction of source storage, file identity, revision, cache, and provider invalidation;
- extraction of syntax-diagnostic conversion and duration validation;
- extraction of completion, signature-help, and documentation query algorithms;
- decomposition of the semantic compiler by statement, expression, provider, browser, and type
  responsibilities;
- preservation of one compiler state and one deterministic output assembly path;
- movement and expansion of tests next to the behavior they protect;
- removal of obsolete imports, dead compatibility wrappers, and unnecessarily broad visibility;
- documentation of the resulting internal module contract;
- focused and workspace-wide formatting, test, Clippy, native, and portable verification.

Small private naming or signature refinements are allowed when they clarify responsibility and do
not alter behavior. Existing `description` imports may be adjusted to use new root re-exports, but
description content and mechanics are otherwise outside the refactor.

## 5. Non-goals

This milestone does not:

- add modules, imports, exports, functions, fixtures, declaration indexes, symbols, references,
  rename, code actions, folding, selection ranges, inlay hints, or test discovery;
- implement Salsa or another dependency-tracked database;
- add immutable snapshots, cancellation, parallel queries, cross-file invalidation, workspace
  graphs, filesystem discovery, or bounded historical-revision retention;
- change the language grammar, CST, typed AST, HIR, provider schemas, type system, capabilities,
  transferability rules, browser operations, assertions, or plan format;
- add retries, actionability, traces, parallel execution, modules, or any other roadmap behavior;
- change any diagnostic code, severity, source, wording, ordering, range, semantic detail, repair
  hint, candidate bound, or reference query intentionally;
- change completion labels, order, insertion text, details, documentation, or half-typed behavior;
- change signature labels, parameter order, or active-parameter selection;
- change hover/documentation precedence or type-hover formatting in `editor`;
- change `FileId` allocation, path identity, source revision calculation, cache granularity, or
  provider-schema invalidation rules;
- stop plan construction when diagnostics exist or allow clients to execute a statically invalid
  plan;
- optimize by caching individual compiler subqueries or sharing analysis databases across current
  app command invocations;
- change `webtest describe` content, IDs, aliases, search results, examples, limits, or native/WASM
  parity;
- change editor runtime-observation composition, LSP UTF-16 conversion, DAP one-based positions, or
  any adapter protocol DTO;
- split `analysis` into additional crates;
- update the WebTest agent skill or examples when no author-facing behavior changes.

Behavioral improvements discovered during extraction should be recorded as follow-up work. They
must not be smuggled into D.6 unless they fix a regression caused by the refactor itself.

## 6. Compatibility contract

The refactor is complete only if the observable behavior below remains compatible.

### 6.1 Root-level public API

Preserve the current root-level names and method signatures, including:

- `AnalysisDatabase` and `AnalysisError`;
- `Diagnostic`, `DiagnosticSeverity`, and `DiagnosticSource`;
- `TypeFact`;
- `Completion`, `CompletionKind`, `Signature`, `SignatureParameter`, and `DocumentationFact`;
- `RepairHint`, `RepairHintKind`, and `RepairReplacement` re-exports;
- every current description DTO and the free `describe` function;
- `AnalysisDatabase::default` and `with_provider_registry`;
- file lifecycle and lookup methods;
- source, path, and revision methods;
- parse, HIR, diagnostic, type-fact, type-at, and plan methods;
- provider schema hash and replacement methods;
- completion, signature-help, documentation, and database description methods.

Preserve derives and serialized shapes. In particular, completion/signature serialization and
`CompletionKind`'s `snake_case` representation must not drift. Preserve `AnalysisError`'s display
text and `UnknownFile(FileId)` payload.

Do not require downstream imports from `webtest_analysis::database` or another internal module.
Existing root imports must compile unchanged.

### 6.2 File lifecycle and source identity

Preserve:

- monotonically increasing `FileId` allocation for new paths;
- the same `FileId` when an already-open path is opened again;
- source update behavior through `open_file` and `set_file_text`;
- no revision/cache change when replacement text hashes to the current revision;
- per-file cache eviction when source revision changes;
- path and cache removal on `close_file`;
- `None` from `file_for_path` after close;
- `UnknownFile` from fallible read/query methods after close or for an unknown ID;
- the current no-op behavior when `set_file_text` receives an unknown ID;
- source text returned as `Arc<str>` and path returned as the stored path string;
- `SourceRevision::of` as the only source revision calculation.

Path strings remain explicit caller-supplied identities. Analysis does not canonicalize URIs or
filesystem paths; project/editor adapters continue to own that policy.

### 6.3 Cache coherence and provider invalidation

One cached entry must always represent one coherent tuple:

```text
(FileId, SourceRevision, provider schema hash set)
  -> Parse + HirFile + diagnostics + type facts + TestPlan
```

Preserve lazy population through `ensure_queries` and reuse across public queries. A query must not
return a new parse with old type facts or an old plan with new provider diagnostics.

`set_provider_registry` preserves the current sequence:

1. compute current provider-name-to-schema-hash values;
2. compute incoming values;
3. replace the registry;
4. clear all cached file queries if and only if those maps differ.

Equivalent schema inputs therefore retain cached semantic results, while an added, removed, or
changed schema invalidates diagnostics, type facts, completions, signatures, documentation, plans,
and descriptions derived from the registry. The database must never retain a separate stale
provider view in an extracted query module.

### 6.4 Parse, HIR, and diagnostic sequencing

Preserve the cache-population order:

1. parse the exact source text with `webtest_syntax::parse`;
2. adapt parser errors to `DiagnosticSource::Syntax` in parser-provided order;
3. append invalid-duration semantic diagnostics in CST traversal order;
4. lower the same `Parse` to HIR with the current `FileId`;
5. compile the HIR with the same source revision and provider registry;
6. append semantic diagnostics in encounter order while building type facts and the plan;
7. cache the complete bundle atomically with respect to the mutable database call.

Malformed and half-typed source remains lossless and queryable. A syntax or semantic diagnostic is
not an `AnalysisError`. The compiler continues producing a deterministic best-effort plan and facts
for invalid source; app/editor/DAP callers remain responsible for refusing execution when an error
severity is present.

### 6.5 Semantic analysis and type facts

Preserve all current rules and fallback behavior, including:

- declaration collection before statement compilation so use-before-definition and unknown-name
  diagnostics remain distinct;
- per-test binding/name reset;
- duplicate binding and duplicate record-field detection;
- contextual typing for lists/records and the empty-list diagnostic;
- primitive, collection, option, record, response, process, locator, browser, path, and temporary
  directory type lowering;
- unary and binary operator validation and result types;
- record/member resolution and known-member repair candidates;
- JSON-to-shaped-value decode insertion and response-operation provenance;
- server-to-browser transferability validation;
- direct-effect provider-call restrictions;
- type facts for expression and binding ranges with the same capability;
- `type_at` choosing the smallest fact whose range contains the offset or ends at the offset.

Unknown and error recovery values remain represented as they are today so one error does not cause
unbounded or nondeterministic follow-on failures.

### 6.6 Provider analysis and plan lowering

Preserve:

- built-in schemas in `AnalysisDatabase::default`;
- the reserved `app` behavior when no application schema is configured;
- exact provider, operation, argument, duplicate, conflict, missing, capability, and type
  diagnostics;
- positional versus named argument matching;
- mutual exclusion among HTTP `json`, `text`, `bytes`, and `form` body arguments;
- result types, schema hashes, retry-safety, secret argument names, and recursively collected secret
  result fields in `ServerProviderCall`;
- provider calls as explicit plan steps when they are direct statements or binding values;
- `EvaluatePure` steps for pure expressions;
- result binding IDs, result names, and result types;
- provider schema hashes returned in deterministic provider-name order.

Candidate repair ranking remains bounded to five and deterministic by edit distance followed by
candidate string ordering. Edit distance continues to operate on Unicode scalar values, not bytes.

### 6.7 Browser operations, assertions, and source origins

Preserve the exact lowering and origin choice for:

- navigation and raw browser evaluation;
- click, fill, type, press, check, uncheck, select, and hover;
- locator and URL waits;
- locator-state and URL assertions;
- value assertions and matcher conversion;
- string-value typing for fill/type/press/select;
- key-chord validation;
- browser capability diagnostics outside browser scope;
- locator conversion through `webtest_plan::locator_from_hir` and state conversion through
  `locator_state_from_hir`.

Assertions remain explicit `TestOperation::Assertion` steps, not opaque browser callbacks. Failure
ranges must continue to underline the current narrow locator, value, member, or URL origin.

### 6.8 Deterministic plan construction

Preserve:

- source-order tests and statements;
- HIR-provided test IDs;
- file-global, monotonically increasing `StepId` allocation;
- exact operation variants and nesting of every `PlanExpr`;
- expression precedence in the plan tree;
- file and source revision on `TestPlan`;
- sorted, deduplicated required host capabilities;
- stable binding IDs and result metadata;
- exact plan equality for a fixed source and provider schema input.

The refactor must not renumber steps, reset IDs per test, reorder diagnostics/capabilities/provider
arguments, or change a best-effort recovery expression merely because methods moved into different
files.

### 6.9 Completion, signature, and documentation queries

Preserve editor-query behavior for valid and half-typed source:

- select the smallest containing call/member node by byte range;
- suggest only absent named parameters for a resolved provider call;
- preserve parameter schema order, labels, types, optional markers, documentation, kinds, and
  insertion text;
- complete provider operations after a provider member expression;
- complete record fields from the shared `TypeFact` set;
- retain the lossless-CST fallback that recovers a prior binding's fact for half-typed `value.`;
- return no speculative result when the provider, operation, receiver, or type cannot be resolved;
- construct provider signatures from the same operation schema used by semantic analysis;
- preserve parameter ordering and active-parameter clamping/counting;
- preserve provider-call documentation range and contents, including result type and retry safety;
- continue letting `editor` prefer provider documentation over type hover.

Extracted editor-query functions should receive the cached `Parse`, type facts, and provider
registry explicitly. They must not retain their own source, provider, or type cache.

### 6.10 Description, native, and portable parity

The free `describe` function and `AnalysisDatabase::describe` continue using the same active
`ProviderRegistry`. Existing description catalog files, IDs, aliases, categories, limits,
sanitization, canonical examples, and project-provider projection remain unchanged.

All installed canonical examples must continue to parse and statically analyze through the normal
database/compiler path. Native and WASM description, diagnostics, completion, signature, hover, and
portable plan results remain structurally equal for the same portable inputs.

Because D.6 adds no author-facing construct, operation, type, capability, provider feature, or
guidance, it does not add a `webtest describe` topic and does not update the canonical WebTest skill.

## 7. Target module layout

The target layout is:

```text
crates/analysis/src/
├── lib.rs
├── database.rs
├── diagnostic.rs
├── facts.rs
├── syntax_diagnostics.rs
├── editor_queries.rs
├── compiler.rs
├── compiler/
│   ├── statements.rs
│   ├── expressions.rs
│   ├── provider_calls.rs
│   ├── browser_operations.rs
│   ├── type_system.rs
│   └── tests.rs
├── description.rs
└── description/
    ├── app_bridge.rs
    └── catalog.rs
```

This layout is prescriptive about ownership, not every private function name. A compiler child may
be split further if it remains too large, but it may not be collapsed into a new omnibus
`semantic.rs` or left as a renamed copy of the current 1,200-line `Compiler` region.

As a practical review target:

- `lib.rs` normally remains below 100 lines;
- no new production module should normally exceed 500 lines;
- line-count targets do not justify macros, generated indirection, artificial one-function files,
  or separation of code from its focused tests.

The existing description modules are exempt from the new-module line-count target because their
large catalog decomposition is not part of D.6. Their future refactoring, if needed, should be a
separate milestone.

### 7.1 `lib.rs`

Owns only:

- the crate-level documentation;
- private module declarations;
- root-level public re-exports for database, diagnostics, facts, feedback, and description.

It must not import HIR expression variants, plan operations, provider operation schemas, syntax
nodes/tokens, collection types used only by implementation, or hold a `#[cfg(test)] mod tests`
containing semantic behavior.

### 7.2 `database.rs`

Owns:

- `AnalysisError`;
- `SourceFile`;
- `CachedQueries`;
- `AnalysisDatabase` storage and `Default`;
- file/path lifecycle;
- source and revision access;
- provider registry ownership, schema hashes, replacement, and invalidation;
- public parse/HIR/diagnostic/type/plan queries;
- `type_at` selection over cached facts;
- delegation to editor-query and description functions;
- the single `ensure_queries` orchestration path.

`database.rs` may call `syntax_diagnostics::collect`, `compiler::compile`,
`editor_queries::*`, and `description::describe`. It must not contain semantic match trees,
provider argument validation, browser lowering, completion CST scanning, or catalog content.

Keep `SourceFile` and `CachedQueries` private. Do not expose cache entries or provider registries to
downstream crates as an expedient extraction seam.

### 7.3 `diagnostic.rs`

Owns:

- `DiagnosticSeverity`;
- `DiagnosticSource`;
- `Diagnostic`;
- shared constructors or builders for syntax and semantic diagnostics where they reduce repeated
  field assembly without hiding the resulting fields;
- bounded text-replacement hint construction;
- deterministic nearest-string ranking and Unicode edit distance;
- shared default reference-query routing and type-reference naming.

It depends on `webtest-feedback`, `webtest-provider` type names where required, and text ranges. It
must not know about `AnalysisDatabase`, compiler binding state, plans, CST traversal, editor offsets,
or adapters.

The public diagnostic structs retain public fields because `editor` currently constructs runtime
diagnostics. Internal constructors supplement that contract; they do not make the type opaque.

### 7.4 `facts.rs`

Owns the public data-only result types:

- `TypeFact`;
- `CompletionKind` and `Completion`;
- `SignatureParameter` and `Signature`;
- `DocumentationFact`.

It contains derives and field definitions, not query algorithms. This keeps the semantic compiler
from depending on CST editor-query implementation merely to emit `TypeFact` values.

### 7.5 `syntax_diagnostics.rs`

Owns:

- parser-error adaptation to analysis `Diagnostic` values;
- syntax diagnostic reference-query selection;
- lookup of an offending lossless token for scope-specific browser references;
- invalid positive-duration validation over CST duration tokens;
- diagnostic ordering for those pre-HIR phases.

Expose one narrow crate-private entry point such as:

```rust,ignore
pub(crate) fn collect(parse: &Parse) -> Vec<Diagnostic>;
```

`database` should not separately call multiple syntax-diagnostic helpers because their ordering is
part of the compatibility contract. This module may inspect CST tokens; it must not lower HIR or
construct plan operations.

### 7.6 `editor_queries.rs`

Owns:

- meaningful-token extraction needed by editor queries;
- provider/operation token recognition from a call/member CST;
- named-argument token recognition;
- provider signature rendering;
- completion selection and construction;
- signature-help selection and construction;
- provider documentation lookup and construction.

Its core functions should be pure over explicit inputs, for example:

```rust,ignore
pub(crate) fn completions(
    parse: &Parse,
    type_facts: &[TypeFact],
    providers: &ProviderRegistry,
    offset: TextSize,
) -> Vec<Completion>;

pub(crate) fn signature_help(
    parse: &Parse,
    providers: &ProviderRegistry,
    offset: TextSize,
) -> Option<Signature>;
```

The exact private names may differ. The important boundary is that the database ensures and passes
one coherent cache entry; the query module performs no mutation, parsing, HIR lowering, type
inference, provider loading, or independent caching.

### 7.7 `compiler.rs`

Owns the compiler family's shared state and narrow entry point:

- `CompileResult`;
- `BindingState`;
- `TypedExpr`;
- `CompiledProviderCall` if it remains shared across children;
- `Compiler` fields and constructor;
- per-file/per-test state reset;
- final diagnostic, type-fact, capability, and `TestPlan` assembly;
- shared diagnostic emission methods whose only purpose is to append to compiler state;
- child module declarations.

The database-facing entry should be a narrow function rather than exposing `Compiler`, for example:

```rust,ignore
pub(crate) fn compile(
    file: FileId,
    revision: SourceRevision,
    providers: &ProviderRegistry,
    diagnostics: Vec<Diagnostic>,
    hir: &HirFile,
) -> CompileResult;
```

`Compiler` and its fields remain private to the compiler family. `CompileResult` exposes only what
`database` must cache. Do not turn compiler state into a public session object.

### 7.8 `compiler/statements.rs`

Owns:

- server/browser block traversal;
- statement dispatch;
- binding declaration/duplicate handling;
- direct provider-call versus pure-binding step selection;
- contextual annotation/decode handling at a binding;
- value expectation compilation;
- declaration-name precollection across nested blocks;
- ordered `PlannedStep` insertion and `StepId` assignment.

It coordinates expression, provider, browser, and type methods on `Compiler`; it must not duplicate
their validation logic.

### 7.9 `compiler/expressions.rs`

Owns recursive HIR expression inference and `PlanExpr` construction for:

- literals;
- resolved and unresolved names;
- lists and records;
- member access;
- calls in expression position;
- unary expressions;
- binary expressions;
- missing/error-recovery expressions;
- expression-range `TypeFact` emission.

It may request provider-call resolution and type validation through `Compiler` methods, but it must
not query the database or parse CST/source text. Preserve recursive evaluation order because it
affects diagnostic and type-fact ordering.

### 7.10 `compiler/provider_calls.rs`

Owns:

- recognition of direct provider calls from typed HIR;
- reserved/unknown provider handling;
- operation lookup and capability validation;
- named/positional argument matching;
- duplicate, conflicting, unknown, and missing argument diagnostics;
- contextual argument inference and type validation;
- provider schema hash, result type, retry-safety, redacted arguments, and secret result-field
  projection;
- response-operation provenance used by JSON decode failures.

Provider operation signatures used by completion remain in `editor_queries`; both paths consume the
same `OperationSchema`, and neither recreates schemas.

Provider arguments contain expressions, and expression-position calls ask whether a call is a
provider call. This mutual semantic recursion should remain explicit through narrow `Compiler`
methods. Do not resolve it by giving either module a database handle or by duplicating call
recognition.

### 7.11 `compiler/browser_operations.rs`

Owns:

- browser-capability validation;
- lowering of every current `HirBrowserOp`;
- conversion of value-bearing actions through shared expression inference;
- key-chord validation;
- selection of operation/assertion origins;
- browser construct reference queries;
- locator/state conversion calls.

It produces protocol-neutral plan operations only. It must not depend on `webtest-browser`, CDP,
runtime, editor, LSP, DAP, or app types.

### 7.12 `compiler/type_system.rs`

Owns:

- HIR type lowering;
- type-pattern lowering;
- expected/actual compatibility checks;
- type-mismatch details and reference queries;
- binary operator validation and result typing;
- primitive named-type resolution;
- decodable-type checks;
- numeric checks/result promotion;
- known-member projection;
- value-matcher conversion;
- typed and unknown recovery expression construction;
- recursive secret record-field collection if it is not kept with provider calls.

This is a module of current explicit rules, not a new generic type engine. It must remain independent
of source storage, CST scanning, editor positions, and adapters.

### 7.13 `compiler/tests.rs`

Owns integration-level semantic compiler characterization where a single behavior crosses multiple
compiler child modules. Focused leaf tests may remain beside their implementation when they do not
need the full source → parse → HIR → compiler pipeline.

The current mixed top-level tests move as follows:

- source-update/cache behavior to `database` tests;
- scope-specific syntax reference behavior to `syntax_diagnostics` tests;
- completion/signature/documentation cases to `editor_queries` tests;
- typed flow, transferability, names, expression precedence, provider corrections, and plan
  determinism to compiler tests.

Do not recreate a 1,000-line `compiler/tests.rs`. Prefer focused test modules and shared fixture
constructors scoped under `#[cfg(test)]` when the suite grows.

## 8. Internal dependency direction

The desired internal dependency direction is:

```text
lib (facade/re-exports)
  -> database, diagnostic, facts, description

database
  -> syntax_diagnostics
  -> compiler
  -> editor_queries
  -> description

syntax_diagnostics
  -> diagnostic

editor_queries
  -> facts
  -> syntax + provider schemas

compiler family
  -> diagnostic + facts
  -> HIR + provider schemas + plan + text

description
  -> syntax + provider schemas
  -> root analysis API only in its tests/examples
```

The following internal edges are forbidden:

- `diagnostic` or `facts` depending on `database` or compiler implementation;
- compiler modules depending on `database`, `description`, editor services, or adapters;
- `editor_queries` depending on compiler state or constructing a second semantic environment;
- `syntax_diagnostics` depending on HIR, plan, or compiler state;
- `description` importing private compiler internals;
- a compiler child importing a sibling's private state directly instead of collaborating through
  the shared `Compiler` implementation;
- a new global mutable cache or registry;
- a generic context containing optional database, CST, HIR, provider, plan, and editor state.

The `database` is the only module allowed to coordinate the whole per-file pipeline. The compiler
is the only module family allowed to turn HIR into semantic diagnostics/type facts/plan operations.

## 9. Current symbol migration map

| Current symbol or group | Destination |
|---|---|
| public description re-exports | `lib.rs`, still backed by existing `description` |
| feedback repair re-exports | `lib.rs` |
| `DiagnosticSeverity`, `DiagnosticSource`, `Diagnostic` | `diagnostic.rs`, re-exported by `lib.rs` |
| `TypeFact`, completion/signature/documentation DTOs | `facts.rs`, re-exported by `lib.rs` |
| `AnalysisError` | `database.rs`, re-exported by `lib.rs` |
| `SourceFile`, `CachedQueries`, `AnalysisDatabase` | `database.rs` |
| all file/source/path/revision methods | `database.rs` |
| parse/HIR/diagnostic/type/plan methods and `ensure_queries` | `database.rs` |
| provider hash and replacement/invalidation methods | `database.rs` |
| `type_at` | `database.rs` over cached `TypeFact`s |
| database `describe` method | `database.rs`, delegating to `description` |
| completion/signature/documentation methods | public wrappers in `database.rs`, algorithms in `editor_queries.rs` |
| `meaningful_tokens`, provider/member/argument CST token helpers | `editor_queries.rs` unless syntax-diagnostic-specific |
| `provider_signature` | `editor_queries.rs` |
| `syntax_reference_queries`, invalid duration collection | `syntax_diagnostics.rs` |
| `CompileResult`, compiler shared state/result types | `compiler.rs` |
| `Compiler::new`, final `compile` assembly | `compiler.rs` behind crate-private `compile` |
| statement/block/binding/expectation compilation | `compiler/statements.rs` |
| recursive `infer_expr` | `compiler/expressions.rs` |
| provider call and argument validation | `compiler/provider_calls.rs` |
| browser operation lowering and browser helpers | `compiler/browser_operations.rs` |
| HIR type/pattern lowering and type/operator helpers | `compiler/type_system.rs` |
| compiler error append methods | `compiler.rs`, using shared primitives from `diagnostic.rs` |
| text hints, nearest strings, edit distance, default references | `diagnostic.rs` |
| `collect_binding_names` | `compiler/statements.rs` |
| `secret_record_fields` | `compiler/provider_calls.rs` or `compiler/type_system.rs`, with one owner |
| current top-level tests | owning module tests plus `compiler/tests.rs` |

Do not leave forwarding copies in `lib.rs` after migration. Public methods remain inherent methods on
`AnalysisDatabase`, but their implementation owner is `database.rs`.

## 10. Interface design

### 10.1 Database orchestration

`database` should exchange narrow owned/borrowed values with leaf modules. Representative shapes
are:

```rust,ignore
let parse = webtest_syntax::parse(&source.text);
let diagnostics = syntax_diagnostics::collect(&parse);
let hir = Arc::new(webtest_hir::lower(file, &parse));
let compiled = compiler::compile(
    file,
    source.revision,
    &self.providers,
    diagnostics,
    &hir,
);
```

This is illustrative, not permission to hold an immutable borrow of `self.files` across mutation of
`self.cache` in a way Rust rejects. The implementation may copy `SourceRevision` and clone the
source `Arc<str>` before compilation, provided observable behavior and memory ownership remain
equivalent.

Do not expose an all-purpose `QueryContext`. The database already owns the explicit inputs and
should pass only the inputs each operation needs.

### 10.2 Compiler result

The database needs one owned result from the compiler:

```rust,ignore
pub(crate) struct CompileResult {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) type_facts: Vec<TypeFact>,
    pub(crate) plan: TestPlan,
}
```

The exact field visibility may be narrower through accessors. Do not return unexplained tuples, a
partially initialized cache entry, or references tied to temporary compiler state.

### 10.3 Compiler collaboration

Keep one `Compiler<'a>` owning:

- immutable file, revision, and provider inputs;
- accumulated diagnostics and type facts;
- current binding/name/declaration state;
- accumulated required capabilities;
- next file-global step ID.

Child modules may add inherent `impl Compiler<'_>` blocks. Methods called by sibling compiler
modules should be `pub(super)`; leaf-only helpers stay private. Shared state should not be broken
into multiple independently mutable objects unless doing so clearly eliminates, rather than hides,
borrow complexity.

### 10.4 Diagnostic construction

Shared constructors may reduce repeated initialization, but call sites must still visibly choose
code, range, message, details, hints, and reference queries. Avoid a stringly typed diagnostic
registry or builder whose defaults silently change existing output.

Candidate computation should return owned, deterministically ordered values. The current five-item
bound belongs at the semantic call sites or in a clearly named bounded helper; it must not become an
unbounded editor/API payload.

### 10.5 Tests and visibility

Prefer testing through the public database for end-to-end semantic contracts. Use private leaf tests
for pure functions such as edit distance, key chords, type-name lowering, and source-node selection.
Do not make an implementation helper `pub` merely so an external integration test can call it.

## 11. Delivery slices

Each slice must compile, format, and pass the focused analysis tests before the next slice starts.
Prefer one focused commit per slice. A slice may move tests before production code, but it must not
leave duplicate semantic implementations active.

### Slice 1 — Characterize the current boundary

Before moving production code:

1. Record the current root-level public imports in a compile-only or downstream compatibility test.
2. Add database lifecycle tests for open/reopen/update/unchanged/close/unknown-file behavior.
3. Add cache-coherence and provider-registry invalidation tests for equivalent, changed, added, and
   removed schemas.
4. Add query-order tests proving parse, HIR, diagnostics, type facts, and plan all carry one current
   revision after edits.
5. Add deterministic plan fixtures covering multiple tests, file-global step IDs, capability order,
   exact origins, bindings, provider metadata, and expression precedence.
6. Add editor-query fixtures for provider operation completion, missing-argument completion,
   record-member completion, half-typed `value.`, nested calls, signature active parameters, and
   documentation.
7. Expand structured diagnostic fixtures for syntax ordering, semantic ordering, ranges, details,
   references, and bounded corrections.
8. Retain the installed description-example and project-provider parity tests.

Characterization tests should assert durable contracts, not private hash-map layout, allocation
addresses, compiler method names, or module paths.

### Slice 2 — Extract public data-only types

1. Move diagnostics to `diagnostic.rs` without changing fields, derives, or visibility.
2. Move type/editor query facts to `facts.rs` without changing serialized shapes.
3. Move `AnalysisError` to the initial `database.rs` shell if doing so does not yet require moving
   database behavior.
4. Add root re-exports and prove downstream imports remain unchanged.
5. Move tests of DTO equality/serialization/display with their owners.

At the end of this slice, `lib.rs` may still contain behavior, but it no longer owns public DTO
definitions.

### Slice 3 — Extract diagnostic primitives and syntax diagnostics

1. Move edit distance, candidate ordering, bounded text hints, default references, and type
   reference naming to `diagnostic.rs`.
2. Add focused Unicode and deterministic tie-order tests.
3. Move syntax error adaptation/reference selection and invalid-duration diagnostics to
   `syntax_diagnostics.rs` behind one ordered `collect` function.
4. Move the illegal-browser-action-in-server-scope test and add malformed/half-typed duration
   cases.
5. Verify diagnostic list order and exact ranges before continuing.

Do not move provider/member CST token helpers used by completion into syntax diagnostics merely
because both inspect tokens; their policies differ.

### Slice 4 — Extract editor query projections

1. Move meaningful-token, provider-call-token, named-argument, and signature rendering helpers to
   `editor_queries.rs`.
2. Move completion, signature-help, and documentation algorithms as pure functions over explicit
   parse/fact/provider inputs.
3. Keep the public inherent methods on `AnalysisDatabase` as narrow wrappers.
4. Add/retain half-typed CST recovery and smallest-containing-node tests.
5. Verify LSP and WASM completion/signature/hover parity.

The extracted functions must consume cached facts. They must not call `webtest_syntax::parse` or
lower HIR themselves.

### Slice 5 — Establish the compiler family and type rules

1. Create `compiler.rs` with `Compiler`, shared state/result types, the narrow `compile` entry, and
   final `TestPlan` assembly.
2. Move diagnostic append and type-fact append methods without changing call order.
3. Move HIR type/pattern lowering, compatibility, numeric/operator, matcher, known-member, and
   recovery helpers to `compiler/type_system.rs`.
4. Keep method bodies mechanically equivalent first; narrow visibility after the slice passes.
5. Move relevant type/operator tests with the new owner.

The current compiler in `lib.rs` may temporarily delegate into this family during the slice, but
there must never be two active compilers or two plan-construction paths.

### Slice 6 — Extract expression and provider compilation

1. Move provider recognition, lookup, argument validation, redaction metadata, and response
   provenance to `compiler/provider_calls.rs`.
2. Move recursive expression inference and `PlanExpr` construction to
   `compiler/expressions.rs`.
3. Preserve the mutual call path between provider arguments and expression inference through
   narrow `Compiler` methods.
4. Move nearest-name/member/provider/operation/argument diagnostic cases with these owners.
5. Compare exact diagnostics, type facts, and plan JSON for the characterization fixtures.

Expression recursion order and provider argument iteration order are behavior. Do not refactor them
into unordered traversal or a generic visitor in this slice.

### Slice 7 — Extract browser and statement compilation

1. Move browser operation/assertion lowering, origins, references, string values, and key chords to
   `compiler/browser_operations.rs`.
2. Move block/statement traversal, binding compilation, expectations, declaration collection, and
   step insertion to `compiler/statements.rs`.
3. Remove the old compiler implementation from `lib.rs` immediately as each owner takes over.
4. Verify every `HirBrowserOp` and assertion variant through exact plan/range tests.
5. Verify multiple-test step IDs and required capabilities remain unchanged.

At the end of this slice, no semantic compiler implementation remains in `lib.rs`.

### Slice 8 — Extract the database and finalize the facade

1. Move source/cache/database storage and all inherent database methods to `database.rs`.
2. Make `ensure_queries` the only coordinator of syntax diagnostics, HIR lowering, compiler
   execution, and cache insertion.
3. Remove behavioral imports and mixed tests from `lib.rs`.
4. Reduce `lib.rs` to documentation, private module declarations, and root re-exports.
5. Narrow all new visibility and remove dead transitional forwarding functions.
6. Inspect the internal module graph for forbidden reverse dependencies.

### Slice 9 — Downstream parity and cleanup

1. Run focused editor, LSP, WASM, DAP, app-bridge, and app tests.
2. Compare representative native and WASM diagnostics/completions/signatures/hover/plans.
3. Run the installed description catalog examples through the normal pipeline.
4. Run workspace formatting, Clippy, tests, and representative CLI static checks.
5. Confirm `Cargo.toml` has no unnecessary new dependency or feature.
6. Update this specification's status only after every acceptance criterion passes.

## 12. Testing requirements

### 12.1 Database and invalidation tests

Required coverage includes:

- first open, duplicate-path open, text-changing reopen, and unchanged reopen;
- monotonic IDs and no accidental ID reuse after close;
- source/path/revision lookup and exact unknown-file errors;
- close removing source, path mapping, and cached results;
- changed source invalidating only the changed file in the current per-file model;
- unchanged source retaining the current cached revision bundle;
- equivalent provider schema hashes retaining cached semantic results;
- changed/added/removed provider schemas invalidating every cached file;
- replacement registry being used by description and editor queries after invalidation;
- diagnostics/type facts/plans reflecting the same current revision after rapid updates.

Tests may inspect private cache state from `database.rs` where necessary to prove invalidation, but
public behavior should be preferred when it makes the contract observable without coupling to a
specific map implementation.

### 12.2 Compiler and plan tests

Required coverage includes:

- typed pure/server/browser flows;
- multiple tests with exact file-global step IDs;
- exact `FileId`, source revision, `TestId`, `BindingId`, `StepId`, and `SyntaxOrigin` values;
- deterministic required capability ordering;
- direct provider statement, bound provider call, effectful call nested in an expression, and
  unknown/reserved provider behavior;
- positional/named/optional/missing/duplicate/unknown/conflicting arguments;
- schema hash, retry-safe, redacted argument, and nested secret-result-field metadata;
- list/record inference, annotations, JSON decode, response operation, and transferability;
- literals, names, members, unary/binary operations, precedence, and error recovery;
- duplicate/use-before/unknown name distinctions;
- every browser operation, wait, and assertion with exact operation and narrow range;
- valid and invalid key chords;
- best-effort plan determinism in the presence of syntax or semantic diagnostics.

Where practical, assert exact plan DTOs rather than only matching the outer operation variant.

### 12.3 Diagnostic and repair tests

Required coverage includes:

- parser diagnostic conversion and ordering;
- invalid zero/malformed/overflow duration diagnostics and references;
- exact semantic codes, severity, source, message, and byte ranges;
- typed mismatch details;
- capability and transferability details/references;
- provider, operation, argument, name, and member known-value payloads;
- correction ranking, Unicode edit distance, deterministic ties, five-item bound, and repair byte
  ranges;
- default reference queries for all currently routed code families;
- syntax scope references for every recognized browser keyword;
- malformed and half-typed input remaining lossless through the shared parser.

Do not update an expected diagnostic merely because extraction changed traversal order. First treat
that difference as a compatibility regression.

### 12.4 Editor query tests

Required coverage includes:

- provider operation completion and order;
- named argument completion excluding already-present parameters;
- optional markers, labels, details, documentation, kinds, and insert text;
- record property completion from exact and nested type facts;
- half-typed receiver recovery after `value.`;
- unknown provider/operation/receiver returning no speculative results;
- smallest containing member/call selection in nested expressions;
- signature label/parameters/documentation and active parameter at opening delimiter, argument
  boundaries, commas, nested calls, and the closing delimiter;
- provider documentation range, result type, and retry-safety text;
- `type_at` smallest-range and end-offset behavior;
- provider schema replacement updating completion/signature/documentation without a source edit.

### 12.5 Cross-crate compatibility tests

Required downstream coverage includes:

- editor diagnostics combining current static and revision-matched runtime observations;
- editor format/hover/completion/signature/run gating;
- LSP conversion of every diagnostic source/severity and completion kind;
- DAP static-error gating and plan/source mapping;
- app check/build/test analysis paths and project application schemas;
- app-bridge provider repair hints;
- WASM diagnostics, plan, completion, signature, hover, and description parity;
- installed description examples parsing/analyzing through the normal pipeline.

No test should introduce a second parser or reproduce expected semantics in TypeScript.

### 12.6 Verification commands

Every delivery slice runs:

```sh
cargo fmt --all -- --check
cargo test -p webtest-analysis
cargo clippy -p webtest-analysis --all-targets -- -D warnings
```

Slices touching public DTO placement, database methods, or editor queries additionally run:

```sh
cargo test -p webtest-editor
cargo test -p webtest-lsp
cargo test -p webtest-wasm
```

The final slice runs:

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
target/debug/webtest check examples/plain-html/sign-in.webtest
d6_verify_dir="$(mktemp -d)"
target/debug/webtest build --emit "$d6_verify_dir/plan.json" examples/plain-html/sign-in.webtest
rm -rf "$d6_verify_dir"
```

Resolve and inspect `d6_verify_dir` before removal if these commands are adapted for automation; it
must be the directory returned by that `mktemp -d` invocation, never an empty, broad, or inherited
path. The checked-in workspace must remain clean. If `wasm32-unknown-unknown` is installed, also
run:

```sh
cargo check -p webtest-wasm --target wasm32-unknown-unknown
```

This refactor does not require extension changes. If no extension files change, extension compile
or packaging is not a completion gate.

## 13. Review checkpoints

Review each slice against these questions:

1. Does every moved item have one clear owner and reason to change?
2. Can existing downstream crates still import every public item from `webtest_analysis` unchanged?
3. Is there still exactly one parse → HIR → semantic compile → plan path?
4. Does one cache entry contain results from exactly one source revision and provider schema set?
5. Did any diagnostic field, ordering, candidate bound, or source range change?
6. Did any type fact, plan operation, ID, capability, provider metadata, or expression order change?
7. Do editor queries consume the same cached CST, type facts, and provider schemas as analysis?
8. Did extraction make a private module public or broaden helpers beyond their real consumers?
9. Did a vague context, visitor, trait, macro, or abstraction appear without a present need?
10. Did the work accidentally implement or constrain Milestone F instead of isolating the current
    model behind `database.rs`?
11. Are focused tests beside their owner, with cross-module compiler tests limited to true vertical
    behavior?
12. Could a contributor find provider validation, browser lowering, type rules, invalidation, or
    completion behavior from the module name alone?

Stop and correct the slice if an answer exposes semantic drift, a second implementation, broad
visibility, or a replacement monolith.

## 14. Risks and mitigations

### 14.1 Hidden public API breakage

Moving a public type into a private module can change import paths, re-exports, derives, auto traits,
or rustdoc visibility. Keep root re-exports, add downstream compile coverage early, and compare
serialized DTOs where applicable.

### 14.2 Cache incoherence

Separating database orchestration from compilation can accidentally cache a new parse beside old
facts or fail to invalidate editor results after a schema change. Construct the complete cache entry
only after all phases finish, retain the current revision check, and test source and schema changes
through multiple query orders.

### 14.3 Diagnostic order or range drift

Moving recursive methods or collecting diagnostics in intermediate vectors can reorder failures.
Changing a helper's input range can broaden underlines. Characterize exact ordered diagnostics and
narrow ranges before extraction; append at the same semantic encounter points afterward.

### 14.4 Plan identity drift

Resetting compiler state at the wrong boundary can renumber steps, leak bindings between tests, or
change required capabilities. Keep file-global and per-test fields visibly distinct in `Compiler`
and assert multiple-test exact plans.

### 14.5 Compiler module cycles and visibility explosion

Expressions and provider arguments are mutually recursive, while statement and browser compilation
both require expression inference. Use inherent methods on one parent-owned `Compiler` with
`pub(super)` collaboration. Do not give children database handles, publish compiler fields, or merge
everything back into one file to avoid privacy work.

### 14.6 Editor regression on malformed source

Completion currently uses the lossless CST to recover useful results when HIR omits a half-typed
member. A seemingly cleaner HIR-only extraction would lose this behavior. Keep the focused CST
fallback in `editor_queries` and test malformed editing states explicitly.

### 14.7 Premature Milestone F architecture

It may appear efficient to introduce Salsa, snapshots, workspace IDs, module graphs, or granular
memoization while touching database code. Doing so combines a behavioral redesign with a
decomposition and makes regressions difficult to isolate. D.6 isolates current responsibilities;
Milestone F replaces or extends them under its own performance, invalidation, and identity tests.

### 14.8 Duplicated type or provider policy

Splitting editor queries from semantic compilation can invite duplicated signature rendering, type
resolution, or provider operation lookup. Both modules must consume the same `ProviderRegistry` and
`Type` values. Editor presentation may format those facts, but it must not recreate semantic
acceptance rules.

### 14.9 Oversized replacement modules

Moving the 1,200-line compiler wholesale to `compiler.rs` would shorten `lib.rs` without improving
ownership. Enforce the child responsibilities and line-count review target, while avoiding
artificial fragmentation of tightly recursive logic.

## 15. Acceptance criteria

Milestone D.6 is complete only when:

1. `crates/analysis/src/lib.rs` contains only crate documentation, private module declarations, and
   root-level public re-exports, and normally remains below 100 lines.
2. Source/file/cache/provider invalidation, public facts, diagnostic primitives, syntax diagnostics,
   editor queries, and each semantic compiler responsibility have the explicit owners defined in
   this plan.
3. No new production module is a renamed replacement monolith, no vague helper module exists, and
   new modules normally remain below the 500-line review target.
4. Existing downstream imports from `webtest_analysis` compile unchanged; public fields, derives,
   serialized forms, error text, method signatures, and feedback/description re-exports are
   compatible.
5. File IDs, path identity, source revisions, open/update/close behavior, lazy caching, per-file
   source invalidation, and provider-schema-hash invalidation remain unchanged and are directly
   tested.
6. There is exactly one parser, one typed-AST-to-HIR path, one semantic compiler, one plan
   construction path, and one canonical formatter/description source across native and portable
   consumers.
7. Diagnostics retain exact codes, severities, sources, ordering, ranges, messages, structured
   details, bounded repair hints, replacement ranges, and reference queries.
8. Type/capability inference, binding scope, provider validation, transferability, decode behavior,
   browser/assertion lowering, and malformed-source recovery remain behaviorally identical.
9. A fixed fixture produces an exactly equal `TestPlan`, including file/revision, test/binding/step
   IDs, operation ordering, expression trees, origins, required capabilities, schema hashes,
   retry-safety, and redaction metadata, before and after the refactor.
10. Completion, signature help, documentation, and `type_at` preserve exact valid, nested, boundary,
    and half-typed behavior and update after provider-schema replacement without a source edit.
11. Installed description examples still parse/analyze, project provider descriptions use the
    active registry, and native/WASM portable results remain in parity.
12. Compiler/database/editor-query tests reside with their owners, transitional wrappers are
    removed, and implementation visibility is no broader than necessary.
13. No Salsa/workspace/module/snapshot/cancellation feature, language feature, plan change, adapter
    change, new crate, or unnecessary dependency is introduced.
14. Focused analysis/editor/LSP/WASM tests, full workspace tests, workspace Clippy, formatting,
    build, representative CLI check/build, and portable-target checking when available all pass.

The milestone is a successful refactor only if a future contributor can change one analysis
responsibility without reopening a 2,400-line root module, while every current native and portable
consumer observes the same source interpretation, diagnostics, semantic facts, and deterministic
plans.
