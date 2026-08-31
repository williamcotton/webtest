# Milestone D.8.1 — Explicit Runtime Outcomes and Truthful Cancellation

## 0. Status and dependencies

**Status: proposed (research baseline verified at `580b5708a20707449b8487f393fc46b11bd6c628`).**

This corrective milestone follows the implemented runtime and CDP decompositions in
[`milestone-d-7.md`](./milestone-d-7.md) and [`milestone-d-8.md`](./milestone-d-8.md). It is the
first part of the D.8 runtime-correctness sequence:

```text
D.8.1 explicit outcomes and cancellation
  -> D.8.2 failure classification
  -> D.8.3 finalization and terminal events
  -> D.8.4 per-test deadlines
```

Those four milestones prepare the existing sequential runner for the structured scheduler in
[`milestone-e.md`](./milestone-e.md). They do not add parallelism, retries, timeout blocks,
cancellation tokens, traces, or CLI-to-LSP IPC.

The current runtime suite passes, but its characterization test
`cancellation_and_snapshot_gating_keep_the_existing_hook_order` proves the bug described here:

- cancellation before the first test returns an empty `tests` list and emits ordinary
  `RunFinished`;
- cancellation after `TestStarted` but before its first step emits `TestFinished { passed: true }`;
- `TestResult.passed` is computed only as `failure.is_none()`;
- `RunResult` has no run outcome and therefore cannot distinguish a complete run from a cancelled
  prefix of one.

The green baseline is evidence of current behavior, not the desired contract.

## 1. Outcome

Cancellation is a first-class terminal outcome and can never be rendered, counted, emitted, or
returned as a pass. Every planned test has a result disposition, every started test has exactly one
terminal event, and every started run has exactly one terminal event.

The target runtime model is structurally equivalent to:

```rust
pub enum TestOutcome {
    Passed,
    Failed(StepFailure),
    TimedOut { timeout: Duration },
    Cancelled { reason: CancellationReason },
    Skipped { reason: SkipReason },
    Aborted { failure: RunError },
}

pub enum RunOutcome {
    Completed,
    Cancelled { reason: CancellationReason },
    Aborted { failure: RunError },
}

pub struct TestResult {
    pub name: String,
    pub outcome: TestOutcome,
    pub duration: Duration,
    pub bindings: BTreeMap<String, Value>,
}

pub struct RunResult {
    pub execution_id: ExecutionId,
    pub outcome: RunOutcome,
    pub tests: Vec<TestResult>,
    pub events: Vec<ExecutionEvent>,
    pub duration: Duration,
}
```

The exact spelling may change to avoid recursive or oversized enum representations, but the
invariants may not be weakened. In particular, a failed test must carry its `StepFailure` in the
same variant, so an impossible `passed = false, failure = None` state cannot be constructed.

`TimedOut` is reserved here so D.8.4 can add a per-test deadline without treating timeout as
cancellation or infrastructure failure. `Aborted` represents an infrastructure or internal
execution failure associated with an active test; D.8.2 defines its classification and D.8.3
defines its cleanup precedence.

`Runner::run` and `run_with_control` should return a terminal `RunResult` for all runtime outcomes,
including browser/provider/internal aborts. Static-analysis and project-composition errors remain
outside the runner. Returning the structured result is necessary to retain the complete event
prefix, partial test results, skipped tests, cleanup facts, and final outcome when execution aborts.

## 2. Research baseline

### 2.1 Current cancellation path

`crates/runtime/src/runner.rs` checks `RunControl::is_cancelled()` before each test and simply
breaks the test loop. It then emits the payload-free `RunFinished`, closes the session, and returns
a normal `RunResult`.

`crates/runtime/src/execution.rs` checks the same boolean before a step and again after the
pre-step debugger hook. Both branches simply break the step loop. The function later computes:

```text
passed = failure.is_none()
```

Cancellation does not create `failure`, so the test is reported as passed. No runtime type records
whether cancellation occurred before a test, while a test was paused, between steps, or between
tests.

### 2.2 Current consumers of the boolean model

| Consumer | Current assumption that must change |
|---|---|
| `crates/observation` | `TestFinished` carries `passed: bool`; `RunFinished` carries no outcome |
| `crates/app/src/commands/test.rs` | maps `test.passed` directly to success/test-failure exit classes |
| `crates/app/src/report.rs` | computes `failed = tests - passed`, so cancellation would be forced into one side |
| `crates/app/src/runtime_output.rs` | event reports expose only an optional `passed` boolean |
| `crates/app/src/test_progress.rs` | prints `ok` for the current false-pass cancellation path |
| `crates/dap` | prints `ok` or `FAILED` and derives its exit code only from `RunResult::failed()` |
| `crates/editor` | exposes `RunResult` and uses pass/fail counts in tests |
| runtime public tests | construct arbitrary `passed`/`failure` combinations |

No adapter should infer cancellation from a short test vector, a DAP shutdown flag, absence of a
failure, or missing events. Runtime owns the outcome fact.

### 2.3 Current DAP behavior

DAP uses `DebugState.shutting_down` as `RunControl::is_cancelled`. A disconnect or terminate request
sets the flag and sends `Continue`, which wakes a paused pre-step hook. The next runtime cancellation
check is therefore reachable, but today it produces a false pass. The DAP server suppresses its
normal exit/terminated events after shutdown; that adapter behavior does not authorize runtime to
discard the cancelled outcome.

### 2.4 Dependency constraint for event DTOs

`observation` cannot depend on `runtime`. Full outcomes containing `StepFailure` or `RunError`
therefore remain runtime types. `observation` may own small protocol-neutral summary enums used by
`ExecutionEvent`, for example `TestOutcomeKind` and `RunOutcomeKind`. Do not create a dependency
from `observation` back to `runtime` or flatten terminal outcomes into strings.

## 3. Required semantics

### 3.1 Cancellation safe points

D.8.1 preserves the current cooperative safe points:

1. before allocating work for the first/next test;
2. before preparing and starting a step;
3. after `RunControl::before_step*` returns;
4. after a debugger pause is released by disconnect/terminate.

It does not claim to interrupt an already-running provider/browser future. D.8.4 bounds such work
with a deadline, and Milestone E adds propagated cancellation tokens and explicit host-operation
cancellation.

### 3.2 Result population

The result vector remains in deterministic plan order and contains one entry per planned test:

- completed tests keep `Passed` or `Failed`;
- the active test at the first observed cancellation becomes `Cancelled`;
- tests that never start because of cancellation become `Skipped { RunCancelled }`;
- the active test associated with an infrastructure/internal failure becomes `Aborted`, and later
  tests become `Skipped { RunAborted }`;
- cancellation before the first test yields no `TestStarted` events, but every planned test still
  has a skipped result;
- an empty plan can complete normally; cancellation of an empty plan produces a cancelled run only
  when the control was already cancelled at the initial safe point.

A run with ordinary failed tests but no cancellation/abort is still `RunOutcome::Completed`.
Completion means the runner reached the end of its planned scheduling policy, not that all tests
passed.

### 3.3 Event ordering

Add explicit terminal payloads and a skipped-test event:

```text
TestFinished { ..., outcome: Passed | Failed | TimedOut | Cancelled | Aborted }
TestSkipped  { ..., reason }
RunFinished  { ..., outcome: Completed | Cancelled | Aborted }
```

Required examples are:

```text
cancel before test:
RunStarted -> TestSkipped* -> RunFinished(Cancelled)

cancel after TestStarted, before StepStarted:
RunStarted -> TestStarted -> TestFinished(Cancelled)
           -> TestSkipped* -> RunFinished(Cancelled)

cancel between tests:
... -> TestFinished(Passed|Failed) -> TestSkipped* -> RunFinished(Cancelled)
```

`TestSkipped` does not require a preceding `TestStarted`. Every `TestStarted` must have exactly one
`TestFinished`. D.8.3 strengthens the terminal-event rule by moving cleanup before these events.

### 3.4 Counting and adapter behavior

Replace arithmetic derived from booleans with exhaustive methods:

```text
passed()
failed()       # ordinary assertion/value/browser/provider test failures only
timed_out()
cancelled()
skipped()
aborted()
```

Human and concise output uses `ok`, `FAILED`, `TIMED OUT`, `CANCELLED`, `SKIPPED`, or `ABORTED`.
DAP must never print `ok` for a cancelled test and must return a nonzero debug exit status for a
cancelled or aborted run unless the DAP disconnect path intentionally suppresses process-exit
events.

JUnit maps never-started skipped tests to `<skipped>`, ordinary failures/timeouts to `<failure>`,
and infrastructure/internal aborts to `<error>`. JSON and JSONL expose an `outcome` string rather
than coercing every state into `passed: bool`.

This is a report-schema change. Increment the machine report/event schema version, update the
golden fixtures, and document the compatibility break. Do not silently change the meaning of
schema version 1.

## 4. Implementation scope

### 4.1 Runtime and observations

- replace `TestResult.passed` plus optional failure with one exhaustive outcome;
- add `RunResult.outcome` and return terminal run results for execution aborts;
- replace `RunResult::failed = len - passed` with exhaustive counters;
- add typed cancellation/skip reasons with a small initial set;
- update runner/execution control flow to produce cancelled and skipped results explicitly;
- update `ExecutionEvent` terminal variants with protocol-neutral outcome summaries;
- keep full structured failures in runtime results rather than duplicating them into event enums.

### 4.2 App, editor, and DAP

- map outcomes exhaustively in CLI reports, progress, JSON/events, JUnit, and exit classes;
- keep editor `RunResult` revision behavior unchanged while exposing the richer result;
- remove DAP's boolean result inference and render cancelled/skipped/aborted states distinctly;
- update all root-public-API tests and downstream pattern matches so a future enum variant causes a
  compile-time review rather than falling into a default branch.

## 5. Out of scope

- active interruption of an in-flight provider, process, CDP command, or filesystem operation;
- cancellation-token propagation into browser/provider traits;
- cancellation reasons for race losers, fail-fast, retry, or parent task failure;
- parallel scheduling, task trees, trace events, or DAP thread modeling;
- a public DSL cancellation construct.

## 6. Required tests

Add deterministic tests for:

1. cancellation before browser startup and before the first test;
2. cancellation between two tests;
3. cancellation before the first step of a started test;
4. cancellation between two steps after the first has passed;
5. cancellation while paused by DAP, released by disconnect/terminate;
6. no `Passed` outcome or `passed: true` compatibility field anywhere in a cancelled result/event;
7. one result per planned test and exact plan-order skipped results;
8. one terminal event per started test and one per run;
9. exact outcome counts for mixed passed, failed, cancelled, skipped, timed-out, and aborted values;
10. human, concise, JSON, events, and JUnit golden output for cancellation.

## 7. Verification

```sh
cargo test -p webtest-runtime
cargo test -p webtest-observation -p webtest-editor -p webtest-dap -p webtest
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## 8. Acceptance criteria

1. No cancellation path can construct, emit, serialize, print, or count a pass.
2. Every planned test has an explicit result disposition in deterministic order.
3. Every started test and run has exactly one typed terminal event.
4. DAP disconnect while paused produces a cancelled runtime outcome and completes cleanup through
   the shared runner path.
5. Adapters pattern-match outcomes exhaustively and version their changed machine schema.
6. No scheduler, parser, plan-node, provider, browser, or adapter-specific execution path is added.
