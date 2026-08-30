# Milestone D.5 — Application Composition-Root Decomposition

## 0. Status and dependencies

**Status: implemented (2026-08-28).**

This maintenance milestone follows the implemented application bridge in
[`milestone-d.md`](./milestone-d.md) and precedes feature work that would add more orchestration to
the native executable. It does not add a language, runtime, provider, browser, editor, LSP, DAP,
WASM, or CLI capability. Its purpose is to decompose `crates/app/src/main.rs` before additional
features make the composition root harder to understand, verify, and change safely.

The current file is approximately 2,679 lines and owns or directly contains all of the following:

- Clap declarations and reporter selection;
- process startup, tracing, error display, and exit-code mapping;
- dispatch for every native command;
- project discovery and source loading;
- analysis-database and provider-registry composition;
- application-provider configuration and lifecycle;
- runner-option construction;
- Chrome resolution and browser-manager commands;
- `check`, `fmt`, `build`, `test`, `inspect`, and `describe` orchestration;
- LSP multi-project editor caching, reconfiguration, invalidation, and provider shutdown;
- DAP composition;
- emitted-plan literal-secret rejection;
- source-span and machine-source conversion;
- static diagnostic, runtime failure, semantic detail, and event-report conversion;
- path, revision, and duration presentation helpers;
- unit tests for several of those behaviors.

All of those responsibilities belong to the native `app` crate, but they do not belong in one Rust
module. The refactor must preserve `app` as the sole native composition root while giving each
responsibility a narrow internal owner.

## 1. Outcome

`crates/app/src/main.rs` becomes a small executable shell that does only four things:

1. declare the crate's internal modules;
2. initialize stderr tracing;
3. parse `Cli` and preserve Clap's help/error exit behavior;
4. call the command dispatcher and translate `ExitClass` or `AppError` into `ExitCode`.

The intended top-level flow is:

```text
main.rs
  |
  +-- cli.rs ----------------------- argument and reporter enums
  +-- error.rs --------------------- application error classification
  +-- commands/mod.rs -------------- thin command dispatch
        |
        +-- command modules -------- command-specific orchestration
              |
              +-- shared app modules
                    project / analysis / configuration / providers
                    chrome / plan security / source reporting
                    runtime reporting / LSP project cache
              |
              +-- existing reusable crates
                    analysis / project / runtime / browser / editor / LSP / DAP
```

The final `main.rs` should normally remain below 100 lines. This is a design target, not permission
to hide the same logic behind macros or a single oversized `run` function in another file.

Every command must continue to use the shared Rust implementations owned by the existing crates.
This milestone reorganizes native adapter code; it does not introduce another parser, formatter,
semantic model, plan representation, provider contract, browser backend, reporter schema, or editor
service.

## 2. Architectural rules

### 2.1 Preserve the crate boundary

The `app` crate remains the composition root and sole native executable. Internal modules may depend
on any crate that `app` is already authorized to compose, but this refactor must not create a
production dependency from a reusable crate back into `app`.

No code is moved into another crate merely to shorten files. A helper moves across a crate boundary
only if a separate reviewed change proves that the helper is protocol-neutral reusable behavior and
belongs to that crate's documented ownership. Cross-crate movement is not part of this milestone.

### 2.2 Organize by responsibility, not by call order

Modules should own cohesive policies:

- command modules own sequencing and user-facing command behavior;
- provider composition owns construction of analysis and runtime provider registries;
- runtime configuration owns translation from project configuration to reusable runtime DTOs;
- browser resolution owns Chrome precedence and path validation;
- diagnostic modules own conversion into app-level report DTOs;
- the LSP project cache owns cache mutation and provider replacement lifecycle;
- plan security owns the build-time literal-secret traversal.

Do not produce files named `helpers.rs`, `utils.rs`, or `common.rs`. A function that cannot be placed
in a specifically named module is a signal that its responsibility is not yet understood.

### 2.3 Keep dependency direction visible

The desired internal dependency direction is:

```text
main
  -> cli, error, commands

commands
  -> project_context, project_analysis, runtime_configuration
  -> provider_composition, chrome, plan_security
  -> diagnostic_output, runtime_output, report, init

project_analysis
  -> provider_composition, diagnostic_output

diagnostic_output
  -> source_output, report

runtime_output
  -> source_output, report

lsp_projects
  -> project_context, runtime_configuration, provider_composition
```

The following internal edges are forbidden:

- `cli` depending on command implementations;
- shared composition modules depending on `commands`;
- source or diagnostic conversion depending on Clap;
- `report` or report conversion starting Chrome, providers, LSP, DAP, or execution;
- `provider_composition` printing to stdout or selecting a reporter;
- command modules reaching into another command module to reuse behavior;
- a universal context object containing optional state for every command;
- new global mutable state.

Shared behavior needed by two commands must move to a narrowly named shared module rather than one
command calling another. For example, `build` must consume a shared project-analysis operation; it
must not invoke the `check` command adapter as though it were a library.

### 2.4 Preserve structured errors

`AppError` continues to carry an `ExitClass` and message. Usage, infrastructure, and internal
failures must remain distinct. Command modules may add narrow error context, but they must not erase
typed errors below the presentation boundary or convert all failures to strings earlier than the
current code does.

### 2.5 Preserve stdout ownership

- LSP and DAP stdout remains reserved for framed protocol messages.
- Tracing and command-level fatal errors remain on stderr.
- Machine reporters remain valid JSON, JSONL events, or JUnit without incidental text.
- Human-only progress messages remain confined to commands that already print them.

No extracted helper may print as a side effect unless presentation is its explicit responsibility.
Where practical, human renderers should accept `&mut dyn Write` so output can be characterized
without redirecting process-global stdout.

## 3. Scope

This milestone includes:

- characterization tests for the current CLI and composition behavior;
- an explicit internal module layout for the `app` crate;
- extraction of Clap declarations and reporter conversions;
- extraction of application error classification;
- extraction of project/source, analysis, provider, runtime-option, and Chrome composition;
- one module per substantial command path;
- extraction of LSP project caching and provider replacement;
- extraction of plan-secret validation;
- extraction of diagnostic, failure, semantic-detail, and event conversion;
- movement of existing tests next to their new owners;
- removal of obsolete imports and unnecessarily broad visibility;
- final workspace formatting, tests, Clippy, and representative CLI verification.

Refactoring existing `init.rs` or `report.rs` is allowed only where needed to give a moved function a
coherent owner. Neither file should become the replacement monolith.

## 4. Non-goals

This milestone does not:

- add, remove, rename, or reorder commands, flags, arguments, help text, defaults, or conflicts;
- change exit-code meanings or error-class precedence;
- change human, concise, JSON, JSONL event, or JUnit schemas or wording intentionally;
- change project discovery or file ordering;
- change parser, analysis, formatter, plan, runtime, browser, provider, editor, LSP, or DAP semantics;
- optimize analysis by combining databases that are currently separate;
- add parallel file execution, retries, cancellation, actionability, or traces;
- change application-provider start/shutdown frequency or ownership;
- change Chrome resolution precedence or fallbacks;
- change evidence, inspection, redaction, or secret-detection policy;
- change the plan format, test/step ID assignment, or deterministic ordering;
- introduce dependency injection frameworks, dynamic dispatch registries, plugins, or a generic
  service locator;
- split the `app` crate into additional crates;
- update archived milestone behavior unrelated to this maintenance plan.

Any desirable behavior change discovered during extraction must be documented separately and made
after this milestone, unless it fixes a regression introduced by the refactor itself.

## 5. Compatibility contract

The refactor is complete only if the observable behavior below remains compatible.

### 5.1 CLI surface

Preserve:

- the executable name, version, and about text;
- all ten top-level commands;
- all four `browser` subcommands;
- positional path behavior, including empty path lists;
- `init` defaulting to `.`;
- `describe` query/search conflicts and optional project path;
- `test` and `inspect` headed behavior;
- DAP being headed by default and accepting `--headless`;
- reporter value names and defaults;
- Clap's code `0` help/version behavior and code `2` parse-error behavior.

Capture the root help plus each subcommand's help before extraction. Normalize only the executable
path or platform-specific line endings in snapshots.

### 5.2 Project and configuration behavior

Preserve:

- nearest-root discovery and path-oriented file selection;
- unknown-configuration warnings;
- configured schema-path resolution relative to the project root;
- normalized display paths;
- project-specific description limits;
- configuration revision and project identity calculation;
- strict eviction of a stale app schema after an invalid LSP reload.

### 5.3 Static analysis and formatting

Preserve:

- built-in and project app schemas as explicit analysis inputs;
- per-file diagnostic source revision and precise range;
- `config.missing_base_url` and `config.timeout_exceeds_test` conditions and details;
- warning versus error exit behavior;
- lossless parser use by the canonical formatter;
- `fmt --check` being non-mutating and returning test-failure status on a difference;
- no formatter output for an already formatted file.

### 5.4 Build behavior

Preserve:

- check-before-emit behavior;
- no output plan after a failed check;
- source-file ordering;
- global deterministic reassignment of test and step IDs;
- sorted required host capabilities;
- compiler and plan format versions;
- provider schema hashes;
- project identity construction;
- pretty JSON encoding and destination-parent creation;
- literal-secret rejection, including binding traversal and configured HTTP JSON/header fields.

For a fixed fixture and compiler version, the emitted plan must remain byte-identical before and
after the refactor.

### 5.5 Test behavior

Preserve:

- analysis before execution and execution suppression after static errors;
- a fresh observation store per file;
- runtime provider registry construction once per command;
- application-provider startup before a file run and shutdown after the command;
- lazy Chrome resolution only for plans requiring browser capability;
- configured and CLI headed behavior;
- whole-file timeout classification;
- the distinction between test failure and infrastructure failure;
- continued processing of later files where the current implementation continues;
- teardown warnings and their influence on the aggregate exit class;
- event order and redacted provider event fields.

### 5.6 Inspect and describe behavior

Preserve:

- inspect URL fallback and relative URL resolution;
- isolated browser session/context/page lifecycle and cleanup ordering;
- configured viewport, test-ID attribute, bounds, hidden-element policy, and redaction;
- human inspection columns and truncation notice;
- built-in-only description fallback when project discovery fails;
- configured app operations in project-aware description;
- every human description response variant;
- secret-like resolved-command argument redaction;
- usage failure for a description diagnostic.

### 5.7 Chrome, LSP, and DAP behavior

Preserve Chrome precedence exactly:

1. command-line path;
2. `WEBTEST_CHROME_PATH`;
3. configured `browser.path`;
4. current managed Chrome when the managed channel is selected;
5. system discovery.

Also preserve:

- strict failure for an invalid explicitly selected path;
- browser-manager error classification;
- LSP startup without requiring Chrome to resolve;
- one editor service per project root;
- existing editor-service reconfiguration rather than replacement;
- best-effort shutdown of replaced LSP app providers;
- shutdown of all cached providers when LSP exits;
- DAP startup without requiring immediate Chrome resolution;
- DAP's use of the shared runner and provider configuration;
- DAP app-provider shutdown after serving.

### 5.8 Machine feedback

Preserve:

- diagnostic and repair-hint schema versions;
- UTF-8 byte ranges and one-based human positions;
- Unicode-safe character-column calculation;
- stable `runtime.*` error codes;
- assertion diffs, artifacts, repair hints, page summaries, and secondary failures;
- browser, provider, assertion, decode, and evaluation semantic details;
- bridge-specific schema/handshake/protocol details and reference queries;
- all execution event kinds and their IDs;
- path separator normalization, revision hex encoding, and saturating nanosecond conversion.

## 6. Target module layout

The target layout is:

```text
crates/app/src/
├── main.rs
├── cli.rs
├── error.rs
├── commands/
│   ├── mod.rs
│   ├── check.rs
│   ├── format.rs
│   ├── build.rs
│   ├── test.rs
│   ├── inspect.rs
│   ├── describe.rs
│   ├── browser.rs
│   ├── lsp.rs
│   └── dap.rs
├── project_context.rs
├── project_analysis.rs
├── runtime_configuration.rs
├── provider_composition.rs
├── chrome.rs
├── plan_security.rs
├── diagnostic_output.rs
├── runtime_output.rs
├── source_output.rs
├── lsp_projects.rs
├── init.rs
└── report.rs
```

This layout is prescriptive about responsibilities, not necessarily every private function name.
Files may be split further when a cohesive responsibility remains too large, but they may not be
collapsed into a new omnibus `application.rs` or `commands.rs`.

### 6.1 `main.rs`

Owns only:

- module declarations;
- tracing initialization;
- `Cli::try_parse` handling;
- the call to `commands::run`;
- final stderr error display and `ExitCode` conversion.

It must not import analysis, project, provider, browser, runtime, editor, LSP, DAP, plan, observation,
or feedback types directly.

### 6.2 `cli.rs`

Owns:

- `Cli`;
- `Command`;
- `BrowserCommand`;
- `CheckReporter`, `TestReporter`, and `ReferenceReporter`;
- conversion from CLI reporter enums to the internal `Reporter`.

The declarations should initially move without reshaping them. Converting variants to nested
`Args` structs is allowed only later and only if help snapshots prove exact compatibility.

### 6.3 `error.rs`

Owns `AppError`, its constructors, and `Display`. Visibility remains `pub(crate)` or narrower. The
module must not acquire command-specific enums or become a string-only replacement for typed lower
layer errors.

### 6.4 `commands/mod.rs`

Owns the exhaustive `Command` match and delegates each branch to one command module. It may perform
small branch-local setup, but substantial orchestration belongs in the leaf module.

The dispatcher must remain easy to audit: every CLI variant appears once, and adding a future
variant produces an exhaustive-match compiler error.

### 6.5 Command modules

Each command module owns its command-level sequence and presentation decisions:

- `check.rs`: collect the shared analysis result, write the requested report, return its exit class;
- `format.rs`: compare/rewrite files and print configuration/formatting messages;
- `build.rs`: gate on shared analysis, merge plans, assign global IDs, validate plan security, emit;
- `test.rs`: coordinate analysis, providers, lazy Chrome, runner timeout, and aggregate reporting;
- `inspect.rs`: coordinate URL resolution, browser lifecycle, inspection, and human/JSON output;
- `describe.rs`: resolve optional project context, construct description request/limits, and render
  human/JSON responses;
- `browser.rs`: implement install/list/path/clean presentation over `BrowserManager` and `chrome`;
- `lsp.rs`: assemble the host, initial editor cache, callbacks, service, and final shutdown;
- `dap.rs`: assemble project, host, runner options, providers, service, and final shutdown.

The existing initializer implementation remains in `init.rs`; its current `init_project` wrapper
moves there as the initializer's command-facing entry point. This avoids a second file named
`commands/init.rs` whose only purpose would be forwarding.

### 6.6 `project_context.rs`

Owns native filesystem-facing project conveniences:

- project discovery wrapper and usage-error mapping;
- source-file reads and their path-rich errors;
- display-path normalization;
- platform path normalization;
- source revision hex rendering;
- duration-to-nanosecond saturation if it remains shared by multiple commands.

It must not create analysis databases, providers, browsers, or runners.

### 6.7 `project_analysis.rs`

Owns reusable analysis orchestration shared by `check`, `build`, and `test`:

- construction of an analysis database for a project;
- opening one source in a database;
- collection of syntax/semantic diagnostics and the `TestPlan`;
- addition of configuration-sensitive diagnostics;
- a narrow owned result such as `AnalyzedFile` containing source, revision, plan, and reports.

The extraction should remove the duplicated read/database/diagnostic/plan/config-diagnostic sequence
from `check_project` and `test_project` without altering database lifetime or invalidation semantics.
The first implementation may retain separate databases per file where the current commands do so.
Combining or caching those databases is a later performance change.

`build` may still use a fresh shared database for its existing cross-file envelope assembly. That
choice is observable through IDs, provider hashes, and deterministic output and must not be changed
incidentally.

### 6.8 `runtime_configuration.rs`

Owns pure translation from `Project` to runtime-facing values:

- `RunnerOptions`;
- `ResolvedRuntimeConfiguration` used by `describe`;
- browser context and inspection options;
- evidence and artifact settings;
- native HTTP/process/filesystem provider configuration;
- secret-like command-argument redaction.

This module must not start providers, create Chrome, read source files, print output, or execute a
plan. Pure translation functions should receive `&Project` and return owned DTOs.

### 6.9 `provider_composition.rs`

Owns:

- normalized application-manifest loading;
- built-in plus app schema registration for analysis;
- executable built-in plus `AppProvider` registration for runtime;
- translation to `AppProviderConfig`;
- adapter and transport mapping;
- application process, health-check, compatibility HTTP, and limit mapping.

Return a named result type for runtime composition, for example:

```rust,ignore
pub(crate) struct RuntimeProviders {
    pub(crate) registry: ProviderRegistry,
    pub(crate) app: Option<Arc<AppProvider>>,
}
```

This makes ownership and shutdown responsibility explicit without returning an unexplained tuple.
The struct must not automatically start or stop the app provider; the command or LSP cache that owns
the lifecycle remains responsible for doing so.

### 6.10 `chrome.rs`

Owns:

- `ChromeProvenance`;
- `ResolvedChrome`;
- resolution precedence;
- configured relative-path resolution;
- selected-path existence checks and canonicalization;
- `BrowserManagerError` classification shared with the browser command.

Separate precedence selection from environmental discovery where practical. A pure candidate-
selection function can be exhaustively unit tested, while the production wrapper reads the
environment, managed installation, and system discovery. Do not introduce a general dependency
injection framework for this seam.

### 6.11 `plan_security.rs`

Owns all build-time traversal that rejects literal secrets:

- provider redacted-argument checks;
- configured HTTP JSON/header sensitive-field checks;
- pure-binding tracking;
- recursive traversal of every `PlanExpr` variant;
- cycle protection;
- construction of the focused usage error.

Tests must cover direct and binding-mediated literals, nested records/lists, unary/binary/member/
decode expressions, configured case-insensitive field names, nonliteral types, and repeated binding
references. Extraction must not broaden or narrow the security rule accidentally.

### 6.12 `source_output.rs`

Owns conversion from internal byte ranges to app presentation coordinates:

- `source_span`;
- `machine_source`;
- line/column calculation;
- source-line extraction and underline width;
- safe flooring to a UTF-8 character boundary.

It depends on report DTOs and text primitives only. It must remain ignorant of analysis and runtime
error enums.

### 6.13 `diagnostic_output.rs`

Owns:

- conversion from analysis `Diagnostic` to `DiagnosticReport`;
- configuration-sensitive plan diagnostics;
- literal URL extraction and syntactic absolute-URL recognition.

It depends on `source_output` for coordinates and must not read project files or create analysis
databases.

### 6.14 `runtime_output.rs`

Owns all runtime-to-report conversion:

- stable browser, run, and step error codes and messages;
- `StepFailure` to `FailureReport` conversion;
- assertion diffs and evidence artifact paths;
- browser/provider/assertion/decode/evaluation semantic details;
- bridge-specific diagnostic reference queries;
- extraction and description of browser-error locators;
- `RuntimeFailure` conversion;
- `ExecutionEvent` to `EventReport` conversion.

Keep this separate from `report.rs`: `runtime_output` understands runtime/domain errors, while
`report.rs` owns generic report DTOs and reporter serialization/rendering.

### 6.15 `lsp_projects.rs`

Owns `LspProjectEditor`, `LspProjectEditors`, and their complete lifecycle:

- editor lookup by document path and project root;
- fallback to built-in providers after initial app-provider load failure;
- tracked configuration and app-manifest paths;
- canonical event-path handling;
- project reload and editor reconfiguration;
- stale provider invalidation after a failed reload;
- asynchronous shutdown of replaced providers;
- final shutdown of all cached providers;
- poison-tolerant mutex access.

The LSP command owns service assembly; this module owns cache semantics. Callbacks passed to the LSP
adapter should be one-line delegations into this type.

## 7. Current symbol migration map

Every substantial symbol currently in `main.rs` has an intended destination:

| Current symbol or group | Destination |
|---|---|
| `Cli`, `Command`, `BrowserCommand`, reporter enums/conversions | `cli.rs` |
| `AppError` and constructors | `error.rs` |
| `main` | `main.rs` |
| `run` | `commands/mod.rs` |
| `init_project` | `init.rs` |
| `project`, `read_source`, display/normalized path, revision, duration helpers | `project_context.rs` |
| `LspProjectEditor`, `LspProjectEditors`, schema/event path and replacement shutdown | `lsp_projects.rs` |
| `inspect_page` | `commands/inspect.rs` |
| `describe_reference`, `write_description_human` | `commands/describe.rs` |
| resolved runtime description and argument redaction | `runtime_configuration.rs` |
| `check_project` | `commands/check.rs`, using `project_analysis.rs` |
| `format_project` | `commands/format.rs` |
| `build_project` | `commands/build.rs` |
| literal-secret validation and recursive expression traversal | `plan_security.rs` |
| `test_project` | `commands/test.rs` |
| `base_report`, `write_report` | `report.rs` or a narrow report-facing API |
| `runner_options` | `runtime_configuration.rs` |
| Chrome types and resolution | `chrome.rs` |
| `browser_command` | `commands/browser.rs` |
| browser-manager error classification | `chrome.rs` |
| analysis database construction/opening | `project_analysis.rs` |
| app manifest and provider registry/configuration | `provider_composition.rs` |
| analysis/config diagnostic conversion | `diagnostic_output.rs` |
| source span and machine source conversion | `source_output.rs` |
| runtime code/message/failure/semantic/event conversion | `runtime_output.rs` |
| unit tests | the module that owns the tested policy |

If implementation reveals a symbol with two unrelated reasons to change, split it before moving it.
Do not keep a compatibility re-export from `main.rs`; internal callers should use the new owner.

## 8. Command interface design

Command entry points should use narrow inputs. Representative shapes are:

```rust,ignore
pub(crate) fn run_check(paths: Vec<PathBuf>, reporter: CheckReporter)
    -> Result<ExitClass, AppError>;

pub(crate) async fn run_test(
    paths: Vec<PathBuf>,
    chrome_path: Option<PathBuf>,
    headed: bool,
    reporter: TestReporter,
) -> Result<ExitClass, AppError>;
```

The exact names may differ, but avoid a generic `CommandContext` containing optional project,
browser, providers, writers, and runtime state. Most commands need only a subset, and the type system
should make that subset apparent.

Where a command has a naturally coherent parsed argument group, it may accept a command-specific
input struct defined in `cli.rs`. Those structs remain CLI DTOs and must not leak into reusable
crates.

Long-lived resources require explicit owners:

- `commands/test.rs` owns the runtime app provider for the duration of the command;
- `lsp_projects.rs` owns cached LSP app providers;
- `commands/dap.rs` owns the DAP app provider until serving completes;
- `commands/inspect.rs` owns its browser session/context/page lifecycle;
- runtime `Runner` continues to own execution behavior, not the CLI module.

## 9. Delivery slices

Each slice must compile, format, and pass the focused `webtest` binary tests before the next slice
starts. Prefer one focused commit per slice.

### Slice 1 — Characterize the current boundary

Before moving code:

1. Record root and subcommand help output.
2. Add missing parser tests for every command, reporter default, conflict, and headed/headless mode.
3. Add focused tests for exit-class translation and stderr/stdout expectations where not already
   covered by `report.rs`.
4. Add golden fixtures for representative `check` JSON, `build` plan, and `describe` JSON output.
5. Add or retain unit tests for Chrome precedence, plan-secret traversal, Unicode spans, resolved
   argument redaction, bridge semantic details, and LSP reload invalidation.

Characterization tests should assert intended contracts, not incidental allocation choices or
private function names.

### Slice 2 — Extract leaf types and pure presentation primitives

1. Move `AppError` to `error.rs`.
2. Move CLI declarations and reporter conversions to `cli.rs` without reshaping them.
3. Move source-coordinate conversion to `source_output.rs`.
4. Move path/revision/duration helpers to `project_context.rs`.
5. Move tests with their owners.

At the end of this slice, `main.rs` still dispatches commands but no longer owns generic types or
source-coordinate algorithms.

### Slice 3 — Extract configuration and provider composition

1. Move `runner_options` and resolved description configuration to
   `runtime_configuration.rs`.
2. Move argument redaction and its tests with that configuration.
3. Move manifest loading, analysis/runtime registry construction, and `AppProviderConfig`
   translation to `provider_composition.rs`.
4. Replace the runtime provider tuple with a named ownership type.
5. Verify stopped-app analysis and runtime app lifecycle behavior.

Do not merge analysis schemas and executable provider registries; they serve different boundaries.

### Slice 4 — Extract Chrome resolution

1. Move provenance, result, validation, precedence, and browser-manager error mapping to `chrome.rs`.
2. Add table-driven precedence tests, including invalid higher-priority paths.
3. Keep environment access in a small production wrapper.
4. Move only browser subcommand orchestration to `commands/browser.rs`.

Verify that test/inspect remain strict while LSP/DAP continue best-effort startup behavior.

### Slice 5 — Extract static and runtime output conversion

1. Move analysis diagnostic conversion and configuration diagnostics to `diagnostic_output.rs`.
2. Move runtime code/message/failure/semantic/event conversion to `runtime_output.rs`.
3. Keep generic serialization and renderer behavior in `report.rs`.
4. Add exhaustive tests that fail to compile or fail assertions when new relevant error/event
   variants are not mapped.

Avoid wildcard match arms for closed error and event enums where exhaustive matching provides a
useful maintenance signal.

### Slice 6 — Extract plan security and shared project analysis

1. Move the complete literal-secret traversal to `plan_security.rs`.
2. Add the missing recursive and binding-mediated security cases.
3. Introduce the narrow `AnalyzedFile` result in `project_analysis.rs`.
4. Make check and test consume the same analysis/config-diagnostic collection path.
5. Preserve build's existing shared-database envelope pass.

This slice should reduce duplication without changing when files are read, revisions are computed,
or errors stop execution.

### Slice 7 — Extract non-protocol command modules

Move command orchestration in increasing risk order:

1. formatter;
2. checker;
3. browser manager;
4. descriptor;
5. inspector;
6. builder;
7. tester.

After each move, the dispatcher delegates directly to the new module and no compatibility wrapper
remains in `main.rs`.

### Slice 8 — Extract LSP and DAP composition

1. Move the multi-project editor cache to `lsp_projects.rs`.
2. Move LSP host/service assembly to `commands/lsp.rs`.
3. Move DAP host/provider/service assembly to `commands/dap.rs`.
4. Add focused lifecycle tests for replaced, invalidated, and final provider shutdown.
5. Verify no stdout logging is introduced in either protocol mode.

Do this after provider and Chrome composition are stable because LSP/DAP use both with deliberately
different failure timing.

### Slice 9 — Finalize the shell and remove accidental coupling

1. Move the exhaustive match to `commands/mod.rs`.
2. Reduce `main.rs` to startup, parsing, delegation, and exit mapping.
3. Remove unused imports and narrow every new symbol to the minimum visibility.
4. Inspect the internal module graph for forbidden reverse dependencies.
5. Ensure no extracted file has become a second composition-root monolith.
6. Update this specification's status only after all acceptance criteria pass.

## 10. Testing requirements

### 10.1 Focused unit tests

Required unit coverage includes:

- every CLI variant and reporter conversion;
- help/version versus parse-error exit behavior;
- application error-class construction;
- project path normalization on platform-appropriate fixtures;
- Unicode, multiline, zero-width, out-of-bounds, and non-boundary source spans;
- absolute versus relative URL recognition;
- both configuration-sensitive diagnostics and their exact source ranges/details;
- runtime option translation for every mapped project setting;
- secret-like command argument redaction;
- application adapter and transport mapping;
- Chrome precedence, configured relative paths, invalid selections, managed absence, and system
  fallback;
- every literal-secret expression form and binding path;
- every browser/run/step/runtime failure mapping;
- bridge semantic details and reference queries;
- every execution event variant;
- LSP path matching, reconfiguration, invalidation, and shutdown.

### 10.2 Command characterization tests

Use temporary projects and deterministic fixtures to prove:

- `check` human/concise/JSON output and exit class are unchanged;
- `fmt` rewrites only changed files and `fmt --check` never writes;
- `build` output is byte-identical and refuses representative secret plans;
- `describe` index/query/search and project-provider output are unchanged;
- `inspect` human formatting remains stable using protocol-neutral fake inspection data where
  possible;
- `test` classifies static, test, timeout, provider-start, browser-resolution, and teardown outcomes
  identically using fake lower-layer hosts/providers where available;
- browser list/path presentation remains unchanged after normalizing platform paths;
- LSP/DAP setup retains headed/headless and provider configuration behavior.

Tests must not depend on fixed ports. Real-Chrome coverage remains in the existing backend and
end-to-end suites; this refactor should prefer fake browser traits for app-level orchestration.

### 10.3 Machine-output compatibility

Golden or structural assertions must cover:

- report schema version;
- diagnostic and repair-hint schema versions;
- machine source revision/range/position fields;
- plan envelope fields and deterministic IDs;
- JSONL command, warning, diagnostic, runtime event, infrastructure, and completion records;
- JUnit distinction between static failures, test failures, and infrastructure errors;
- XML escaping;
- omission of runtime events from aggregate JSON where currently specified.

Do not update a golden merely because extraction changed output. First determine whether the change
violates this compatibility contract.

### 10.4 Verification commands

Every delivery slice runs:

```sh
cargo fmt --all -- --check
cargo test -p webtest --bin webtest
cargo clippy -p webtest --all-targets -- -D warnings
```

The final slice additionally runs:

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
target/debug/webtest check examples/plain-html/sign-in.webtest
```

If `wasm32-unknown-unknown` is installed, also run:

```sh
cargo check -p webtest-wasm --target wasm32-unknown-unknown
```

The refactor does not require extension changes. If no extension files change, extension packaging is
not a completion gate for this milestone.

## 11. Review checkpoints

Review each slice against these questions:

1. Does the moved code have one clear reason to change?
2. Is its dependency direction visible from imports?
3. Did output, ordering, error classification, cleanup, or fallback behavior change?
4. Did a command start depending on another command rather than shared policy?
5. Did a generic context, trait, or abstraction appear without at least two concrete consumers?
6. Are long-lived Chrome/provider/editor resources still owned and shut down by an explicit scope?
7. Are tests next to the policy they protect?
8. Could a future contributor locate this behavior from the module name without searching the whole
   crate?

Stop and correct the slice if the answer exposes a new monolith or hidden lifecycle ownership.

## 12. Risks and mitigations

### 12.1 Output drift

Moving print and serialization paths can change whitespace, ordering, or stdout/stderr selection.
Characterize output first, keep renderers intact during moves, and compare machine output
structurally plus byte-for-byte where it is a stable contract.

### 12.2 Lifecycle regressions

Provider and browser resources currently have command-specific cleanup paths. Extraction can cause
early returns to skip shutdown or can move ownership into a short-lived helper. Use explicit owner
types, retain cleanup order, and test startup failure, primary failure, cleanup failure, and normal
completion separately.

### 12.3 Error reclassification

`map_err` placement determines whether a failure is usage, test, infrastructure, or internal.
Preserve mappings while moving code and assert exit classes at command boundaries.

### 12.4 Accidental semantic optimization

It may appear attractive to reuse one analysis database for every command/file or start providers
once in a different scope. Such changes can affect revisions, invalidation, IDs, lifecycle, and
diagnostics. Retain the current sequence during this milestone; optimize only in a separately
specified change.

### 12.5 Module cycles and broad visibility

Naive extraction can create cycles between commands, reporting, and composition. Follow the target
dependency direction, use owned intermediate DTOs at boundaries, and prefer `pub(crate)` over `pub`.
Do not resolve cycles by merging modules back into an omnibus file.

### 12.6 Test interference through process globals

Chrome resolution reads an environment variable, and command output uses process-global streams.
Isolate pure precedence logic, serialize any unavoidable environment-mutating tests, restore prior
values, and prefer injected writers for human renderers. Never make parallel tests depend on a
shared mutable environment value without synchronization.

## 13. Acceptance criteria

Milestone D.5 is complete only when:

1. `crates/app/src/main.rs` contains only module declarations, tracing setup, CLI parsing, dispatcher
   invocation, error display, and exit-code conversion, and normally remains below 100 lines.
2. Every current command is implemented in a clearly named command module or the existing focused
   `init.rs`, and `commands/mod.rs` is only an exhaustive dispatcher.
3. Project analysis, runtime configuration, provider composition, Chrome resolution, plan security,
   source conversion, diagnostic conversion, runtime conversion, and LSP project caching each have
   one explicit internal owner.
4. No replacement monolith, generic service locator, second parser/formatter/runtime, new crate, or
   reverse dependency into `app` is introduced.
5. Root and subcommand help, reporter defaults, command output schemas, error classes, exit codes,
   project discovery, file ordering, and stdout/stderr discipline remain compatible.
6. A fixed successful build fixture produces a byte-identical plan envelope before and after the
   refactor, and all existing secret-plan rejection cases still fail before emission.
7. Static diagnostics retain source revisions, byte ranges, Unicode-correct positions, semantic
   details, repair hints, and reference queries.
8. Runtime failures and events retain stable codes, ordering, evidence, diffs, semantic details,
   redaction, and schema versions.
9. Chrome resolution precedence, lazy browser creation, app-provider lifecycle, LSP stale-schema
   invalidation, and DAP headed-by-default behavior are covered by focused tests and unchanged.
10. Tests reside with their owning modules, new APIs have minimum necessary visibility, and no dead
    compatibility wrappers remain in `main.rs`.
11. The focused app tests, full workspace tests, workspace Clippy, formatting check, build, and
    representative CLI static check all pass.

The milestone is a successful refactor only if future command work can be placed by responsibility
without reopening a multi-thousand-line `main.rs`, while the executable remains behaviorally the
same native composition root.
