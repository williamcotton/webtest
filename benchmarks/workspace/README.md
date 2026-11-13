# Workspace responsiveness fixture

The checked-in manifest and deterministic Rust generator define 1,000 actual module files,
5,000 declarations, and a 1,000-file import chain. Each module exports a helper and a suite
fixture and contains three tests; malformed input occurs every 97 files. Runtime
specialization resets every eight modules to keep executable plans bounded while retaining
the full import chain. The benchmark host supplies every input explicitly.

The original pre-F baseline used commented import paths and five sequential tests per file.
`baseline.json` preserves that historical measurement. `milestone-f.json` records the actual
F corpus. The local-edit workload changes a helper body in module 8 without replacing its
imports or declarations, checks exact parse/HIR/compiler counts across 100 edits, and proves
an unrelated module reuses its memo while a dependent module recompiles without reparsing.

Run `cargo run --release -p webtest-analysis --example workspace_benchmark -- --check`.
The F corpus captured on 2026-11-13 on the same Apple Silicon/macOS host measured
719.03 ms initial analysis, 188.21 microseconds warm completion p95, and 20.20 ms
for 100 local edits, within all three original budgets. See `milestone-f.json`.
Baseline captured before the database migration on 2026-11-12, Apple Silicon macOS,
Homebrew Rust 1.95, release profile: 96.93 ms initial diagnostics, 12.75 microseconds
warm completion p95, and 1.14 ms for 100 local edits. See `baseline.json` for raw values.

Initial CI budgets (release, Ubuntu GitHub-hosted runner) are 1,000 ms initial
analysis, 1,000 microseconds warm completion p95, and 100 ms for 100 local edits.
These deliberately allow platform variation. Correctness gates additionally require
unchanged parse/HIR/compiler query counts for unrelated inputs, immutable completed
snapshot results, cancellation of stale requests, cycle diagnostics, and no retained
historical memo chain after repeated edits. Query memo LRU capacity is 2,048 per query;
caller-held snapshots/results account for their own bounded request lifetime.

Salsa 0.26.2 is the dependency engine. Its tracked source, lowering, schema, and
compilation queries replace provider-triggered full-file recomputation. Native adapters
remain responsible for filesystem/configuration inputs. Salsa request snapshots are
short-lived read handles: a write cancels their queries and waits for their release;
requests must drop their snapshot on cancellation. Completed owned query results remain
immutable and usable after the write. The editor must not hold snapshots across host I/O.
