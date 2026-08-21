# Milestone E — Structured Execution and Observability

## 0. Status and dependencies

This specification expands Milestone E in [`future-functionality.md`](./future-functionality.md). It depends on the typed plan/provider model in [`milestone-c.md`](./milestone-c.md), the application lifecycle/bridge in [`milestone-d.md`](./milestone-d.md), and the existing revision-safe observations and DAP `RunControl` hook.

Milestone E changes how operations are scheduled and observed. It must preserve the same compiler, plan, runner, provider, browser, editor, and debugger paths used by sequential execution.

## 1. Outcome

Tests can express bounded parallelism, races, retries, and timeouts without leaking child work or losing cleanup. Every attempt and cancellation remains source-mapped in terminal output, traces, editor observations, and DAP.

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
- `Acquire`/body/`Teardown` resource scopes;
- cancellation propagation and bounded cleanup;
- `--jobs N` test-level concurrency with isolation;
- versioned, serializable execution events and attachments;
- atomic observation replacement and expanded runtime evidence;
- local CLI-to-LSP observation IPC;
- portable trace artifacts and a local viewer;
- DAP threads/frames/scopes/stepping for concurrent and retried work;
- deterministic identities and aggregation rules under concurrency.

## 3. Non-goals

This milestone does not add distributed/remote execution, cross-machine scheduling, shared mutable DSL state, unbounded background tasks, arbitrary user-defined async functions, time-travel debugging, browser video recording, visual snapshot approval, or a hosted trace service. `parallel` is structured syntax, not a general task-spawn primitive.

## 4. Language semantics

### 4.1 Sequence

Test and ordinary block bodies remain ordered sequences. A child begins only after the previous child completes. Failure skips remaining ordinary children and enters enclosing teardown.

```webtest
sequence {
    server { let user = app.create_user(email: "a@example.com") }
    browser { open "/users" }
}
```

Explicit `sequence` is primarily useful inside `parallel`, `race`, `retry`, and reusable fixtures; normal blocks already have sequence semantics.

### 4.2 Parallel

```webtest
parallel {
    sequence { /* branch 0 */ }
    sequence { /* branch 1 */ }
}
```

All direct children start as sibling tasks under one parent scope. The parent waits for every child and every child teardown. Assertion/provider test failures are collected without automatically canceling otherwise healthy siblings. An infrastructure/internal failure cancels siblings because the execution environment may be invalid. Explicit cancellation or enclosing timeout always cancels all children.

The result is successful only if every child succeeds. Multiple failures are preserved as an ordered aggregate by stable child plan order, not reduced to one string.

### 4.3 Race

All children start together. The first child to complete successfully wins. The parent cancels losers and awaits their teardown before completing. A failed child does not win while another child can still succeed. If every child fails, return an ordered aggregate of their failures. A non-recoverable infrastructure/internal failure cancels the race immediately.

Race results may be bound only when all branches have a statically compatible result type. Values from losing/cancelled branches never enter the parent environment.

### 4.4 Retry

```webtest
retry 3 backoff 200ms max 2s {
    expect text("processed").visible
}
```

The integer is the total maximum attempt count and must be positive. Every attempt receives a stable `AttemptId`, fresh child scope, and fresh transient resources. Attempt teardown completes before backoff and the next attempt.

By default, assertion failures, locator/actionability timeouts, and provider errors marked `retryable` can retry. Static/configuration errors, cancellation, internal errors, authentication/schema drift, and non-retryable infrastructure errors cannot. Explicit future policy syntax may narrow retry classes; broadening beyond the safe default is not implicit.

Backoff uses a monotonic clock, is cancellation-aware, and may apply deterministic jitter derived from the execution seed. The final result retains bounded evidence from every attempt.

### 4.5 Timeout

```webtest
timeout 10s {
    /* child subtree */
}
```

The deadline covers child execution plus ordinary waits but not an unlimited cleanup window. At expiry, cancel the child subtree, run teardown under a separate bounded cleanup deadline, then return `TimedOut` with cleanup outcome. A child failure immediately before the deadline remains that failure; timeout wins only when the deadline triggers first according to the scheduler's monotonic clock.

Nested deadlines use the earliest effective deadline. Operations receive remaining time through their execution context and cannot extend the parent deadline.

## 5. Binding and data-flow rules

Concurrent branches start from immutable snapshots of visible transferable bindings. A branch may create local bindings, but siblings cannot read them while running. After `parallel`, branch-local values do not merge automatically. After `race`, only the explicitly typed winner result may be bound.

There is no shared mutable DSL variable in this milestone. Provider-side external effects can still race and are the test author's responsibility; plans and traces make that concurrency explicit.

Static analysis rejects:

- use of a branch-local binding outside its scope;
- parallel access to an exclusively owned runtime resource;
- race branches with incompatible bound result types;
- retry of a scope containing a non-repeatable resource/operation unless its schema marks retry safety;
- unbounded or non-positive concurrency/retry/timeout settings.

## 6. Versioned plan model

The plan adds explicit controlling nodes:

```text
Sequence { children }
Parallel { children, failure_policy }
Race { children, result_type }
Retry { child, attempts, backoff, retry_policy }
Timeout { child, duration, cleanup_timeout }
ResourceScope { acquire, body, teardown }
```

Every node carries:

```text
PlanNodeId
stable child ordinal/path
SyntaxOrigin
SourceRevision
capability/resource requirements
effective timeout policy
```

Plans are deterministic and serializable. Runtime task handles, cancellation tokens, clocks, providers, bridge connections, and browser contexts are injected at execution and never serialized.

## 7. Scheduler and cancellation model

### 7.1 Task tree

Execution forms a tree mirroring plan ownership:

```text
Execution
  -> Test task
      -> control-node task
          -> child operation tasks
              -> owned resource scopes
```

Every task has one parent, cancellation token, effective deadline, stable task path, and event channel. A parent cannot complete until all children and child teardowns complete or are recorded as cleanup failures.

### 7.2 Cancellation

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

Providers and browser operations receive cancellation/deadline context. Dropping a future is not considered sufficient cleanup for processes, bridge calls, pages, or contexts. Each owned host operation defines explicit cancel behavior and bounded completion.

### 7.3 Resource scopes

A resource scope has three phases:

1. acquire;
2. execute body if acquisition succeeded;
3. teardown exactly once if ownership was acquired, regardless of body result.

Teardown failures are retained alongside the primary failure. If the body succeeded and teardown failed, teardown determines the failure class. If both fail, the body remains primary unless the teardown reveals an internal/infrastructure condition of higher severity.

Application processes, bridge endpoints, browser contexts, temporary directories, and future fixtures use this model.

## 8. Test-level concurrency

`webtest test --jobs N` runs at most `N` tests concurrently. `N=1` preserves sequential behavior. `N=0`, overflow, or values beyond a documented safety maximum are configuration errors.

Each test receives isolated browser context, binding environment, artifact namespace, observation collection, and fixture/resource scope. Suite/file/worker fixture sharing is deferred until Milestone F defines lifetimes. A shared Chrome process may host multiple contexts only after the backend passes concurrent context conformance tests.

Fail-fast stops scheduling new tests and cancels running tests only when explicitly configured. Final reporting orders tests by deterministic project/test identity, not completion time.

## 9. Event schema

### 9.1 Envelope

Every event uses a versioned envelope:

```text
Event {
    schema_version,
    execution_id,
    event_sequence,
    timestamp,
    test_id,
    task_path,
    plan_node_id,
    parent_node_id,
    attempt_id,
    source_revision,
    origin,
    kind,
    payload,
}
```

`event_sequence` is a total order assigned by the execution event collector. It records observed runtime order; it is not promised to be identical across runs. Stable task paths and plan ordinals let aggregate reporters render deterministic summaries independent of completion timing.

### 9.2 Required kinds

```text
ExecutionStarted / ExecutionFinished
TestStarted / TestFinished
NodeStarted / NodeFinished
AttemptStarted / AttemptFinished
ResourceAcquired / ResourceReleased
OperationStarted / OperationFinished
AssertionFailed / ProviderFailed / InfrastructureFailed
CancellationRequested / Cancelled / TimedOut
AttachmentCreated
OutputCaptured
CleanupFailed
```

Events are immutable facts. Reporters subscribe through bounded channels or read a persisted stream. A slow optional reporter must not block CDP/provider IO indefinitely; backpressure/drop policy is explicit per reporter and never drops terminal/failure events silently.

## 10. Observations

Observation kinds expand to include assertion diffs, ambiguous/actionability locators, HTTP/provider failures, console/network errors, attempts, timings, timeout/cancellation, and evidence links.

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

`webtest test` sends a versioned batch containing canonical project-relative path/URI, source revision, execution/test/node IDs, exact ranges, observation kind, summary, and artifact references. It never sends arbitrary filesystem paths outside the workspace.

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

Stepping never bypasses `Runner`; it changes `RunControl` scheduling decisions. Debug disconnect triggers structured cancellation and full resource cleanup.

## 14. Architecture and crate responsibilities

- `syntax`/`hir`/`analysis` add explicit control constructs, scope/type rules, and diagnostics.
- `plan` owns serializable control/resource nodes and deterministic task paths.
- `runtime` owns the scheduler, task tree, cancellation, deadlines, environments, resource scopes, and event collector.
- `browser`, provider traits, and bridge transport accept cancellation/deadline context but do not schedule DSL nodes.
- `observation` owns atomic revision-bound batches and evidence references.
- A trace component owns artifact writing/reading schemas; reporters/viewers consume events rather than runtime internals.
- `lsp` owns only local observation-protocol conversion/authentication; `editor` applies revision checks.
- `dap` maps runtime task/frame/value DTOs to DAP without implementing scheduling.
- `app` supplies clocks/executors, parses `--jobs`/trace options, and selects reporters.

## 15. Delivery slices

1. Version the event envelope and introduce stable plan/task paths under sequential execution.
2. Add cancellation/deadline context to browser/provider/bridge operations and explicit resource scopes.
3. Implement `Timeout`, then `Retry`, with deterministic fake-clock tests.
4. Implement `Parallel` and `Race` plus immutable branch environments and failure aggregation.
5. Add isolated test scheduling and `--jobs`.
6. Expand atomic observations and implement authenticated CLI-to-LSP IPC.
7. Implement trace writer/reader, capture budgets, and local viewer.
8. Add DAP task threads, nested frames, values, exception breakpoints, and structured stepping.
9. Add reporters/examples/docs for concurrency, cancellation, retries, and traces.

## 16. Testing requirements

Required coverage includes:

- syntax/HIR/type/plan tests for every control construct and invalid scope transfer;
- deterministic plan/task/attempt identity snapshots;
- model/fake-clock scheduler tests for success, aggregate failure, timeout races, retry policy, cancellation, and cleanup exactly once;
- property tests ensuring no child outlives a completed parent;
- provider/process/browser/bridge cancellation and leaked-resource tests;
- test-level isolation and deterministic final ordering under `--jobs`;
- event schema/golden tests, bounded-channel pressure, and terminal-event retention;
- observation replacement, stale-revision rejection, IPC auth/workspace/path/size tests;
- trace checksum, traversal, truncation, redaction, compatibility, and viewer tests;
- DAP thread/frame/step/exception/disconnect protocol tests;
- real Chrome and app-bridge integration tests under cancellation and concurrency.

Stress tests use deterministic seeds and print the seed on failure.

## 17. Acceptance criteria

Milestone E is complete only when:

1. Parallel/race/retry/timeout semantics match this specification under deterministic scheduler tests.
2. No child process, bridge call, browser context, temporary resource, or runtime task survives parent completion/cancellation in integration tests.
3. `--jobs` preserves per-test isolation and deterministic aggregate output.
4. CLI test runs publish only current-revision observations to a running LSP and a later successful run removes prior diagnostics.
5. A trace reconstructs parallel branches, attempts, failures, cleanup, sources, and evidence without project execution.
6. DAP can pause, inspect, step, continue, and disconnect safely during concurrent/retried execution.
7. Full workspace, browser, bridge-conformance, LSP/DAP, trace, and extension gates pass.

The roadmap acceptance statement is thereby satisfied: concurrent and retried tests remain deterministic, diagnosable, and source-mapped in terminal, trace, and editor.
