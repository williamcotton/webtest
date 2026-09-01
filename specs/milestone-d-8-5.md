# Milestone D.8.5 — Per-Test Capabilities and Lazy Browser Allocation

## 0. Status and dependencies

**Status: implemented (research baseline verified at `580b5708a20707449b8487f393fc46b11bd6c628`; implementation completed 2026-09-01).**

**Implementation status (2026-09-01):** Plan format 2 records a sorted capability set on every
test and validates the plan-level union. Analysis records capability use through one per-test/file
helper, and native/WASM plans remain identical. Runtime validates the metadata, associates lazy
browser acquisition with the active test and its deadline, creates contexts/pages only for tests
declaring `Browser`, reuses healthy sessions, and defers replacement after timeout taint until the
next browser test. The app-owned lazy Chrome host preserves configuration and resolution
precedence, caches successful executable resolution, and reports actual process start/close
boundaries. Fake lifecycle, malformed-plan, build JSON, CLI progress, and native/WASM parity tests
cover the acceptance contract.

This milestone follows the runtime outcome/finalization work in
[`milestone-d-8-1.md`](./milestone-d-8-1.md) through
[`milestone-d-8-4.md`](./milestone-d-8-4.md). It adds no language syntax and changes no capability
legality rule. It makes already-computed capability facts precise enough for the runtime's
per-test resource lifetime.

The plan-level capability union remains useful for build metadata, host preflight, and portable
consumers. It is no longer the only capability granularity available to execution.

## 1. Outcome

Every `PlannedTest` carries a deterministic sorted capability set:

```rust
pub struct PlannedTest {
    pub id: TestId,
    pub name: String,
    pub required_host_capabilities: Vec<Capability>,
    pub steps: Vec<PlannedStep>,
    pub origin: SyntaxOrigin,
}
```

The enclosing `TestPlan.required_host_capabilities` is exactly the set union of its tests. A
`PlanEnvelope` retains both the envelope-level union and each serialized test's requirements.

Runtime allocates browser resources only when executing a test whose requirements contain
`Capability::Browser`:

```text
provider-only test -> no Chrome start, context, or page
pure/value test    -> no Chrome start, context, or page
browser test       -> lazily ensure session, then create one context/page
```

A browser session may remain shared across normally completed browser tests, but server-only tests
between them do not receive contexts/pages. A tainted or timed-out session is closed and restarted
only when the next browser test actually needs it.

## 2. Research baseline

### 2.1 Current plan shape

`crates/plan/src/lib.rs` stores `required_host_capabilities` on `TestPlan` and `PlanEnvelope` only.
`PlannedTest` has ID, name, steps, and origin. The plan format is version 1.

### 2.2 Current compiler accumulation

`crates/analysis/src/compiler.rs` owns one `Compiler.required: BTreeSet<Capability>` for the whole
file. Browser operations, provider calls, and assertions insert into that shared set while tests
are compiled. The compiler has enough information to collect per-test requirements, but currently
discards the boundary between tests.

The relevant insertions are distributed across:

- `compiler/browser_operations.rs` for `Capability::Browser`;
- `compiler/provider_calls.rs` for the provider schema's capability;
- `compiler/statements.rs` for `Capability::Test` assertions.

Do not recover test capabilities later by scanning serialized operations in runtime. Analysis is
already the canonical semantic owner.

### 2.3 Current runner allocation

`crates/runtime/src/runner.rs` checks the plan-wide union once and starts a browser session before
the test loop. `execute_test` then creates a context and page whenever the run has a session,
including for tests containing only server/provider or pure/value operations.

Consequences:

- a server-only test before the first browser test waits for and depends on Chrome startup;
- every test in a mixed file pays context/page setup and cleanup;
- context close can taint the session after a test that never used the browser;
- a browser launch failure loses the chance to report earlier independent tests.

### 2.4 Current app pre-resolution

`crates/app/src/commands/test.rs` uses the plan-wide union to resolve a Chrome executable before it
calls `Runner`. Merely moving `browser.start()` inside the runtime loop is therefore insufficient
for the CLI: a missing managed/system browser would still stop a mixed file before its initial
server-only tests run.

Executable resolution and launch must both be lazy while remaining app-owned composition policy.
Do not move browser-manager/configuration resolution into `runtime` or `browser-cdp`.

## 3. Plan and compiler contract

### 3.1 One capability recording helper

During compilation of one test, maintain a local `BTreeSet<Capability>`. Every capability use goes
through one helper that inserts into both the current test set and file/plan set. Reset only the
test set between tests; preserve deterministic capability enum order from `BTreeSet`.

This avoids three invalid implementations:

- cloning the cumulative file set into later tests;
- scanning `PlannedStep` in runtime to infer requirements;
- manually updating plan and test sets in separate branches that can drift.

After compilation, assert/test:

```text
plan.required_host_capabilities
  == sorted union(test.required_host_capabilities for test in plan.tests)
```

Malformed source may still produce a non-executable plan alongside diagnostics. Capability facts
remain deterministic and conservative for the operations analysis recognized.

### 3.2 Plan format compatibility

Adding a required serialized field to `PlannedTest` changes the emitted plan contract. Increment
`PLAN_FORMAT_VERSION` to 2 and update deterministic build fixtures. Do not deserialize a version 1
envelope as version 2 by silently defaulting every test to the plan-wide union; that recreates the
allocation bug and hides missing information.

If explicit version-1 migration is desired, put it in a version-aware decoder that derives each
test's capabilities from version-1 operations once and validates the envelope union. The current
product only emits plans and has no production plan-execution command, so migration is optional for
this milestone; false compatibility is not.

WASM compilation continues to expose the plan-level union for host-capability metadata and now also
serializes per-test requirements in its returned plan. Native/WASM plan parity must remain exact.

## 4. Runtime resource semantics

### 4.1 Lazy session acquisition

Before one test body begins, runtime checks that test's capability set. A browser test lazily
acquires a session if none exists. The acquisition is associated with that test's started/aborted
outcome so a launch failure has a clear identity and later tests receive skipped dispositions from
D.8.1.

Server/pure tests never call:

- `BrowserHost::start`;
- `BrowserSession::new_context`;
- `BrowserContext::new_page`;
- browser context/session restart logic.

`execute_test` should reject a malformed plan in which a browser operation exists without the
Browser capability as an internal failure. It must not silently allocate by rescanning the step and
thereby mask an analysis/plan invariant bug.

### 4.2 Session reuse and taint

After a normal browser test, keep the session for a later browser test. Server-only tests may run
while the healthy session remains idle, but receive no page/context handle.

After a timeout or another explicitly recoverable taint policy from D.8.4, close the session. Do not
start its replacement until the next Browser-capable test. An infrastructure/internal cleanup abort
from D.8.3 still aborts the run rather than attempting to hide the failure through restart.

Normal run finalization closes a started session exactly once. A plan with no Browser-capable tests
never starts or closes one even if its enclosing/malformed plan union incorrectly claims Browser;
the invariant mismatch should instead be rejected as internal during plan validation.

### 4.3 Lazy app composition

Add an app-owned lazy browser host/resolver that captures the resolved project configuration,
explicit `--chrome-path`, headed setting, managed-browser policy, and command/navigation timeouts.
Its first `start()` resolves the executable and constructs `ChromeHost`; later starts can reuse the
resolved executable while still creating a new owned Chrome process/session.

The wrapper belongs in `crates/app`, because only the composition root may combine project config,
browser-manager resolution, CLI precedence, and `ChromeHost`. It implements the protocol-neutral
`BrowserHost` trait and exposes no app type to runtime.

Human progress must not print "starting Chrome" before earlier provider-only tests. Either add a
typed browser-starting runtime event or have the lazy app host own a narrow progress callback. Do
not infer actual Chrome allocation solely from the file-level union.

The plan-level union may still be used for static host-capability declarations, build metadata, and
whether a lazy host is supplied. It must not trigger eager executable resolution or launch.

## 5. Implementation scope by crate

### `plan`

- add per-test capabilities;
- increment and test the plan format;
- retain envelope/test-plan union fields and add invariant helpers/tests if useful.

### `analysis`

- collect one local set per test through the existing typed compilation path;
- preserve exact global union, order, diagnostics, steps, IDs, and origins;
- add mixed-test plan snapshots and native/WASM parity coverage.

### `runtime`

- validate and consume the per-test field directly;
- acquire/reuse/close sessions lazily;
- create contexts/pages only for Browser-capable tests;
- preserve deterministic test/event ordering and D.8.1–D.8.4 outcomes.

### `app`

- defer configured/managed/system executable resolution until `BrowserHost::start`;
- keep all precedence and ChromeHost construction in the composition root;
- align live human progress with actual allocation.

## 6. Out of scope

- parallel test scheduling or multiple simultaneous contexts;
- per-step resource acquisition or closing the context between browser blocks in one test;
- fixture/suite/worker capabilities;
- new capability kinds or language syntax;
- moving Chrome resolution into reusable runtime/plan/browser crates;
- changing the current shared-session policy after healthy browser tests.

## 7. Required tests

1. compiler test with server-only, browser-only, mixed, and pure/value tests asserting each exact
   set and the exact plan union;
2. plan-envelope v2 serialization, deterministic ordering, and rejection of unknown/v1 versions;
3. WASM/native plan parity including per-test sets;
4. provider-only plan and mixed-file provider-only tests never start Chrome or create contexts;
5. `server -> browser -> server -> browser` creates contexts/pages only for the two browser tests
   and shares one healthy session;
6. browser timeout/taint closes the session and restarts only at the next browser test;
7. missing/broken Chrome still permits earlier server-only tests before the browser test aborts;
8. malformed browser step without per-test Browser capability is internal, not silently repaired;
9. malformed plan union inconsistent with test sets is rejected;
10. CLI human progress starts/stops Chrome at actual lazy allocation boundaries;
11. build JSON fixtures expose plan format 2 and exact per-test requirements.

## 8. Verification

```sh
cargo test -p webtest-plan -p webtest-analysis -p webtest-wasm
cargo test -p webtest-runtime -p webtest
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p webtest-wasm --target wasm32-unknown-unknown
```

Run the WASM check when the target is installed. Fake-host coverage is mandatory even when Chrome
is unavailable.

## 9. Acceptance criteria

1. Every planned test has exact deterministic capability metadata and the plan union is provably
   its set union.
2. Provider-only and pure tests allocate no browser context/page and do not require successful
   Chrome resolution before they execute.
3. Browser allocation is lazy, per need, and remains behind the protocol-neutral host trait.
4. The emitted plan version changes explicitly and native/WASM plans remain equal.
5. No adapter, runtime path, or TypeScript host reconstructs capabilities by parsing source or
   scanning for syntax.
