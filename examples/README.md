# Examples

`semantic-discovery/` is the deterministic semantic-inspection, language-description, and locator-repair acceptance fixture for Milestone C.5. Its README documents the closed `describe` → `inspect` → `check` → `test` loop.

Each directory is a self-contained WebTest project with its own `webtest.toml`.

- `plain-html` exercises browser operations against files served by Python's static HTTP server.
- `simple-server` exercises a typed `server` block and transfers the created user into a browser sign-in flow against a Python standard-library application.

Run the fixture for an example first, then pass its directory or a test file to `target/debug/webtest` from the repository root. The repository [README](../README.md#examples) contains the exact commands.
