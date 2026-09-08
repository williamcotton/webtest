# Milestone E — Structured Execution and Observability

## 0. Status and dependencies

This specification expands Milestone E in [`future-functionality.md`](./future-functionality.md).
Milestone E follows the implemented A through D foundation and the pre-E semantic-ownership and
typed-failure hardening specified by [`milestone-d-11.md`](./milestone-d-11.md); its syntax, plan
nodes, scheduler, IPC, traces, and debugger behavior are not part of the current implemented slice
merely because this document specifies them. It depends on the typed plan and provider contracts in
[`milestone-c.md`](./milestone-c.md), the structured machine-feedback contracts in
[`milestone-c-5.md`](./milestone-c-5.md), the application lifecycle/bridge in
[`milestone-d.md`](./milestone-d.md), D.11's corrected type ownership and typed runtime failure
identity, and the existing revision-safe observations and DAP `RunControl` hook.

Milestone E changes how operations are scheduled, owned, cancelled, and observed. It must preserve the same compiler, plan, runner, provider, browser, editor, and debugger paths used by sequential execution, while establishing generic execution-scope, resource-lifecycle, cancellation, deadline, wait, and event-journal abstractions that later milestones can extend without introducing another runtime architecture. [`milestone-f.md`](./milestone-f.md), [`milestone-h.md`](./milestone-h.md), and [`milestone-i.md`](./milestone-i.md) are forward-compatibility constraints, not dependencies and not authorization to implement their public features early.

### Implementation progress — 2026-09-07

Milestone E is **not complete**. The execution-tree, resource/wait foundations, timeout, public parallel, public race/result paths, public retry, and isolated test-root jobs are implemented:

- Tests and capability blocks lower through explicit `Sequence` nodes. Leaf operations exist only
  in that tree; diagnostic, debugger, and secret-checking traversal is a read-only projection.
- Declaration identities use explicit source identity, name, and duplicate-name ordinal. Node
  identities add structural child path and versioned node kind, independently of file-opening
  order, unrelated declarations, step allocation, and runtime occurrence allocation.
- The sequential runner dispatches the tree recursively and emits parented scope and operation
  occurrences. Browser acquisition is an explicit lexical `ResourceScope` in the plan; a test root includes test cleanup. Descendants interrupted
  by the existing test deadline record cancellation and its causing scope; the root records its
  final outcome after cleanup. Cancellation facts retain their typed reason and causing scope.
- Plan format 9 and runtime semantics 6 version the execution-tree shape and behavior independently. Native builds fingerprint
  resolved configuration, keeping configuration values out of the artifact. Shared validation
  checks tree structure, identities, revisions, capability requirements, and explicit execution
  inputs for detectable drift. There is still no CLI command to execute an emitted plan.
- CLI report/event schema 6 includes typed scope facts, shared typed cancellation reasons, and ordered branch aggregates. This remains the existing event stream,
  not yet E's authoritative bounded journal with replay-safe event identity.
- Runtime observations accumulate privately and commit as a complete batch. Starting another
  run clears prior observations and prevents an older in-flight run from overwriting the newer
  batch. Cross-process publication remains unimplemented.

- The runtime resource registry enforces acquisition acknowledgement, shared/exclusive leases,
  generation identity, and exactly-once terminal teardown. The generic adapter driver has fake
  conformance tests for acquisition/body/release cancellation, readiness barriers, cleanup failure
  aggregation, wait rejection, and one shared cleanup budget.
- Browser contexts use that resource driver. CDP interrupts owned targets before disposing the
  context; structured wait/resource events accompany scope events. Native direct-process calls
  bound output reads and explicitly kill and reap on cancellation, including blocked stdin.
  Direct calls and the application command adapter share the native process-capture driver.
  On Unix, successful calls also terminate background descendants in their owned process group;
  failed cleanup retains a distinct typed cause and any primary process failure.
- Scope cancellation propagates downwards, and operation contexts carry inherited remaining time.
  RunControl cancellation wakes active root waits; DAP disconnect retains its distinct cause.
  Provider-only and browser-owning bodies await cooperative host interruption under the cleanup bound.
- `[timeouts].cleanup` configures an independent positive cleanup budget (default five seconds).
  `describe runtime.configuration` reports it in milliseconds. Paused Tokio clocks verify that
  inherited deadlines only shrink and interruption/teardown share an absolute cleanup deadline.

- `timeout <duration> { ... }` is implemented through the shared syntax/HIR/analysis/plan/runtime
  path in flow, server, and browser domains. Nested deadlines only shrink, child bindings stay
  local, and timeout observations retain the exact control origin and cancellation cause.
  Canonical descriptions, semantic tokens, formatting, and portable compilation use that path.
  `examples/structured-execution` is a self-contained passing fixture for this checkpoint.
- Temporary directories returned by providers, including discarded results, enter the resource
  registry. They belong to the nearest timeout or test scope and emit terminal teardown events
  before their owner finishes. Inner cleanup failure prevents subsequent ordinary steps.

- Active Protocol 1 bridge calls send cancellation and await a terminal acknowledgement under
  a separate cleanup bound. Failed acknowledgement remains a typed cleanup failure; a partial
  frame write invalidates the transport. Reader and fallback cancellation tasks are owned and
  finalized during transport shutdown. Cancellation tests cover a successful sibling call,
  missing acknowledgement, and command-adapter cancellation with blocked stdin.

Remaining work includes complete resource ownership and explicit host interruption across HTTP,
bridge/application startup and lifecycle, non-Unix process trees, and browser sessions;
the authoritative event journal;
trace artifacts/viewer; observation IPC; concurrent DAP behavior; and their conformance/stress
coverage. The acceptance criteria below remain normative and unsatisfied as a whole.

### Checkpoint 2 handoff — 2026-09-06

This checkpoint stops after the timeout and native cancellation/resource work above. Before
implementing real sibling concurrency, decompose `ScopeTree`/`TreeExecution`: their current mutable
stack represents one active branch. Each branch needs its own binding environment, active operation,
local resource ownership, and explicit scope parentage. Share only run services such as identity
allocation, resource/wait registries, and event collection. Do not place the whole execution state
behind an `Arc<Mutex<_>>` and share it across sibling futures.

Remaining foundation gaps include HTTP interruption, startup/application ownership, generic plan
resource scopes, and full host-resource conformance. Browser contexts are currently test-owned;
retry and concurrent branches will need explicit lexical resource acquisition and generation rules.
The synchronous event sink and unbounded event buffer still need replacement by E's bounded
authoritative journal and bounded subscriber projections. Parallel/race/retry, jobs, traces, IPC,
and concurrent DAP have not been implemented.

Checkpoint verification: full workspace tests (including Chrome and LSP/DAP protocol tests), Rust
formatting, warning-free workspace Clippy, portable WASM check, Node/Ruby SDK tests and protocol
conformance, extension compilation/package smoke, and the structured-execution CLI example.

### Checkpoint 3 and parallel continuation — 2026-09-06

Checkpoint 3 (`8cf33801dd7bc3136fb4dd23f11c3cd8f667e877`) is accepted. Its execution-tree
model, immutable `ExecutionServices`, branch-owned `BranchState`, explicit scope parentage,
and structured sibling ownership remain the foundation. Siblings never share a mutable binding
environment, active operation, page/session handle, or resource-ownership stack.

The continuation implements public `parallel` through contextual tokens, typed AST, independently
scoped HIR, static analysis, portable plans, and the shared runner. Direct branches must be capability
or control blocks (1–64). Transferable outer bindings are snapshotted, captures of native handles are
rejected, and branch-local declarations do not escape. Flow-domain browser blocks inside branches acquire their
own lexical context inside any enclosing timeout; analysis rejects concurrent use of an inherited exclusive browser context.
The generic typed resource-access checker remains the shared conflict contract.

The scheduler owns every sibling future and receives bounded, coalescing primary-failure signals.
Infrastructure/internal failures signal before evidence gathering, failure debugger hooks, or slow
resource teardown; failed resource acquisition/readiness uses the same primary-observation hook.
Signals propagate to enclosing schedulers. Test failures allow healthy siblings to continue, while
teardown remains structurally awaited even after cancellation or cleanup timeout. Cancellation
also establishes one monotonic cleanup epoch for existing and late descendants; delayed polling
cannot grant a new cleanup window, and smaller lexical cleanup budgets are inherited.

Runtime outcomes and observations now use the host cancellation vocabulary (`ParentFailed`,
`RaceLost`, `Timeout`, `FailFast`, `DebugDisconnect`, and the other defined reasons); scope facts retain
the causing scope. A primary failure known before cancellation is retained through slow failure
observation and cleanup. `TestResult.branches` recursively preserves every completed child outcome,
including success, failure, cancellation, and typed secondary cleanup failure, in source order.
The top-level outcome is its deterministic severity summary, not a replacement for the aggregate.
An enclosing timeout retains completed child results. JSON and event reports expose the aggregate;
human/concise reporters print branch failures, JUnit retains structured branch data, and DAP output
includes recursive results. Concurrent debugger threads/frames/stepping remain pending.

Focused paused-clock tests cover sibling overlap, independent bindings and resources, prompt primary
failure signalling before slow teardown, acquisition failure, multiple test failures completing out
of order, every typed cancellation reason, enclosing timeout, and terminal events after bounded
cleanup expiry and inherited smaller cleanup budgets. A real Chrome test proves sibling storage
isolation. Syntax/HIR/plan ranges, recovery, invalid captures/resources/bounds, canonical
descriptions, native/WASM parity, and CLI reporter coverage accompany the public feature.

Validation for this continuation: `cargo test --workspace` passed with default test threading, including
Chrome and LSP/DAP tests. Focused runtime/lifecycle, language/description, editor/formatter, and
native/WASM parity suites passed after the final refinements, as did workspace Clippy with
`-D warnings`, Rust formatting, the `wasm32-unknown-unknown` check, and all three tests in
`examples/structured-execution`. A follow-up to intermittent protocol failures reproduced premature
HTTP fixture responses to incomplete request headers. The fixtures now handle connections
independently, wait for complete headers, drain responses, and cancel and join all connection work
on shutdown. Regression tests cover fragmented headers, idle browser preconnections, and shutdown;
protocol timeout diagnostics now retain bounded subprocess stderr. This replaces the earlier
serialized workspace validation workaround. Five consecutive `cargo test --workspace --test protocol`
runs also passed with default threading. The paused-clock sibling concurrency tests still run concurrent
futures within each test.

Race, retry, jobs, full host-resource coverage, the bounded authoritative journal, traces, observation
IPC, and concurrent DAP control remain unimplemented. The acceptance criteria below are still
unsatisfied as a whole.

### Checkpoint 4 continuation — race execution core

Checkpoint 4 (`04008e1`) preserves the accepted branch-local architecture and the passing default
workspace test command. The next slice introduces a distinct `Race` plan node and shared sibling
scheduling policies for all-success and first-success completion. Plan format 7/runtime semantics 4
reject older execution contracts rather than silently interpreting the new node as parallel.

A successful child can win only after its resource and branch teardown finishes. Selection cancels
unfinished alternatives with `RaceLost`, preserves the first cancellation cause, and awaits every
child under the existing inherited cleanup deadline. A failed alternative cannot win while another
can succeed; all-failed races retain the source-ordered aggregate. Infrastructure/internal primary
signals still propagate immediately through nested schedulers. Signals published while polling a
completed sibling are checked before awarding success, and cleanup failure prevents a successful
race outcome even after a provisional winner has been selected.

Recovered alternative failures remain in branch results and execution events. A lexical observation
projection keeps them out of current editor diagnostics without deleting unrelated failures before
or after the race. Interrupted projections preserve already collected observations. Parallel uses
the same completion, ownership, resource, cancellation, and observation paths.

Focused plan and paused-clock scheduler/lifecycle tests cover distinct identity and serialization,
resource conflicts and bounds, first-winner stability, primary signalling during the same poll as
success, every parent cancellation reason, late loser cleanup failure, cleanup expiry, enclosing
timeout before and after selection, nested unhealthy branches, deterministic all-failure reporting,
and recovered diagnostics and binding isolation.

Validation: `cargo test --workspace` passed with default threading, including Chrome and protocol
tests; workspace Clippy with `-D warnings`, Rust formatting, and the portable
`wasm32-unknown-unknown` check passed. Fourteen new focused tests cover the race core; the runtime
lifecycle suite now has 74 passing tests.

This is execution-core progress, not a public language claim. Public `race`/`provide` syntax,
compatible result-type analysis, explicit winner-value binding and reporting, and their description,
formatter, editor and portable compiler coverage remain the next vertical slice. Race tests currently
construct the distinct plan node from compiler-produced parallel branch recipes; the parser and
`describe` still correctly reject/do not advertise the unimplemented public syntax. Retry, jobs,
remaining host-resource conformance, journal, traces, IPC, and concurrent DAP remain pending.

### Checkpoint 5 continuation — public race and winner values — 2026-09-07

Checkpoint 5 (`1f2997f`) is accepted. Public `race { ... }`, `provide <expression>`, and
`let selected [: Type] = race { ... }` now run through the shared contextual lexer/parser, typed
AST, independently scoped HIR, analysis, plan, and execution paths. `provide` evaluates a pure
transferable expression and terminates its branch; it must be the final statement, including through
nested timeout/capability blocks. Provider calls use an ordinary local `let` before `provide`.
Nested parallel branches cannot provide to an enclosing race. Nested races bind their own result
locally and can explicitly provide it to an outer branch.

Bound races require every direct branch to provide a compatible transferable type. Explicit
annotations constrain every result; inference preserves compatible numeric, nullable and compound
types. Captures and browser ownership use parallel's existing generic conflict checks and lexical
contexts. The parent binds only the final winner value after all sibling teardown. Native handle
results are rejected, and runtime transfer carries redaction metadata alongside the value so parent
bindings and debugger views cannot reveal provider secrets. Losing values never enter the parent
environment. The emitted-plan secret checker follows all possible winner expressions, including
nested result bindings, so race cannot bypass literal-secret rejection.

Plan format 8/runtime semantics 5 represent explicit `Provide` operations and typed race result
bindings; readers reject the earlier contracts. CLI report/event schema 6 and DAP branch projections
mark `race_winner` on the final selected branch. All alternatives retain ordered typed outcomes and
cleanup facts. The existing first-success scheduler, cancellation causality, observation recovery,
and bounded structured teardown remain shared with parallel.

Canonical `control.race` and `statement.provide` descriptions include constraints, result rules,
contexts, examples and search/category membership. CST formatting and semantic tokens preserve
contextual names; hover and portable WASM compilation use the same result-type facts and plan.
The installed authoring skill and initializer parity checks are updated. The passing
`examples/structured-execution/race.webtest` demonstrates compatible records and nested values.

Focused coverage now includes lossless partial syntax, AST/HIR/plan origins and binding identity,
invalid result flow/type/native captures, runtime winner-only value transfer and redaction, nested
race/timeout results, plan-result validation, all CLI reporters, emitted-secret rejection, formatter
idempotence, semantic tokens/hover, and native/WASM parity. The checkpoint 5 scheduler tests now
execute actual race source through the public compiler path.

Validation: `cargo test --workspace` passed with default threading, including Chrome and LSP/DAP
protocol tests. The final diagnostic-range refinement also passed the analysis suite. Workspace
Clippy with `-D warnings`, Rust formatting, and the portable `wasm32-unknown-unknown` check passed.
The rebuilt CLI checks and formats all three structured-execution files, runs all five tests
successfully, and describes both public topics. The runtime lifecycle suite now has 77 passing tests.

Milestone E remains incomplete: retry, isolated jobs, full host-resource conformance, the authoritative
bounded event journal, traces/viewer, observation IPC, and concurrent DAP control remain pending.

### Checkpoint 6 continuation — retry execution core — 2026-09-07

Checkpoint 6 (`dfb7617`) is accepted. A distinct `Retry { child, settings }` plan node now executes
through the existing tree. Plan format 9/runtime semantics 6 reject older contracts. Retry settings
bound total attempts to 1–64, with deterministic exponential backoff capped by an explicit maximum
(no greater than 24 hours); zero backoff is supported. Jitter is not implemented.

The shared owned-child driver now serves both sibling branches and sequential attempts. Each attempt
has a fresh `AttemptId`, explicit parent scope, independent transferable binding snapshot, and fresh
generations for resources lexically inside it. Terminal cleanup and scope facts precede backoff and
the next attempt. Proven observation-only browser work can exclusively reuse an enclosing context
without acquiring or releasing it; that handle is handed to one awaited attempt at a time. No native
resource handle is cloned into sibling futures. Nested attempt identities remain distinct from both
their parent attempt and the reused static node/path.

Portable resource/operation repeatability summaries reject unsafe retry plans. Provider calls require
the schema-derived `retry_safe` contract; browser assertions and waits are repeatable, while browser
mutations currently have no repeatability contract. Runtime eligibility is separately restricted to
assertion failures, typed browser assertion/action timeouts, and explicitly retryable application
errors. Decode/evaluation failures, cancellation, control timeouts, internal/infrastructure errors,
and failed teardown cannot retry. Every unrecovered failure in a parallel/race aggregate must qualify;
a retryable severity summary cannot hide another non-retryable child failure.

Backoff uses the generic owned timer/wait registration with inherited absolute deadlines and typed
cancellation. Attempt outcomes and nested branches remain in execution order in the existing child
aggregate; scope facts carry their already-versioned attempt identity. A successful retry recovers
its prior observations while retaining all result/event facts. `provide` transfers only a successful
attempt's explicit value and redaction metadata to its enclosing race. Browser artifacts use an
execution/attempt directory so repeated static steps cannot overwrite earlier evidence. The attempt
bound and existing per-failure evidence limits bound retained attempt evidence; E's authoritative
journal and trace-wide budgets remain pending.

Focused tests cover bounds/serialization/identity, safe and unsafe effects, backoff saturation,
exhaustion, nested attempts, bindings, recovery, all cancellation reasons during execution/backoff,
lexical resource reacquisition, safe enclosing-context reuse, cleanup failure/expiry, prompt fatal
signalling through a retry to sibling schedulers, mixed aggregate eligibility, winner transfer,
separate evidence files, and terminal scope/wait ownership.

Validation: `cargo test --workspace` passed with default threading, including Chrome and LSP/DAP
protocol tests. The focused plan/runtime suites passed (91 runtime lifecycle tests, including 14
new retry cases). Final plan repeatability-summary and validation-order refinements passed the plan
suite. Workspace Clippy with `-D warnings`, Rust formatting, and the portable
`wasm32-unknown-unknown` check also passed.

This is execution-core progress. Public `retry`/`backoff`/`max` parsing, HIR/analysis and native-capture
rules, lexical browser lowering inside public retry, canonical descriptions, formatter/editor/WASM
coverage, and author-facing examples remain the next vertical slice. Tests currently replace explicit
compiler-produced timeout recipe markers with Retry nodes; lexical resource tests use the existing
parallel lowering to place browser contexts inside those recipes. Installed descriptions still do
not advertise public retry. Jobs, remaining host-resource conformance, the bounded authoritative
journal, traces, observation IPC, and concurrent DAP also remain pending.

### Checkpoint 7 continuation — public retry — 2026-09-07

Checkpoint 7 (`f5fefc4`) is accepted. `retry <Int> [backoff <Duration> [max <Duration>]] { ... }`
now lowers through contextual syntax, typed AST/HIR, analysis, and the existing Retry plan node.
It inherits the enclosing capability domain, permits 1–64 total attempts, and keeps declarations
local. Omitted backoff means zero delay; omitted max means constant delay. Explicit zero is legal
for retry delays while ordinary duration values/deadlines remain positive. A supplied max must be
at least the initial delay, and both are bounded to 24 hours. Settings have exact source origins and
shared type facts for editor hover.

Compiler checks reuse the plan's resource/operation repeatability summaries and provider schema
`retry_safe` facts, with source-mapped errors and canonical reference queries. Native captures are
rejected at attempt boundaries, including nested retries. Provider-schema changes invalidate these
checks without source edits. Browser blocks declared inside retry lower to lexical resource scopes;
observation-only work inside an enclosing browser block retains its context. Runtime handoff now
consults explicit subtree resource requirements, so a retry with its own lexical context does not
inherit an unrelated native context kept alive by an earlier browser block.

The checkpoint 7 lifecycle suite now executes real retry source rather than rewriting timeout
recipes. Additional coverage checks coexistence of outer and per-attempt contexts, contextual names,
partial/invalid syntax, AST/HIR/plan origins and binding identities, bounds/defaults, unsafe effects,
native captures, provider-schema invalidation, formatter idempotence, semantic tokens/hover, portable
WASM parity, and all CLI reporters. Human/concise output identifies the attempt ID; structured
reports retain the existing typed attempt scope and ordered outcomes. Plan format 9/runtime
semantics 6 and report/event schema 6 remain unchanged because the execution contracts already
represent retry.

`control.retry` documents syntax, contexts, settings, safe effects, eligible failures, cancellation,
resource ownership and canonical examples. Provider descriptions explain explicit retry eligibility.
The installed authoring skill, initializer parity assertions, and structured-execution examples are
updated. Jobs, remaining host-resource conformance, the bounded authoritative journal, traces/viewer,
observation IPC, and concurrent DAP remain pending; Milestone E is still incomplete.

### Checkpoint 8 continuation — isolated test-root jobs — 2026-09-07

Checkpoint 8 (`28cbf76`) is accepted. `webtest test --jobs N` now accepts 1–64 concurrent
roots and defaults to 1. Invalid, zero, overflowing, and above-maximum values fail CLI argument
validation before analysis or execution. The runtime's typed `JobLimit` carries the same bound.
`run_jobs` accepts prepared file runs, admits their test roots in input/plan order, and returns
file/test results in that order even when completion order differs. The CLI supplies deterministic
project discovery order. Nested parallel/race/retry scheduling remains inside each test tree.

Every concurrent root has its own branch state, resource/wait registries, pending observations,
and browser session. Shared services are immutable providers, identity allocation, and event
publication. No execution-state lock or detached task was introduced. A slot stays occupied until
all root teardown finishes, including session close under the same cleanup deadline; session
cleanup failure appears in the test outcome before its terminal event, preserving any primary
failure. `--jobs 1` delegates to the existing sequential runner, preserving file-local session reuse.
Shared native browser processes across test roots remain disabled.

Primary infrastructure/internal failure signals reuse the existing branch notification service to
stop admitting more tests in the affected file before slow teardown completes. Assertion failures
continue admission; already admitted roots are awaited and retain every independent outcome.
Explicit `RunControl` cancellation wakes active tests and preserves typed reasons including
`FailFast`. No CLI fail-fast policy is exposed yet. Final run failure summaries select a stable
source-ordered failure and retain cancellation whether it finishes before or after an independent
abort, while the complete test/branch aggregates retain all failures.

Observation collection is private to each root, merged in test order, and committed once per file
through the existing latest-execution gate. Artifact names retain execution/test/step and attempt
identity, including when different files use the same local IDs. Human progress correlates tests
by `(ExecutionId, TestId)` and uses complete browser status lines during concurrent execution;
final human, concise, JSON, events, and JUnit reports retain deterministic ordering. The existing
plan format 9, runtime semantics 6, and report/event schema 6 already represent these root facts.

Focused tests cover a global bound across files, teardown before slot reuse, independent bindings
and observations, atomic publication, early failure notification, full infrastructure aggregates,
typed cancellation and awaited cleanup, sequential session reuse, nested parallel independence,
and artifact isolation. CLI tests include a two-file HTTP barrier, argument bounds, reporter
ordering, and native Chrome storage isolation. The test declaration description, CLI help,
installed authoring skill/parity assertions, and structured-execution example instructions are
updated. Complete host-resource conformance, the authoritative journal, traces/viewer,
observation IPC, and concurrent DAP remain pending; Milestone E is still incomplete.

Checkpoint verification: `cargo test --workspace` passes with default test threading, including
102 runtime lifecycle tests (10 focused jobs tests), native Chrome isolation, and LSP/DAP protocol
tests. `cargo clippy --workspace --all-targets -- -D warnings`, Rust formatting, and the portable
WASM check pass. All seven structured-execution examples pass with `--jobs 2`; their static and
format checks and the updated test declaration description also pass.

### Worker-owned application continuation — 2026-09-07

For concurrent runs with owned `[app]`, the composition root now starts up to one application
per job and lends each worker's provider registry and runtime URL options to one test root at a
time. The worker remains occupied through test teardown and reuses its application for later
tests. Each process receives `WEBTEST_WORKER_ID`, `WEBTEST_APP_PORT`, and `WEBTEST_APP_URL`;
applications must bind the injected endpoint. Matching loopback browser/server/HTTP-adapter and
health URLs are rebased to that endpoint with their paths preserved. Socket bridges use independent
existing authenticated lifecycles. No execution-state sharing or nested scheduling change is added.
All applications are shut down after the roots finish, including workers started before another
worker's startup failure. Jobs 1 retains the existing application lifecycle. Unowned applications
remain shared; owned command/stdio adapters are explicitly rejected for concurrent workers.
Absolute source URLs and external databases are not rewritten/provisioned.

A native regression exercises bridge writes, HTTP reads, browser reads, application reuse, and
closed worker listeners after both success and partial startup failure. The F# demo now reads
`WEBTEST_APP_URL` with its existing 5055 fallback; after rebuilding with .NET 9, its original
18-test suite passes with `--jobs 5`.

Worker continuation verification: full workspace tests, warning-free workspace Clippy, Rust
formatting, and portable WASM compilation pass, including the native worker routing and partial
startup cleanup regression.

### Native journal identity continuation — 2026-09-07

The runtime collector now retains `RecordedEvent` facts with native envelope version 1,
typed `(ExecutionId, EventSequence)` identity, separate wall-clock/monotonic elapsed timestamps,
and the original typed event payload. Sequences start at zero independently for each execution
and represent observed collection order. The collector assigns and retains each record before
publishing it, releases its append lock before invoking a sink, and does not share branch state.
`RunResult.journal` exposes these authoritative native records; existing `RunResult.events` is
now a compatibility projection of their payloads. Record-aware sinks can consume the assigned
identity; existing sinks retain their previous callback through a default projection.

The observation crate owns a bounded replay index. It accepts out-of-order delivery, keeps each
execution in sequence order, treats exact duplicates idempotently without consuming capacity,
and rejects conflicting payloads/timestamps, mismatched execution identities, unsupported native
record versions, and capacity overflow without changing already accepted facts. Deterministic tests
cover these rules, backwards wall clocks, parallel producers, publication after retention, and
agreement between native records and existing events during multi-file jobs.

This is the identity/replay foundation, not completion of section 9. Authoritative retention
budgets and their infrastructure failure policy, bounded live subscribers, the complete serialized
envelope/context projection, and the expanded event vocabulary remain pending. CLI report/event
schema 6 is unchanged. Execution IDs still use the current process-local allocator; durable
cross-process identity must be settled before serialized replay/IPC is exposed. Traces, IPC,
and concurrent DAP remain later slices.

Journal identity verification: full workspace tests (including Chrome, worker application
isolation, and LSP/DAP), warning-free workspace Clippy, Rust formatting, and portable WASM
compilation pass. Existing report compatibility fixtures remain unchanged and pass.

### Bounded native journal and subscription continuation — 2026-09-08

Each native file run now has a configurable `RunnerOptions.journal_max_events` count
budget (default 100,000, minimum 1), including one reserved `RunFinished` record.
Exhaustion latches `JournalOverflow` with the exact first/last rejected identities and
omitted count, stops further admission in that file, and cancels registered active test
roots with `RunnerShutdown` without overwriting an earlier cancellation cause. Existing
branch ownership still awaits all resource teardown. Subsequent omitted events consume
sequence identities, making the gap explicit; the reserved terminal record reports an
aborted run. Original test/branch outcomes and independent cleanup failures remain in
results. CLI and DAP expose the typed infrastructure code `journal_capacity_exceeded`
and loss details even when an existing test abort would normally suppress a duplicate
run summary. An exhausted journal is explicitly incomplete, including any omitted
teardown facts; it must not be presented as a complete trace.

`Runner::subscribe` provides bounded asynchronous native projections. Producers use
non-awaiting queue publication after authoritative retention. A full subscriber drains
its accepted prefix, receives one explicit `SubscriberCapacity` marker, and ends; a
collector failure instead closes healthy projections with `JournalCapacity`. Both markers
identify the first rejected record and the relevant capacity. Consumers must resynchronize
from retained results/journals and inspect any authoritative gap. A slow or disconnected
subscriber cannot cancel tests, lose authoritative records, or hold up teardown. Healthy
subscriptions preserve per-execution collection order across runs and close when their
runner and active publishers are dropped. The append service and subscriber queues share
only narrowly scoped service state; no branch execution state is shared.

Deterministic coverage checks exhaustion at every event boundary, reserved terminal
retention, cancellation before slow cleanup, prevention of new job admission, preservation
of earlier failure/cancellation causes, closed and pressured subscribers, repeated runs,
and exact CLI/DAP loss projections. These are native count budgets, not serialized-byte
budgets. Trusted legacy `RunEventSink` callbacks remain synchronous fast hooks; adapters
have not yet migrated to asynchronous subscriptions. Project configuration for journal
budgets, complete serialized envelopes/context and event vocabulary, durable cross-process
identity, traces, IPC, and concurrent DAP remain pending. Native envelope version 1 and
CLI report/event schema 6 are unchanged.

Verification: full workspace tests, warning-free workspace Clippy, Rust formatting,
and portable WASM compilation pass. The workspace run also exposed a serial HTTP
fixture in the editor Chrome test that could block behind an idle preconnection.
The fixture now handles connections independently with owned, awaited tasks, and
keeps an idle connection open deliberately during both browser runs as a regression.

### Project journal budget continuation — 2026-09-08

`webtest.toml` now accepts `[journal].max_events`, a positive integer with default
100000. Project validation rejects zero, negative, non-integer, and overflowing
values before execution; unknown journal keys remain ordinary configuration warnings.
The resolved nonzero count flows through the existing composition root to CLI tests,
worker options, LSP, and DAP. It retains the prior per-file native journal semantics:
all roots, branches, and attempts in one file share the budget, including one reserved
terminal record. Exhaustion stops admission in that file, cancels active roots, and
awaits owned teardown while preserving original results and the explicit missing interval.

`webtest describe runtime.configuration` reports `journal_max_events` and explains
configuration, defaults, overflow, and recovery through shared native/WASM guidance.
The setting also participates in serialized project-input fingerprints without changing
source revisions or lowered test bodies. Canonical agent guidance, initializer parity
assertions, the structured-execution example, and current repository status now describe
the implemented budget rather than leaving it as a native-API-only setting.

Focused coverage exercises invalid configuration, default parity, resolved descriptions
and discovery, CLI overflow and successful reruns under jobs 1 and 2, and configuration
fingerprints. This completes project configuration for native count budgets; serialized
byte budgets, complete event envelopes/context and vocabulary, durable cross-process
execution identity, traces, IPC, and concurrent DAP remain pending. No plan/runtime/report
schema versions change in this slice.

Verification: `cargo test --workspace`, warning-free workspace Clippy, Rust formatting,
and portable WASM compilation pass. The structured-execution example passes all seven
tests with jobs 2 and its documented journal configuration.

## 1. Outcome

Tests can express bounded parallelism, races, retries, and timeouts without leaking child work or losing cleanup. Every attempt and cancellation remains source-mapped in terminal output, traces, editor observations, DAP, and versioned machine output.

```webtest
test "notifications arrive" {
    timeout 30s {
        parallel {
            server {
                retry 3 {
                    let mail = app.latest_email(to: "alice@example.com")
                    expect mail.subject == "Welcome"
                }
            }

            browser {
                open "/dashboard"
                expect text("Account ready").visible
            }
        }
    }
}
```

## 2. Scope

Milestone E includes:

- explicit `Sequence`, `Parallel`, `Race`, `Retry`, and `Timeout` plan nodes;
- generic `Acquire`/ready/body/`Teardown` resource scopes with runtime identity, ownership, generation, access, and lifecycle state;
- cancellation propagation and bounded cleanup;
- scheduler-owned deadline and wait registrations suitable for later event-source waits;
- `--jobs N` test-level concurrency with isolation;
- versioned, serializable execution events and attachments;
- atomic observation replacement and expanded runtime evidence;
- local CLI-to-LSP observation IPC;
- portable trace artifacts and a local viewer;
- DAP threads/frames/scopes/stepping for concurrent and retried work;
- deterministic identities and aggregation rules under concurrency.

## 3. Non-goals

This milestone does not add distributed/remote execution, cross-machine scheduling, shared mutable DSL state, unbounded background tasks, arbitrary user-defined async functions, reactive event selection, event-pattern matching, test cases/variants, actors, repeat execution, fixtures or modules, explicit pages/frames/downloads, dialog or route policies, browser-state artifacts, time-travel debugging, browser video recording, visual snapshot approval, or a hosted trace service. `parallel` is structured syntax, not a general task-spawn primitive. The generic resource machinery is exercised with existing host resources and fake conformance resources; it does not expose a generic user-defined resource API.

`race` is structured concurrency: it runs child computations and chooses the first successful completion. It does not subscribe to event sources or dispatch an event handler. A future event-selection construct may reuse E's cancellation, deadline, and wait infrastructure, but it has distinct syntax, plan semantics, and failure behavior.

## 4. Language semantics

### 4.1 Sequence

Test, capability, fixture, and ordinary control-branch bodies remain ordered sequences and lower to explicit `Sequence { children }` plan nodes. A child begins only after the previous child completes. Failure skips remaining ordinary children and enters enclosing teardown.

A dedicated `sequence {}` surface form is not required for this milestone because ordinary blocks already have sequence semantics. Concurrent constructs treat each direct child block as one branch; that branch's body lowers to `Sequence` when it contains multiple statements. Grammar work may add named or explicit branch syntax later only if it represents a distinct authoring need. The plan node is normative regardless of surface spelling.

### 4.2 Parallel

```webtest
parallel {
    browser { /* sequential branch 0 */ }
    browser { /* sequential branch 1 */ }
}
```

All direct children start as sibling tasks under one parent scope. The parent waits for every child and every child teardown. Assertion/provider test failures are collected without automatically canceling otherwise healthy siblings. An infrastructure/internal failure cancels siblings because the execution environment may be invalid. Explicit cancellation or enclosing timeout always cancels all children.

The result is successful only if every child succeeds. Multiple failures are preserved as an ordered aggregate by stable child plan order, not reduced to one string.

### 4.3 Race

```webtest
race {
    browser {
        expect text("Dashboard").visible
        provide "dashboard"
    }

    browser {
        expect text("Verify your email").visible
        provide "verification"
    }
}
```

All children start together. The first child to complete successfully wins. The parent cancels losers and awaits their teardown before completing. A failed child does not win while another child can still succeed. If every child fails, return an ordered aggregate of their failures. A non-recoverable infrastructure/internal failure cancels the race immediately.

Race results may be bound only when all branches have a statically compatible result type. Values from losing/cancelled branches never enter the parent environment.

A race child is a computation that may itself perform waits, actions, assertions, or provider calls. This remains different from a future reactive selection node that registers event sources and dispatches one matching alternative. The two concepts may share cancellation machinery but are never represented by the same plan variant.

### 4.4 Retry

```webtest
retry 3 backoff 200ms max 2s {
    expect text("processed").visible
}
```

The integer is the total maximum attempt count and must be positive. Every attempt receives a distinct execution-scoped `AttemptId`, fresh child execution scope, and new resource generation for resources lexically owned by that child. Attempt teardown completes before backoff and the next attempt. A resource outside the retry scope survives an inner retry only when analysis and its schema prove that repeated child effects are safe.

By default, assertion failures, locator/actionability timeouts, and provider errors marked `retryable` can retry. Static/configuration errors, cancellation, internal errors, authentication/schema drift, and non-retryable infrastructure errors cannot. Explicit future policy syntax may narrow retry classes; broadening beyond the safe default is not implicit.

Backoff uses a monotonic clock, is cancellation-aware, and may apply deterministic jitter derived from the execution seed. The final result retains bounded evidence from every attempt.

`AttemptId` means a retry occurrence, not a generic execution number. Later intentional repeat execution uses a distinct `IterationId` above retry attempts; Milestone E neither defines repeat syntax nor collapses that future identity into `AttemptId`.

### 4.5 Timeout

```webtest
timeout 10s {
    /* child subtree */
}
```

The deadline covers child execution plus ordinary waits but not an unlimited cleanup window. At expiry, cancel the child subtree, run teardown under a separate bounded cleanup deadline, then return `TimedOut` with cleanup outcome. A child failure immediately before the deadline remains that failure; timeout wins only when the deadline triggers first according to the scheduler's monotonic clock.

Nested deadlines use the earliest effective deadline. Operations receive remaining time through their execution context and cannot extend the parent deadline.

The `Timeout` scope records `TimedOut`. Descendants interrupted because that ancestor expired record `Cancelled` with the timeout cause and causing scope identity; a descendant records its own `TimedOut` only when its own effective deadline triggers first. These are distinct runtime facts even when a human reporter summarizes them together.

## 5. Binding and data-flow rules

Concurrent branches start from immutable snapshots of visible transferable bindings. A branch may create local bindings, but siblings cannot read them while running. After `parallel`, branch-local values do not merge automatically. After `race`, only the explicitly typed winner result may be bound.

There is no shared mutable DSL variable in this milestone. Provider-side external effects can still race and are the test author's responsibility; plans and traces make that concurrency explicit.

Static analysis rejects:

- use of a branch-local binding outside its scope;
- parallel access to an exclusively owned runtime resource;
- race branches with incompatible bound result types;
- retry of a scope containing a non-repeatable resource/operation unless its schema marks retry safety;
- unbounded or non-positive concurrency/retry/timeout settings.

Analysis computes subtree effect and resource-access summaries at the shared HIR/plan boundary. Milestone E needs only the effects and resources it implements, but the conflict checker must accept later typed resource accesses without being replaced by resource-specific parallel analyzers. Concurrent shared access is allowed only when the resource contract permits it; mutable or order-sensitive access is exclusive by default.

## 6. Versioned plan model

Milestone E extends the existing `PlanEnvelope` rather than creating a remote- or adapter-specific wrapper:

```text
PlanEnvelope {
    format_version,
    runtime_semantics_version,
    compiler_version,
    project_identity,
    source_files_and_revisions,
    project_input_fingerprint,
    required_host_capabilities,
    provider_schema_hashes,
    tests,
}
```

`format_version` identifies the serialized shape; `runtime_semantics_version` identifies the execution behavior required by its control/resource nodes. `project_input_fingerprint` deterministically covers semantic compile inputs not already represented by source revisions or provider-schema hashes, including relevant resolved project configuration, without embedding secret values. Readers reject unsupported format/runtime semantics, missing host capabilities, and detectable input drift before execution rather than guessing compatibility. This contract benefits local emitted plans and native/WASM parity; it does not add remote submission or worker negotiation to E.

The plan distinguishes structural/control nodes, resource scopes, and existing leaf operations:

```text
PlanNode
  structural/control
    Sequence { children }
    Parallel { children, failure_policy }
    Race { children, result_type }
    Retry { child, attempts, backoff, retry_policy }
    Timeout { child, duration, cleanup_timeout }

  resource scope
    ResourceScope { resource_kind, acquire, body, teardown, access_policy }

  leaf operation
    Eval / ProviderCall / BrowserOperation / Assert
```

Every test body has an explicit root `Sequence`, including a body containing one or zero operations. The executor dispatches structural and resource protocols generically; it must not assume that every non-leaf node is one of the five E control constructs. Later milestones may add typed semantic scope nodes such as actor, page, frame, dialog, route, guard, or fixture scopes by lowering them through this ownership protocol rather than creating another executor. Those nodes and their surface syntax remain outside E.

Every node carries:

```text
PlanNodeId
stable child ordinal/path
SyntaxOrigin
SourceRevision
capability/resource requirements
timeout policy
```

`PlanNodeId` is static identity derived from stable test/declaration identity, the node's structural child path, and versioned node kind. It must not derive solely from lowering order, a process-global counter, a display name, pointer identity, runtime scheduling, or completion time. Sibling ordinals are semantic/source order and remain available even when their runtime events interleave.

Plans are deterministic and serializable. They may contain semantic resource references, acquisition/teardown operations, capabilities, and access requirements. Runtime task/resource handles, generations, cancellation tokens, clocks, providers, bridge connections, files, sockets, processes, and browser backend IDs are injected at execution and never serialized.

Plan evolution must leave room for a distinct future reactive node, conceptually:

```text
Select { event_sources, alternatives, deadline }
```

Milestone E does not serialize or execute `Select`; this shape records the compatibility constraint that event selection must not be encoded as `Race` or as an opaque callback.

## 7. Scheduler, ownership, and cancellation model

### 7.1 Execution-scope tree

Execution forms a lifetime/ownership tree mirroring plan structure, not merely a collection of Tokio tasks:

```text
Run ExecutionScope
  -> run-owned resource                     application/bridge process
  -> TestExecution
      -> Test ExecutionScope                root Sequence
          -> ExecutionScope                 control/resource child
              -> child computation          Parallel / Race
              -> OperationExecution         leaf
              -> registered deadline/wait   Timeout / Retry backoff
              -> owned runtime resource
```

Every non-root scope has one parent; every scope has a source `PlanNodeId`, cancellation token, effective deadline, stable task path, resource set, and event context. A parent cannot complete until every child has reached a terminal outcome and every acquired child resource has completed bounded teardown. The same tree later accepts fixture, actor, guard, page, frame, route, dialog, download, and other resource-heavy scopes without changing the scheduler's ownership rules.

### 7.2 Static and runtime identities

Static identity and runtime occurrence identity are distinct:

```text
ExecutionId               one Runner invocation/run
TestExecutionId
PlanNodeId                 static compiled node
ExecutionScopeId           runtime occurrence of a structural/resource node
AttemptId                  runtime retry occurrence
OperationExecutionId       runtime occurrence of a leaf
RuntimeResourceId          semantic runtime resource entry
ResourceGenerationId       one acquisition incarnation
```

All runtime IDs are typed, unique and stable for the lifetime of one execution and serializable into events/traces. A single `PlanNodeId` may correspond to multiple scope, operation, attempt, and resource-generation IDs because of retry and, later, repeat or reacquisition. Runtime resource generations are allocated by the runtime and associated with an owning scope; their numeric encoding must not freeze today's hierarchy or conflate retry, future iteration, and acquisition identities.

### 7.3 Deadlines and wait registrations

Deadlines and waits are scheduler-owned registrations, not necessarily spawned tasks. Registration, readiness, cancellation, and cleanup use a bounded protocol so a control node can suspend on time or a future event source without leaking a detached future. Every blocking provider, browser, process, bridge, timer, and retry-backoff operation must either observe cancellation directly or install an interrupt/wakeup registration; scattered subsystem-specific `select` loops are not the ownership model.

Internally, deadlines are absolute monotonic instants. The effective deadline is the earliest of the inherited scope deadline, local control-node deadline, operation deadline, and resource/policy deadline. Cleanup runs under its separate bounded cleanup deadline and cannot be made unbounded by a child.

Milestone E implements deadlines and proves the generic wait contract with fake sources. Milestone H may reuse it for reactive waits, but browser/console/network/WebSocket event journals, checkpoints, guards, and public event-selection syntax remain deferred. The serialized execution-event stream in section 9 records facts after they occur and is not itself an event source on which a reactive control node waits.

### 7.4 Cancellation

Cancellation is idempotent and carries a reason:

```text
user_cancelled
parent_failed
race_lost
timeout
debug_disconnect
fail_fast
runner_shutdown
```

Cancellation propagates from parent to descendants, wakes registered waits, stops new child operations, asks owned host operations to interrupt, and enters teardown. Providers and browser operations receive cancellation/deadline context. Dropping a future is not considered sufficient cleanup for processes, bridge calls, browser work, files, or other owned resources; each host operation defines explicit interrupt behavior and bounded completion.

For `race`, selecting a successful winner is not the parent's terminal point. The parent requests cancellation of every loser, waits for their registered waits and resources to finish bounded cleanup, records any cleanup failures, and only then completes. Dropping losing futures is non-conforming.

### 7.5 Generic runtime resource registry

The runtime maintains typed entries for acquired or acquiring resources:

```text
RuntimeResourceEntry {
    resource_id,
    generation_id,
    owner_scope_id,
    resource_kind,
    acquisition_state,       Acquiring / Ready / Failed
    capability_set,
    access_state,            Shared / Exclusive leases
    cancellation_state,
    teardown_state,          Pending / Releasing / Released / Failed
    redacted_debug_metadata,
}
```

Acquisition completion is an ordering guarantee: the resource or policy is fully ready, including any required backend acknowledgement/barrier, before its body starts. A sleep or race with body execution is not a substitute. Future resource kinds may add typed metadata and backend handles behind protocol-neutral traits; plan execution never downcasts a generic entry to CDP or another adapter type.

Every lease is owned by a scope and generation. Shared access is permitted only when the resource contract declares it safe; mutable or order-sensitive resources require exclusive access. Static analysis rejects provable conflicts, and the runtime validates leases as an invariant rather than relying on scheduler timing.

Application processes, bridge endpoints, browser contexts, temporary directories, and fake conformance resources exercise this model in E. Later fixtures, actors, pages, downloads, route/dialog scopes, and browser-state resources extend it rather than introducing a parallel registry.

### 7.6 Async teardown and outcome aggregation

A resource scope has four phases:

1. begin acquisition;
2. await ready/acknowledged acquisition;
3. execute the body if acquisition succeeded;
4. cancel if necessary and teardown exactly once if ownership was acquired.

Teardown is an explicit asynchronous phase, not only `Drop`. It runs on success, test failure, timeout, parent cancellation, race loss, fail-fast, debug disconnect, and runner shutdown. A parent records a terminal resource outcome before completing even when cleanup exceeds its bound or fails.

Execution outcomes preserve typed causality:

```text
ExecutionOutcome {
    primary,
    secondary_cleanup_failures,
}
```

If the body succeeded and teardown failed, teardown determines the failure class. If both fail, the body remains primary unless teardown reveals an internal/infrastructure condition that makes a clean semantic result impossible; both facts and their sources remain available. Adapters may emphasize one fact but never concatenate failures into an unstructured string.

### 7.7 Retry generations

Each retry attempt is a fresh child execution generation. Resources lexically inside the retry are reacquired with new `ResourceGenerationId`s; resources outside survive only according to their declared lifetime and retry-safety contract. Attempt teardown must finish, including terminal cleanup events, before backoff or the next attempt begins. No resource handle or wait registration may cross an incompatible attempt generation.

## 8. Test-level concurrency

`webtest test --jobs N` runs at most `N` tests concurrently. `N=1` preserves sequential behavior. `N=0`, overflow, or values beyond a documented safety maximum are configuration errors.

Each test receives isolated browser context, binding environment, artifact namespace, observation collection, and fixture/resource scope. Suite/file/worker fixture sharing is deferred until Milestone F defines lifetimes. A shared Chrome process may host multiple contexts only after the backend passes concurrent context conformance tests.

The jobs scheduler accepts test execution roots, not arbitrary plan nodes. Nested `parallel`/`race` scheduling remains owned by the execution tree within one test; it is not implemented by submitting descendant nodes to the global jobs queue. Later discovery, variant expansion, filtering, sharding, and repeat expansion may determine the roots before jobs scheduling without changing this boundary.

Fail-fast stops scheduling new roots and cancels running tests only when explicitly configured. Final reporting orders tests by deterministic project/test identity, not completion time. Scheduling order, observed event order, and final presentation order are separate concepts.

## 9. Event schema

### 9.1 Envelope

Every event uses a versioned envelope:

```text
Event {
    schema_version,
    execution_id,
    event_sequence,
    timestamp,
    execution_context: {
        test_execution_id,
        test_id,
        task_path,
        scope_id,
        parent_scope_id,
        plan_node_id,
        attempt_id,
        operation_execution_id,
    },
    source_revision,
    origin,
    kind,
    payload,
}
```

`event_sequence` is monotonic and unique within one `ExecutionId`, assigned centrally by that execution's authoritative event collector. The pair `(execution_id, event_sequence)` is the immutable event identity. Re-delivery of the same identity and payload is idempotent; reuse of one identity for different payloads is schema corruption. No total order is promised across distinct `ExecutionId` streams.

The sequence records observed runtime order within that execution; it is not promised to be identical across runs. Source/semantic order, observed event order, and wall-clock time remain separate. Stable task paths and plan ordinals let aggregate reporters render deterministic summaries independent of completion timing.

Optional context fields are absent when they do not apply. Resource IDs and generations belong in typed resource event payloads; later milestones may add optional typed identities such as `TestVariantId`, `IterationId`, actor/page/frame/download/route IDs, or guard/select IDs without making browser-specific fields mandatory for every E event.

### 9.2 Required kinds

```text
ExecutionStarted / ExecutionFinished
TestStarted / TestFinished
ScopeStarted / ScopeFinished
AttemptStarted / AttemptFinished
WaitRegistered / WaitReady / WaitCancelled
ResourceAcquireStarted / ResourceReady / ResourceAcquireFailed
ResourceReleaseStarted / ResourceReleased / ResourceReleaseFailed
OperationStarted / OperationFinished
AssertionFailed / ProviderFailed / InfrastructureFailed
CancellationRequested / Cancelled / TimedOut
AttachmentCreated
OutputCaptured
```

Events are immutable facts. The authoritative journal is the source for traces and complete post-run projections; live reporters, editor IPC, and other subscribers are bounded projections and do not determine execution semantics. Runtime/browser/provider protocol-read loops never await an optional consumer indefinitely. Subscriber overflow or truncation is explicit, and terminal/failure/resource-outcome events are never silently dropped. If the authoritative collector cannot retain a required semantic event within its configured bound, execution fails with a structured infrastructure outcome rather than continuing with a falsely complete trace.

## 10. Observations

Observation kinds expand to include assertion diffs, ambiguous/actionability locators, HTTP/provider failures, console/network errors, attempts, timings, timeout/cancellation, and evidence links. Existing C.5 diagnostic codes, semantic details, bounded repair hints, source identity, and redaction remain typed fields as observations flow into events, traces, editor services, and DAP; the scheduler does not flatten them into strings.

The runtime accumulates observations per file/revision/execution. Completion atomically replaces the current observation set for that file/revision. Starting a new run marks previous observations stale/cleared immediately; a successful run leaves no old failures.

Adapters publish only when `SourceRevision` equals the current document revision. Stale facts may remain in a trace but never appear as current editor diagnostics.

## 11. CLI-to-LSP observation IPC

### 11.1 Endpoint discovery

The LSP process creates one workspace-scoped local endpoint and a metadata file under a user-private runtime directory:

```json
{
  "protocol": 1,
  "pid": 1234,
  "workspace_id": "blake3:<canonical-root>",
  "endpoint": "<socket-or-pipe>",
  "token": "<random-secret>",
  "created_at": "..."
}
```

The metadata file and endpoint are readable only by the user. TCP fallback is loopback-only. The LSP removes owned metadata/endpoints on shutdown; clients validate PID/workspace freshness and safely ignore stale entries.

### 11.2 Publication

`webtest test` sends a versioned batch containing canonical project-relative path/URI, source revision, execution/test/scope/node/attempt/resource identities as applicable, exact ranges, observation kind, summary, and artifact references. It never sends arbitrary filesystem paths outside the workspace.

The receiver validates authentication, protocol, workspace membership, path canonicalization, size limits, and current source revision before atomically storing the batch. Rejection affects editor publication but does not change the already completed test result.

This protocol is independent from the App Bridge Protocol even if both reuse framing utilities.

## 12. Trace artifact

### 12.1 Layout

A trace is a versioned directory or deterministic archive:

```text
trace.webtest-trace/
├── manifest.json
├── events.jsonl
├── sources/
├── attachments/
├── screenshots/
├── network/
├── console/
└── snapshots/
```

The manifest records format/compiler versions, execution/project identity, source revisions, configuration fingerprint with secrets removed, browser/provider versions, capture policy, and checksums for contained files.

Sources are included only under configured policy and keyed by revision. Attachment references are relative, normalized, and checksum-verified. Readers reject traversal, symlink, oversized, or malformed entries.

`events.jsonl` is the append-only serialized authoritative event journal plus relative references to immutable artifacts. The trace does not invent a second step/execution representation. Its hierarchy is reconstructed from plan, scope, parent, attempt, operation, and resource causality in the events. Milestones H and I extend this schema with new typed identities and event payloads; they do not replace it.

### 12.2 Capture policy

```toml
[trace]
mode = "retain-on-failure" # off | on | retain-on-failure
sources = true
screenshots = "on-failure"
network = "metadata"
console = true
max_bytes = 104857600
```

Body/DOM/console/process capture is bounded and redacted before persistence. When the total budget is reached, the trace records an explicit truncation event.

### 12.3 Viewer

`webtest trace <artifact>` validates the artifact and serves a local read-only HTML viewer on a random loopback port (or writes static output when requested). The viewer shows test/operation timeline, retries, parallel branches, source, assertions, evidence, console/network metadata, and cleanup.

The viewer consumes only the versioned trace schema. It does not import the runtime or execute project code. Opening a trace must not make external network requests by default.

## 13. DAP behavior

Each concurrently running test/branch is represented by a stable DAP thread derived from task identity. Stack frames mirror nested test/control/resource/operation scopes. Variables expose lexical bindings, provider results, assertion values, attempt metadata, and bounded/redacted evidence.

When one branch reaches a breakpoint, the default is to pause all WebTest-managed tasks at their next safe point so the displayed state is coherent. The stopped event identifies the triggering thread. Continue resumes all; single-thread resume may be added only if providers/browser contexts can guarantee it safely.

Required controls:

- pause and continue;
- step in/over/out across sequence/control scopes;
- restart the current debug execution;
- exception breakpoints for assertion, provider, infrastructure, and internal failures;
- deterministic unverified-breakpoint messages for non-executable or revision-mismatched lines.

Stepping never bypasses `Runner`; it changes `RunControl` scheduling decisions. Debug disconnect requests cancellation through `RunControl` and the same execution-scope tree used by normal timeout/race/fail-fast cleanup. DAP never closes provider/browser resources or implements teardown policy itself.

## 14. Architecture and crate responsibilities

- `syntax`/`hir`/`analysis` add explicit control constructs, scope/type rules, and diagnostics.
- `plan` owns serializable control/resource nodes, stable structural child paths, static `PlanNodeId`s, requirements, and access summaries.
- `runtime` owns execution identities, the scheduler/ownership tree, cancellation, deadlines/wait registrations, environments, resource registry/generations/leases, async teardown, typed outcome aggregation, and the authoritative event collector.
- `browser`, provider traits, and bridge transport accept cancellation/deadline/resource context and implement protocol-neutral acquisition/readiness/interruption/teardown contracts, but do not schedule DSL nodes.
- `observation` owns atomic revision-bound batches and evidence references.
- A trace component owns artifact writing/reading schemas; reporters/viewers consume the event journal rather than runtime internals or reconstructing semantics from logs.
- `lsp` owns only local observation-protocol conversion/authentication; `editor` applies revision checks.
- `dap` maps runtime task/frame/value DTOs to DAP without implementing scheduling.
- `app` supplies clocks/executors, parses `--jobs`/trace options, and selects reporters.

## 15. Delivery slices

1. Replace flat sequential plan execution with the recursive execution-tree IR. Lower every existing body through an explicit root `Sequence`, assign stable structural `PlanNodeId`s, and preserve all current sequential behavior.
2. Introduce the typed static/runtime identity hierarchy and serialize it in plan/event test fixtures without adding concurrency.
3. Refactor sequential execution around generic parented `ExecutionScope`s so scope entry/exit and leaf occurrences are observable before tasks can overlap.
4. Add the generic runtime resource registry, generations, shared/exclusive leases, and fake resource adapter under sequential execution.
5. Add cancellation propagation, interruptible wait registration, and cancellation/deadline context to browser/provider/bridge operations.
6. Add inherited absolute deadlines and a fake monotonic clock; establish a separate bounded cleanup deadline.
7. Add acknowledged acquisition barriers, explicit async teardown, exactly-once lifecycle rules, and typed primary/secondary outcome aggregation.
8. Implement `Parallel` with immutable branch environments, subtree effect/access validation, deterministic aggregation, and existing-resource conformance tests.
9. Implement first-success `Race`; cancel and fully teardown losing scopes before the race completes.
10. Implement `Timeout` through the general deadline/cancellation/resource machinery rather than call-site wrappers.
11. Implement `Retry` with fresh attempt scopes and resource generations, teardown-before-backoff, retry-safety enforcement, and bounded per-attempt evidence.
12. Add isolated test-root scheduling and `--jobs`, keeping it separate from nested plan-node scheduling.
13. Version the authoritative event journal and immutable event envelope around the settled runtime topology; add bounded live-subscriber projections.
14. Implement the trace writer/reader, capture budgets, schema extension rules, and local viewer as consumers of that journal.
15. Expand atomic observations and implement authenticated CLI-to-LSP observation IPC through the same structured facts.
16. Add DAP task threads, nested scope frames, values, exception breakpoints, structured stepping, and `RunControl` cancellation.
17. Complete reporters, examples, `webtest describe` entries, implementation-status documentation, compatibility fixtures, and structured-concurrency stress/property tests.

## 16. Testing requirements

Required coverage includes:

- syntax/HIR/type/plan tests for every public control construct, implicit `Sequence` lowering, and invalid scope transfer;
- plan-envelope golden/compatibility tests for independent format/runtime-semantics versions, deterministic project-input fingerprints, capability validation, and input-drift rejection;
- deterministic structural `PlanNodeId`/task-path snapshots and distinct scope/operation/attempt/resource-generation identity tests;
- sequential-regression tests proving the execution-tree refactor preserves existing operation order, failures, observations, and source ranges;
- model/fake-clock scheduler tests for success, aggregate failure, timeout races, retry policy, inherited effective deadlines, cancellation, and cleanup exactly once;
- fake wait-source tests for registration, readiness, cancellation, and cleanup without introducing public reactive syntax;
- fake resource tests for acquire acknowledgement, exclusive/shared leases, cancellation while acquiring/active/releasing, retry reacquisition, cleanup failure aggregation, and terminal event emission;
- property tests ensuring no task, resource, wait registration, or handler outlives its owner; teardown occurs at most once after successful acquisition; and every terminal resource outcome is represented in events;
- stress cases for timeout around a resource, parallel independent/conflicting resources, a race loser holding a resource, retry around a resource, cleanup failure after a primary failure, and debug disconnect during acquisition/teardown;
- provider/process/browser/bridge cancellation and leaked-resource tests;
- test-level isolation and deterministic final ordering under `--jobs`;
- event schema/golden tests for explicit causality, execution-scoped sequence identity, idempotent duplicate delivery, conflicting-payload rejection, source/event/time ordering, bounded-channel pressure, slow subscribers, explicit truncation/overflow, and terminal-event retention;
- observation replacement, stale-revision rejection, IPC auth/workspace/path/size tests;
- trace checksum, traversal, truncation, redaction, compatibility, and viewer tests;
- DAP thread/frame/step/exception/disconnect protocol tests;
- real Chrome and app-bridge integration tests under cancellation and concurrency.

Stress tests use deterministic seeds and print the seed on failure.

## 17. Acceptance criteria

Milestone E is complete only when:

1. Parallel/race/retry/timeout semantics match this specification under deterministic scheduler tests; `race` selects the first successful child computation and does not act as reactive event selection.
2. All existing tests lower through an explicit root `Sequence` and retain their prior behavior, deterministic plans, exact source ranges, and structured outcomes.
3. The plan envelope versions serialized shape separately from required runtime semantics, fingerprints all semantic compile inputs without embedding secrets, and rejects unsupported or detectably drifted inputs before execution.
4. Static plan identity remains distinct from scope/operation/attempt/resource-generation occurrences, and retry creates a fresh child generation without conflating future repeat identity.
5. No child process, bridge call, browser context, temporary resource, runtime resource, wait registration, or task survives parent completion/cancellation; each acquired resource is torn down at most once and every terminal resource outcome is emitted.
6. Resource acquisition is acknowledged before body execution, access conflicts are rejected or trapped through generic leases, and typed cleanup failures never erase the primary failure.
7. `--jobs` schedules isolated test roots, remains separate from nested structured concurrency, and preserves deterministic aggregate output.
8. CLI test runs publish only current-revision observations to a running LSP and a later successful run removes prior diagnostics.
9. The authoritative event journal preserves hierarchy, causality, attempts, resource generations, semantic order metadata, observed order, and terminal facts without letting slow optional consumers block browser/provider protocol IO indefinitely. `(ExecutionId, event_sequence)` is replay-safe identity, and no cross-execution total order is required.
10. A trace reconstructs parallel branches, attempts, resources, failures, cleanup, sources, and evidence without project execution, and later typed resource/event identities can extend rather than replace its schema.
11. DAP can pause, inspect, step, continue, and disconnect safely during concurrent/retried execution while cleanup remains owned by `Runner`/`RunControl`.
12. Scheduler wait registrations pass deterministic readiness/cancellation/cleanup tests, leaving future event selection, fixtures, actors, pages, downloads, policies, and repeat execution additive rather than requiring `Race`, retry, or resource ownership to change meaning.
13. Full workspace, browser, bridge-conformance, LSP/DAP, trace, and extension gates pass.

The roadmap acceptance statement is thereby satisfied: concurrent and retried tests remain deterministic, diagnosable, and source-mapped in terminal, trace, and editor.
