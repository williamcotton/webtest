# Structured execution

This project runs without an application server or Chrome. From the repository root:

```sh
cargo build
target/debug/webtest describe control.timeout
target/debug/webtest check examples/structured-execution
target/debug/webtest test examples/structured-execution
```

The first test demonstrates inherited bindings and nested deadlines. Child bindings remain
local, and an inner timeout cannot extend an outer deadline. The second transfers a temporary
directory to the timeout scope even though its result is discarded; cleanup finishes before
the following assertion. `[timeouts].cleanup` supplies the separate teardown budget.

Parallel, race, and retry examples will be added with their implementations.
