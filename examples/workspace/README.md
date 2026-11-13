# Typed workspace example

From this directory, run `webtest check`, `webtest test --jobs 2`, and
`webtest describe project.declarations --reporter json`. No application server or
Chrome is required by these pure examples.

`test_roots` selects executable test files; `source_roots` also includes reusable
support modules. Imports resolve explicit exports and aliases through Rust
analysis. The test fixture retains setup locals through teardown; the suite seed
is shared across files. Retry attempts receive fresh test fixtures.

Open the directory in Cursor with the WebTest extension to use the Testing view,
module navigation, rename, completion, and Run/Debug. Breakpoints in the helper
body and fixture setup/teardown resolve to the captured source revisions.

For application-backed fixtures, query the configured `app.*` signatures and
compose them in `setup server` / `teardown server`; this example declares no
application-specific bridge operations. Failure traces retain captured dependency
sources when the configured trace policy keeps the run.
