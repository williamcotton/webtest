# Milestone D — Language-Neutral Application Bridge

## 0. Status and dependencies

**Status: implemented (2026-08-24).** Protocol-1 schemas and generated DTOs, offline analysis,
runtime transports/adapters/lifecycle, Node and Ruby SDKs, the no-SDK executable, native/LSP/DAP/WASM
editor parity, shared black-box conformance, and all nine web-server examples are present. The
repository verification commands and CI matrix enforce deterministic schemas, stopped-app analysis,
end-to-end execution, owned-process/port cleanup, and LSP invalidation when the resolved offline
manifest or project configuration changes.

This specification expands Milestone D in [`future-functionality.md`](./future-functionality.md). It depends on the typed values, provider schemas, capability analysis, `ServerProviderCall` IR, and built-in providers from [`milestone-c.md`](./milestone-c.md), plus the static description, machine-diagnostic, and repair-hint contracts from [`milestone-c-5.md`](./milestone-c-5.md).

Milestone D adds one stable application-fixture integration contract. Node, Ruby, Go, Python, Elixir, Java, .NET, Rust, PHP, and unofficial tools do not implement WebTest; they implement or bind the same small bridge protocol.

## 1. Outcome

The same test source works against applications written in different host languages:

```webtest
test "created user can sign in" {
    server {
        let user = app.create_user(email: "alice@example.com")
    }

    browser {
        open "/login"
        fill label("Email") with user.email
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
```

The `app.create_user` signature is available to `webtest describe`, `check`, LSP, DAP, WASM analysis, and plan compilation while the application is stopped. Its bounded documentation, parameter semantics, result type, capability, redaction, and retry-safety metadata come from the same provider schema used by analysis. At runtime, the call is transported to an application bridge and returns a validated transferable record.

## 2. Architectural rule

`server {}` is the test runner's server-capability domain. `app` is one configured provider within that domain. A bridge may run in the application process or a sibling process, but the language does not observe that deployment choice.

```text
WebTest DSL
    |
    v
server { app.create_user(...) }
    |
    v
typed ServerProviderCall
    |
    v
AppProvider in WebTest runtime
    |
    v
App Bridge Protocol
    |
    +----------+----------+----------+----------+
    |          |          |          |          |
    v          v          v          v          v
  Node       Ruby        Go       Python    any executable
```

No SDK parses `.webtest`, constructs `TestPlan`, drives Chrome, publishes editor diagnostics, or changes provider semantics. The bridge is an application-integration protocol, not an agent protocol; external tools discover `app.*` through the ordinary C.5 description and diagnostic interfaces.

## 3. Scope

Milestone D includes:

- a canonical, versioned App Bridge Protocol and schema;
- generated Rust protocol DTOs plus generators/templates for SDK wire types;
- typed `app.*` resolution from `.webtest/app-schema.json`;
- bounded operation/parameter/result documentation plus optional/default/secret/retry-safety metadata;
- participation in `webtest describe` and shared machine-readable diagnostic/repair DTOs;
- runtime schema verification and drift errors;
- Unix-domain-socket, Windows named-pipe, loopback TCP fallback, and stdio transports;
- per-run authentication and local endpoint discovery;
- persistent bridge, stdio executable, per-call command, and declarative HTTP adapters;
- runner-managed application start, health check, bridge readiness, and teardown;
- thin reference SDKs in at least two distinct ecosystems;
- a runnable cross-language example suite in which real web servers share application state with the bridge;
- a language-neutral protocol conformance suite;
- optional framework lifecycle helpers that depend on a core language SDK.

## 4. Non-goals

This milestone does not execute arbitrary source code named in the DSL, expose a privileged public HTTP endpoint by default, transmit live ORM/browser/process objects, define framework-specific DSL syntax, load arbitrary native plugins into the `webtest` process, provide remote/cloud bridges, implement an agent-specific protocol, or implement mail/database/queue providers. The cross-language examples are not production starter templates or a commitment to support every framework in each ecosystem. Structured concurrency beyond lifecycle-safe cleanup remains Milestone E.

## 5. Protocol package and compatibility

The repository gains a canonical protocol package:

```text
protocol/
├── protocol.md
├── schema.json
├── types.json
├── examples/
├── conformance/
└── generated/
```

`protocol.md` is normative for state transitions and semantics. JSON schemas validate every wire message and offline manifest. Generated output is reproducible and checked in only when needed by downstream package tooling.

Protocol versions are positive integers. A version defines message shapes, required behavior, and compatibility. Adding optional fields is backward-compatible within a version only when unknown fields are already required to be ignored. Renaming/removing fields, changing meaning, or changing framing requires a new protocol version.

Each SDK declares:

```text
SDK name and version
minimum/maximum protocol version
generated schema revision
supported transports
```

The WebTest release documentation publishes the tested compatibility matrix. Protocol negotiation selects the highest mutually supported version; no overlap produces a structured handshake failure.

## 6. Framing and connection state

### 6.1 Framing

Version 1 uses one UTF-8 JSON object per line. Newlines inside string values are JSON-escaped. Each frame has a configurable maximum size with a conservative default. Invalid UTF-8, invalid JSON, non-object messages, or oversized frames terminate the connection with a bounded diagnostic.

Stdio stdout and socket bytes contain protocol frames only. SDK logs go to stderr or a host-language logger configured away from the protocol stream.

### 6.2 State machine

```text
Connected
    |
    v
AwaitHello
    |
    +--> reject/auth/version failure --> Closing
    |
    v
Ready
    |
    +--> describe / call / cancel / ping / event
    |
    v
Draining
    |
    v
Closed
```

Only `hello` is accepted before readiness. After `shutdown`, new calls are rejected, in-flight calls receive a bounded grace period, and the bridge confirms shutdown before close. EOF during a call becomes an infrastructure failure for that call and all pending calls.

## 7. Message contract

All request/response operations use an unsigned correlation ID unique among in-flight requests on that connection. Unknown optional fields are ignored; unknown message types produce a protocol error without panic.

### 7.1 Handshake

Bridge to runner:

```json
{
  "type": "hello",
  "protocol_versions": [1],
  "sdk": "webtest-python",
  "sdk_version": "0.4.0",
  "token": "<per-run-secret>",
  "capabilities": {
    "cancel": true,
    "events": true
  }
}
```

Runner to bridge:

```json
{
  "type": "hello_ok",
  "protocol": 1,
  "run_id": "01K...",
  "max_message_bytes": 1048576
}
```

Authentication or negotiation failure returns `hello_error` with a stable code and closes the connection. Tokens are compared safely and never logged.

### 7.2 Schema discovery

```json
{ "type": "describe", "id": 1 }
```

```json
{
  "type": "schema",
  "id": 1,
  "protocol": 1,
  "schema_hash": "blake3:<hex>",
  "functions": {
    "create_user": {
      "documentation": "Create a user directly in the test database.",
      "retry_safe": false,
      "params": {
        "type": "object",
        "fields": {
          "email": {
            "type": "string",
            "documentation": "Unique sign-in email.",
            "secret": false
          },
          "admin": {
            "type": "boolean",
            "documentation": "Grant administrative access.",
            "optional": true,
            "default": false,
            "secret": false
          }
        }
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

The hash is computed from one specified canonical JSON encoding of semantic schema content. Documentation-only changes may be excluded only if the canonicalization rules say so. Type, required/optional/default, secret, capability, and retry-safety changes are semantic and participate in compatibility/hash decisions.

### 7.3 Calls and results

```json
{
  "type": "call",
  "id": 42,
  "function": "create_user",
  "arguments": {
    "email": "alice@example.com"
  },
  "deadline_ms": 10000
}
```

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

The SDK validates incoming arguments and outgoing values against the registered schema. The runner validates them again at the trust boundary. Duplicate IDs, results for unknown IDs, multiple terminal responses, or values outside the schema are protocol failures.

### 7.4 Errors

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

Application errors are test/provider failures. Transport loss, malformed messages, invalid schema results, and bridge process death are infrastructure failures. SDKs do not transmit stack traces by default. Debug details use a separately bounded/redacted field when enabled.

### 7.5 Cancellation, events, liveness, and shutdown

```json
{ "type": "cancel", "id": 42, "reason": "test_timeout" }
{ "type": "event", "call_id": 42, "kind": "progress", "value": { "phase": "creating" } }
{ "type": "ping", "id": 7 }
{ "type": "pong", "id": 7 }
{ "type": "shutdown", "id": 9 }
{ "type": "shutdown_ok", "id": 9 }
```

Cancellation is cooperative but observable. The SDK cancels the host-language context/task when supported and eventually returns one terminal cancellation error/result. Events are bounded, ordered per call, and cannot contain unredacted secrets or arbitrary binary bodies; artifacts are referenced through separately controlled mechanisms.

## 8. Schema and value model

The initial bridge schema uses exactly the transferable Milestone C value model:

```text
null, boolean, integer, float, string
array<T>, optional<T>, object fields
```

Semantic aliases such as `Url`, `Email`, `DateTime`, and `UserId` declare a wire base type plus validation/display metadata. Schema recursion, arbitrary unions, executable validators, host-language class names, and object identity are not included in protocol version 1.

Every function schema also carries bounded human documentation and explicit execution metadata. Parameter schemas distinguish required parameters from optional parameters, encode any typed default, and mark secret values before diagnostics/events are constructed. Retry safety is operation-level metadata because it describes whether repeating the call is semantically permitted; a particular application error's `retryable` field remains the runtime fact describing whether that failure may be retried. Milestone E requires both the enclosing retry policy and the operation metadata to permit repetition.

Documentation is plain bounded text, not executable markup or an alternate semantics channel. Analysis, editor services, `webtest describe`, and SDK schema export all consume the same `ProviderSchema`, `OperationSchema`, `ParameterSchema`, and `TypeSchema` representation.

These values cannot cross the bridge:

```text
ActiveRecord/Ecto/ORM instances
database connections or transactions
Node streams, Ruby IO, file descriptors
browser Page/Element objects
closures, functions, process handles, sockets
```

An SDK converts application objects to declared DTO records before returning them.

## 9. Offline manifest and static analysis

SDK tooling emits deterministic `.webtest/app-schema.json`:

```json
{
  "manifest_version": 1,
  "protocol": 1,
  "provider": "app",
  "sdk": "webtest-ruby",
  "sdk_version": "0.4.0",
  "schema_hash": "blake3:<hex>",
  "functions": {}
}
```

Project configuration identifies the manifest:

```toml
[server.app]
adapter = "bridge"
transport = "auto"
schema = ".webtest/app-schema.json"
```

Analysis treats the manifest as a revision-keyed input. It resolves `app.create_user`, validates arguments, infers the result record, powers completion/hover/signature help, contributes the `app` provider to the static C.5 description DTO, and embeds the schema hash in every related plan call.

Unknown functions/arguments/members, missing arguments, invalid defaults, and type/capability mismatches use the stable machine diagnostic codes and semantic-detail fields established in C.5. Bounded name/member/argument repair candidates come from the same resolved schema; the CLI, LSP, DAP, and WASM adapters do not infer them from formatted messages or maintain a second app-function registry.

At runtime, the live `describe` schema must match the planned hash. Mismatch stops affected tests before the first app call with a schema-drift infrastructure diagnostic showing expected/live hashes and regeneration instructions. WebTest does not silently fall back to dynamic calls.

## 10. Transports and discovery

### 10.1 Runner-managed application bridge

The default local flow is:

```text
webtest creates endpoint + random token
    |
    v
webtest launches configured application with environment
    |
    v
application SDK connects outward to endpoint
    |
    v
hello/authenticate/describe
```

Use a Unix domain socket on Unix and a named pipe on Windows. Socket/pipe permissions are restricted to the current user. If native local IPC is unavailable, use a random loopback TCP listener; never bind an unspecified/public interface.

The runner supplies:

```text
WEBTEST_BRIDGE=<socket, pipe, or loopback endpoint>
WEBTEST_TOKEN=<cryptographically random secret>
WEBTEST_PROTOCOL=1
```

### 10.2 Persistent stdio executable

Any executable can implement the same protocol:

```toml
[server.app]
adapter = "bridge"
transport = "stdio"
command = "./scripts/webtest-bridge"
schema = ".webtest/app-schema.json"
```

The runner spawns it without a shell, writes protocol requests to stdin, reads responses from stdout, captures bounded stderr as logs, and keeps it alive for the configured test scope.

### 10.3 Compatibility adapters

`command` invokes an explicit executable per call with one JSON arguments document on stdin and one JSON result/error document on stdout. It uses the same offline schema but is slower and has no persistent events.

`http` maps typed `app.*` functions to explicitly configured test-only endpoints. It is distinct from directly authored `http.*` calls. It must never infer or expose `/__webtest` automatically.

Adapters change transport/hosting only. They return the same typed values and structured provider errors and do not change DSL semantics.

## 11. Application lifecycle

Application startup belongs to project configuration, not `server {}`:

```toml
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

Lifecycle phases are explicit events:

```text
configuration resolved
bridge endpoint created
application spawned
health check ready
bridge connected/authenticated/schema verified
tests executed
bridge drained/shutdown
application terminated/reaped
endpoint/profile/artifacts finalized
```

Startup uses executable/argument arrays and never an implicit shell. Health readiness and bridge readiness have separate deadlines and errors. If WebTest owns the application process, it always attempts bounded teardown after failure, cancellation, or DAP disconnect. An externally managed application can disable process ownership while retaining bridge checks.

## 12. SDK architecture

The generic language SDK owns:

- function registration and duplicate detection;
- schema construction/export;
- generated wire DTOs/codecs;
- transport selection and framing;
- argument/result validation;
- host error conversion and cancellation;
- lifecycle hooks and logging separation.

Framework helpers only arrange boot and lifecycle:

```text
@webtest/node -> optional @webtest/express
webtest Ruby  -> optional webtest-rails
webtest Python -> optional webtest-django/webtest-fastapi
```

Do not implement the protocol separately in Express, Nest, Next, Rails, Sinatra, Django, FastAPI, Flask, Phoenix, Spring, or ASP.NET packages.

Reference SDK APIs should feel native while producing identical schemas/messages. Milestone D ships at least two independently implemented SDKs—recommended TypeScript/Node and Ruby—to prove the contract, plus the stdio fixture executable. Generated type packages/templates should make Go, Python, Elixir, JVM, and .NET SDKs straightforward follow-up work.

### 12.1 Cross-language web-server examples

Milestone D ships a visible product example for every host-language family named by this specification, not only protocol-level fixtures. The initial matrix is:

| Directory | Host language/runtime |
| --- | --- |
| `examples/application-bridge/node/` | TypeScript on Node.js |
| `examples/application-bridge/ruby/` | Ruby |
| `examples/application-bridge/go/` | Go |
| `examples/application-bridge/python/` | Python |
| `examples/application-bridge/elixir/` | Elixir |
| `examples/application-bridge/java/` | Java on the JVM |
| `examples/application-bridge/dotnet/` | C# on .NET |
| `examples/application-bridge/rust/` | Rust |
| `examples/application-bridge/php/` | PHP |

These are runnable application examples, distinct from the transport-independent protocol examples and conformance corpus under `protocol/`. Each directory is a self-contained WebTest project and contains:

- one byte-identical `created-user.webtest` scenario shared by the matrix;
- a checked-in `.webtest/app-schema.json` so `describe`, `check`, editor features, and WASM analysis work without starting the application;
- a minimal, conventional HTTP server written in the named host language;
- bridge registration and startup in that application's test boot path;
- native dependency and lock files where that ecosystem supports them;
- a README with exact prerequisites, install, schema-regeneration, check, and test commands.

The common scenario calls `app.create_user`, then signs in through the application's public browser UI. `create_user` must write to the same in-process application store read by the login handler. It must not call a test-only HTTP route, write a canned result, or delegate state to a separately implemented sample server. This demonstrates the defining bridge property: typed fixture code and the web application can use host-language objects internally while only a declared transferable record crosses into WebTest.

All examples expose the same semantic operation schema and browser-observable behavior:

```text
create_user(email: String, admin?: Bool = false)
    -> { id: Int, email: String, admin: Bool }

GET /health
GET /login
POST /login
```

Markup, package layout, and server libraries may follow host conventions, but accessible labels, status text, and navigation used by `created-user.webtest` remain equivalent. Each application starts with isolated in-memory state, binds only to loopback, supports runner-owned startup and teardown, and connects outward through the authenticated runner-managed bridge endpoint. Examples use the generic core SDK when one exists. An ecosystem without a published SDK may keep a small example-local binding generated from the canonical protocol types; that binding implements only protocol transport, registration, schema export, validation, and error conversion, and must not parse the DSL or duplicate WebTest semantics. Example-local bindings are not advertised as supported SDK packages.

`examples/application-bridge/README.md` presents the matrix from one entry point, explains which examples use official SDKs versus example-local bindings, and gives a command that selects and runs one host at a time. The existing `examples/simple-server/` remains the built-in `http.*` provider example; it must not be relabeled as an application-bridge example.

Repository checks enforce byte equality of the common `.webtest` scenario, semantic equality of exported operation schemas, and clean deterministic schema regeneration. Each example has an end-to-end smoke job on at least one supported CI platform. Smoke jobs allocate loopback ports dynamically and prove process cleanup; they do not depend on the fixed ports documented for optional manual runs. A missing local host toolchain may produce an explicit skip, but release CI for the example's declared platform may not treat a missing toolchain or bridge connection as success.

## 13. Runtime and plan integration

`app.*` lowers to the existing Milestone C shape:

```text
ServerProviderCall {
    provider: "app",
    operation: "create_user",
    typed_arguments,
    result_binding,
    result_type,
    schema_hash,
    timeout,
    retry_safety,
    redaction,
    origin,
}
```

An `AppProvider` implements the same `ServerProvider` trait as built-in providers. It owns transport correlation and converts bridge results/errors into shared `Value`/`ProviderError` types. The runner remains unaware of Node, Rails, Python, or any SDK package.

Events include provider/function, duration, execution/test/step identity, transport kind, and redacted value summaries. Tokens, secrets, raw stack traces, and unbounded application output never enter observations or traces.

## 14. Editor and debugger behavior

Offline schema powers:

- `webtest describe` entries for `app.*` operations and their metadata;
- completion for `app.` functions;
- signature help and named-argument completion;
- hover documentation and result types;
- member completion on returned records;
- diagnostics for unknown calls/fields, missing arguments, and type mismatch.

These work while the app is stopped in the native CLI/LSP and WASM analysis. Description, diagnostics, semantic details, and repair hints use the versioned C.5 DTOs. The extension contains no schema parser; it displays Rust editor-service results.

DAP pauses before an app call. Scopes show already evaluated arguments and, after stepping, the redacted result. Bridge/app lifecycle failures can participate in infrastructure exception breakpoints, while application `error` messages participate in provider-failure breakpoints.

## 15. Security and privacy

- Generate a new high-entropy token and endpoint for each owned run.
- Restrict socket/pipe/cache permissions and reject non-loopback TCP endpoints by default.
- Do not put the token in plans, events, diagnostics, artifacts, process listings, or traces.
- Bound frames, pending calls, events, stderr, result depth, string lengths, and collection sizes.
- Validate function names/arguments/results against the planned schema on both sides.
- Redact schema-marked values before formatting or event construction.
- Never enable the bridge outside an explicit test environment in official helpers.
- Treat bridge functions as privileged test code and document their production-disable requirements.
- Do not expose a default application HTTP control route.

## 16. Conformance suite

One black-box suite runs against every SDK/executable. It covers:

- successful/failed version negotiation and authentication;
- deterministic schema generation/hash and `describe`;
- bounded documentation plus required/optional/default/secret/retry-safety metadata and compatibility behavior;
- all primitive/nested/optional value forms;
- valid calls, application errors, validation errors, and Unicode;
- concurrent calls, out-of-order results, duplicate/unknown IDs;
- cancellation, deadlines, ping/pong, graceful shutdown, and abrupt EOF;
- malformed JSON, invalid UTF-8, oversized/deep messages, unknown fields/types;
- stdout/log separation and redaction;
- local IPC, TCP fallback where supported, and stdio.

The corpus consists of transport-independent input/output fixtures plus an executable harness. SDK-specific tests may verify idioms, but passing the shared suite is the compatibility gate.

## 17. Delivery slices

1. Publish normative protocol/schema documents and generate Rust DTOs/codecs.
2. Add offline app-schema loading, hashing, shared `describe` projection, machine diagnostics/repair details, and plan integration.
3. Implement `AppProvider` correlation/state machine against an in-memory transport.
4. Add local socket/named-pipe/TCP discovery and authentication.
5. Add persistent stdio, command, and declarative HTTP adapters.
6. Add application process/health/bridge lifecycle orchestration.
7. Build the conformance harness and no-SDK reference executable.
8. Implement and package the first language SDK, then a structurally different second SDK.
9. Add the shared application-bridge scenario and the Node/Ruby examples using the reference SDKs.
10. Add the Go, Python, Elixir, Java, .NET, Rust, and PHP web-server examples using generated example-local bindings where an official SDK does not yet exist.
11. Add editor completion/hover/signature behavior, C.5 description/repair parity, DAP values/failures, the cross-language example matrix, and documentation.

Do not begin framework helper packages until their generic language SDK passes conformance independently.

## 18. Acceptance criteria

Milestone D is complete only when:

1. One unchanged `.webtest` file calls `app.create_user` successfully against applications using two different official language SDKs.
2. The same call succeeds against an independently implemented stdio executable with no SDK.
3. All three implementations pass the same protocol conformance suite.
4. `webtest describe`, `check`, LSP, and WASM expose the same documented `app.*` signatures, call/member diagnostics, semantic details, repair hints, and completion from `.webtest/app-schema.json` with the applications stopped.
5. Runtime rejects schema drift, bad authentication, invalid values, malformed frames, and oversized messages with distinct structured failures.
6. Owned app/bridge processes and endpoints are cleaned after success, timeout, test failure, and DAP termination.
7. The byte-identical `created-user.webtest` scenario passes against the Node, Ruby, Go, Python, Elixir, Java, .NET, Rust, and PHP web-server examples, with `app.create_user` mutating the same application state used by each login handler.
8. Every cross-language example regenerates the same semantic operation schema deterministically, can be checked while stopped, has reproducible run instructions, and has an end-to-end CI smoke job that verifies cleanup.
9. No framework-specific parser, plan operation, runtime branch, or editor semantic model is introduced.

The roadmap acceptance statement is thereby satisfied: different language stacks and a no-SDK executable expose the same typed application function through one stable bridge protocol. The broader example matrix additionally demonstrates that the contract is practical across the host-language families named by this milestone.
