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
