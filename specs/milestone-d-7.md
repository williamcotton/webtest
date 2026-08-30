# Milestone D.7 — Runtime Execution-Layer Decomposition

## 0. Status and dependencies

**Status: proposed (2026-08-30).**

This maintenance milestone follows the implemented application bridge in
[`milestone-d.md`](./milestone-d.md), the application composition-root decomposition in
[`milestone-d-5.md`](./milestone-d-5.md), and the implemented analysis decomposition in
[`milestone-d-6.md`](./milestone-d-6.md). It prepares the current sequential runtime for the
structured execution work specified in [`milestone-e.md`](./milestone-e.md), but it does not
implement any Milestone E syntax, plan node, scheduler, cancellation, retry, timeout, trace, or
concurrency capability.

The current `crates/runtime/src/lib.rs` is 2,667 lines. It combines:

- the crate's complete public facade, error model, result DTOs, options, and debugger control hook;
- run, browser-session, test, context, page, step, and cleanup orchestration;
- per-test binding, name, secret, and redacted-field state;
- pure expression evaluation, typed JSON decoding, value comparison, assertions, and bounded diffs;
- provider argument evaluation, dispatch, call context, result binding, event summaries, and secret
  collection;
- plan-to-browser operation, locator, state, timeout, and URL conversion;
- failure classification, redaction, evidence capture, semantic inspection, repair hints, artifacts,
  observations, and execution events;
- bounded debugger snapshots and transferable final-binding projection;
- eight unit tests spanning several of those responsibilities.

Those responsibilities belong in `runtime`, but they do not belong in one Rust module. The goal is
to make the existing sequential behavior easier to characterize, review, and extend without moving
runtime policy into the app, editor, DAP, provider, browser, plan, or observation crates.

At the time this plan was drafted:

- `cargo test -p webtest-runtime` passed all 8 runtime tests and its doc tests;
- `cargo clippy -p webtest-runtime --all-targets -- -D warnings` passed;
- `cargo test -p webtest-editor -p webtest-dap -p webtest` passed the focused editor, debugger,
  application, CLI, and protocol suites.

These results establish the starting baseline. They are not sufficient completion evidence. The
current runtime tests leave important lifecycle, event-ordering, provider, browser-operation,
redaction, artifact, cleanup, and `RunControl` behaviors implicit in downstream code.

## 1. Outcome

`crates/runtime/src/lib.rs` becomes a small, stable public facade. It declares private modules and
re-exports the same root-level API used today. It contains no runner loop, operation dispatch,
expression evaluator, redaction traversal, evidence capture, observation conversion, artifact I/O,
or mixed test suite.

The intended top-level flow remains sequential:

```text
TestPlan + BrowserHost + ProviderRegistry + RunnerOptions
                         |
                         v
runner.rs
  clear prior observations
  allocate execution identity and run events
  conditionally start one browser session
  execute planned tests in source order
                         |
                         v
execution.rs
  create one context/page and one TestExecutionState per test
  invoke RunControl at the existing boundaries
  execute planned steps in order until failure/cancellation/end
                         |
       +-----------------+-----------------+
       |                 |                 |
       v                 v                 v
  operations/       evaluation.rs     failure.rs
  provider/browser  assertions.rs     redaction/evidence/
  and dispatch      typed decode      repair/observation
       |                 |                 |
       +-----------------+-----------------+
                         |
                         v
        TestResult + ordered ExecutionEvent values
```

There remains exactly one normal execution route:

```text
Runner::run
  -> Runner::run_with_control
      -> execute one test
          -> execute one planned step
              -> evaluate / provider / browser / assertion
          -> enrich one failure through the shared failure path
          -> finalize bindings and owned temporary resources
```

The final `lib.rs` should normally remain below 100 lines. This is a design target, not permission
to hide the current file behind a macro, `include!`, generated source, or one replacement module of
similar size. New production modules should normally remain below 500 lines; a larger module needs
a cohesive reason documented in review.

The public crate path remains `webtest_runtime::*`. Downstream crates must not change imports merely
because internal ownership improved.

## 2. Research baseline

### 2.1 Current responsibility map

The current file divides approximately as follows:

| Lines | Current responsibility |
|---|---|
| `1–32` | crate documentation and all imports |
| `33–147` | artifact and step-failure/error DTOs plus display and classification |
| `148–194` | run error and test/run result DTOs |
| `195–250` | evidence and runner options/defaults |
| `251–291` | `Runner` storage and public `RunControl` contract |
| `292–369` | run lifecycle, browser-session selection, test loop, and restart policy |
| `370–681` | per-test state, debugger hooks, events, failure enrichment, context cleanup, final bindings, and temporary-resource cleanup |
| `682–758` | step dispatch and provider invocation |
| `759–838` | revision-bound observation construction |
| `839–918` | event failure conversion and failure-specific repair-hint selection |
| `919–1119` | step/browser/locator/text/URL redaction |
| `1120–1242` | browser-operation execution |
| `1243–1318` | locator, URL, value, and typed-match assertion execution |
| `1319–1558` | expression evaluation, numeric operations, and typed decode |
| `1559–1747` | assertion matching, messages, bounded diffs, comparison, and display |
| `1748–1881` | transferable binding selection and bounded debugger snapshots |
| `1882–2014` | provider secret collection, debugger argument projection, and bounded event summaries |
| `2015–2132` | temporary-resource discovery, timeout/locator/state conversion, and URL resolution |
| `2133–2221` | artifact-directory creation, artifact writing, and duration conversion |
| `2222–2667` | shared fake browser plus eight mixed unit tests |

These ranges describe the current implementation only. They are not future APIs and should not be
preserved after extraction.

### 2.2 Current public consumers

The compatibility surface is broader than the runtime crate's tests:

| Consumer | Runtime surface used |
|---|---|
| `crates/app` | `Runner`, `RunnerOptions`, `EvidenceOptions`, `RunError`, `StepError`, `StepFailure`, and `resolve_browser_url`; it constructs configuration, executes plans, and converts structured runtime output |
| `crates/editor` | `Runner`, `RunnerOptions`, `RunResult`, and `RunError`; it executes the current analyzed plan and composes revision-matched observations into diagnostics |
| `crates/dap` | `Runner`, `RunnerOptions`, `RunControl`, `StepError`, and `StepFailure`; it pauses before shared runtime steps and after selected app-provider failures, with bounded redacted bindings |
| `crates/observation` | receives the event, failure, diff, and revision-bound observation values constructed by runtime |
| `crates/browser` | supplies the protocol-neutral host/session/context/page traits, actions, evidence, inspection, repair, and typed browser errors consumed by runtime |
| `crates/provider` | supplies the registry, typed values, call context, schemas, results, redaction, and infrastructure/test failure distinction consumed by runtime |
| CLI/protocol tests | depend on server-only execution without Chrome, stable event conversion, source-mapped failures, DAP stepping, and clearing stale editor observations |

`crates/runtime` has no production dependency on `app`, `editor`, `lsp`, `dap`, `browser-cdp`,
`analysis`, `syntax`, `project`, or reporter formatting. This refactor must preserve that boundary.

### 2.3 Current run and browser lifecycle

The implemented execution model is:

- starting a run clears every stored observation for the plan's `FileId` before browser startup;
- each run allocates one `ExecutionId` and starts an in-memory ordered event vector with
  `RunStarted`;
- a browser session is started only when `required_host_capabilities` contains `Browser`;
- provider-only plans execute without starting Chrome;
- tests execute in plan order;
- one browser session is shared across the run, while each browser-enabled test receives a new
  context and page;
- a context-close failure taints the session; before a later test, the old session is best-effort
  closed and a new session is started;
- a normal run closes its session and returns any session-close failure as `RunError::Browser`;
- a test failure stops only that test; later tests still run;
- a browser/provider infrastructure step error aborts the run as `RunError`;
- `Runner::run` is the no-control wrapper around `run_with_control`.

The app currently provides its outer wall-clock test timeout with `tokio::time::timeout`. Within
runtime, operation timeouts are passed to browser/provider calls as currently implemented. D.7 must
not quietly move, duplicate, or reinterpret deadline ownership; Milestone E defines that redesign.

### 2.4 Current step and control order

For each planned step, the current order is significant:

1. honor `RunControl::is_cancelled` before beginning the step;
2. for provider calls, collect secret argument values before exposing debugger bindings;
3. call `should_capture_bindings` and then either `before_step_with_bindings` or `before_step`;
4. re-check cancellation after the pre-step hook;
5. append `StepStarted`;
6. for provider calls, append `ProviderCallStarted` with bounded redacted argument summaries;
7. execute the step;
8. on success, collect provider result secrets, append `ProviderCallFinished` when applicable, then
   append `StepPassed`;
9. on failure, redact the error first, call `after_step_failure` with redacted visible bindings,
   capture relevant evidence/inspection/repair facts, append provider/step failure events, and record
   a non-infrastructure observation;
10. stop the current test after its first failure.

Moving hook calls or event construction to visually cleaner locations can change debugger behavior,
secret exposure, event order, elapsed-time measurement, or cancellation semantics. Exact order is
part of the compatibility contract for this refactor.

### 2.5 Current per-test state and binding visibility

Each test currently owns four related mutable collections:

- `environment: HashMap<BindingId, Value>` for evaluated values;
- `binding_names: HashMap<BindingId, String>` for author-facing debugger/result names;
- `secrets: Vec<String>` for concrete sensitive values discovered from provider arguments/results;
- `redacted_fields: Vec<String>` initialized from options and extended by provider result metadata.

Pure evaluations and provider calls may bind values. Provider arguments are evaluated against the
same environment. Debugger snapshots may include server-only values such as responses, bytes,
process results, and temporary paths, but final `TestResult.bindings` contains only recursively
transferable values. Both surfaces are redacted.

Debugger values are bounded to 16 KiB and 256 items. The implementation preserves enough pre-
redaction overlap to catch a secret crossing the final text boundary and then applies the hard
bound again. `RunControl::should_capture_bindings` exists to avoid constructing these potentially
expensive snapshots unless the debugger needs them.

### 2.6 Current operation semantics

Runtime consumes syntax-independent `TestPlan` operations:

- `EvaluatePure` recursively evaluates one `PlanExpr` and optionally binds it;
- `ServerProviderCall` evaluates a deterministic `BTreeMap` of arguments, supplies provider,
  operation, schema identity, project root, timeout, and redaction context to `ProviderRegistry`,
  and optionally binds the returned value;
- `Browser` translates plan locators/states into browser DTOs and invokes the shared `Page` trait;
- `Assertion` executes locator/URL waits, value comparisons, or typed `matches` decoding.

Expression evaluation implements literals, bindings, lists, records, members, unary operators,
short-circuit boolean operators, numeric/string binary operations, containment, and typed decode.
Division by zero and unavailable response decoding are dynamic evaluation failures. Invalid states
that should have been excluded by analysis remain internal errors.

Typed record decode projects declared fields, inserts `Null` for missing optional fields, rejects a
missing required field at its exact JSON path, and recursively decodes list/record members. Value
assertions preserve numeric int/float equality, string/numeric ordering, containment behavior, and
bounded structural diffs. String diff offsets are Unicode character counts, not byte offsets.

### 2.7 Current failure, evidence, and observation model

`StepError` retains distinct browser, provider, assertion, decode, evaluation, and internal
variants. Browser/provider errors decide whether a failure is infrastructure. Non-infrastructure
step failures become `StepFailure` values and revision-bound observations. Infrastructure failures
abort the run and are not recorded as runtime observations.

For non-infrastructure browser failures, runtime currently:

- requests page evidence using the step's locator when one exists;
- includes screenshot/DOM requests only according to `EvidenceOptions`;
- performs semantic page inspection even when persisted artifacts are disabled;
- converts inspection failure into a bounded secondary failure rather than replacing the primary;
- derives only failure-specific repair hints;
- applies the precise step origin as each repair hint's source range;
- writes requested artifacts under deterministic execution/test/step names;
- records a source-revision-bound observation with structured evidence and elapsed time.

Repair hints are currently produced for missing/ambiguous locators, missing select options, and URL
mismatches. Actionability failures are not presented as automatically healable. Locator/option
candidates remain bounded by `webtest_browser::MAX_CANDIDATES`.

### 2.8 Known sharp edges that D.7 must expose, not silently redefine

The current sequential design predates Milestone E. In particular, cancellation is a polling hook,
the app owns the outer wall-clock timeout, and early-return cleanup is not represented as an
explicit resource scope. Characterization may reveal an early infrastructure or dropped-future path
whose cleanup behavior is weaker than the intended product invariant.

D.7 must not encode an unsafe behavior as a desirable new contract or fix it invisibly during file
movement. If characterization finds a leaked temporary directory, context, page, session, or
provider-owned resource, handle it in one of two explicit ways:

1. land a narrowly reviewed safety fix with a failing regression test before the affected code is
   extracted; or
2. document the exact deferral to Milestone E when the correction genuinely requires structured
   cancellation/deadline ownership.

The same rule applies to cancellation result semantics. Mechanical extraction and behavioral
redesign must remain distinguishable in review.

## 3. Architectural rules

### 3.1 Preserve the crate boundary and dependency direction

All moved behavior remains in `crates/runtime`. Runtime continues to depend only on protocol-neutral
plan, provider, browser, feedback, observation, HIR identity/operator, and standard/runtime support
crates already authorized by the repository architecture.

The refactor must not introduce:

- parsing, HIR lowering, static analysis, project discovery, configuration-file reading, or terminal
  reporting in runtime;
- a dependency on `browser-cdp`, `app-bridge`, `app`, `editor`, `lsp`, `dap`, `wasm`, or `syntax`;
- runtime policy in a downstream adapter merely to shorten this crate;
- plan construction or mutation inside runtime;
- CDP JSON or adapter protocol DTOs in runtime APIs.

No new crate is required. Cross-crate movement is out of scope unless separately reviewed against
the ownership guidelines.

### 3.2 Preserve one sequential executor

There must be one `Runner`, one test loop, one step dispatcher, one expression evaluator, one
provider call path, one browser operation path, one assertion implementation, and one failure-
enrichment path.

Do not leave forwarding copies of old algorithms active during extraction. Temporary delegation is
acceptable within a delivery slice, but each old implementation must be removed as soon as its new
owner is used.

Adapters continue to call the same `Runner`. DAP controls that runner through `RunControl`; it does
not acquire a debugger-specific executor.

### 3.3 Keep the public facade stable

`lib.rs` privately declares implementation modules and re-exports the existing root API. Internal
modules remain private. New implementation symbols use the narrowest visibility that works:

- `pub` only for the existing external contract;
- `pub(crate)` for necessary collaboration between runtime modules;
- `pub(super)` within the execution/operation family;
- private for leaf helpers and fields wherever possible.

Do not make `execution`, `evaluation`, `redaction`, or another implementation module public to avoid
writing root re-exports or fixing internal imports.

### 3.4 Make ownership explicit without inventing a scheduler

`Runner` owns stable run configuration, provider access, and the observation store. The run loop
owns the browser session and event/result aggregation. One test execution owns its context, page,
binding environment, binding names, discovered secrets, result-field redactions, and temporary
resources.

An internal `TestExecutionState` is appropriate because those collections share exactly one test
lifetime and must be updated together. It must not become a universal context containing optional
browser, provider, observation, control, artifact, CLI, clock, scheduler, or reporter state.

D.7 does not add task trees, cancellation tokens, clocks, deadlines, event channels, retry attempts,
resource scopes, or parallel environments. Milestone E owns those abstractions and their semantics.

### 3.5 Preserve typed failures and classification

Do not flatten any error below an adapter. Preserve `StepError`, `RunError`, `AssertionFailure`,
`DecodeFailure`, `EvaluationFailure`, `RuntimeFailure`, `ValueDiff`, browser/provider error values,
repair hints, evidence, secondary failures, and observations.

Infrastructure classification remains delegated to the typed browser/provider errors. Extraction
must not infer failure class from messages, codes, module placement, or whether a page exists.

### 3.6 Preserve source, revision, identity, and event order

Every observation retains the plan's exact `FileId` and `SourceRevision`, plus the current
`ExecutionId`, `TestId`, `StepId`, and smallest step `SyntaxOrigin` range. Repair hints use that same
step range.

Execution identities remain monotonic through `ExecutionId::next`. Plan identities are consumed,
not regenerated. Events retain current variant payloads and order. Collection type changes must not
make provider arguments, visible bindings, artifacts, tests, or events nondeterministic.

### 3.7 Treat redaction as a boundary, not presentation cleanup

Concrete provider secrets must be collected before they can flow into debugger snapshots, event
summaries, browser errors, evidence requests, inspections, repair hints, final bindings, or adapter
output. Assertion messages/diffs must be recomputed from redacted values. URL query-parameter
redaction remains case-insensitive and concrete secret replacement remains active.

No test fixture may assert a raw secret in a failure string merely to make extraction convenient.
Security tests should assert absence from the complete reachable structured output.

### 3.8 Keep bounded work and cleanup visible

Preserve current bounds for debugger bytes/items, displayed values, diff segments/field lists,
repair candidates, DOM evidence, inspection, and event summaries. Do not replace a bounded clone
with an unbounded convenience clone.

Context/session close and temporary-directory cleanup remain explicit async lifecycle steps. Artifact
I/O remains failure-tolerant secondary evidence behavior; it must not replace the primary test
failure. A future change from synchronous artifact writes to async/atomic storage is separate from
this decomposition.

### 3.9 Refactor before redesign

Initial extraction should move cohesive behavior with minimal rewriting. After parity tests pass,
internal signatures may be narrowed and per-test state may be encapsulated. Do not combine D.7 with
a generic executor trait, visitor framework, dependency-injection container, event bus, error
registry, state machine framework, async stream API, scheduler prototype, or pluggable artifact
backend without a present requirement.

## 4. Scope

This milestone includes:

- characterization of the current public API, defaults, lifecycle, ordering, failures, redaction,
  and cleanup behavior;
- root-level API compatibility coverage for app, editor, and DAP consumers;
- extraction of public errors, results, options, artifacts, and `RunControl`;
- extraction of pure expression evaluation, typed decode, assertions, comparison, and bounded diffs;
- extraction of provider and browser operation execution;
- explicit encapsulation of per-test binding/name/secret/redaction state;
- extraction of debugger snapshot and provider-summary behavior;
- extraction of URL, locator, state, and timeout conversion;
- extraction of redaction, evidence, inspection, repair, artifact, event-failure, and observation
  behavior;
- reduction of the root module to a facade;
- focused unit tests beside each new owner plus vertical runner/downstream compatibility tests;
- documentation of any discovered cleanup sharp edge that requires a separate safety patch or
  Milestone E.

## 5. Non-goals

This milestone does not add or change:

- DSL syntax, HIR, analysis, plan variants, plan serialization, or `webtest describe` content;
- parallel, race, retry, timeout-block, test-job, fail-fast, or scheduling semantics;
- cancellation tokens, structured task/resource trees, fake clocks, deadline propagation, or wait
  registrations;
- browser actionability, automatic retries, polling policy, traces, video, network/console event
  capture, or CLI-to-LSP observation IPC;
- application-bridge protocol, provider schemas, retry-safety policy, lifecycle ownership, or
  transport behavior;
- browser traits, CDP mechanics, Chrome launch, or browser-manager behavior;
- observation/event schema versions or atomic observation-batch semantics;
- DAP protocol, breakpoint selection, thread model, stepping model, or variable rendering;
- app reporter schemas, exit classes, CLI flags, project configuration, or outer command timeout;
- a second runtime backend or adapter-specific operation implementation;
- performance optimization not needed to preserve the current bounds.

Because no author-facing language/provider capability changes, this milestone does not update the
description catalog or canonical WebTest authoring skill. Any unexpected need to do so signals scope
creep.

## 6. Compatibility contract

### 6.1 Root-level public API

The following remain importable from `webtest_runtime` with their current names:

```text
ArtifactKind
Artifact
StepFailure
StepError
AssertionFailure
DecodeFailure
EvaluationFailure
RunError
TestResult
RunResult
EvidenceOptions
RunnerOptions
Runner
RunControl
resolve_browser_url
```

Preserve public fields, enum variants, derives, auto-trait behavior, error sources, display text,
method signatures, and async behavior. In particular:

- `StepError::code` and `StepError::is_infrastructure` remain available;
- `RunError::code` remains available;
- `RunResult::passed` and `RunResult::failed` retain their count semantics;
- `Runner::new`, `with_options`, `with_provider_registry`, `run`, and `run_with_control` retain
  their signatures and builder precedence;
- `RunControl` retains all current default methods and remains `Send + Sync`;
- `resolve_browser_url` continues to return `BrowserError` for app-level reuse.

If an internal module owns a public item, `lib.rs` re-exports it. Downstream imports must not switch
to `webtest_runtime::errors::*` or another implementation path.

### 6.2 Options and defaults

`EvidenceOptions::default` retains:

- screenshot and DOM capture disabled;
- `max_dom_bytes = 1_048_576`;
- `.webtest/artifacts` as the default directory.

`RunnerOptions::default` retains:

- no browser base URL;
- five-second action and assertion timeouts;
- a sixty-second test timeout;
- default browser context, provider configuration, and inspection options;
- project root `.`;
- the current case-insensitive redacted field list.

`with_options` continues rebuilding built-in providers from the supplied `provider_config` before
storing options. A following `with_provider_registry` continues to take precedence. Changing this
order can silently discard a configured application provider.

### 6.3 Run, test, browser, and cleanup lifecycle

Preserve:

- observation clearing before any run work that can fail;
- conditional browser startup from plan capabilities;
- one shared session and one context/page per browser-enabled test;
- source-order tests and steps;
- first-step-failure termination of the current test;
- continuation to later tests after ordinary test failure;
- session restart after a context-close failure when another test remains;
- normal context and session close behavior and error classification;
- test/run duration measurement with `Instant` and saturated millisecond event conversion;
- transferable, redacted final bindings;
- recursive temporary-directory discovery and cleanup at the current test-finalization boundary,
  subject to the explicit sharp-edge review in section 2.8.

Do not start Chrome for a server-only plan merely because the refactored executor accepts a browser
reference.

### 6.4 `RunControl` and debugger behavior

Preserve the exact hook order from section 2.4. The default `should_capture_bindings` remains `true`;
its false result skips construction of the pre-step snapshot. The default
`before_step_with_bindings` delegates to `before_step`.

Pre-step bindings include existing named environment values and `argument.<name>` entries for the
current provider call. Secret provider arguments appear as `[redacted]`. Server-only values remain
inspectable in bounded debugger form. `after_step_failure` receives the already-redacted error and
bindings before page evidence/inspection is attempted.

Cancellation remains the current cooperative pre-test/pre-step hook for this refactor. D.7 does not
reinterpret a disconnect as a Milestone E task-tree cancellation result.

### 6.5 Provider execution and events

Preserve:

- argument evaluation against the current test environment;
- deterministic argument names/order through `BTreeMap`;
- provider/operation/schema identity passed unchanged;
- `CallContext.project_root` from options;
- explicit call timeout or current test-timeout fallback;
- current global redacted-field context;
- binding insertion only after a successful result;
- transport-kind lookup from the same registry;
- secret argument collection before pre-step debugger exposure;
- secret result-field collection before result summaries and later steps;
- bounded redacted argument/result event summaries;
- provider start/finish/failure event placement and elapsed-time measurement.

Provider errors remain typed. Schema drift, handshake, transport, application, filesystem, HTTP,
and process failures must not be collapsed or reclassified during module moves.

### 6.6 Browser operations, URLs, locators, and timeouts

Every existing `BrowserOperation` variant keeps its mapping to the shared `Page` API:

- navigate and wait-for-URL resolve evaluated strings through the same URL policy;
- evaluate forwards the authored expression to `Page::evaluate`;
- click/fill/type/press/check/select/hover construct the same `Action` values;
- locator waits and assertions translate every locator state;
- action operations use `action_timeout`;
- waits/assertions use their explicit timeout or `assertion_timeout`, bounded by `test_timeout`.

Relative root paths continue to replace the configured base path, other relative paths append to the
trimmed base, absolute scheme-like URLs bypass the base, and bare absolute hosts gain a trailing
slash. A relative URL without `browser.base_url` remains `BrowserError::NavigationFailed`.

Plan locators and states remain translated explicitly. Do not merge plan and browser DTOs or make
`plan` depend on `browser` to remove these conversions.

### 6.7 Expression evaluation and typed decode

Preserve exact semantics for every `PlanExpr` and operator, including:

- left-to-right recursive evaluation;
- boolean short-circuiting;
- int/float promotion and equality;
- integer subtraction/multiplication retention and floating division;
- dynamic division-by-zero failure;
- string addition and containment;
- member access and response decode failure classification;
- type-pattern rejection outside `matches`;
- exact binding lookup behavior;
- recursive list/record construction;
- exact decode paths, optional-null behavior, field projection, int-to-float promotion, response
  provenance, and resource/native value handling.

The evaluator remains independent of browser sessions, provider registries, observations, options,
filesystem I/O, and adapters.

### 6.8 Assertions, messages, and diffs

Preserve locator/URL wait assertions and value matcher behavior for truthy, equality, inequality,
ordering, containment, and typed matches.

Failure messages remain derived from bounded displayed values. Diffs retain:

- Unicode-character common-prefix counts and bounded string segments;
- maximum 20 differing list indices;
- maximum 20 missing, unexpected, and mismatched record fields per collection;
- scalar and contains fallbacks;
- numeric int/float equality.

After redaction, assertion messages and diffs are recomputed from redacted values so a previously
constructed message cannot retain a secret.

### 6.9 Failure classification and structured output

Ordinary assertion, decode, evaluation, and non-infrastructure browser/provider errors remain test
failures. Browser/provider infrastructure errors remain run errors. Internal invariant failures
remain distinct and must not become ordinary evaluation failures merely because their helpers move.

`StepFailure` retains the planned step, typed error, evidence, artifacts, optional inspection,
repair hints, and secondary failures. The runtime-to-observation and runtime-to-event conversions
retain typed failure data and current error codes.

### 6.10 Redaction and debugger bounds

Preserve exhaustive redaction for every current `BrowserError` variant, including locator contents,
options, keys, URLs, navigation reasons, protocol fields, crash/launch messages, and evaluated
expression errors. Provider errors continue to use their typed redaction method.

Preserve:

- exact concrete-secret replacement;
- case-insensitive URL query-parameter redaction;
- recursive configured record-field redaction;
- debugger limits of 16 KiB and 256 items;
- UTF-8 boundary-safe truncation;
- pre-redaction overlap followed by a second hard bound;
- bounded response, process, bytes, headers, list, and record previews;
- omission of non-transferable values only from final result bindings, not debugger snapshots.

### 6.11 Evidence, repair hints, artifacts, events, and observations

Preserve evidence capture eligibility, request fields, inspection options, repair-hint kinds/reasons,
candidate bounds, source ranges, secondary inspection failures, and screenshot suppression.

Artifact behavior remains:

- no directory creation when there is no persistable evidence;
- deterministic stem `test-<id>-step-<id>-execution-<id>`;
- `.png`, `.dom.html`, and `.evidence.txt` outputs with their current `ArtifactKind` values;
- directory/write failures appended to `PageEvidence.capture_failures` rather than replacing the
  primary failure;
- artifact paths attached to failures and observations.

Observations remain keyed by plan file/revision, recorded only for non-infrastructure failures, and
cleared at the beginning of a later run. Successful reruns leave no stale editor runtime diagnostic.

Events retain their current variants, fields, provider summaries, source identities, and ordering.
This milestone does not version or redesign the event envelope.

### 6.12 Downstream parity

The app must continue to render stable failure codes/details and run server-only tests without
Chrome. Editor runs must still reject static errors, publish only current-revision runtime
diagnostics, and clear them after success or infrastructure rerun. DAP must still pause before the
shared runtime step, skip expensive snapshots when no pause is possible, expose bounded redacted
values, and pause after configured app-provider failures.

## 7. Target module layout

```text
crates/runtime/src/
├── lib.rs
├── artifacts.rs
├── control.rs
├── error.rs
├── options.rs
├── result.rs
├── runner.rs
├── execution.rs
├── execution/
│   ├── state.rs
│   ├── steps.rs
│   ├── provider.rs
│   ├── browser.rs
│   └── failure.rs
├── evaluation.rs
├── assertions.rs
├── redaction.rs
├── url.rs
└── tests.rs
```

The layout is a responsibility map, not a mandate for empty wrapper files. If implementation shows
that two proposed leaf modules are too small and have exactly one reason to change, they may be
combined under a specific name. Combining them into `helpers.rs`, `utils.rs`, `common.rs`,
`runtime.rs`, or another catch-all is not acceptable.

### 7.1 `lib.rs`

Owns only:

- crate-level documentation;
- private module declarations;
- root re-exports of the public contract.

It contains no wildcard re-export of implementation internals and no tests.

### 7.2 `error.rs`

Owns:

- `StepError`;
- `AssertionFailure`;
- `DecodeFailure`;
- `EvaluationFailure`;
- `RunError`;
- error codes, infrastructure classification, `Display`, and `Error` implementations.

It does not capture evidence, format CLI reports, inspect pages, or record observations.

### 7.3 `result.rs`

Owns:

- `StepFailure`;
- `TestResult`;
- `RunResult`;
- passed/failed count methods.

These are public data-only execution results. Failure-enrichment algorithms live elsewhere.

### 7.4 `options.rs`

Owns `EvidenceOptions`, `RunnerOptions`, and their exact defaults. It may import browser/provider
configuration DTOs but does not build a project or read `webtest.toml`.

### 7.5 `control.rs`

Owns the public `RunControl` trait and default methods. It contains no DAP types or breakpoint
policy. DAP remains one consumer of this protocol-neutral hook.

### 7.6 `artifacts.rs`

Owns `ArtifactKind`, `Artifact`, deterministic artifact names, directory creation, writes, evidence
summary construction, and write-failure attachment. It accepts explicit paths/IDs/evidence and has
no access to `Runner`, provider registries, controls, or reporters.

### 7.7 `runner.rs`

Owns:

- public `Runner` storage/builders/entry points;
- observation clearing and execution-ID allocation;
- run-level event/result aggregation;
- conditional browser-session start and normal close;
- source-order test iteration;
- tainted-session restart policy.

It delegates one test to `execution`. It does not evaluate expressions, inspect page failures,
redact values, or write artifacts directly.

### 7.8 `execution.rs`

Owns the one sequential test coordinator:

- test start/finish events and duration;
- context/page creation and close;
- step iteration and `RunControl` ordering;
- coordination of state, step execution, success events, and failure enrichment;
- final transferable bindings and temporary-resource cleanup;
- the context-tainted result returned to `runner`.

This module may be the largest coordinator, but should remain readable as lifecycle code rather
than absorbing leaf algorithms.

### 7.9 `execution/state.rs`

Owns a private `TestExecutionState` containing only per-test semantic state:

```text
environment: HashMap<BindingId, Value>
binding_names: HashMap<BindingId, String>
secrets: Vec<String>
redacted_fields: Vec<String>
```

It owns binding insertion, secret collection, visible debugger binding projection, final
transferable binding projection, and temporary-directory enumeration. It may call the shared
evaluator/redaction helpers. It does not own page/context/session handles, controls, events,
observations, providers, clocks, or options wholesale.

### 7.10 `execution/steps.rs`

Owns the exhaustive `TestOperation` dispatcher and binding of successful pure/provider results. It
delegates to evaluation, provider, browser, and assertion owners and returns `Result<(), StepError>`.

It must not introduce an opaque callback or erase operation variants needed for debugging and
events.

### 7.11 `execution/provider.rs`

Owns provider argument evaluation, `ProviderCall` and `CallContext` construction, registry dispatch,
and returned `Value` extraction. Provider event construction stays with the test coordinator because
its order is part of the step lifecycle; bounded provider summaries and secret state live with
`TestExecutionState`/redaction.

### 7.12 `execution/browser.rs`

Owns exhaustive browser-operation execution plus plan-to-browser locator/state conversion and
step-locator extraction. It uses shared evaluation and URL resolution. It does not import CDP or
construct evidence/repair output.

### 7.13 `execution/failure.rs`

Owns the single post-step failure pipeline:

- redacted error input;
- conditional page evidence capture;
- semantic inspection and secondary failure capture;
- failure-specific repair hints and source ranges;
- artifact persistence;
- event `RuntimeFailure` conversion;
- revision-bound observation construction/recording;
- final `StepFailure` assembly;
- infrastructure-to-`RunError` conversion after the required events/cleanup boundary.

The coordinator must keep the current ordering visible. It must not render terminal/DAP/LSP output.

### 7.14 `evaluation.rs`

Owns pure `PlanExpr` evaluation, binary/numeric operations, typed decode, value equality/order/
containment, string extraction, display values, and transferability facts used by runtime.

If display/transferability helpers are shared with assertions/state, they remain named functions in
this module rather than moving to a vague common module.

### 7.15 `assertions.rs`

Owns assertion execution, matcher semantics, failure messages, bounded value rendering, structural
diffs, and Unicode-safe string segments. Browser assertion branches may call the protocol-neutral
page through explicit parameters; value assertions remain pure apart from evaluation.

### 7.16 `redaction.rs`

Owns:

- step and browser error redaction;
- locator/text/URL redaction;
- recursive sensitive-value collection;
- bounded debugger snapshots;
- visible binding and provider argument/result summary helpers;
- the existing byte/item/summary bounds.

This is security policy, not reporter formatting. Tests should cover every browser-error variant and
all complex provider value shapes.

### 7.17 `url.rs`

Owns public `resolve_browser_url` plus private absolute-URL recognition and normalization. Runtime
step code maps its `BrowserError` into `StepError` at the call site.

### 7.18 `tests.rs`

Owns vertical runner characterization that crosses multiple production modules, along with shared
fake host/session/context/page/control/provider fixtures. Leaf unit tests remain beside their owner.
Test-only helpers must not become production APIs.

## 8. Internal dependency direction

The desired dependency flow is:

```text
lib
  -> public re-exports

runner
  -> options, result, error, control, execution

execution
  -> state, steps, failure
  -> options, result, control

steps
  -> evaluation, assertions, provider, browser

failure
  -> error, result, redaction, artifacts
  -> browser evidence/inspection/repair DTOs
  -> observation DTOs/store

state
  -> evaluation, redaction

provider/browser/assertions
  -> evaluation and narrow options/facts

redaction/evaluation/url/artifacts
  -> protocol-neutral leaf DTOs only
```

Forbidden internal edges include:

- leaf modules depending on `Runner`;
- `evaluation` depending on browser, provider registry, observation store, artifacts, control, or
  options;
- `redaction` starting providers/pages or recording observations;
- `artifacts` classifying the primary failure;
- `error` depending on execution orchestration;
- `result` depending on reporters or adapters;
- `control` depending on DAP;
- `runner` importing app/project configuration or CDP;
- operation modules calling adapters;
- a cycle resolved by publishing internal fields at the crate boundary;
- duplicate failure or event construction paths.

## 9. Current symbol migration map

| Current symbol/group | Target owner |
|---|---|
| `ArtifactKind`, `Artifact`, `write_artifacts`, `write_artifact` | `artifacts.rs` |
| `StepError`, `AssertionFailure`, `DecodeFailure`, `EvaluationFailure`, `RunError` | `error.rs` |
| `StepFailure`, `TestResult`, `RunResult` | `result.rs` |
| `EvidenceOptions`, `RunnerOptions` | `options.rs` |
| `RunControl` | `control.rs` |
| `Runner`, `new`, builder methods, `run`, `run_with_control` | `runner.rs` |
| `run_test` | `execution.rs` |
| environment/name/secret/redacted-field handling | `execution/state.rs` |
| `execute_step` | `execution/steps.rs` |
| `execute_provider` | `execution/provider.rs` |
| `execute_browser`, `browser_locator`, `browser_state`, `step_browser_locator` | `execution/browser.rs` |
| failure capture/enrichment coordination | `execution/failure.rs` |
| `record_observation`, `runtime_failure`, `repair_hints_for_error`, edit distance | `execution/failure.rs` |
| `evaluate`, `evaluate_binary`, `numeric_binary`, `decode_value` | `evaluation.rs` |
| value equality/order/containment/display/transferability/string conversion | `evaluation.rs` |
| `execute_assertion`, matcher/message/diff/bounded segment helpers | `assertions.rs` |
| error/locator/text/URL redaction and sensitive collection | `redaction.rs` |
| debugger snapshot/binding/provider summary helpers and bounds | `redaction.rs` or a narrowly split `debug_values.rs` if the module exceeds the review target |
| `temporary_directories` | `execution/state.rs` |
| `bounded_timeout` | the browser/assertion owner using it; one shared narrow helper only if both call it |
| `resolve_browser_url`, `resolve_url`, absolute detection, normalization | `url.rs` plus call-site error mapping |
| `duration_millis` | `execution.rs` or a narrow event helper within the execution family |
| shared fake browser and vertical tests | `tests.rs` |

The optional `debug_values.rs` split is allowed only because bounded debugger materialization and
security redaction are independently testable policies. It must remain private and may not duplicate
secret replacement or value traversal.

## 10. Interface design

### 10.1 Public facade

Prefer explicit root re-exports:

```rust
pub use artifacts::{Artifact, ArtifactKind};
pub use control::RunControl;
pub use error::{AssertionFailure, DecodeFailure, EvaluationFailure, RunError, StepError};
pub use options::{EvidenceOptions, RunnerOptions};
pub use result::{RunResult, StepFailure, TestResult};
pub use runner::Runner;
pub use url::resolve_browser_url;
```

This is illustrative. The final file must also preserve crate documentation and test configuration
without exposing internal modules.

### 10.2 Per-test state

Encapsulate related maps/vectors only after characterization. Suggested narrow operations are:

```text
bind(id, optional_name, value)
prepare_provider_arguments(call)
accept_provider_result_metadata(call)
visible_bindings(optional_current_step)
final_transferable_bindings(default_redacted_fields)
temporary_directories()
environment()
```

These names are conceptual. Avoid methods that both mutate state and append events/capture pages.
Provider argument expressions must still be evaluated once for actual dispatch; summary/debugger
evaluation must preserve current failure and side-effect assumptions because `PlanExpr` evaluation is
pure.

### 10.3 Test execution return

The boundary between `execution` and `runner` needs only the current result and session-taint fact:

```text
ExecutedTest {
    result: TestResult,
    session_tainted: bool,
}
```

A private struct may replace the current tuple if it improves clarity. It must not become a future
scheduler result with speculative cancellation/attempt fields.

### 10.4 Step dispatch

Keep the current narrow result:

```text
execute_step(...) -> Result<(), StepError>
```

Do not introduce a boxed operation callback. `PlannedStep` remains available to the coordinator for
events, source identity, debugging, evidence locators, and failure assembly.

### 10.5 Failure processing

The failure owner may use a private input struct to avoid a long argument list, but it must contain
only facts needed for one failed step, mostly by reference. It must not own or hide run/test loops.
The output should distinguish:

- an enriched ordinary `StepFailure` that ends the test; and
- an infrastructure `RunError` that aborts the run.

Primary error, secondary evidence failures, cleanup failures, and artifacts remain distinct.

### 10.6 Explicit collaborators

Pass `ProviderRegistry`, `RunnerOptions`, `ObservationStore`, `RunControl`, page/context handles, and
event vectors explicitly at the coordinator boundary. Do not put all of them into an `Arc<Runtime>`
service locator. Explicit borrowing makes ownership and the future Milestone E replacement boundary
reviewable.

### 10.7 Test visibility

Leaf tests may use private functions through their module's `#[cfg(test)]` block. Cross-module tests
should exercise `Runner` or a narrow `pub(crate)` coordinator, not broaden production visibility.

## 11. Delivery slices

Each slice must compile and pass focused tests. Code movement should be mechanical after its
behavior is characterized.

### Slice 1 — Characterize the public and lifecycle boundary

Before moving production code:

1. Add a compile-time/root-import test for every public runtime item used by app/editor/DAP.
2. Add exact default-option and builder-precedence tests.
3. Add fake host/session/context/page counters proving conditional browser startup, one session per
   run, one context/page per browser test, source-order execution, normal close, and tainted-session
   restart.
4. Add provider-only plan coverage proving no browser start.
5. Add ordinary failure versus infrastructure abort coverage across multiple tests.
6. Add exact event-order fixtures for pure, provider, browser, assertion, success, and failure steps.
7. Add `RunControl` fixtures for snapshot gating, pre-step order, post-failure redaction, and current
   cancellation polling behavior.
8. Add cleanup characterization for contexts, sessions, nested temporary directories, artifact
   errors, and early infrastructure paths; follow section 2.8 for any defect.
9. Retain existing CLI server-only, editor observation, and DAP breakpoint tests.

Tests should assert durable external behavior, not current private function names or hash-map memory
layout.

### Slice 2 — Extract public data-only types and options

1. Move errors/failures to `error.rs` without changing variants, fields, displays, or codes.
2. Move results to `result.rs` without changing fields or count behavior.
3. Move options/defaults to `options.rs`.
4. Move `RunControl` to `control.rs`.
5. Move public artifact DTOs to `artifacts.rs` while leaving writes temporarily delegated if useful.
6. Add explicit root re-exports and rerun downstream compile/tests.

At the end of this slice, `lib.rs` may still own behavior but no longer owns public DTO definitions.

### Slice 3 — Extract pure evaluation and assertions

1. Move expression recursion, binary/numeric operations, value comparison, decode, and display
   helpers to `evaluation.rs` mechanically.
2. Move assertion matching, messages, structural diffs, and assertion execution to `assertions.rs`.
3. Preserve recursion/evaluation order and exact internal versus dynamic failure choices.
4. Move existing decode, diff, and dynamic-expression tests with their owners.
5. Add exhaustive expression/operator/type-decode/matcher fixtures before narrowing visibility.

These modules must remain free of runner/provider-registry/artifact/observation dependencies.

### Slice 4 — Extract URL and browser/provider operations

1. Move public URL resolution and private normalization to `url.rs`.
2. Move plan-to-browser conversion and every browser operation to `execution/browser.rs`.
3. Move provider call construction/dispatch to `execution/provider.rs`.
4. Establish `execution/steps.rs` as the only `TestOperation` dispatcher.
5. Add exhaustive browser variant, timeout, URL, provider context, schema, binding, and failure tests.
6. Verify provider-only execution still does not start a browser.

No operation module may construct adapter output or a second event stream.

### Slice 5 — Extract redaction and per-test state

1. Move error/locator/text/URL redaction and sensitive-value traversal to `redaction.rs`.
2. Move bounded debugger snapshots and summaries, splitting `debug_values.rs` only if needed for
   cohesion/size.
3. Create private `TestExecutionState` and move binding/name/secret/redacted-field ownership into it.
4. Preserve provider argument-before-hook and result-before-summary secret collection.
5. Preserve final transferable binding projection and nested temporary-resource discovery.
6. Add exhaustive no-secret-reachable and byte/item/Unicode-bound tests.
7. Verify DAP snapshot gating and value previews.

Do not move DAP rendering or provider schema policy into runtime state.

### Slice 6 — Extract failure evidence, observations, and artifacts

1. Complete `artifacts.rs` extraction with deterministic paths and secondary write failures.
2. Move runtime-failure conversion, observation construction, repair ranking, and source-range
   assignment to `execution/failure.rs`.
3. Move page evidence/inspection/artifact coordination into the same failure path.
4. Preserve evidence eligibility and primary/secondary failure separation.
5. Add exhaustive failure-class, observation-field, repair-bound, redaction, inspection-failure, and
   artifact tests.
6. Compare app event/failure reports and editor observations before continuing.

There must be one ordinary failure-enrichment path and one explicit infrastructure exit.

### Slice 7 — Establish the test execution coordinator

1. Move `run_test` to `execution.rs` with the exact hook/event/order contract.
2. Replace parallel local maps/vectors with `TestExecutionState` only after parity tests pass.
3. Keep context/page lifetime and close order explicit.
4. Keep first-failure behavior and later-test continuation unchanged.
5. Return a named private result/taint value if useful.
6. Remove moved code from the old root immediately.
7. Run lifecycle, control, operation, failure, and cleanup suites together.

### Slice 8 — Extract the run coordinator and finalize the facade

1. Move `Runner` and run/session orchestration to `runner.rs`.
2. Ensure `Runner::run` still delegates to `run_with_control`.
3. Preserve provider builder precedence and observation clearing.
4. Reduce `lib.rs` to documentation, private modules, and explicit root re-exports.
5. Move vertical fake infrastructure to `tests.rs` and focused tests beside leaf owners.
6. Narrow transitional visibility and remove dead delegators/imports.
7. Inspect the internal module graph and file sizes.

### Slice 9 — Downstream parity and cleanup

1. Run runtime, editor, DAP, app, CLI, and protocol tests.
2. Verify server-only execution does not resolve/start Chrome.
3. Verify a browser failure publishes one precise current-revision observation and a later success
   clears it.
4. Verify breakpoint pause/resume and app-provider exception pause use the shared runner.
5. Compare representative human/JSON/events failure codes, details, hints, artifacts, and ordering.
6. Run workspace formatting, Clippy, tests, and representative CLI checks.
7. Confirm no production dependency was added and no authoring/description surface changed.
8. Update this milestone's status only after all acceptance criteria pass.

## 12. Testing requirements

### 12.1 Public API and options

Required coverage includes:

- root imports for every item in section 6.1;
- exact enum variants/public fields used by app/DAP construction and matching;
- exact `Display`, `Error`, code, and infrastructure-classification behavior;
- exact `EvidenceOptions` and `RunnerOptions` defaults;
- `with_options` rebuilding built-ins from provider configuration;
- explicit provider registry winning when applied last;
- `RunResult` counts for empty, all-pass, mixed, and all-fail runs;
- `resolve_browser_url` remaining a public `BrowserError` API.

### 12.2 Run, browser, and test lifecycle

Required fake-based coverage includes:

- observation clearing before browser-start failure;
- no browser start for a plan without `Capability::Browser`;
- exactly one session start for ordinary multi-test browser plans;
- one context/page per test and ordered closes;
- ordinary test failure not aborting later tests;
- infrastructure failure aborting the run;
- context-close failure restarting the session only when another test remains;
- session/context/page creation and close failures retaining typed classification;
- zero-test plans;
- control cancellation before the first test, before a step, and after the pre-step hook, preserving
  current results/events until Milestone E changes them explicitly;
- nested temporary-directory cleanup and explicit treatment of early-return cleanup sharp edges.

Use fakes for deterministic ownership checks. Real Chrome is a downstream proof, not the primary
way to test lifecycle policy.

### 12.3 Step, event, and control ordering

Assert exact event variant order and identities for:

- pure success/failure;
- provider success/test failure/infrastructure failure;
- browser success/test failure/infrastructure failure;
- locator, URL, value, and decode assertions;
- multiple tests and steps.

Also prove:

- `before_step` occurs before `StepStarted`;
- provider secrets are collected before bindings are captured;
- `should_capture_bindings = false` avoids materializing snapshots;
- cancellation is rechecked after the pre-step hook;
- `after_step_failure` sees a redacted error/binding set;
- provider finish/fail precedes the corresponding step terminal event;
- test/run terminal events appear only on the same paths they do today;
- IDs come from the plan/execution and are never regenerated by modules.

### 12.4 Provider execution

Required coverage includes:

- argument expression evaluation and deterministic names;
- provider/operation/schema hash forwarding;
- explicit call timeout and test-timeout fallback;
- project root and redacted-field call context;
- result binding/name insertion;
- no binding after failure;
- transport-kind event metadata;
- secret argument and nested secret result-field handling;
- bounded result/argument summaries and unavailable-evaluation fallback;
- application/test error versus infrastructure provider error;
- temporary-directory results nested in lists/records.

Use a fake `ServerProvider` registered through the real `ProviderRegistry` contract.

### 12.5 Browser operations, assertions, evaluation, and URLs

Required coverage includes:

- every `BrowserOperation` and locator/state variant;
- exact `Action` DTOs and timeout values passed to a fake page;
- explicit timeout/default/bounded-timeout cases;
- base URL root replacement, relative append, absolute normalization, query/fragment handling, and
  missing-base failure;
- every `PlanExpr`, unary operator, binary operator, and short-circuit path;
- all typed decode variants, nested exact paths, optional fields, extra-field projection, and
  int-to-float promotion;
- all value matchers and numeric/string/list/record behavior;
- dynamic division-by-zero and response-decode failures remaining non-infrastructure test failures;
- internal invariant cases remaining distinct;
- Unicode-safe and bounded string/list/record/scalar/contains diffs.

### 12.6 Redaction and debugger safety

Required coverage includes:

- every `BrowserError` variant under concrete secret replacement;
- query-parameter redaction with case variation and multiple parameters;
- assertion expected/actual/message/diff recomputation after redaction;
- typed provider error redaction;
- nested configured secret fields in records/lists;
- secret provider arguments before a paused call and secret result fields in later steps;
- response body/JSON/headers, process stdout/stderr/bytes, lists, records, and raw bytes in debugger
  snapshots;
- exactly bounded bytes/items and valid UTF-8 boundaries;
- a secret crossing the first snapshot boundary;
- server-only values visible to DAP but absent from final transferable bindings;
- final `TestResult`, `StepFailure`, events, observations, inspection, hints, artifact summaries, and
  DAP/app projections containing no raw sentinel secret.

Security tests should serialize or debug-print the complete reachable structures where practical,
not check only the primary message.

### 12.7 Failure evidence, repair, artifact, and observation behavior

Required coverage includes:

- evidence captured only for non-infrastructure browser failures;
- screenshot/DOM flags and maximum DOM bytes forwarded exactly;
- locator and redaction fields in `EvidenceRequest`;
- inspection performed for eligible browser failure even when artifact capture is disabled;
- inspection failure retained as secondary text without replacing the primary;
- missing/ambiguous locator, option, and URL repair behavior;
- no actionability healing hint;
- deterministic candidate distance/tie order and maximum candidate count;
- exact step byte range on repair hints and observations;
- deterministic artifact names/kinds/content and directory/write failure behavior;
- `RuntimeFailure` conversion for every `StepError` variant;
- `RuntimeObservationKind` fields for browser, provider, assertion, decode, evaluation, and internal
  failures;
- no observation for infrastructure failure;
- plan file/revision/test/step/range identity;
- successful and infrastructure reruns clearing stale observations.

### 12.8 Downstream compatibility

Required downstream coverage includes:

- app runtime error/failure/event conversion for every current variant;
- server-only HTTP/decode/assertion CLI execution without Chrome;
- artifact links and structured semantic details in human/JSON/events reporters;
- editor static-error gating, exact revision matching, precise ranges, and clearing on success;
- DAP pause-before-step, snapshot gating, variable preview, app-provider exception pause, continue,
  and disconnect;
- stdio protocol output remaining free of runtime stdout logging;
- representative real-browser vertical coverage when Chrome is available.

### 12.9 Verification commands

Every delivery slice runs:

```sh
cargo fmt --all -- --check
cargo test -p webtest-runtime
cargo clippy -p webtest-runtime --all-targets -- -D warnings
```

Slices touching public types, runner/control behavior, observations, or failures additionally run:

```sh
cargo test -p webtest-editor
cargo test -p webtest-dap
cargo test -p webtest
```

The final slice runs:

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
target/debug/webtest check examples/plain-html/sign-in.webtest
cargo test -p webtest --test cli server_only_http_decode_and_assertion_runs_without_chrome
```

If Chrome is available, also execute the existing real browser/editor/protocol coverage through its
normal test commands. D.7 does not require extension or WASM changes; if those files and APIs remain
untouched, extension packaging and the portable target are not completion gates.

## 13. Review checkpoints

Review every slice against these questions:

1. Does every moved item have one clear owner and reason to change?
2. Can app, editor, and DAP import the root runtime API unchanged?
3. Is there still exactly one sequential run/test/step path?
4. Did browser startup, context/page/session ownership, close order, or restart policy change?
5. Did a `RunControl` hook move relative to secret collection, events, execution, or failure capture?
6. Did any event variant, field, identity, or order change?
7. Did any error code, display text, infrastructure class, or structured field change?
8. Are observations still exact-revision, exact-range, cleared at run start, and absent for
   infrastructure failures?
9. Can any concrete secret reach errors, diffs, events, evidence, hints, artifacts, bindings, or DAP?
10. Did a bounded operation become unbounded or clone a full response/process value?
11. Are provider arguments/result metadata and browser operations still interpreted once by shared
    runtime code?
12. Did extraction introduce a universal context, service locator, public internal module, vague
    helper module, or replacement monolith?
13. Did the change accidentally implement or constrain Milestone E scheduling/cancellation?
14. If cleanup behavior changed, is it an explicit tested safety correction rather than an
    incidental consequence of moving code?
15. Are leaf tests beside their owners and vertical tests limited to true lifecycle integration?

Stop and correct the slice if an answer exposes semantic drift, duplicated execution, weaker
redaction, hidden cleanup change, or premature scheduler design.

## 14. Risks and mitigations

### 14.1 Hidden public API breakage

Moving public types can alter paths, visibility, derives, auto traits, error sources, or construction
patterns. Add root import/construct/match coverage first and retain explicit re-exports.

### 14.2 Event-order drift

Separating provider events, control hooks, and failure enrichment can reorder facts. Characterize
exact sequences for all operation outcomes and keep event append points in the coordinator.

### 14.3 Browser lifecycle and cleanup drift

Moving `run_test` can change drop/close order or session restart behavior. Use counting fakes and
explicit close logs. Treat discovered early-return leaks under section 2.8 rather than hiding them.

### 14.4 Failure reclassification

A convenient shared error conversion can turn test failures into infrastructure failures or flatten
provider/browser details. Match typed variants explicitly and test every class.

### 14.5 Secret leakage across new boundaries

If event/debug/failure modules receive unredacted values at new points, a later presenter may expose
them. Keep secret collection order, redact before cross-boundary failure hooks, and assert absence
from complete structures.

### 14.6 Debugger behavior or performance regression

Eagerly materializing bindings before `should_capture_bindings` defeats the current optimization;
moving the hook after `StepStarted` changes stepping. Test hook order and capture counts directly.

### 14.7 Expression or diff semantic drift

Genericizing value operations can change int/float equality, short-circuiting, decode projection,
or Unicode bounds. Preserve explicit matches first and add exhaustive leaf tests.

### 14.8 Observation identity drift

Constructing observations away from the plan/step coordinator can lose revision or narrow range.
Pass typed identities explicitly and compare exact observations.

### 14.9 Artifact behavior changes the primary failure

Refactoring I/O may accidentally propagate directory/write failures. Preserve them as evidence
capture failures and keep deterministic filenames/content tests.

### 14.10 Module cycles and visibility explosion

State, redaction, evaluation, assertion, and failure code share value helpers. Resolve collaboration
through narrow `pub(crate)` functions and the dependency direction in section 8, not public fields
or a catch-all module.

### 14.11 Premature Milestone E architecture

It may appear efficient to add cancellation tokens, execution contexts, event collectors, resource
scopes, or task identities while touching the runner. Those abstractions require Milestone E's
semantics and deterministic model tests. D.7 isolates current policy only.

### 14.12 Oversized replacement coordinator

Moving most of `lib.rs` into `execution.rs` would not improve ownership. Enforce the leaf boundaries
and review target while keeping truly lifecycle-coupled ordering together.

## 15. Acceptance criteria

Milestone D.7 is complete only when:

1. `crates/runtime/src/lib.rs` contains only crate documentation, private module declarations, and
   explicit root-level re-exports, and normally remains below 100 lines.
2. Public errors, results, options, artifacts, control, runner lifecycle, per-test state, operation
   execution, evaluation/assertions, redaction, URL handling, and failure enrichment have the
   explicit owners defined in this plan.
3. No new production module is a renamed replacement monolith, no vague helper module exists, and
   production modules normally remain below the 500-line review target.
4. Existing downstream imports from `webtest_runtime` compile unchanged; public fields, variants,
   derives, display/error behavior, codes, method signatures, defaults, and builder precedence are
   compatible.
5. There is exactly one sequential `Runner` path shared by CLI, editor, and DAP, with one step
   dispatcher and no adapter-specific executor.
6. Browser capability gating, session/context/page ownership, test/step order, failure continuation,
   infrastructure abort, close, restart, duration, and final-binding behavior are directly tested.
7. `RunControl` hook/cancellation/snapshot behavior and its order relative to secrets/events/steps is
   unchanged and directly tested.
8. Every plan operation, expression, type decode, assertion matcher, locator/state conversion,
   timeout, and URL form has focused parity coverage.
9. Provider calls retain exact argument evaluation, schema identity, call context, timeout,
   transport metadata, result binding, event summaries, and secret metadata behavior.
10. Step/run errors retain typed classification, stable codes/messages, and structured failure data;
    no runtime fact is flattened for an adapter.
11. Events retain exact current variants, identities, fields, bounds, and order.
12. Observations retain plan file/revision/test/step/range identity, are recorded only for current
    non-infrastructure failures, and are cleared by a later run.
13. Evidence, semantic inspection, repair hints, source ranges, secondary failures, and artifact
    names/kinds/failure tolerance retain current behavior.
14. Concrete provider/user secrets cannot reach returned results, errors, diffs, events,
    observations, evidence, repair hints, artifacts, or DAP/app projections in the required tests.
15. Debugger values retain their 16 KiB/256-item hard bounds, UTF-8 safety, secret-boundary overlap,
    and visibility distinction from final transferable bindings.
16. Any cleanup defect discovered during characterization is handled through the explicit safety-fix
    or Milestone E deferral rule, never silently preserved as an intended invariant or silently
    changed during extraction.
17. No runtime dependency on syntax, analysis, project, browser-cdp, app/editor/LSP/DAP/WASM, or
    reporter formatting is introduced; `Cargo.toml` needs no new production dependency for the
    decomposition.
18. No Milestone E concurrency, cancellation/deadline, retry, trace, atomic-observation, or DAP
    thread behavior is implemented or prematurely constrained.
19. Focused runtime/app/editor/DAP suites and the full workspace build/test/Clippy/format gates pass,
    including server-only execution without Chrome.
20. The checked-in change contains only the intended runtime decomposition, tests, and status update;
    unrelated user work remains untouched.
