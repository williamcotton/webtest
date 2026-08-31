# Milestone D.8.2 — Three-Way Execution Failure Classification

## 0. Status and dependencies

**Status: implemented.**

This milestone follows [`milestone-d-8-1.md`](./milestone-d-8-1.md) and makes WebTest's documented
failure taxonomy executable across runtime, observations, events, CLI reports, editor behavior, and
DAP. It should normally land with D.8.1 because the new `Aborted` outcomes need a reliable class.

The repository contract already distinguishes:

```text
invalid test behavior          -> test failure
host/browser/provider failure  -> infrastructure failure
violated WebTest invariant     -> internal failure (bug)
```

The current code loses the third state in two places. `StepError::Internal` is treated as a normal
test failure, while `RunError::Internal` is rendered and exited as infrastructure by the app.

## 1. Outcome

One typed classification function is authoritative below adapters:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    Test,
    Infrastructure,
    Internal,
}
```

The exact owner should be the lowest protocol-neutral crate that legitimately serves runtime,
observations, and adapters. `webtest-feedback` is the preferred location if both observation event
DTOs and runtime/reporting require it. Keeping it private to `runtime` is acceptable only if event
summaries do not duplicate an incompatible classification type. Do not put reporter labels in
`browser` or `provider`.

`StepError` and `RunError` expose typed classification:

```text
StepError::failure_class() -> FailureClass
RunError::failure_class()  -> FailureClass
```

`is_infrastructure()` is removed from runtime or retained only as a short-lived compatibility
wrapper implemented as `failure_class() == Infrastructure`. New runtime code must match the enum.

## 2. Research baseline

### 2.1 Current `StepError` loss

`crates/runtime/src/error.rs` currently maps:

| `StepError` | Current `is_infrastructure()` | Correct class |
|---|---:|---|
| non-infrastructure `Browser` | false | Test |
| infrastructure `Browser` | true | Infrastructure |
| non-infrastructure `Provider` | false | Test |
| infrastructure `Provider` | true | Infrastructure |
| `Assertion` | false | Test |
| `Decode` | false | Test |
| `Evaluation` | false | Test |
| `Internal` | **false** | **Internal** |

`crates/runtime/src/execution/failure.rs` uses the boolean repeatedly to decide evidence capture,
observation recording, `StepFailure` construction, and conversion to `RunError`. Consequently an
internal invariant violation:

- can trigger `after_step_failure` as though the user's test failed;
- is recorded as a revision-bound `ValueFailure { code: "internal_error" }`;
- becomes `TestResult { passed: false, failure: Some(...) }`;
- is rendered with a source underline suggesting the author should fix the test.

Current internal error sources include missing runtime bindings, impossible typed operations,
`PlanExpr::Type` in evaluation, browser operations/assertions without a page, and typed-match
assertions without a type. These are malformed-plan/runtime invariant failures, not author-visible
dynamic test failures after successful analysis.

### 2.2 Current app-boundary loss

`RunError::code()` already returns `internal_error`, and `ExitClass` already contains
`Internal = 4`. Nevertheless `crates/app/src/commands/test.rs` handles every `RunError` identically:

- stores it in `FileReport.infrastructure_error`;
- emits semantic detail `"failure_class": "infrastructure"`;
- combines `ExitClass::Infrastructure`.

The human, concise, events, summary, and JUnit writers also have only an
`infrastructure_error` file field. The runtime code is typed more accurately than its composition
root, but the report model cannot preserve that fact.

### 2.3 Existing lower-level predicates

`BrowserError::is_infrastructure()` and `ProviderError::is_infrastructure()` already encode their
domain-specific test-versus-host boundaries. D.8.2 may use those as inputs. It does not need to move
CLI exit classes into reusable crates or force browser/provider crates to understand `ExitClass`.

Provider application errors, invalid arguments, and path escapes currently classify as test
failures. Browser locator, actionability, assertion, option, invalid-key, URL-mismatch, and
evaluation failures currently classify as test failures. Preserve those decisions unless a
separate product change reviews them.

## 3. Required semantics

### 3.1 Classification table

Runtime classification is exhaustive:

```text
Assertion / Decode / Evaluation                     -> Test
Browser(error) where error.is_infrastructure()      -> Infrastructure
Browser(other)                                      -> Test
Provider(error) where error.is_infrastructure()     -> Infrastructure
Provider(other)                                     -> Test
Internal                                             -> Internal

RunError::Browser / RunError::Provider              -> Infrastructure
RunError::Internal                                   -> Internal
RunError::Cleanup                                    -> class of its typed cause (D.8.3)
```

There is no fallback that maps an unknown variant to test failure. Adding an error variant must
force a classification decision at compile time.

### 3.2 Runtime behavior by class

| Class | Test outcome | Run outcome | Revision-bound observation | Exit class |
|---|---|---|---|---|
| Test | `Failed(StepFailure)` | `Completed` after all scheduled tests | yes | `TestFailure` |
| Infrastructure | active test `Aborted` | `Aborted` | no | `Infrastructure` |
| Internal | active test `Aborted` | `Aborted` | no | `Internal` |

Evidence and repair candidates are collected only for eligible test-class browser failures.
Internal/infrastructure failures may retain already-available bounded diagnostic data, but runtime
must not perform semantic inspection or generate locator repair hints that imply an author error.

`StepFailed` remains a useful event for a started step, but its payload includes `FailureClass`.
The later `TestFinished(Aborted)` and `RunFinished(Aborted)` events carry the same classification.

### 3.3 Primary and secondary failures

Classification applies to the primary outcome, not to arbitrary strings in
`StepFailure.secondary_failures`. D.8.3 introduces typed cleanup failures and defines severity
precedence. An internal or infrastructure cleanup failure cannot be hidden as a secondary string
under a green or ordinary failed test.

### 3.4 Report model

Replace the infrastructure-only file slot with a classified execution-failure model, for example:

```rust
pub struct ExecutionFailureReport {
    pub class: FailureClass,
    pub failure: FailureReport,
}

pub struct FileReport {
    // ...
    pub execution_error: Option<ExecutionFailureReport>,
}
```

Do not put an internal error into a field still named `infrastructure_error`. Update summary counts
to distinguish `infrastructure_errors` and `internal_errors`. Human and concise reporters use
"infrastructure error" and "internal error" respectively. JUnit emits both as `<error>` but retains
the class/code. JSON and JSONL include the typed class. `ExitClass::Internal` remains higher
priority than infrastructure.

Because D.8.1 already requires a machine-schema revision, D.8.2 should be included in that same
version rather than causing two consecutive incompatible report versions.

### 3.5 DAP and editor behavior

DAP must stop calling every `RunError` a "browser infrastructure error". It renders the typed class
and uses a distinct nonzero exit result. Existing app-provider exception filters continue to use
provider-specific classification; a general internal exception breakpoint remains Milestone E
unless it can be added without expanding this correction.

Internal and infrastructure failures do not become editor diagnostics against the user's current
source. Starting the run still clears stale observations. Adapters may surface an out-of-band
execution error, but must not store it as `RuntimeObservationKind::ValueFailure`.

## 4. Implementation scope by crate

### `feedback` or another shared DTO owner

- define and serialize `FailureClass` if event/report DTOs need a shared type;
- keep it free of terminal rendering and exit-code policy.

### `runtime`

- implement exhaustive classification for `StepError` and `RunError`;
- replace every boolean branch in failure processing with an enum match;
- convert `StepError::Internal` directly to `RunError::Internal`/aborted outcomes;
- remove the internal branch from revision-bound observation construction;
- add class to runtime failure events without flattening the typed cause.

### `observation`

- retain `RuntimeFailure::Internal` only as an event payload if needed;
- never add an internal runtime observation to the revision-bound store;
- add the shared class to failure/terminal event summaries.

### `app`, `editor`, and `dap`

- replace infrastructure-only report handling with exhaustive class mapping;
- use `ExitClass::Internal` and distinct report/summary fields;
- update human, concise, JSON, events, and JUnit fixtures;
- preserve typed failures until the final presentation conversion.

## 5. Out of scope

- changing which concrete browser/provider variants are test versus infrastructure failures;
- panic catching or process-wide crash recovery;
- automatic bug-report submission;
- retry policy, aggregate failures, task-tree severity, or exception-breakpoint expansion from
  Milestone E;
- converting static analysis errors into runtime outcomes.

## 6. Required tests

1. exhaustive unit table for every `StepError` and `RunError` variant;
2. malformed plan causing `StepError::Internal` returns internal aborted test/run outcomes;
3. internal step failure emits typed events but records no runtime observation;
4. infrastructure step failure records no observation and retains its typed browser/provider cause;
5. ordinary assertion/evaluation/application-provider failures remain test failures and observations;
6. direct `RunError::Internal` maps to `ExitClass::Internal`, internal summary count, JSON class, and
   JUnit `<error>`;
7. no internal path writes `failure_class: infrastructure` or `FileReport.infrastructure_error`;
8. DAP output distinguishes internal from infrastructure failures;
9. a later successful run still clears observations after an internal abort.

## 7. Verification

```sh
cargo test -p webtest-runtime -p webtest-observation
cargo test -p webtest-editor -p webtest-dap -p webtest
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## 8. Acceptance criteria

1. The three failure classes are represented by one typed, exhaustively matched model.
2. `StepError::Internal` cannot become `StepFailure` or a revision-bound user diagnostic.
3. `RunError::Internal` survives events and every reporter as internal and exits with class 4.
4. Infrastructure and ordinary test behavior remain unchanged except for the richer outcome/event
   model from D.8.1.
5. No adapter reclassifies a typed runtime failure by inspecting its message or code string.
