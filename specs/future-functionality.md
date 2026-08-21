# WebTest — Post-Vertical-Slice Product Specification

## 0. Status and relationship to the initial specification

This document specifies the functionality to build after the initial vertical slice described in [`intitial-vertical-slice.md`](./intitial-vertical-slice.md). It is based on the repository as implemented, not only on the aspirations in the original specification.

The purpose of the next stages is to turn the proven architecture into a useful web-application testing product while preserving its defining constraints:

- one Rust lexer, parser, lossless CST, and syntax-to-HIR lowering path;
- typed AST nodes as Rowan CST views;
- protocol-neutral analysis and editor services;
- execution through a source-mapped `TestPlan`;
- browser semantics independent of CDP;
- structured runtime events and revision-bound observations;
- Tower LSP, Cursor/VS Code, Monaco, CLI, and terminal output as adapters;
- one native `webtest` executable.

This document does not redefine the completed slice. When it conflicts with an implementation detail that is already necessary to preserve the architecture, the architectural invariant wins and the implementation should be migrated deliberately.

---

## 1. Implemented baseline

The repository currently provides:

- a Rust 2024 Cargo workspace with separate text, syntax, HIR, analysis, formatting, planning, browser, runtime, observation, editor, LSP, DAP, WASM, and application crates;
- a lossless, error-tolerant Rowan CST preserving whitespace, line comments, malformed tokens, punctuation, and source ranges;
- typed CST wrappers for tests, browser blocks, `open`, `click`, `expect <locator>.visible`, and `id(...)`/`text(...)` locators;
- HIR and `TestPlan` lowering with typed test/step IDs, distinct browser and assertion operations, BLAKE3 source revisions, and precise locator/assertion origins;
- syntax diagnostics and a CST-based formatter;
- a protocol-neutral browser API and a direct, hand-written CDP subset that launches local Chrome, navigates, resolves ID/text locators, clicks, and evaluates visibility safely;
- headless test execution by default plus `webtest test --headed` for visible browser runs;
- sequential runtime execution, structured events, process-local runtime observations, and a pre-step `RunControl` hook shared with debugging;
- revision-safe editor diagnostics, formatting, running, and CST-backed semantic tokens;
- a Tower LSP server using full-document synchronization, formatting, diagnostics, semantic tokens, and `webtest.runFile`;
- a Cursor/VS Code VSIX that runs the synchronized editor buffer, maps semantic tokens to theme scopes, contributes breakpoints, and launches a headed debug session without requiring `launch.json`;
- a stdio DAP adapter with source-mapped breakpoints, continue/step control, stack frames, scopes, and variables; it executes the same `TestPlan` through the same `Runner` as normal tests;
- `webtest check`, `webtest fmt`, `webtest test`, `webtest lsp`, and `webtest dap`;
- a WASM-compatible facade exposing diagnostics and formatting.

Known limitations are intentional: the language has no general expressions, bindings, server domain, modules, or static type system; the only assertion is locator visibility; runtime is sequential; locator behavior is limited to ID/text and is not actionable; CDP bindings are hand-written; Chrome is not managed; observations do not cross processes; LSP sync is full-file; DAP inspection is plan/step oriented rather than a live DOM object model; and the WASM API is not yet a complete editor service.

---

## 2. Target product model

The native executable should grow toward:

```text
webtest check [paths...]
webtest fmt [paths...] [--check|--stdout]
webtest test [paths...] [--filter PATTERN] [--tag TAG] [--jobs N] [--headed]
webtest run <file> [--test NAME]
webtest build [paths...] [--emit plan.json]
webtest lsp
webtest dap [--headless]
webtest repl
webtest trace <artifact>
webtest browser install|list|path|clean
```

`test` is the test-suite command. `run` executes an explicitly selected file or test and may favor interactive output. `build` performs analysis and emits a versioned, serializable `TestPlan` without executing it. All commands must call the same analysis, formatting, planning, and runtime APIs used by editor services.

A project-level `webtest.toml` should eventually define source roots, default timeouts, browser selection, tags, environment profiles, artifact paths, managed-browser policy, application lifecycle, server providers, and bridge schemas. Configuration inputs must participate in query invalidation and plan identity.

---

## 3. Language evolution

### 3.1 Syntax and expressions

Extend the existing grammar incrementally. Every addition must preserve lossless parsing and useful recovery for incomplete editor input.

Required expression forms:

```text
string, integer, boolean, null, duration
list and record literals
name references
member access
function calls
named arguments
unary and binary operators
```

Required statements:

```text
let binding
expression/action statement
assertion
server block
browser block
sequence, parallel, race, retry, and timeout blocks
```

Illustrative syntax:

```webtest
test "password reset" tags ["browser", "mail"] {
    server {
        let user = app.create_user(email: "me@example.test")
        let reset = http.post("/api/password-reset", json: {
            email: user.email,
        })
        expect reset.status == 202
    }

    browser {
        open "/forgot-password"
        fill label("Email") with user.email
        click role("button", name: "Send reset link")
        expect text("Check your email").visible
    }
}
```

New syntax kinds and AST wrappers must be added to `syntax`; downstream layers must never scan source text or construct an alternate parse tree.

### 3.2 Types and execution capabilities

Introduce a static type system in HIR/analysis with at least:

```text
String, Int, Bool, Null, Duration, Url, Json
List<T>, Option<T>, records
StatusCode, Headers, Request, Response<T>
Locator, Element
Browser, BrowserContext, Page
ProcessResult, FilePath
```

Operations also have an execution capability:

```text
Pure | Server | Browser | Test
```

Analysis must reject operations used in the wrong domain before planning. Values crossing between server and browser blocks must be serializable and explicitly transferable. Strings, numbers, booleans, URLs, and JSON-like records are transferable; pages, sockets, database connections, browser element handles, and file handles are not.

Diagnostics should explain both the expected type/capability and the actual one, point to the smallest useful source range, and offer fixes through protocol-neutral code actions where possible.

### 3.3 Bindings, scope, and identity

Bindings are lexically scoped. Names must resolve to stable semantic IDs rather than strings. Shadowing policy must be explicit and diagnosed consistently. HIR should move toward arena-backed IDs or another compact representation suitable for incremental queries.

Test identity must remain stable enough for filtering, observations, traces, and editor decorations. A test's display name is not its only identity; project, module path, declaration origin, and an internal ID should form the durable identity.

### 3.4 Modules, packages, and fixtures

Add a module graph only after single-file expressions and types are stable. Projects should support imports, reusable fixture declarations, helper functions, and project configuration without executing arbitrary code during analysis.

Fixture lifetimes should be explicit:

```text
test | file | worker | suite
```

Setup and teardown must be represented in the plan and event stream so cancellation and failure cannot silently skip cleanup.

---

## 4. Browser testing functionality

### 4.1 Locator model

Expand the shared semantic locator model to support:

```rust
Id(String)
Role { role, name }
Label(String)
Text(String)
Placeholder(String)
TestId(String)
Css(String)
XPath(String)
```

Role, label, text, and test-ID locators should be the preferred user-facing forms. CSS and XPath remain escape hatches. Locator resolution must return structured results distinguishing zero, one, and multiple matches and should retain candidate/evidence information for diagnostics.

### 4.2 Actions

Add, in order of value:

```text
open, click, fill, type, press
check, uncheck, select
hover, focus, blur
scroll, upload, download
new page/tab, close page
wait for URL, event, response, or locator state
```

Language-level actions remain independent of CDP request names. Browser traits may evolve into context/page/element abstractions, but CDP types must not escape `browser-cdp`.

### 4.3 Actionability and auto-waiting

Replace JavaScript `element.click()` with an actionability engine in the browser abstraction. A click should:

1. repeatedly resolve the locator until its timeout;
2. require exactly one attached element;
3. verify visibility, stability, enabled state, and pointer hit testing;
4. scroll into view;
5. calculate a valid click point;
6. dispatch physical input through CDP;
7. observe navigation or resulting page state.

Timeouts, retries, and polling intervals must be configurable and observable. Failures must say which actionability condition failed and attach available evidence.

### 4.4 Browser contexts and state

Support isolated contexts, pages, cookies, local/session storage, permissions, geolocation, viewport, user agent, locale, timezone, downloads, dialogs, console events, and network events. Tests should default to isolation. Reuse a browser process for performance without reusing test state unless explicitly requested.

### 4.5 Network control

After passive network events are reliable, add request routing, aborting, modification, and deterministic response fulfillment. Matchers should operate on structured URL, method, headers, and resource type rather than raw CDP events. Mocks and their invocation counts must appear in the plan, event stream, assertions, and trace.

---

## 5. Server capability domain and application bridge

### 5.1 Meaning of `server {}`

`server { ... }` means **execute operations in the test runner's server-side capability domain**. It does not mean "evaluate arbitrary code inside the application process," start the application, or select a particular framework integration.

Operations are owned by explicit providers:

```webtest
server {
    let response = http.post("/api/test/users", json: {
        email: "alice@example.com",
    })

    let user = app.create_user(email: "alice@example.com")
    let file = fs.read("fixtures/welcome.txt")
    let check = process.run("bin/check-user", args: [user.id])
}
```

The architecture is:

```text
WebTest DSL
    |
    v
server { provider.operation(...) }
    |
    v
Server Runtime / provider registry
    |
    +----------------+----------------+----------------+----------------+
    |                |                |                |                |
    v                v                v                v                v
  http.*           app.*          process.*          fs.*        future providers
    |                |                                                 mail.*, queue.*,
    v                v                                                 postgres.*, redis.*
HTTP client      App Bridge Protocol
                     |
          +----------+----------+----------+
          |          |          |          |
          v          v          v          v
        Node       Ruby        Go       any executable
```

Namespaces make ownership and host requirements explicit. `app.create_user` is an application-defined function; it is not a core keyword. The plan must preserve provider, operation, typed arguments/result, capability requirements, schema identity, and source origin as data rather than compiling the call into an opaque callback.

### 5.2 Integration tiers

The ecosystem must not require a bespoke WebTest implementation per language or framework. Support three cumulative tiers:

1. **Black-box HTTP.** `http.*` works with every application and requires no application library. This is the default integration level.
2. **Generic App Bridge Protocol.** `app.*` works with any executable that implements one language-neutral contract. No official SDK is required.
3. **Official language SDK and optional framework helpers.** SDKs provide registration, schema generation, transport, serialization, error conversion, and lifecycle conveniences without reimplementing WebTest semantics.

For example, a portable black-box setup can use:

```toml
[server]
base_url = "http://127.0.0.1:3000"
```

```webtest
server {
    let response = http.post("/api/test/users", json: {
        email: "alice@example.com",
    })
    expect response.status == 201
}
```

A typed application fixture uses the bridge instead:

```webtest
server {
    let user = app.create_user(email: "alice@example.com")
}

browser {
    fill label("Email") with user.email
}
```

Only the second form needs an `app` provider. Both forms remain framework-neutral.

### 5.3 One bridge contract

All official and unofficial SDKs implement exactly the same versioned protocol:

```text
                         WebTest DSL
                             |
                             v
                    server { app.* }
                             |
                             v
                    App Bridge Protocol
                    language-neutral wire
                             |
          +------------------+------------------+
          |                  |                  |
          v                  v                  v
       Node SDK           Ruby SDK            Go SDK
          |                  |                  |
          v                  v                  v
    Express/Nest          Rails/etc.        net/http/etc.

          +------------------+------------------+
          |                  |                  |
          v                  v                  v
      Python SDK         Elixir SDK          JVM/.NET SDK
          |                  |                  |
          v                  v                  v
   Django/FastAPI          Phoenix          Spring/ASP.NET
```

The bridge is a small typed RPC protocol, not a remote parser, compiler, planner, or browser driver. An SDK must never implement WebTest syntax or execution semantics. Conceptually:

```json
{
  "type": "call",
  "id": 42,
  "function": "create_user",
  "arguments": {
    "email": "alice@example.com"
  }
}
```

receives:

```json
{
  "type": "result",
  "id": 42,
  "value": {
    "id": 123,
    "email": "alice@example.com"
  }
}
```

The protocol should be deliberately boring and implementable in a few hundred lines. Version 1 should use UTF-8 JSON messages and define:

```text
hello     protocol/SDK identification and authentication
describe  request the currently registered schema
schema    return provider function schemas
call      invoke a function with a correlation ID
result    return a successful serializable value
error     return a structured application/validation error
event     publish bounded progress or attachment metadata
cancel    request cancellation of an in-flight call
ping      verify liveness
shutdown  request graceful bridge shutdown
```

The protocol definition must specify framing, maximum message sizes, correlation-ID uniqueness, concurrent-call behavior, cancellation, ordering, shutdown, version negotiation, and unknown-message handling. The initial stdio/socket framing should be one complete JSON value per UTF-8 line; embedded newlines remain JSON-escaped. Protocol stdout is reserved for framed messages and SDK logging goes to stderr.

Handshake:

```json
{
  "type": "hello",
  "protocol": 1,
  "sdk": "webtest-python",
  "sdk_version": "0.4.0",
  "token": "<launch-token>"
}
```

Structured errors must be safe to show in terminal, trace, and editor adapters:

```json
{
  "type": "error",
  "id": 42,
  "code": "user.email_taken",
  "message": "a user with that email already exists",
  "retryable": false,
  "data": {}
}
```

Do not transmit language stack traces or secrets by default. A debug configuration may attach redacted details separately.

### 5.4 Schema-first functions and editor intelligence

Schema is part of the integration contract, not optional RPC documentation. Every function declares parameter and result types. A `describe` request returns a canonical schema such as:

```json
{
  "type": "schema",
  "protocol": 1,
  "functions": {
    "create_user": {
      "params": {
        "type": "object",
        "fields": {
          "email": { "type": "string" },
          "admin": { "type": "boolean" }
        },
        "required": ["email"]
      },
      "returns": {
        "type": "object",
        "fields": {
          "id": { "type": "integer" },
          "email": { "type": "string" }
        }
      }
    }
  }
}
```

SDKs also generate a deterministic offline manifest, by default:

```text
.webtest/app-schema.json
```

The LSP, WASM editor service, `check`, and `build` load this file without starting the application. It enables function completion, named-argument validation, result-member completion, and static errors such as passing a string to a Boolean field. The runtime obtains the live schema during handshake and detects incompatible drift from the schema hash embedded in the plan. Drift is a configuration/infrastructure failure, not an untyped runtime surprise.

The initial schema value model is intentionally small:

```text
Null, Bool, Int, Float, String
List<T>, Option<T>, Record
```

Semantic types such as `Url`, `Email`, `DateTime`, and `UserId` may layer validation and display metadata over those wire values. The following cannot cross the bridge:

```text
ORM/model instances, database connections, streams, IO handles,
browser pages/elements, sockets, closures/functions, process handles
```

Application code returns a serializable record, never a live ActiveRecord, Prisma, Ecto, or other framework object. The decoded value becomes an ordinary typed DSL value and may cross into a browser block only if it satisfies the normal transfer rules.

### 5.5 Canonical protocol definitions and generated types

Keep a canonical, implementation-independent protocol package:

```text
protocol/
├── schema.json
├── types.json
├── conformance/
└── protocol.md
```

Generate mechanical wire types and codecs where practical:

```text
canonical protocol
        |
        v
   code generator
        |
   +----+---------+---------+---------+---------+
   |              |         |         |         |
   v              v         v         v         v
 Rust         TypeScript    Go      Python    Ruby/JVM/.NET/Elixir
 types           types     types     models         records/structs
```

The manually maintained SDK surface should be limited to transport, function registration, serialization, schema export, application-error conversion, and lifecycle integration. Generated code must not leak into the DSL compiler's semantic model; Rust protocol DTOs adapt into the same provider traits used by built-in server capabilities.

Run every SDK against one language-neutral conformance suite covering handshake, schema, calls, errors, concurrency, cancellation, malformed messages, size limits, authentication, and shutdown. Protocol compatibility, not framework-specific test suites, is the primary cross-language guarantee.

### 5.6 Tiny language SDKs, optional framework helpers

The generic language package owns the protocol implementation. Framework packages, when justified, only add boot and lifecycle conveniences:

```text
@webtest/node       webtest (Ruby)       webtest (Python)
    |                    |                     |
    v                    v                     v
core bridge          core bridge           core bridge
    |                    |                     |
    v                    v                     v
@webtest/express    webtest-rails       webtest-django / webtest-fastapi
(optional)           (optional)               (optional)
```

Do not create separate Express, Nest, Next, Rails, Sinatra, Django, FastAPI, Flask, Phoenix, Spring, and ASP.NET implementations of the wire contract. Most frameworks should need no package at all.

Illustrative SDK APIs follow the host language's conventions while producing the same schema and messages. Go:

```go
bridge := webtest.New()

bridge.Function(
    "create_user",
    webtest.Schema{
        Params: webtest.Object{"email": webtest.String},
        Returns: webtest.Object{
            "id": webtest.Int,
            "email": webtest.String,
        },
    },
    func(ctx context.Context, args map[string]any) (any, error) {
        user, err := db.CreateUser(ctx, args["email"].(string))
        if err != nil {
            return nil, err
        }
        return map[string]any{"id": user.ID, "email": user.Email}, nil
    },
)

bridge.Run()
```

Python:

```python
bridge = webtest.Bridge()

@bridge.function(
    "create_user",
    params={"email": str},
    returns={"id": int, "email": str},
)
def create_user(email):
    user = User.objects.create(email=email)
    return {"id": user.id, "email": user.email}

bridge.run()
```

Elixir:

```elixir
WebTest.function "create_user",
  params: %{email: :string},
  returns: %{id: :integer, email: :string} do
  %{email: email} ->
    {:ok, user} = Accounts.create_user(%{email: email})
    %{id: user.id, email: user.email}
end
```

Node:

```ts
const bridge = createWebTestBridge();

bridge.function(
  "create_user",
  {
    params: { email: "string" },
    returns: { id: "integer", email: "string" },
  },
  async ({ email }) => {
    const user = await db.user.create({ data: { email } });
    return { id: user.id, email: user.email };
  },
);

bridge.run();
```

Ruby:

```ruby
WebTest.function(
  "create_user",
  params: { email: :string },
  returns: { id: :integer, email: :string }
) do |args|
  user = User.create!(email: args["email"])
  { id: user.id, email: user.email }
end
```

An Express or Rails application may register these functions in its test boot path, but transport and schema behavior still come from the generic Node or Ruby SDK.

### 5.7 Transport, discovery, and authentication

For a runner-managed application, prefer local IPC:

```text
Unix:       Unix domain socket
Windows:    named pipe
Fallback:   random loopback TCP port
```

The runner creates the endpoint and a cryptographically random per-run token, then supplies both to the launched application:

```text
WEBTEST_BRIDGE=/tmp/webtest-<random>.sock
WEBTEST_TOKEN=<random-secret>
```

The SDK reads those variables and connects to the runner. This avoids port discovery and keeps test control off the application's public HTTP surface. TCP fallback must bind only to loopback. Socket/pipe permissions must be restricted to the current user, tokens must never appear in diagnostics or traces, and stale endpoint files must be cleaned safely.

An HTTP transport may exist for constrained environments, but `http://localhost:3000/__webtest` must not be the default. The public application server and the privileged test bridge are separate security boundaries.

### 5.8 No-SDK and legacy escape hatches

Official SDKs are conveniences, not infrastructure requirements. Any executable can implement the protocol over stdin/stdout:

```toml
[server.app]
adapter = "bridge"
command = "./scripts/webtest-bridge"
transport = "stdio"
schema = ".webtest/app-schema.json"
```

The runner keeps this child alive for the test scope and exchanges the same `hello`/`describe`/`call`/`result` messages over its pipes. The executable can be written in Bash, Perl, PHP, Haskell, Clojure, or an unsupported legacy stack.

A slower per-call command compatibility adapter may also map `app.create_user(...)` to an executable receiving arguments as JSON on stdin and returning one JSON result on stdout. It must use the same schema model and structured errors, must never invoke a shell implicitly, and should be documented as a compatibility mode rather than the preferred bridge lifecycle.

Configuration selects a host adapter without changing the DSL provider:

```text
bridge   persistent App Bridge Protocol over local IPC, TCP fallback, or stdio
command  compatibility mode invoking an explicit executable per call
http     optional declarative mapping from app functions to test-only HTTP APIs
```

The HTTP adapter must obtain its typed operation mapping from configuration/schema and is distinct from directly authored `http.*` calls. Every adapter returns the same transferable values and structured provider errors; none changes language semantics.

The bridge may run inside the application process or as a sibling fixture process importing domain packages or calling the application's API/database:

```text
                     webtest
                        |
                        v
                   app bridge
                  /          \
                 v            v
             database     application API
```

The DSL and protocol do not care which deployment shape is chosen.

### 5.9 Application lifecycle is configuration

Starting and stopping the application is separate from evaluating `server { ... }`. A project configuration may define:

```toml
[project]
name = "my-node-app"

[app]
command = "npm"
args = ["run", "dev"]
working_directory = "."

[app.environment]
NODE_ENV = "test"
WEBTEST = "1"

[app.health]
url = "http://127.0.0.1:3000/health"
timeout = "10s"

[server]
base_url = "http://127.0.0.1:3000"

[server.app]
adapter = "bridge"
transport = "auto"
schema = ".webtest/app-schema.json"

[browser]
base_url = "http://127.0.0.1:3000"
```

An eventual `webtest test` lifecycle is:

```text
read config and compile plans
    |
    v
create bridge endpoint/token
    |
    v
start application without an implicit shell
    |
    v
wait for health check and bridge handshake
    |
    v
launch/reuse isolated browser resources
    |
    v
execute tests
    |
    v
cancel children, shut down bridge, stop application, collect evidence
```

`server {}` never implicitly means "start the server." Lifecycle resources and teardown must be represented explicitly in runtime scope so cancellation, timeout, and debugger termination cannot leak processes or endpoints.

### 5.10 Built-in HTTP, process, and file providers

The `http` provider supplies typed methods, headers, query parameters, JSON/form bodies, authentication, cookies, redirects, timeouts, and response decoding. Results expose status, headers, body text/bytes, and typed JSON. HTTP assertion failures produce structured diffs, and sensitive headers/configured fields are redacted from logs and traces.

The `process` provider uses an explicit executable, argument array, environment, working directory, stdin, timeout, exit status, stdout, and stderr. It is a native host capability unavailable during WASM execution. Cancellation terminates the child process tree where the platform permits. The runtime never invokes a shell implicitly; shell syntax requires an explicit operation and security warning.

The `fs` provider adds sandbox-aware temporary directories, fixture copying, reads/writes, and file assertions. Prefer typed temporary resources that clean up automatically. Project-relative paths resolve through workspace configuration rather than the process's accidental working directory.

### 5.11 Provider ecosystem

`app.*` is the first externally supplied provider, not a one-off grammar feature. The same capability/provider model should later support typed integrations such as:

```webtest
server {
    let user = app.create_user(email: "alice@example.com")
    let mail = mail.latest(to: user.email)
    let job = queue.find("WelcomeEmailJob")
    let row = postgres.query("select ...")
}
```

Potential providers include `app`, `http`, `postgres`, `redis`, `mail`, `queue`, `fs`, and `process`. Do not embed database-, queue-, or framework-specific syntax into the grammar. Prove the provider trait, schema/type model, capability checks, error model, and lifecycle semantics with `app` before generalizing third-party plugins.

---

## 6. Assertions and diagnostics

Expand the existing `expect <locator>.visible` assertion into typed matchers for equality, inequality, containment, patterns, status codes, JSON structures, additional locator states, URLs, and eventually visual snapshots. Preserve the current behavior in which an assertion is a distinct plan operation with the locator/expression's precise source origin.

Snapshot storage must use deterministic project-relative names. Creating or updating snapshots requires an explicit CLI/editor action; ordinary test execution must never silently accept a new result.

Assertion failures must preserve:

- the expression and matcher source ranges;
- expected and actual typed values;
- a structured diff when applicable;
- relevant page/request/process context;
- source revision, execution, test, and step identity.

Avoid compiling assertions into opaque callbacks. They must remain visible in HIR, `TestPlan`, events, traces, and editor observations.

---

## 7. Structured execution and `TestPlan`

Grow `TestOperation` into a versioned IR containing:

```text
Browser, ServerProviderCall, Assertion
Sequence, Parallel, Race, Retry, Timeout
Acquire, Setup, Teardown
```

`ServerProviderCall` contains the provider namespace, operation name, typed arguments/result, provider capability requirements, offline schema identity, and redaction metadata. Built-in `http`, `process`, and `fs` calls and external `app` calls use the same explicit plan shape even though their runtime adapters differ.

Plans must be deterministic, serializable, and independent of syntax nodes. Every executable or controlling node carries a stable plan ID, source origin, capability requirements, timeout policy, and source revision. Bridge connections, live browser objects, closures, and application SDK objects never appear in a plan.

The runtime scheduler must provide structured concurrency:

- child operations cannot outlive their parent scope;
- cancellation propagates predictably;
- `race` cancels losers and awaits cleanup;
- `retry` records each attempt and its evidence;
- `timeout` distinguishes timeout from cancellation and underlying failure;
- teardown executes once when its resource was acquired.

Parallel test execution should be added only after resource isolation and deterministic event ordering are defined. `--jobs` controls test-level concurrency; concurrency inside a test is expressed by the plan.

---

## 8. Runtime events, observations, and traces

### 8.1 Event schema

Extend the existing event stream with timestamps, parent operation IDs, attempt numbers, durations, captured output, attachments, provider-call identity, and explicit cancellation/infrastructure events. The event schema should be serializable and versioned so terminal reporters, traces, editor observations, DAP, and future remote runners consume the same facts. Bridge arguments/results must be summarized and redacted according to their schemas before entering events.

Reporters subscribe to events. The runner must not print directly.

### 8.2 Observation lifecycle

Add observation kinds for ambiguous locators, assertion mismatches, navigation failures, HTTP failures, console errors, network failures, observed values, timings, and retries.

The store must atomically replace observations for a completed file/revision execution. New runs clear prior current observations; successful reruns remove prior failures. No adapter may publish an observation when its `SourceRevision` differs from the current document revision.

### 8.3 CLI-to-LSP bridge

Implement local, versioned IPC so `webtest test` can publish observations to a running `webtest lsp` process. Use Unix sockets on Unix and named pipes on Windows. The LSP process advertises a workspace-scoped endpoint through a metadata file containing PID, workspace identity, endpoint, and protocol version.

Messages contain canonical path/URI, source revision, execution/test/step IDs, range, observation kind, and evidence references. The receiver validates workspace membership, protocol version, and revision before storing anything.

### 8.4 Trace artifacts

Add a portable trace container containing:

```text
manifest.json
events.jsonl
sources and revisions
attachments/
screenshots/
network/
console/
DOM or accessibility snapshots
```

`webtest trace` should open or serve an HTML viewer. Trace capture levels must be configurable to control size and sensitive data.

---

## 9. CDP and managed browsers

Replace the hand-written CDP subset incrementally with generated typed bindings from a pinned Chrome DevTools Protocol schema. Keep a generic command path for protocol evolution and debugging, but normal browser behavior should use generated request/response/event types.

The connection layer must handle target discovery, flattened sessions, events, request cancellation, disconnects, browser crashes, protocol timeouts, and graceful shutdown. Add bounded queues and prevent a slow observer from blocking CDP reads.

Introduce a `BrowserManager` and commands:

```text
webtest browser install
webtest browser list
webtest browser path
webtest browser clean
```

Each WebTest release should declare a tested Chrome for Testing version and verify archive checksums. Store browsers under a platform cache directory, not inside the executable. Resolution order should be explicit: CLI/config override, `WEBTEST_CHROME_PATH`, managed pinned browser, then supported system discovery.

Chrome/CDP remains the primary backend until browser semantics and actionability are stable. The `BrowserHost` boundary should permit later WebDriver BiDi or other backends for Firefox and WebKit without adding browser-specific constructs to the DSL. Cross-browser support must pass the same semantic browser conformance suite.

Never use the user's browser profile or add `--no-sandbox` by default.

---

## 10. Analysis and workspace model

The current database uses revision-keyed memoization suitable for one-file analysis. Before modules and large workspaces, move to a formal incremental query model, preferably Salsa, or document an equivalent dependency-tracked design.

Required inputs include file text, paths/URIs, configuration, environment profile metadata, provider schema manifests and hashes, and module/package graph. Required queries include parse, HIR, name resolution, types/effects, provider/function resolution, static diagnostics, plan construction, symbols, and editor features.

Full-file reparsing remains acceptable until profiling proves it problematic. Query reuse must prevent independent reparses by formatting, diagnostics, semantic tokens, planning, and editor requests.

---

## 11. Editor services, native LSP, and DAP

### 11.1 Editor services and LSP

Add protocol-neutral services in this order:

1. document symbols, folding ranges, and selection ranges;
2. completion and signature help;
3. hover with resolved types/capabilities;
4. definition, references, and rename;
5. code actions for syntax/type/runtime failures;
6. inlay hints and richer semantic tokens;
7. workspace symbols and test discovery.

Runtime-aware features may show match counts, observed values, timings, or candidate locators, but only for the current source revision.

Tower LSP remains an adapter. Add incremental synchronization, cancellation, progress reporting, pull diagnostics if useful, test discovery commands, and protocol-level integration tests. LSP handlers must not implement name resolution, completion logic, or runtime formatting.

The Cursor/VS Code extension remains TypeScript glue. It may provide commands, test-explorer UI, trace opening, and settings, but no parser or semantic model. Server and UI command identifiers must remain distinct to avoid command-registration collisions.

Provider schemas are analysis inputs. Completion and hover for `app.create_user`, its named arguments, and its returned record must work from `.webtest/app-schema.json` while the application and bridge are stopped. Live bridge discovery may report schema drift or runtime evidence, but it must not become the editor's only source of types.

### 11.2 Debug Adapter Protocol

The shipped DAP foundation remains an adapter over `TestPlan`, `Runner`, and `RunControl`. Breakpoints resolve source lines to executable plan steps, and execution pauses before the selected operation so a headed browser can be inspected. Normal runs and debug runs must never acquire separate runtime semantics.

Extend debugging with:

- variables for lexical bindings, typed provider results, assertion values, and bounded/redacted evidence;
- source-mapped frames for nested sequence/retry/fixture scopes;
- pause, continue, step in/over/out, restart, and cancellation semantics aligned with structured concurrency;
- exception breakpoints for assertion, infrastructure, provider, and internal failures;
- optional browser inspection links or metadata without pretending remote DOM nodes are transferable DSL values;
- deterministic behavior when a breakpoint is placed on a non-executable line or a plan no longer matches the source revision.

DAP uses one-based source coordinates and owns stdout while active. Protocol logging goes to stderr. Cursor/VS Code contributes debugger configuration and UI only; breakpoint mapping, scopes, and values remain in Rust.

---

## 12. WASM, Monaco, and browser editor package

Expand `webtest-wasm` from diagnostics/formatting into stable DTO-based APIs:

```text
openDocument(uri, text)
updateDocument(uri, text)
closeDocument(uri)
diagnostics(uri)
format(uri)
semanticTokens(uri)
completion(uri, offset)
hover(uri, offset)
compileTestPlan(uri)
```

Run the WASM module in a Web Worker and communicate with Monaco through messages. Monaco should call editor services directly; it should not require an in-browser LSP server.

Publish `@webtest/editor` containing WASM, worker glue, TypeScript declarations, and a Monaco adapter. Native-only plan operations should still compile in WASM and be marked as requiring host capabilities; the browser build does not launch Chrome or native processes.

Parity tests must run the same syntax, diagnostics, formatting, semantic-token, and plan fixtures against native and WASM builds.

---

## 13. CLI, reporting, and developer experience

Add human, concise, JSON, JUnit, and event-stream reporters. Color must be disabled when output is not a terminal or when requested. Machine-readable output schemas must be versioned.

CLI diagnostics should show source snippets, labels, related ranges, and suggested fixes. Test output should distinguish assertion/test failure from browser/process infrastructure failure and internal bugs through stable exit codes.

Support directory discovery, ignore rules, test/tag filtering, fail-fast, deterministic seeds, default/configurable timeouts, artifact directories, application lifecycle, and provider configuration. Watch mode may be added after incremental workspace invalidation is reliable.

`repl` is a late feature and must reuse the language and runtime rather than evaluating a second ad hoc command language.

---

## 14. Security and privacy

The product intentionally drives browsers, networks, files, and processes, but it must still:

- safely serialize values crossing into JavaScript or protocols;
- avoid implicit shell interpretation;
- redact configured secrets from logs, diagnostics, IPC, and traces;
- bind CDP and local IPC only to local/private endpoints;
- authenticate app bridges with per-run secrets, restrict local endpoint permissions, and reject schema/protocol drift;
- reserve stdio bridge stdout for protocol frames and bound every bridge message;
- use isolated temporary browser profiles;
- validate downloaded browser checksums;
- bound captured body, DOM, console, and process output sizes;
- make destructive file/process operations explicit;
- document that arbitrary test projects should be treated as executable code.

WASM/editor analysis must not gain ambient filesystem or network access.

---

## 15. Testing and quality gates

Every language feature needs:

- lossless valid, invalid, and half-typed syntax fixtures;
- typed AST and exact-range tests;
- HIR/type/effect diagnostics tests;
- deterministic plan snapshots;
- fake-host runtime tests for success, failure, retry, timeout, and cancellation;
- editor revision-safety tests;
- LSP UTF-16 and protocol tests;
- DAP framing, source-to-step breakpoint, pause/continue, and revision tests;
- provider-schema type/completion tests and bridge protocol conformance tests;
- native/WASM parity coverage where portable.

Use real Chrome tests for navigation, locators, actionability, input, contexts, events, and evidence. Serve fixture pages from random loopback ports and skip only when the environment truly lacks Chrome or socket capability. Add parser fuzzing and property tests for losslessness and non-panicking malformed input.

Required gates remain:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd editors/vscode && npm run compile
cargo check -p webtest-wasm --target wasm32-unknown-unknown
```

Release CI should additionally run cross-platform browser integration tests, package/install smoke tests for the VSIX and npm editor package, and the same bridge conformance corpus against every official SDK.

---

## 16. Distribution

Ship precompiled `webtest` binaries for supported macOS, Linux, and Windows targets. Distribution may include release archives, shell/PowerShell installers, Homebrew, winget/Scoop, an optional npm launcher, and a container image.

The official CI container should contain `webtest`, the matching managed Chrome for Testing build, and required Linux libraries. Chromium itself should not be embedded in the Rust executable.

Serializable plans and versioned events may later support remote workers. A remote runner must negotiate plan/event protocol versions and host capabilities, preserve source revisions, and produce the same events as local execution; it must not introduce a second compiler.

Publish the Cursor/VS Code extension as a standard VSIX and, eventually, through compatible marketplaces. Keep server discovery configurable and do not bundle a platform-specific executable into one universal VSIX unless a deliberate multi-platform packaging design is adopted.

Version the App Bridge Protocol independently from any one SDK while documenting the compatibility matrix supported by each WebTest release. Publish canonical schemas, generated types, and the conformance corpus together. Official language SDKs for Node/TypeScript, Ruby, Go, Python, Elixir, JVM languages, .NET, and other ecosystems should follow demonstrated demand; all remain thin bindings over the same contract. Framework convenience packages may release separately but must declare their core SDK and protocol compatibility.

---

## 17. Delivery milestones

### Delivered foundation

The lossless language pipeline, source-mapped browser plan, direct CDP execution, revision-safe runtime diagnostics, `id`/`text` locators, visibility assertions, headed test mode, CST-backed semantic tokens, Tower LSP, Cursor/VS Code VSIX, and source-mapped DAP debugger are implemented. Future milestones extend these components; they must not replace them with adapter-specific implementations.

### [Milestone A — Productize the proven slice](./milestone-a.md)

- project configuration and path discovery;
- managed Chrome for Testing;
- improved CLI diagnostics/reporters and stable exit codes;
- robust CDP disconnect/timeouts;
- protocol-level LSP and DAP tests plus packaged extension smoke tests.

Acceptance: a new user can install WebTest and its browser, run the examples, and see current runtime diagnostics without manually locating Chrome.

### [Milestone B — Useful browser testing](./milestone-b.md)

- semantic locators;
- fill, press, select, check, hover, and waits;
- actionability and auto-waiting;
- locator and page-state assertions;
- browser reuse with isolated contexts;
- screenshots and core evidence.

Acceptance: common form and navigation flows are reliable without manual sleeps or CSS selectors.

### [Milestone C — Typed server/browser workflows](./milestone-c.md)

- expressions, bindings, records, and functions;
- static types and execution capabilities;
- provider namespaces and explicit `ServerProviderCall` plan operations;
- built-in HTTP, process, and file providers;
- transferable cross-domain values;
- typed assertions and structured diffs.

Acceptance: one test can prepare state through black-box HTTP, use the typed result in a browser, and receive static errors for invalid domain/type transfers. `server {}` is demonstrably a runner capability scope rather than an application-process boundary.

### [Milestone D — Language-neutral application bridge](./milestone-d.md)

- canonical versioned protocol schemas, documentation, generated Rust DTOs, and conformance corpus;
- typed `app.*` provider calls and deterministic `.webtest/app-schema.json` loading;
- local socket/named-pipe discovery, per-run authentication, loopback fallback, and persistent stdio executable mode;
- runner-managed application lifecycle and health checks, separate from `server {}` execution;
- reference SDKs in enough distinct ecosystems to prove language neutrality, with optional framework lifecycle helpers;
- offline LSP completion/type checking and runtime schema-drift validation.

Acceptance: the same WebTest source calls `app.create_user` against applications in at least two different host languages and against a no-SDK executable. All three pass one protocol conformance suite, return the same typed DSL record, and provide editor completion while the applications are stopped.

### [Milestone E — Structured execution and observability](./milestone-e.md)

- parallel/race/retry/timeout plan nodes;
- cancellation-safe resource lifecycles;
- expanded event and observation schemas;
- CLI-to-LSP observation IPC;
- trace artifact and viewer;
- richer DAP scopes, failure breakpoints, and structured-concurrency stepping.

Acceptance: concurrent/retried tests remain deterministic, diagnosable, and source-mapped in terminal, trace, and editor.

### [Milestone F — Workspace and editor intelligence](./milestone-f.md)

- incremental query database and module graph;
- completion, hover, navigation, rename, symbols, actions, and test discovery;
- incremental LSP synchronization;
- modules and reusable fixtures.

Acceptance: multi-file projects remain responsive, provider schemas invalidate correctly, and every editor feature derives from shared Rust semantics.

### [Milestone G — Portable editor and distribution](./milestone-g.md)

- complete WASM editor-service facade and worker;
- Monaco adapter and `@webtest/editor`;
- native release automation, VSIX publishing, and CI container;
- native/WASM parity suite.

Acceptance: native, Cursor/VS Code, and Monaco experiences agree on syntax, formatting, diagnostics, semantic tokens, and compiled plans.

---

## 18. Definition of long-term success

WebTest is successful when a user can install one native product, author statically checked tests spanning server and browser domains, run them reliably with managed Chrome and structured concurrency, inspect rich traces, and receive current runtime facts directly in their editor.

The same Rust language implementation must power CLI, runtime planning, Cursor/VS Code, and Monaco. Applications in any host language can expose typed fixtures through one small bridge contract, and SDKs remain ergonomic protocol bindings rather than alternate WebTest implementations. Adding functionality must deepen the shared compiler/runtime/provider architecture rather than creating adapter-, framework-, or language-specific parsers, semantics, or diagnostics.
