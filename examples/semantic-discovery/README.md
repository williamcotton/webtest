# Semantic discovery fixture

This deterministic fixture proves the Milestone C.5 authoring and repair loop without a second browser driver or language implementation.

From this directory, start the fixture and use only WebTest's public interfaces:

```sh
python3 server.py
webtest describe --reporter json
webtest describe browser.fill --reporter json
webtest describe --search "email form input" --reporter json
webtest inspect /login --reporter json
webtest check tests/login.webtest --reporter json
webtest test tests/login.webtest --reporter json
```

For the repair proof, rename `tests/repair.webtest.disabled` to `tests/repair.webtest`, run it, and inspect the `locator_candidate` repair hint. Replacing `Log in` with the returned `Sign in` locator makes the flow pass. The fixture intentionally uses a fixed port for manual work; automated browser tests bind `127.0.0.1:0`.

The normative deterministic harness starts the same application on a random loopback port and acts only through `describe`, `inspect`, `check`, and `test`:

```sh
cargo build
python3 acceptance.py ../../target/debug/webtest
# If Chrome discovery needs help:
python3 acceptance.py ../../target/debug/webtest --chrome-path "/path/to/chrome"
```

It checks the description index and relevant leaf examples, the illegal-context reference query, redacted semantic inspection, ordinary resolution of every emitted locator, structured failure repair, and a passing corrected test. It never uses CDP, Playwright, DOM scraping, or application source to derive the test.

An external-agent evaluation may provide only the application URL, a behavioral requirement, the `webtest` executable, and permission to edit `.webtest` files. Direct CDP, Playwright, DOM scraping, browser MCP, and application-source inspection should be prohibited so the evaluation measures WebTest's own discovery surface.
