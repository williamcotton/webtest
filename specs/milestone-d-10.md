# Milestone D.10 — Project-Owned Semantic Inspection

## 0. Status and relationship to existing milestones

**Status: implemented 2026-09-03.**

Milestone C.5 introduced bounded semantic page inspection and the
`describe → inspect → check → test` authoring loop. Milestone D added runner-owned `[app]`
lifecycle for application-bridge execution. Before D.10 those capabilities did not compose:
`webtest inspect` owned Chrome but required an already-running web server, while `webtest test`
could start the configured application. An author or agent therefore had to infer an undocumented
two-process shell workflow before it could perform the first dynamic discovery operation.

D.10 makes project-mode inspection lifecycle-complete. It adds no DSL syntax, locator semantics,
browser backend, persistent browser session, or application-provider protocol.

## 1. Outcome

From a stopped configured project, this command is sufficient:

```sh
webtest inspect /login --reporter json
```

For an omitted or relative URL, WebTest:

1. discovers the nearest project;
2. resolves the configured runner-owned `[app]` lifecycle;
3. starts the application when `app.owned = true`, or waits for configured external readiness
   when `app.owned = false`;
4. waits for `[app.health]` when configured;
5. resolves the target against `browser.base_url`;
6. creates the ordinary isolated Chrome session, context, and page;
7. opens and semantically inspects the target through the shared browser abstraction;
8. closes browser resources;
9. shuts down the runner-owned application; and
10. emits the existing human or versioned JSON inspection representation.

The application is finalized after navigation, inspection, Chrome startup, and Chrome cleanup
failures as well as after success. JSON stdout remains reserved for the inspection DTO.

## 2. URL modes

Inspection has two intentionally simple modes:

| Invocation | Mode | Application lifecycle | URL resolution |
|---|---|---|---|
| `webtest inspect` | project | use `[app]` when configured | `browser.base_url` |
| `webtest inspect /login` | project | use `[app]` when configured | relative to `browser.base_url` |
| `webtest inspect https://example.test/login` | standalone | do not start or stop `[app]` | absolute URL unchanged |

Only absolute HTTP(S) URLs select standalone mode. Filesystem URLs and malformed scheme-like input
do not silently bypass project lifecycle or normal URL validation.

The default optimizes the canonical authoring command rather than adding a required `--start-app`
flag that humans and agents must remember. D.10 adds no `--with-app`, `--no-app`, or persistent
`webtest dev` supervisor.

## 3. Configuration semantics

The existing section ownership remains authoritative:

- `[app]` owns the application command, arguments, working directory, environment, ownership, and
  optional health check;
- `[browser]` owns the browser base URL and browser/inspection settings;
- `[server.app]` owns the optional typed `app.*` provider adapter, schema, transport, and limits.

`[app]` does not require `[server.app]`. A browser-only project may configure a runner-owned web
application for `test` and project-mode `inspect` without declaring application-provider
operations.

When both sections exist, project-mode inspection starts the application through the configured
application provider. This preserves the bridge transport contract: a socket bridge still creates
the local endpoint before spawning the application and supplies `WEBTEST_BRIDGE`, `WEBTEST_TOKEN`,
and `WEBTEST_PROTOCOL`. Inspection does not invent a second application boot path that omits the
configured bridge environment.

When only `[app]` exists, the shared native application lifecycle starts it directly with the
configured working directory and environment. `owned = false` never spawns or kills a process; it
only applies the configured health readiness policy.

An owned application requires a non-empty command. Failure to spawn, premature process exit,
health timeout, bridge readiness failure, and teardown failure retain structured provider errors
and infrastructure exit classification.

## 4. Architecture

There remains one native process implementation:

```text
project configuration
        |
        v
app-bridge native ApplicationLifecycle
        |
        +---------------- browser-only test / project inspect
        |
        +---------------- AppProvider transport + calls
```

`webtest-app-bridge` already owns native application lifecycle, bounded stderr capture, health
checks, child ownership, and shutdown. D.10 exposes a narrow lifecycle owner over those same
mechanics. The `app` crate remains the composition root and decides whether an invocation uses the
provider-backed lifecycle or the provider-independent lifecycle.

The `inspect` command continues to own its browser session/context/page lifecycle. It does not
construct a hidden `.webtest` file, add an `Inspect` plan operation, parse source, dispatch provider
calls, or move CDP mechanics outside `browser-cdp`.

Application startup is independent of browser allocation. Browser-only `test` runs now honor
`[app]` even when no application manifest is configured, while Chrome remains lazy for plans that
do not require browser capability.

## 5. Failure and cleanup rules

Startup must complete before Chrome is resolved or launched. This prevents inspection from racing
the application and makes `[app.health]` the readiness boundary.

Cleanup order is:

```text
page drop → browser context close → browser session close → application shutdown
```

The primary operation failure remains primary. If application teardown also fails after an
inspection failure, WebTest reports the primary failure and records the teardown problem through
stderr tracing. If inspection succeeds but application teardown fails, the command exits as an
infrastructure error and does not emit a misleading successful inspection report.

Application stdout never contaminates inspection JSON. Runner-owned application stdout remains
discarded for ordinary process lifecycle; Protocol 1 stdio stdout remains reserved for bridge
frames. Stderr remains bounded by the existing native lifecycle policy.

## 6. Description and authoring guidance

`webtest describe` remains a language, provider, and resolved-project reference rather than a
duplicate CLI manual. The existing configuration topics carry the new facts:

- `app.configuration` explains that `[app]` serves browser-only tests and project-mode inspection,
  describes relative/omitted versus absolute inspection, and keeps `[server.app]` optional;
- `runtime.configuration` reports application ownership and whether a health check is configured,
  in addition to the existing redacted command, arguments, paths, adapter, transport, schema, base
  URLs, and timeouts;
- relevant aliases, categories, search terms, relationships, examples, and completeness tests
  continue to route to those canonical topic IDs.

The canonical installed WebTest skill changes surgically:

- tell agents not to start `[app]` separately before project inspection;
- distinguish project and standalone URL modes;
- identify `runtime.configuration` as the resolved lifecycle diagnostic surface; and
- make `webtest inspect [<url>] --reporter json` an explicit step in “Close the loop.”

Initializer parity remains exact because `webtest init` embeds the canonical skill source.

## 7. Required verification

Focused automated coverage must prove:

1. absolute HTTP(S) URL classification selects standalone inspection while omitted and relative
   targets select project inspection;
2. the standalone lifecycle is idempotent, waits for external health without taking ownership,
   rejects an owned empty command, and finalizes owned children;
3. browser-only test composition starts `[app]` without requiring `[server.app]`;
4. provider-backed project inspection preserves bridge startup rather than double-spawning `[app]`;
5. inspection finalizes `[app]` on success and browser failure;
6. description JSON exposes ownership/readiness facts without environment values or secret-like
   arguments;
7. `app.configuration` and `runtime.configuration` lookup and search explain inspection startup;
8. CLI help and initialized-skill parity contain the new contract; and
9. the semantic-discovery acceptance fixture starts from a stopped application and completes the
   public `describe → inspect → check → test` loop without an external browser driver or separately
   managed development server.

Run at minimum:

```sh
cargo fmt --all -- --check
cargo test -p webtest-app-bridge
cargo test -p webtest-analysis description
cargo test -p webtest --test cli_characterization
cargo test -p webtest --test cli
cargo clippy --workspace --all-targets -- -D warnings
```

When Chrome is available, also run:

```sh
cargo build
python3 examples/semantic-discovery/acceptance.py target/debug/webtest
```

## 8. Non-goals

D.10 does not add authenticated setup steps before inspection, reuse a test browser profile,
inspect after an arbitrary test step, preserve the application between CLI invocations, retry
application startup, crawl multiple pages, infer routes from source, or allow an LLM to drive raw
CDP/Playwright as a fallback. Those require separate product designs and must not weaken the
one-parser, one-plan, one-browser-abstraction architecture.

## 9. Acceptance

This milestone is complete when an unfamiliar external client can enter a stopped project, query
the installed description surface, run `webtest inspect /login --reporter json`, receive validated
semantic locators, author and check a normal `.webtest` file, and execute it—without starting a
second terminal process and without receiving private instructions about the application server.
