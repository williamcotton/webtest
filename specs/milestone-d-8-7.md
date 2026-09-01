# Milestone D.8.7 — Asynchronous Runtime Artifact Persistence

## 0. Status and dependencies

**Status: implemented (research baseline verified at `580b5708a20707449b8487f393fc46b11bd6c628`; implementation completed 2026-09-01).**

**Implementation status (2026-09-01):** Runtime artifact directory creation and sequential
screenshot/DOM/summary writes now await Tokio filesystem operations behind a private test seam.
Evidence capture and inspection remain inside the per-test body deadline; an already-determined
failure is then persisted against that same absolute deadline before observations and failure events
are published. Expiration stops later writes, records a bounded secondary capture failure, and
cannot replace the primary step failure. Async unit and lifecycle coverage preserves deterministic
names, bytes, typed ordering, redaction, partial-write behavior, executor yielding, event ordering,
and the absence of false artifact paths.

This low-priority correction follows [`milestone-d-8-3.md`](./milestone-d-8-3.md), which establishes
when evidence persistence occurs relative to cleanup and terminal events, and
[`milestone-d-8-4.md`](./milestone-d-8-4.md), which supplies the remaining test budget when artifact
work needs to be bounded.

It changes only the runtime artifact filesystem path. Build emission, project discovery,
configuration loading, formatter writes, test fixtures, and other intentionally synchronous app
paths are not part of this milestone.

## 1. Outcome

Failure artifact directory creation and file writes no longer execute through `std::fs` on an async
runtime worker. The runtime awaits Tokio filesystem operations while preserving all current
artifact names, contents, ordering, redaction, and best-effort failure semantics.

The target internal API is equivalent to:

```rust
pub(crate) async fn write_artifacts(
    directory: &Path,
    execution_id: ExecutionId,
    test_id: TestId,
    step_id: StepId,
    evidence: &mut PageEvidence,
) -> Vec<Artifact>;
```

`process_failure`/`finish_failure` awaits persistence before it publishes failure events containing
artifact paths. A returned artifact path therefore refers to a successfully completed write.

## 2. Research baseline

`crates/runtime/src/artifacts.rs` currently performs:

- `std::fs::create_dir_all(directory)`;
- up to three sequential `std::fs::write` calls for screenshot, DOM, and evidence summary.

The function is called from async `process_failure` after browser evidence and semantic inspection
have completed. Large DOM snapshots, slow disks, network-mounted CI workspaces, filesystem
contention, or bursts of failing tests can block a Tokio executor thread. That can delay CDP
correlation, provider timeouts, DAP requests, and other unrelated async work.

Current artifact behavior is otherwise sound and must be preserved:

- no directory is created when evidence has nothing persistable;
- deterministic stem `test-<test>-step-<step>-execution-<execution>`;
- screenshot, DOM, then evidence-summary ordering;
- successful writes produce `Artifact` values with typed kinds;
- directory/write failure appends a bounded entry to `PageEvidence.capture_failures`;
- persistence failure does not replace the primary test failure;
- screenshot bytes are removed from returned evidence when screenshot persistence is disabled;
- output is already redacted before the write boundary.

Temporary-directory cleanup in `execution.rs` already uses `tokio::fs::remove_dir_all` and is not
the synchronous I/O problem described here.

## 3. Required async behavior

### 3.1 Sequential deterministic writes

Use `tokio::fs::create_dir_all` and `tokio::fs::write`. Keep writes sequential in the established
artifact order unless measurement proves parallel writes are necessary. Deterministic result order
is more important than micro-optimizing a maximum of three files.

For each artifact:

1. build its deterministic path;
2. await the complete write;
3. append `Artifact { kind, path }` only on success;
4. otherwise append a capture failure and continue to the next artifact.

Directory creation failure records one capture failure and returns no artifacts, matching current
behavior.

### 3.2 Event and deadline interaction

Persistence completes before `StepFailed`/`TestFinished` events that expose artifact references.
This is consistent with D.8.3 even though artifact write failure is secondary and cannot change the
primary outcome.

When D.8.4's remaining test budget expires during best-effort persistence:

- stop attempting additional artifacts;
- retain the original test failure as primary;
- append a bounded capture failure stating that persistence exceeded the remaining budget;
- never return paths for incomplete writes;
- proceed through normal cleanup and terminal outcome calculation.

Do not grant every file a fresh copy of the test timeout. A future trace writer may have its own
budget and queue; this small artifact subsystem does not introduce one.

### 3.3 Error and security behavior

Continue storing normalized project-facing paths only at the app presentation boundary. Runtime
keeps `PathBuf`. Do not include evidence contents in I/O error strings, logs, or events.

This milestone does not weaken filesystem safety by invoking a shell, constructing paths from test
names, or following a second adapter-controlled storage implementation. Filenames continue to use
typed numeric IDs only.

Tokio filesystem APIs use the runtime's blocking pool internally. Do not wrap each Tokio fs call in
another `spawn_blocking`. If Tokio fs proves unsuitable for a platform, use one bounded
`spawn_blocking` operation around the complete synchronous batch, not unbounded detached tasks.

### 3.4 No premature storage abstraction

A private test seam is acceptable if needed to deterministically simulate delayed/failing writes,
but D.8.7 does not add a public pluggable artifact store, remote uploader, trace service, or adapter
callback. Artifact policy remains in runtime and filesystem/terminal presentation remains in app.

## 4. Implementation scope

### `runtime`

- make artifact directory/file persistence async;
- make the failure-finalization call chain await it;
- preserve mutable evidence capture-failure accumulation;
- remove production `std::fs::create_dir_all`/`write` from `artifacts.rs`;
- update unit tests to async tests where appropriate.

### `app`, `observation`, and adapters

No DTO or public behavior change is expected. Existing report and observation tests must prove
artifact paths and capture failures remain identical. DAP/LSP stdout ownership is unaffected.

## 5. Out of scope

- atomic temp-file-plus-rename persistence;
- parallel artifact writes;
- compression, deduplication, quotas, retention, or trace archives;
- remote/object-store uploads;
- making evidence persistence a primary test failure;
- converting all synchronous filesystem calls in the workspace;
- changing artifact filenames or report schema.

Atomic persistence and artifact budgets are worthwhile future hardening, but combining them here
would obscure the executor-blocking correction.

## 6. Required tests

1. empty evidence performs no directory creation;
2. screenshot/DOM/evidence outputs retain exact deterministic names, kinds, contents, and order;
3. directory creation failure remains one secondary capture failure;
4. one file write failure does not prevent later eligible writes and returns no false path;
5. a deliberately delayed test storage seam yields to another Tokio task rather than blocking the
   executor thread;
6. artifact persistence deadline exhaustion stops later writes and preserves the primary failure;
7. event artifact references exist on disk before the event sink receives them;
8. secrets/redacted fields do not appear in persisted files or I/O failure messages;
9. production `crates/runtime/src/artifacts.rs` contains no `std::fs::create_dir_all` or
   `std::fs::write` calls outside test fixtures.

Avoid timing-sensitive wall-clock assertions. A test seam or paused-time coordination is preferred
for executor-yield and deadline cases.

## 7. Verification

```sh
cargo test -p webtest-runtime
cargo test -p webtest-observation -p webtest-editor -p webtest-dap -p webtest
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## 8. Acceptance criteria

1. Runtime artifact directory creation and writes use an awaited async path.
2. Artifact names, bytes, typed kinds, ordering, redaction, and best-effort error behavior are
   unchanged.
3. Failure/terminal events never advertise an incomplete artifact.
4. Slow artifact persistence does not synchronously block the Tokio worker executing the run.
5. No public storage abstraction, second evidence implementation, or unrelated filesystem rewrite
   is introduced.
