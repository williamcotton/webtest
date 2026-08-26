---
name: webtest
description: Bootstrap, discover, author, inspect, validate, format, and run WebTest web-system tests with the installed CLI and its machine-readable language, provider, diagnostic, and page-inspection surfaces.
---

# WebTest

Use WebTest as a statically analyzed test language, not as an ad hoc browser-automation wrapper. Use the installed `webtest` binary and let the resolved project supply current syntax and provider facts.

## Bootstrap the project

Confirm the CLI is installed, then initialize WebTest before creating configuration, schemas, or tests by hand:

```sh
webtest --version
webtest init .
```

`webtest init` creates `webtest.toml`, `.webtest/app-schema.json`, `tests/example.webtest`, and this skill under `.agents/skills/webtest`. It also creates a `.claude/skills/webtest` compatibility link. It never overwrites conflicting files.

Initialization makes the generated test statically checkable, not yet executable. The generated application schema declares `app.echo(message: String) -> String`; before running the test, configure `[app]` and use the application-bridge discovery sequence below. Do not replace the bridge call with a public test-only HTTP route.

The canonical starter configuration is:

```toml
[project]
test_roots = ["tests"]

[server.app]
adapter = "bridge"
transport = "auto"
schema = ".webtest/app-schema.json"

# Configure these sections before running application and browser tests.
#
# [app]
# command = "your-application-command"
# args = []
# working_directory = "."
#
# [browser]
# base_url = "http://127.0.0.1:3000"
#
# [server]
# base_url = "http://127.0.0.1:3000"
```

The generated bridge smoke test is:

```webtest
test "application bridge responds" {
    server {
        let echoed = app.echo(message: "hello from WebTest")
        expect echoed == "hello from WebTest"
    }
}
```

## Discover the application bridge

Do not infer or invent the application bridge from this skill. From the initialized project, run these focused queries in order before editing the application:

```sh
webtest describe app
webtest describe app.schema
webtest describe app.protocol
webtest describe app.pseudocode
webtest describe app.echo
```

The results respectively establish the provider boundary, offline manifest/configuration contract, exact Protocol 1 transport and frames, host-language implementation outline, and generated operation signature. Use the default human output while learning; add `--reporter json` only when consuming the response programmatically. Follow the returned configuration prerequisites and related topics, prefer a maintained SDK when one is available, and keep the bridge behind an explicit test-only application boot path.

After configuring `[app]` and implementing `app.echo`, run `webtest check` and then `webtest test tests/example.webtest --reporter json`. Use any structured failure's code and reference queries to return to the narrowest relevant `describe` topic.

## Test anatomy

A complete system test may create application state on the server, pass transferable typed values into the browser flow, and use semantic locators:

```webtest
test "created user can sign in" {
    server {
        let user = app.create_user(email: "alice@example.com", admin: false)
        let response = http.post("/api/login", json: { email: user.email })
        expect response.status == 200
    }

    browser {
        open "/login"
        fill label("Email") with user.email
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
```

This example requires `server.base_url` and `browser.base_url`, plus an offline application manifest declaring `app.create_user` with the shown parameters and a transferable result containing `email`.

## Discover before authoring

Start with a narrow query and use JSON when consuming results programmatically:

```sh
webtest describe locator.role --reporter json
webtest describe browser.click --reporter json
webtest describe http.post --reporter json
webtest describe --search "json post" --reporter json
```

The no-query index contains canonical topic IDs. A construct response supplies syntax forms, typed parameters/results, legal contexts, constraints, and availability. Installed language and provider leaves include canonical examples whose prerequisites are part of the contract. A project-supplied provider operation may have no examples because the current Protocol 1 manifest does not declare them; do not invent application-specific values.

For a configured application provider, query a concrete project operation such as `webtest describe app.echo`. Use human output while learning and `--reporter json` when consuming the result programmatically. These focused built-in topics provide the deeper provider contract:

- `provider.app`: provider semantics and project requirements.
- `app.schema`: offline Protocol 1 manifest shape and hashing rules.
- `app.protocol`: framing, handshake, correlation, transports, and required wire fields.
- `app.pseudocode`: a non-normative implementation outline that points back to the protocol contract.

Use `webtest <command> --help` for CLI flags and reporters. `describe` documents the language and project-visible provider surface; it is not a duplicate CLI manual.

## Inspect a live page

Run `webtest inspect [<url>] --reporter json` to obtain bounded semantic elements and validated locator candidates. When the URL is omitted, the project must provide `browser.base_url`. Inspection describes the current page state; candidates are evidence, not a guarantee against later page changes.

## Close the loop

1. Query the smallest relevant `describe` topic or inspect the target page.
2. Author the test using returned syntax, contexts, types, and prerequisites.
3. Run `webtest fmt <path>` to apply the canonical formatter, or `webtest fmt --check <path>` for a non-mutating check.
4. Run `webtest check <path> --reporter json` and fix every static error before execution.
5. Run `webtest test <path> --reporter json`; add `--headed` only when visible Chrome is useful.

Diagnostics may contain canonical `reference_queries` and bounded `repair_hints`. Runtime failures may contain locator replacement candidates when WebTest has safe evidence. Treat both as advisory, preserve their source ranges, and rerun `check` or `test` after any edit; WebTest does not apply repairs automatically.

Do not infer unavailable roadmap syntax from prose or model memory. If a query is unknown, search the installed reference or inspect `webtest describe language --reporter json` rather than inventing a construct.
