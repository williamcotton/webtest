# Milestone D.8.4 — Per-Test Deadlines and Distinct Operation Budgets

## 0. Status and dependencies

**Status: implemented (research baseline verified at `580b5708a20707449b8487f393fc46b11bd6c628`; implementation completed 2026-09-01).**

This milestone follows [`milestone-d-8-3.md`](./milestone-d-8-3.md), which makes test resources
finalizable after a body future reaches a deadline. It also relies on the `TimedOut` outcome reserved
by [`milestone-d-8-1.md`](./milestone-d-8-1.md).

It corrects the meaning of the existing `[timeouts].test` setting under the current sequential
runtime. It does not add the DSL `timeout {}` node, nested deadlines, cleanup timeouts, or the
general cancellation/deadline execution context specified by [`milestone-e.md`](./milestone-e.md).

## 1. Outcome

`timeouts.test` is one budget per `PlannedTest`, measured from that test's start through resource
acquisition and body execution. It is not a file/suite deadline and it is not reused as the named
default for every provider call.

The effective model is:

```text
run/file scheduling                         no implicit wall-clock deadline

test deadline                              start + timeouts.test
  browser action budget                    min(timeouts.action, remaining test time)
  browser assertion/wait budget            min(explicit-or-assertion, remaining test time)
  navigation budget                        min(timeouts.navigation, remaining test time)
  provider-call budget                     min(explicit-or-provider_call, remaining test time)
  browser command transport budget         min(browser_command, operation/test remainder)

test cleanup                               outside body budget, still bounded by owned backend
                                           operations and reported by D.8.3
```

A file containing ten tests that each take ten seconds is valid with `timeouts.test = "60s"`.
There is no implicit sixty-second aggregate file deadline. If a future product needs one, it must be
a separately named `timeouts.run`/`timeouts.file` setting with its own documented outcome.

## 2. Research baseline

### 2.1 The app currently makes `test` a file deadline

`crates/app/src/commands/test.rs` wraps one entire `runner.run(&analyzed.plan, browser)` call in:

```text
tokio::time::timeout(project.config.timeouts.test, ...)
```

The timeout message says "test file exceeded its ... timeout". The dropped run future can own a
browser session, active context/page, provider call, and temporary resources; it returns no
structured partial result or terminal events.

### 2.2 Runtime also uses the same duration as an operation cap/default

Current uses of `RunnerOptions.test_timeout` include:

- `min(wait/assertion timeout, test_timeout)` for locator and URL waits;
- the default `CallContext.timeout` for provider calls;
- project validation requiring action/assertion/navigation values not exceed `timeouts.test`.

Actions currently receive `action_timeout` without subtracting elapsed test time. Navigation uses a
CDP-host-level `navigation_timeout`. Provider implementations may let an explicit `timeout`
argument replace rather than cap the context timeout. There is no absolute test deadline in
runtime, so a sequence of individually legal operations may exceed the configured test budget.

### 2.3 Current plan/provider timeout fact

`ServerProviderCall` has an optional `timeout`, but analysis currently emits `None` for every call.
Built-in HTTP/process providers instead inspect an operation argument named `timeout`, while the app
bridge uses `CallContext.timeout`. D.8.4 must define one effective cap at the registry/provider
boundary so an operation-specific value can narrow but never extend the remaining test deadline.

## 3. Configuration contract

### 3.1 Existing keys

Preserve the existing positive duration keys:

```toml
[timeouts]
browser_command = "10s"
action = "5s"
assertion = "5s"
navigation = "30s"
test = "60s"
```

Continue validating that action, assertion, and navigation defaults do not exceed `test`.

### 3.2 Provider-call default

Add a distinct key:

```toml
[timeouts]
provider_call = "60s"
```

Use sixty seconds as the initial default to preserve today's effective default under the standard
configuration. Validate it as positive and no greater than `timeouts.test`. Keep
`server.app.startup_timeout`, `server.app.shutdown_timeout`, and `[app.health].timeout` separate;
they own application lifecycle phases, not one test operation.

Add `provider_call_timeout` to `RunnerOptions`. `provider_config` remains unrelated.

Do not add `file_timeout` or `run_timeout` in this milestone. Remove wording that calls
`timeouts.test` a test-file timeout from diagnostics, README text, examples, and reports.

## 4. Runtime deadline semantics

### 4.1 One monotonic absolute deadline

At `TestStarted`, runtime creates one private monotonic deadline using `tokio::time::Instant` (or a
narrow injected clock abstraction if needed for deterministic tests). It passes remaining
durations down; it does not serialize an instant into `TestPlan` or expose Tokio types from
protocol-neutral browser/provider DTOs.

The deadline covers:

- browser context and page acquisition for the test;
- debugger pre-step hook time while the run is not deliberately paused by a defined debug policy;
- every pure/provider/browser/assertion step;
- failure evidence/inspection needed to determine the body outcome.

Cleanup begins after the provisional body outcome and follows D.8.3. It does not receive a fresh
test budget and cannot change a timeout into a pass. Milestone E adds a distinct bounded cleanup
deadline.

If debugger pause time is excluded, implement that as an explicit paused-clock adjustment owned by
runtime control; do not accidentally grant a fresh duration after every pause. The initial slice may
count pause time if this is documented and DAP tests assert it.

### 4.2 Operation budgets use remaining time

Immediately before each awaited operation, calculate:

```text
remaining = deadline - monotonic_now
effective = min(configured_or_explicit_operation_budget, remaining)
```

Zero remaining time produces `TestOutcome::TimedOut` before starting new work. A completed step
does not reset the deadline.

Browser traits may gain a narrow per-call budget for navigation/evaluation if needed. Do not move
test-deadline policy into `browser-cdp`; the adapter receives an effective duration and implements
the operation. Its command actor may further cap a CDP request by the smaller browser-command
budget.

`CallContext.timeout` becomes the effective maximum for one provider call. Built-in HTTP/process
providers and the application bridge must use:

```text
min(provider operation's explicit timeout, CallContext.timeout)
```

when an explicit value exists. A provider may return earlier with its domain-specific timeout
error, but cannot extend the parent test deadline. Provider schema/analysis stays the sole owner of
typed arguments; runtime must not scan source text or special-case provider names.

### 4.3 Enforcing the body deadline

Use `timeout_at`/`select!` around the test body or active operation only after D.8.3 has moved owned
resources into an outer finalizer. Deadline expiry must:

1. stop polling the active body/operation future;
2. record the active step identity when one exists;
3. produce `TestOutcome::TimedOut { timeout, active_step }`;
4. finalize context and temporary resources;
5. consider the browser session tainted and close it before another browser test;
6. continue with later tests only after successful finalization and, when needed, a fresh session.

The provider/browser implementations must honor the supplied cap so dropping the runtime future
does not leave unbounded work. The built-in process provider retains its process-group cleanup path;
the bridge retains correlated timeout/cancel cleanup; CDP drops abandoned response receivers and
context disposal closes the page. Full active cancellation/reaping guarantees remain Milestone E,
but D.8.4 must add focused no-unbounded-work tests for every built-in path it can time out.

### 4.4 Timeout classification and reporting

A per-test deadline is `TestOutcome::TimedOut`, not `Cancelled`, `Infrastructure`, or `Internal`.
It uses stable code `runtime.test_timeout`, counts as a non-passing test, and normally allows later
tests to run. If finalization fails, D.8.3's higher infrastructure/internal class aborts the run and
retains the timeout as the prior body outcome.

The report includes configured timeout milliseconds and the active test/step identity. Human,
concise, JSON, events, JUnit, and DAP output render `TIMED OUT` consistently. Do not emit the old
"test file exceeded" message.

A source-bound timeout observation may use the active step's exact origin; if expiry occurs during
test resource acquisition, use the `PlannedTest.origin`. It must remain revision-bound and must not
pretend to be an assertion mismatch.

## 5. Implementation scope by crate

### `project` and `app`

- parse/default/validate `timeouts.provider_call`;
- remove the file-wide `tokio::time::timeout` wrapper;
- pass test and provider-call budgets through `RunnerOptions`;
- update README/example/config diagnostics and runtime-configuration facts as applicable;
- report structured runtime timeout outcomes returned by `Runner`.

### `runtime`

- own the per-test monotonic deadline and remaining-budget calculation;
- cap every async step and resource acquisition by remaining time;
- produce typed timeout outcomes/events/observations;
- restart only a tainted browser session and never reset a test deadline between steps.

### `provider`, `app-bridge`, `browser`, and `browser-cdp`

- treat supplied operation duration as a maximum;
- cap explicit provider timeout arguments by `CallContext.timeout`;
- accept effective navigation/evaluation budgets where the current trait lacks them;
- preserve protocol-neutral traits and keep CDP JSON/timing mechanics in `browser-cdp`.

## 6. Out of scope

- a whole-file/run/suite timeout setting;
- DSL timeout blocks or nested deadline precedence;
- cleanup timeout configuration;
- retry/fail-fast/parallel scheduler behavior;
- wall-clock timestamps or serialized runtime instants;
- treating debugger pause as free time unless explicitly implemented and tested.

## 7. Required tests

Use paused Tokio time or an injected fake monotonic clock; do not add minute-long tests.

1. two or more tests whose aggregate duration exceeds `timeouts.test` but each test does not;
2. one test whose cumulative steps exceed its deadline although each individual default is legal;
3. remaining time, not the original test duration, caps a late browser wait/action/navigation;
4. remaining time caps default and explicit HTTP/process/app-bridge calls;
5. an explicit provider timeout can shorten but not extend `CallContext.timeout`;
6. timeout during context/page acquisition still performs acquired-resource cleanup;
7. timeout during a DAP pre-step pause follows the documented pause policy;
8. timed-out browser test gets a fresh session before the next browser test;
9. provider-only tests require no browser restart;
10. cleanup failure after timeout outranks it while retaining both typed facts;
11. project validation/default/unknown-key coverage for `provider_call`;
12. exact human/JSON/events/JUnit timeout output and absence of "test file exceeded";
13. no outer app future drops the entire runner or loses partial results/events.

## 8. Verification

```sh
cargo test -p webtest-project -p webtest-runtime -p webtest-provider -p webtest-app-bridge
cargo test -p webtest-browser -p webtest-browser-cdp
cargo test -p webtest-editor -p webtest-dap -p webtest
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## 9. Acceptance criteria

1. `timeouts.test` is enforced once per planned test and nowhere as an implicit file/run budget.
2. Every operation receives no more than the remaining test budget.
3. Provider calls have a separately named default and explicit values cannot extend their parent
   deadline.
4. Timeout produces a typed test outcome, truthful terminal events, complete finalization, and
   retained partial run results.
5. Aggregate file duration alone cannot cause a timeout.
6. No adapter or backend implements a second test-deadline policy.
