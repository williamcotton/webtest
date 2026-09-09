# Structured execution

This project runs without an application server or Chrome. From the repository root:

```sh
cargo build
target/debug/webtest describe control.timeout
target/debug/webtest describe control.parallel
target/debug/webtest describe control.retry
target/debug/webtest describe control.race
target/debug/webtest describe statement.provide
target/debug/webtest check examples/structured-execution
target/debug/webtest test examples/structured-execution
target/debug/webtest test examples/structured-execution --jobs 2 --reporter json
target/debug/webtest test examples/structured-execution/retry.webtest --reporter events
```

The first test demonstrates inherited bindings and nested deadlines. Child bindings remain
local, and an inner timeout cannot extend an outer deadline. The second transfers a temporary
directory to the timeout scope even though its result is discarded; cleanup finishes before
the following assertion. `[timeouts].cleanup` supplies the separate teardown budget.

`parallel.webtest` demonstrates independent branch snapshots under an enclosing deadline.
Both branches finish before the parent continues; their local declarations stay local.
The JSON/events reporters retain a typed `branches` aggregate in stable source order.
`race.webtest` demonstrates a failed alternative followed by a successful typed winner,
and nested races whose local values do not escape. `provide` is the last statement of each
value-producing branch. The report retains failed alternatives and marks `race_winner`;
a recovered alternative does not make the test fail.

`retry.webtest` demonstrates local attempt bindings, capped backoff syntax, and a successful
attempt providing a race result. The count includes the first execution. Omitting backoff uses
zero delay; omitting max keeps the delay constant. Retry requires repeatable operations: provider
calls need schema `retry_safe`, and browser assertions/waits are safe while mutations are rejected.
Every attempt finishes teardown before backoff or another attempt. Cancellation and cleanup
failures stop retry, and all attempt outcomes and evidence remain available in reports.

`--jobs 2` admits up to two test roots across these files, including their teardown. It is
independent of the nested `parallel`/`race` scheduler. Bindings, browser ownership, resources,
observations, and artifact names stay isolated; final results retain file/test source order.
The default is `--jobs 1`, with the existing sequential execution and browser-session reuse.
The accepted range is 1–64. Assertion failures do not stop other tests; an infrastructure error
stops admission in its file and every already admitted test is awaited.

`[journal].max_events` sets the positive native event-count limit for each file run (default
100000), shared by its tests, branches, and attempts. Inspect the resolved value with
`webtest describe runtime.configuration --project examples/structured-execution`.
Exhaustion reports `journal_capacity_exceeded`, cancels active roots, and awaits cleanup.
Original test outcomes and the final run record survive; the reported missing interval marks
an incomplete journal. Increase the budget and rerun when appropriate. This is not trace export.

Retry event output includes `attempt_started` and `attempt_finished`, carrying the
one-based ordinal and `max_attempts` alongside scope/source identity. The terminal
event records the final outcome and any cancellation cause after owned teardown;
it precedes backoff and the next attempt. Nested retries number their attempts locally.

Event reports preserve the journal sequence, clocks, and source/operation/attempt
metadata. When browser failure evidence is configured, each acknowledged file write
adds `attachment_created` with its kind/path, byte length, and BLAKE3 digest. Failed
or expired writes remain capture failures and do not produce attachment references.
Use `webtest describe runtime.configuration` for the shared evidence/journal guidance.
