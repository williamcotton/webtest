# Milestone D.8.3 — Finalization Before Terminal Events

## 0. Status and dependencies

**Status: implemented.**

**Implementation status (2026-08-31):** Runtime test execution now retains a private provisional
outcome through explicit context and provider-owned temporary-directory finalization, and run
execution retains its provisional outcome through browser-session close. Typed cleanup resources,
causes, I/O kinds, stable codes, bounded messages, severity-aware aggregation, and typed prior test
outcomes survive into runtime results and adapters. `CleanupFailed` is emitted before the relevant
test/run terminal event, context cleanup aborts later scheduling, and sink events remain identical
to `RunResult.events`. Machine report/event schema version 3 adds the structured cleanup event and
resource identity. Fake lifecycle coverage exercises acquisition, pass, ordinary failure,
cancellation, provisional timeout combination, infrastructure/internal abort precedence,
deterministic temporary-resource ownership/de-duplication, and exactly-once close/removal paths;
artifact persistence remains best effort.

This milestone follows the explicit outcomes and failure classes in
[`milestone-d-8-1.md`](./milestone-d-8-1.md) and
[`milestone-d-8-2.md`](./milestone-d-8-2.md). It deliberately precedes the timeout correction in
[`milestone-d-8-4.md`](./milestone-d-8-4.md): a per-test deadline must not be implemented by
dropping the whole `execute_test` future before resource ownership and finalization are explicit.

This is a narrow sequential-lifecycle correction. It does not implement Milestone E's general
resource-scope plan node, cancellation tree, cleanup deadline, or concurrent teardown.

## 1. Outcome

The runtime obeys one terminal-event invariant:

> Once a test or run terminal event is published, no operation capable of changing that outcome
> remains.

Every started test progresses through explicit phases:

```text
acquire test resources
  -> execute body to a provisional outcome
  -> release page/context and owned temporary resources
  -> combine body and cleanup facts into the final outcome
  -> snapshot final transferable bindings
  -> emit TestFinished exactly once
```

The run follows the same rule:

```text
schedule tests to a provisional run outcome
  -> close/reap the browser session and finish runner-owned resources
  -> combine run and cleanup facts
  -> emit RunFinished exactly once
  -> return the identical RunResult
```

No `?`, early return, cancellation branch, or timeout branch may bypass owned-resource finalization
after the corresponding start event has been emitted.

## 2. Research baseline

### 2.1 Test terminal event currently precedes fallible cleanup

At the end of `crates/runtime/src/execution.rs`, the current order is:

1. drop the page;
2. close the browser context, reducing any error to `cleanup_failed: bool`;
3. compute `passed = failure.is_none()`;
4. emit `TestFinished { passed }`;
5. compute final bindings;
6. recursively remove provider-owned temporary directories with `tokio::fs::remove_dir_all`;
7. use `?` to turn a removal failure into `RunError::Provider`.

An event sink can therefore observe `TestFinished { passed: true }` and then see the run future
return an error. The final `RunResult` is lost and no `RunFinished` is emitted.

### 2.2 Context cleanup currently loses its actual failure

`context.close().await.is_err()` discards the typed `BrowserError`. A close failure only sets
`session_tainted`; the current test may still report success. If another test exists, the runner
best-effort closes the old session and starts a replacement. If no later test exists, the close
failure never affects any result or report.

A failed isolated-context disposal means isolation/resource cleanup was not proven. It is an
infrastructure failure, not a successful test with an invisible implementation detail.

### 2.3 Run terminal event currently precedes session close

`crates/runtime/src/runner.rs` emits `RunFinished` and only then calls `session.close().await?`.
Session-close failure can thus turn a supposedly finished run into `Err(RunError::Browser)`.

### 2.4 Early infrastructure return bypasses normal finalization

When `process_failure` returns an infrastructure error, `execute_test` drops the page,
best-effort closes the context, and returns immediately. Temporary directories discovered in the
test environment are not enumerated. `run_with_control` propagates the error with `?`, so normal
session close and `RunFinished` are also skipped. D.7 documented this as a pre-existing sharp edge.

### 2.5 Artifact failures are intentionally different

Artifact directory/write failures are currently appended to `PageEvidence.capture_failures` and do
not replace the primary test failure. Preserve that policy. Evidence persistence is best effort;
browser context, browser session, and provider-owned temporary resource cleanup are lifecycle work
that can change the execution outcome.

## 3. Required lifecycle model

### 3.1 Provisional body outcome

The test body coordinator returns a private provisional value instead of returning from the whole
test:

```text
Passed
Failed(StepFailure)
TimedOut(details)
Cancelled(reason)
Aborted(RunError)
```

Acquisition failures after `TestStarted` are also provisional aborts. If a context was created but
page creation fails, the context is still closed before the test becomes terminal.

The coordinator retains its `TestExecutionState` for all provisional outcomes so nested
`TempDirectory` values can be discovered and cleaned even after an infrastructure/internal step
failure.

### 3.2 Typed cleanup failures

Add a typed cleanup error rather than manufacturing a provider call error or storing a string:

```rust
pub enum CleanupResource {
    BrowserContext,
    BrowserSession,
    TemporaryDirectory { path: PathBuf },
}

pub struct CleanupFailure {
    pub resource: CleanupResource,
    pub cause: CleanupCause,
}
```

`CleanupCause` retains `BrowserError`, `std::io::Error` facts needed for presentation, or an
internal invariant cause without requiring adapters to parse a message. If `std::io::Error` cannot
be cloned for result/event DTOs, normalize it once into typed `kind`, path, and bounded message.

The runtime error model may add `RunError::Cleanup(CleanupFailure)`. Its `FailureClass` is derived
from the typed cause. Temporary-directory I/O and browser close failures are infrastructure; an
impossible ownership/state transition is internal.

### 3.3 Cleanup ordering and exactly-once ownership

For one test:

1. stop executing body steps;
2. drop the page handle;
3. close an acquired context exactly once;
4. enumerate temporary directories from the retained state;
5. de-duplicate normalized owned paths before removal;
6. remove each owned directory exactly once in deterministic path order;
7. calculate transferable bindings from values, not from filesystem existence;
8. combine outcomes and emit terminal events.

Do not recursively delete an unvalidated arbitrary path merely because a provider value contains
`TempDirectory`. Preserve the current provider ownership contract and add tests that all cleanup
targets came from provider results under the configured project/temp policy. Any broader path
hardening belongs in the provider that creates the value.

For one run:

1. stop scheduling tests after cancellation or an infrastructure/internal abort;
2. produce skipped results for unscheduled tests as specified by D.8.1;
3. close the current browser session exactly once if it was started;
4. combine session cleanup with the provisional run outcome;
5. emit `RunFinished` and return.

### 3.4 Outcome precedence

Never hide cleanup failure under a pass. Use the following severity order:

```text
Internal > Infrastructure > TimedOut/Cancelled/Test failure > Passed/Skipped
```

Required combination behavior:

| Body | Cleanup | Final test/run behavior |
|---|---|---|
| pass | clean | passed/completed |
| ordinary test failure | clean | failed; run may continue |
| cancelled | clean | cancelled |
| infrastructure/internal abort | clean | aborted with body cause |
| pass/fail/cancel | infrastructure cleanup failure | aborted as infrastructure |
| any lower-severity body | internal cleanup failure | aborted as internal |
| body abort plus cleanup failure | higher class wins; every typed cause is retained |

When a cleanup failure outranks an earlier ordinary `StepFailure`, retain that step failure as a
typed prior/secondary outcome in the aborted test result. Do not concatenate it into the cleanup
message. If body and cleanup have the same abort class, keep the body cause primary and cleanup in
an ordered typed collection.

A context-close failure aborts the run. The session may still be closed/reaped, but the runtime
does not continue later tests on an execution environment whose isolation cleanup failed. The
existing tainted-session restart behavior is replaced by this explicit outcome; D.8.5 may lazily
start a fresh session only after normal test completion or a test timeout policy that explicitly
allows continuation.

### 3.5 Event ordering

Add a structured `CleanupFailed` execution event. It is emitted after the relevant cleanup attempt
and before the terminal test/run event. It carries the resource identity, typed failure class,
stable code, and bounded message.

Examples:

```text
TestStarted -> ... -> CleanupFailed(context) -> TestFinished(Aborted)
            -> CleanupFailed(session) -> RunFinished(Aborted)

TestStarted -> StepFailed(test) -> CleanupFailed(tempdir)
            -> TestFinished(Aborted) -> RunFinished(Aborted)
```

The terminal event payload must equal the returned outcome summary. Event-sink output and the
`RunResult.events` vector remain identical and in the same order on success, cancellation, timeout,
and abort.

## 4. Implementation shape

The refactor should keep orchestration visible in `execution.rs` and `runner.rs`. A private
`TestResources`/`TestFinalizer` helper is appropriate if it:

- owns only one test's page, context, state, and temporary-resource set;
- has an explicit async `finish` method;
- prevents double close/removal through ownership rather than flags spread across branches;
- does not become a general service locator or hidden scheduler;
- has a defensive `Drop` fallback only for best-effort safety, never as the normal async cleanup
  path.

Likewise, a private run finalizer may own the browser session. It does not belong in `browser-cdp`:
runtime owns when a plan/run is terminal, while the browser adapter owns how a session closes.

## 5. Out of scope

- general `ResourceScope` plan nodes and fixture lifetimes;
- propagated cancellation for an in-flight browser/provider call;
- configurable cleanup deadlines or retrying failed cleanup;
- parallel cleanup, aggregate task failures, or trace attachments;
- making best-effort evidence persistence determine test success;
- changing Chrome process reaping mechanics inside `browser-cdp` except where a focused regression
  proves its `close` contract is incorrect.

## 6. Required tests

Use fake host/session/context/page/provider values and deterministic event sinks to cover:

1. context close failure after an otherwise passing test;
2. context close failure after an ordinary test failure;
3. session close failure after all tests passed;
4. temporary-directory cleanup failure after an otherwise passing provider-only test;
5. temporary-directory cleanup after an infrastructure step abort;
6. page creation failure after successful context acquisition closes that context;
7. no later test starts after context cleanup aborts the run;
8. all resources are closed/removed exactly once on success, failure, cancellation, timeout, and
   abort;
9. `CleanupFailed` precedes terminal events and returned/sink event vectors match;
10. artifact write failure remains secondary evidence and does not replace the primary outcome;
11. body and cleanup causes remain separately typed and severity precedence is exact;
12. no `TestFinished(Passed)` or `RunFinished(Completed)` can precede a returned cleanup failure.

## 7. Verification

```sh
cargo test -p webtest-runtime
cargo test -p webtest-browser -p webtest-browser-cdp
cargo test -p webtest-editor -p webtest-dap -p webtest
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Real-Chrome cleanup tests may skip only when Chrome or loopback sockets are genuinely unavailable;
fake lifecycle coverage must always run.

## 8. Acceptance criteria

1. Test and run terminal events are emitted only after all outcome-changing finalization.
2. Every acquired context/session and every provider-owned temporary directory follows one explicit
   exactly-once cleanup path on every provisional outcome.
3. Cleanup failures are typed, classified, reported, and cannot be hidden under a pass.
4. Early infrastructure/internal failures no longer bypass temporary-resource or session cleanup.
5. Event sinks and returned results describe the same final outcomes even when cleanup fails.
