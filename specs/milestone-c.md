# Milestone C — Typed Server/Browser Workflows

## 0. Status and dependencies

This specification expands Milestone C in [`future-functionality.md`](./future-functionality.md). It depends on the product/configuration foundation in [`milestone-a.md`](./milestone-a.md) and reliable browser semantics in [`milestone-b.md`](./milestone-b.md).

Milestone C introduces the expression, type, capability, provider, and value-transfer foundations consumed next by semantic inspection and machine feedback in [`milestone-c-5.md`](./milestone-c-5.md) and required by the language-neutral application bridge in [`milestone-d.md`](./milestone-d.md). It proves those foundations first with built-in HTTP, process, and filesystem providers.

**Implementation status (2026-08-22): complete.** The shared Rust pipeline now implements the syntax, typed HIR, static analysis, provider schemas, serializable plan, native providers, typed runtime, revision-bound observations, editor/LSP/DAP services, and WASM compilation DTOs described here. Compiler, runtime, editor, and protocol tests cover the typed server-to-browser workflow.

Plan emission has a deliberately strict secret policy: `webtest build --emit` rejects literal values in schema-secret arguments and configured sensitive HTTP header/JSON fields. A future late-bound secret source may make those plans emit-safe; the current compiler never substitutes a redacted placeholder that would change runtime behavior.

## 1. Outcome

A test can prepare state through a typed black-box server operation, make assertions about the result, transfer serializable values into a browser flow, and receive static diagnostics for invalid calls or domain crossings:

```webtest
test "created user can sign in" {
    server {
        let response = http.post("/api/test/users", json: {
            email: "alice@example.com",
        })
        expect response.status == 201

        let user: { id: Int, email: String } = response.json
    }

    browser {
        open "/login"
        fill label("Email") with user.email
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
```

`server {}` is a runner capability scope. It does not mean code executes inside the application process and it does not start the application.

## 2. Scope

Milestone C includes:

- literal, collection, name, member, call, unary, and binary expressions;
- named arguments and local `let` bindings;
- static primitive, collection, record, response, and host-resource types;
- explicit `Pure`, `Server`, `Browser`, and `Test` execution capabilities;
- provider namespaces and typed operation signatures;
- explicit `ServerProviderCall` plan nodes;
- built-in `http`, `process`, and `fs` providers;
- runtime values and validated server-to-browser transfer;
- typed equality/state assertions and structured diffs;
- `webtest build --emit` for a versioned serializable plan.

## 3. Non-goals

This milestone does not implement semantic page inspection, `webtest describe`, machine repair hints, `app.*`, bridge protocols/SDKs, application startup, modules/imports, user-defined function declarations, reusable fixtures, parallel/race/retry blocks, remote workers, database-specific syntax, or arbitrary plugin loading. Function **calls** and typed provider signatures are included; Milestone C.5 exposes them for machine discovery, and multi-file helper/fixture declarations belong to Milestone F.

## 4. Language surface

### 4.1 Expressions

Required expression forms are:

```text
String, Int, Float, Bool, Null, Duration literals
list and record literals
name references
member access
function/provider calls with positional and named arguments
parenthesized expressions
unary ! and numeric -
binary ==, !=, <, <=, >, >=, +, -, *, /, &&, ||
```

Examples:

```webtest
let retries = 3
let timeout = 5s
let headers = {
    "x-test-run": "signup",
    "accept": "application/json",
}
let tags = ["smoke", "browser"]
let successful = response.status >= 200 && response.status < 300
```

Operator precedence and associativity are fixed in the grammar and documented. Error recovery must preserve the enclosing statement/block when an operator or operand is missing.

### 4.2 Bindings and scopes

`let name = expression` creates a stable `BindingId`. A type annotation is optional:

```webtest
let user: { id: Int, email: String } = response.json
```

The test body is a sequential flow scope. `server` and `browser` select capabilities but do not hide transferable bindings from later statements in the same test. A value produced in `server` is available to a later `browser` block only after successful execution and only when its type is transferable.

Duplicate declarations in the same flow scope are errors. Nested control/function scopes may shadow an outer binding, but every reference resolves to a semantic ID, never a name string. Use-before-definition is an error. Failed operations do not produce a usable binding.

### 4.3 Provider calls

Built-in providers are explicit namespaces:

```webtest
http.get("/api/users", query: { active: true })
http.post("/api/users", json: { email: "alice@example.com" })
process.run("bin/seed", args: ["--count", "1"], timeout: 10s)
fs.read_text("fixtures/user.json")
fs.write_text("tmp/result.txt", contents: "ok")
```

Bare calls do not resolve implicitly to a provider. The `app` namespace is reserved for Milestone D. Provider names, operations, parameter names, defaults, result types, capability, redaction, and documentation are described by shared typed schemas.

## 5. Static type system

### 5.1 Core types

Milestone C defines:

```text
Null, Bool, Int, Float, String, Duration, Url, Json
List<T>, Option<T>, Record { fields... }
StatusCode, Headers, Bytes
Response<T>, ProcessResult, FilePath, TempDirectory
Locator, BrowserPage
```

Primitive literals infer their obvious type. Lists are homogeneous; an empty list needs contextual type information. Records are structural and field order does not affect compatibility. Required and optional fields are distinct. There are no implicit string/number/Boolean coercions. `Int` may widen to `Float`; all other conversions require explicit built-ins.

`Null` inhabits `Option<T>` rather than every type. `Json` is a tagged JSON-like runtime value; assigning it to a declared structural type performs a statically planned runtime decode with path-aware errors.

### 5.2 Type checking

Analysis validates:

- operator operand/result types;
- call arity, named arguments, duplicates, unknown names, and required parameters;
- record fields, optionality, and contextual decoding;
- member existence and resulting type;
- assertion matcher compatibility;
- serializability/transferability;
- operation capability at the call site.

Diagnostics include expected and actual types plus the smallest relevant source range. Cascading errors should be suppressed with an internal error/unknown type without losing independent diagnostics.

### 5.3 Typed HTTP decoding

`http.*` returns `Response<Json>` unless a typed decoder is requested. `response.status` is `StatusCode`, `headers` is `Headers`, `body` is `Bytes`, `text` is a fallible string decode, and `json` is `Json`.

```webtest
let user: {
    id: Int,
    email: String,
    admin: Option<Bool>,
} = response.json
```

Runtime decode errors identify a JSON path, expected type, actual JSON kind, response operation, and source range of the type-constrained expression.

## 6. Capabilities and transfer

Every expression has a type and an execution capability:

```text
Pure      literals, records, deterministic pure operations
Server    http/process/fs provider calls and server resources
Browser   browser actions, locators, pages, browser-only values
Test      orchestration, assertions, and control over child domains
```

The analyzer rejects a `Server` operation in `browser`, a `Browser` operation in `server`, and any host operation in a pure context. Capability is attached to resolved operations/signatures, not inferred from spelling alone.

Transferable values are deeply composed from:

```text
Null, Bool, Int, Float, String, Duration, Url, Json,
List<transferable>, Option<transferable>, Record<transferable fields>
```

The following are not transferable:

```text
BrowserPage, element identity, ProcessResult handles/streams,
TempDirectory ownership, open files, sockets, provider sessions
```

A record containing one non-transferable field is non-transferable. Diagnostics point to both the use crossing the boundary and the field/type making transfer invalid.

## 7. Provider model

### 7.1 Shared contracts

Introduce a protocol-neutral provider core, preferably an isolated crate such as `crates/provider`, containing:

```rust
ProviderName
OperationName
ProviderSchema
OperationSchema
ParameterSchema
TypeSchema
Value
ProviderCall
ProviderResult
ProviderError
ProviderCapabilities
```

The runtime-facing trait conceptually accepts an already validated call and cancellation/deadline context:

```rust
trait ServerProvider {
    async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<Value, ProviderError>;
}
```

Schemas are analysis inputs. Provider implementations do not participate in parsing or type checking at runtime. A registry maps a resolved provider name to one implementation at composition time.

### 7.2 HTTP provider

Support methods, base-URL resolution, query parameters, headers, JSON/text/bytes/form bodies, cookie handling, redirect policy, and per-call timeout. The provider never logs sensitive headers or configured JSON fields. Response bodies and captured evidence have size limits; exceeding a limit is explicit rather than silently truncating a value used by the test.

Transport errors are infrastructure failures. An HTTP 4xx/5xx is a successful HTTP operation returning that status until an assertion says otherwise.

### 7.3 Process provider

`process.run` accepts an executable plus argument array, optional environment overrides, working directory, stdin, and timeout. It returns exit status, stdout bytes/text, and stderr bytes/text after completion. It never invokes a shell implicitly.

Cancellation/timeout terminates the process tree where supported, waits a bounded grace period, and reports whether cleanup succeeded. Environment and output redaction apply before events/reporters.

### 7.4 Filesystem provider

Milestone C supports project-relative reads, explicitly permitted writes, fixture copying, and managed temporary directories. Paths normalize against the configured project root. A relative path that escapes the permitted root after canonicalization is rejected.

Temporary resources have explicit plan ownership and cleanup. Destructive recursive operations and arbitrary absolute-path mutation are not included.

## 8. Assertions and structured diffs

Extend assertions beyond browser visibility:

```webtest
expect response.status == 201
expect user.email == "alice@example.com"
expect result.exit_code == 0
expect result.stdout contains "seeded"
expect body matches { id: Int, email: String }
```

Assertions resolve a typed matcher during analysis and lower to explicit plan operations. Failures preserve typed expected/actual values and a matcher-specific diff:

- scalar equality shows expected and actual;
- strings show a bounded Unicode-aware diff;
- lists show differing indexes and bounded context;
- records show missing, unexpected, and mismatched fields;
- JSON/type decode shows a path-aware structural diff.

Secrets are replaced before diff construction so no adapter can accidentally reveal them.

## 9. Versioned `TestPlan`

Milestone C establishes a serializable plan envelope:

```text
PlanEnvelope {
    format_version,
    compiler_version,
    project_identity,
    source_files_and_revisions,
    required_host_capabilities,
    provider_schema_hashes,
    tests,
}
```

New operation families include:

```text
EvaluatePure
ServerProviderCall {
    provider,
    operation,
    arguments,
    result_binding,
    result_type,
    schema_hash,
    timeout,
    redaction,
}
Assertion { matcher, operands, diff_policy }
Browser { existing/new browser operation }
```

Every executable node carries deterministic ID, `SyntaxOrigin`, and source revision. Runtime values and live resources never appear in a serialized plan. Secret literal values require a documented redacted/late-bound representation before plan emission is considered safe.

`webtest build [paths...] --emit plan.json` performs analysis and emits only when there are no static errors. Unsupported plan versions fail clearly; there is no best-effort execution of unknown operations.

## 10. Runtime, events, and failures

Execution remains sequential. A per-test environment maps `BindingId` to typed `Value` or owned resource. Provider calls emit start/finish/failure events with provider/operation identity, duration, step/source identity, and redacted summaries.

Keep failure classes distinct:

```text
StaticError          invalid syntax/type/capability/provider call; no run
AssertionFailure     typed mismatch; test failed
ProviderFailure      application-level structured provider error; test failed
InfrastructureError  network transport, process spawn, filesystem host failure
InternalError        violated compiler/runtime invariant
```

An HTTP status alone is not a provider failure. A process nonzero exit is data until an operation/matcher requires success. Host inability to spawn the process is infrastructure.

Observations remain revision-bound. A successful rerun atomically removes previous type/runtime diagnostics for the file revision.

## 11. Configuration

Milestone C adds:

```toml
[server]
base_url = "http://127.0.0.1:3000"

[server.http]
follow_redirects = true
max_response_bytes = 8388608

[server.process]
allowed_working_roots = ["."]
max_output_bytes = 1048576

[server.fs]
read_roots = ["fixtures"]
write_root = ".webtest/tmp"

[redaction]
headers = ["authorization", "cookie", "set-cookie"]
json_fields = ["password", "token", "secret"]
```

Configuration participates in analysis invalidation and plan identity. The LSP receives resolved configuration through the project model rather than reading process-global state in handlers.

## 12. Editor, DAP, and WASM behavior

Protocol-neutral editor services expose static type/capability diagnostics, binding/member semantic tokens, and basic hover text when the shared analysis can provide it. Full completion/navigation arrives in Milestone F.

DAP scopes show already-evaluated transferable bindings with bounded nested values. Secrets remain redacted. Breakpoints pause before provider calls and assertions; stepping still uses `RunControl` and the normal runner.

WASM can parse, analyze, format, and compile native-capability plan nodes. It returns required capability metadata but cannot execute HTTP, process, filesystem, or Chrome operations.

## 13. Architecture and crate responsibilities

- `syntax` remains the only grammar/CST and adds typed AST views for expressions, bindings, types, server blocks, and assertions.
- `hir` owns semantic expressions, `BindingId`, resolved references, type syntax lowering, and precise origins; it does not own host implementations.
- `analysis` owns name resolution, type/capability checking, provider-schema lookup, diagnostics, and deterministic plan construction.
- A shared value/type/provider crate prevents HTTP/process/app adapters from inventing separate value models.
- `plan` owns serializable provider/assertion operations and capability requirements, independent of CST nodes and live traits.
- Built-in native provider implementations sit behind the provider trait; `runtime` owns sequencing, environments, deadlines, and structured events.
- `app` composes configuration, built-in providers, reporters, and CLI plan emission.
- `editor`, `lsp`, `dap`, and `wasm` convert shared DTOs only.

## 14. Delivery slices

1. Add expression/type syntax and lossless recovery without execution changes.
2. Add HIR bindings, stable reference identity, scopes, and a typed `Value` model.
3. Implement core type checking, capability checking, transferability, and diagnostics.
4. Add provider schemas/registry and explicit `ServerProviderCall` plan lowering.
5. Implement typed HTTP provider and response decoding end to end.
6. Add typed assertions and structural diffs.
7. Implement process and filesystem providers with sandbox/lifecycle constraints.
8. Add versioned plan serialization and `webtest build --emit`.
9. Expose revision-safe diagnostics/values through editor, LSP, DAP, and WASM DTOs.

Every slice includes parser/HIR/analysis/plan/runtime/editor tests as applicable; no adapter receives a shortcut parser or evaluator.

## 15. Testing requirements

Required coverage includes:

- lossless valid/invalid/half-typed expressions, types, bindings, calls, and server blocks;
- precedence/property tests and parser non-progress regression tests;
- exact binding/reference/origin and deterministic plan-ID tests;
- type inference, record compatibility, options, contextual JSON decode, and diagnostic suppression;
- capability/transfer tests for nested transferable and non-transferable values;
- provider-schema hash and plan serialization golden tests;
- fake-provider runtime tests for values, errors, cancellation, timeout, redaction, and observation clearing;
- local HTTP fixture tests for methods, bodies, redirects, limits, Unicode, and transport failure;
- process tests without a shell, including timeout and child cleanup;
- filesystem traversal/symlink/sandbox/temporary cleanup tests;
- typed diff golden tests;
- native/WASM analysis/plan parity tests;
- LSP UTF-16 diagnostics and DAP redacted-variable tests.

## 16. Acceptance criteria

Milestone C is complete only when:

1. End-to-end coverage proves that a flow can prepare state with `http.*`, decode a typed record, and use it in the browser.
2. Invalid fields, call arguments, matcher types, provider domains, and non-transferable crossings fail statically at precise ranges.
3. HTTP, process, and filesystem calls lower to the same explicit provider-call IR and run behind one registry contract.
4. Provider/assertion failures produce structured, redacted events, observations, CLI output, and DAP values.
5. `webtest build --emit` is deterministic for identical source/config/schema inputs and rejects unknown plan versions.
6. WASM produces the same portable diagnostics/plan DTOs while refusing native execution.
7. Full workspace, protocol, browser, and extension quality gates pass.

The roadmap acceptance statement is thereby satisfied: one test can prepare state through a server operation, use it in a browser, and receive static errors for invalid domain/type transfers.
