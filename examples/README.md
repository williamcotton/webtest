# Examples

`semantic-discovery/` is the deterministic semantic-inspection, language-description, and locator-repair acceptance fixture for Milestone C.5. Its README documents the closed `describe` → `inspect` → `check` → `test` loop.

Each directory is a self-contained WebTest project with its own `webtest.toml`.

- `plain-html` exercises browser operations against files served by Python's static HTTP server.
- [`chat`](chat/README.md) is a working two-person WebSocket app: isolated Alice/Bob sessions, concurrent sign-in/reload, checkpointed cross-user delivery, patterns, guards, labeled cases, and traces. WebTest owns its application lifecycle.
- `test-modeling` exercises labeled cases, reusable patterns, soft checks, and semantic preconditions without a server or Chrome; its README links the real-browser actor/reactive acceptance fixture.
- `workspace` exercises imported typed helpers, fixture lifetimes, shared suite results, and retries without a server or Chrome.
- `structured-execution` exercises nested timeouts and lexical temporary-resource cleanup without a fixture server or Chrome.
- `simple-server` exercises a typed `server` block and transfers the created user into a browser sign-in flow against a Python standard-library application.
- `application-bridge` runs one byte-identical `app.create_user` scenario against nine host-language web servers and includes an independent no-SDK stdio fixture.

For manually served fixtures, start their server first. Runner-managed examples such as `chat` and `application-bridge` start their configured application automatically; follow each README. Pass the directory or a test file to `target/debug/webtest` from the repository root. The repository [README](../README.md#examples) contains the exact commands.
