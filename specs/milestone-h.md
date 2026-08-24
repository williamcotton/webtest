# Milestone H — Test Modeling and Reactive Workflows

## 0. Status and dependencies

This specification expands Milestone H following [`milestone-g.md`](./milestone-g.md). It depends on the typed expression/value/provider model from [`milestone-c.md`](./milestone-c.md), semantic inspection and machine-readable feedback from [`milestone-c5.md`](./milestone-c5.md), the typed application bridge from [`milestone-d.md`](./milestone-d.md), structured execution/resource ownership/event infrastructure from [`milestone-e.md`](./milestone-e.md), and workspace/declaration/test identity from [`milestone-f.md`](./milestone-f.md).

Milestone G is the expected delivery predecessor and must expose the portable static portions of Milestone H through the same native/WASM language services, but Milestone H's runtime semantics do not depend on browser-side WASM execution.

Milestone H moves WebTest beyond sequential browser automation into explicit modeling of:

* data-driven test variants;
* reusable typed structural patterns;
* multiple independent browser participants;
* bounded observation of asynchronous browser events;
* persistent scoped invariants;
* accumulated assertion failures;
* explicit semantic test verdicts.

The milestone preserves Milestone E's structured concurrency semantics. In particular, `race {}` remains a computation-racing primitive. Reactive event selection is introduced separately and does not redefine `race`.

**Implementation status: proposed.**

[`future-functionality.md`](./future-functionality.md) must be updated to add Milestone H and to treat test variants, actors, patterns, and reactive event selection as first-class long-term semantic concepts rather than editor/runtime conveniences.

## 1. Outcome

WebTest can describe a test in terms of participants, test data, stimuli, observable events, acceptable structures, temporal requirements, and semantic outcomes rather than forcing those concepts to be reconstructed from general-purpose control flow.

A representative test can express:

```webtest
pattern ChatMessage(from: String, body: String) = subset {
    id: any,
    from: from,
    body: body,
    timestamp: any,
}

guard clean_browser(actor: BrowserActor) {
    when actor.console.error(e) {
        fail "console error: {e.message}"
    }

    when actor.network.response(r)
        if r.status >= 500 {
        fail "unexpected server error: {r.status} {r.url}"
    }
}

test "messages propagate"
cases [
    case "alice-to-bob" {
        sender: "alice",
        receiver: "bob",
    },

    case "bob-to-alice" {
        sender: "bob",
        receiver: "alice",
    },
]
as case {
    actor sender browser
    actor receiver browser

    sender {
        open "/chat?user={case.sender}"
    }

    receiver {
        open "/chat?user={case.receiver}"
    }

    let mark = checkpoint(receiver)

    with guards [
        clean_browser(sender),
        clean_browser(receiver),
    ] {
        sender {
            fill label("Message") with "hello"
            click role("button", name: "Send")
        }

        select receiver since mark within 5s {
            when websocket.received_json(m)
                if m.value matches ChatMessage(case.sender, "hello") {
                pass
            }

            timeout {
                fail "message was not delivered within 5s"
            }
        }
    }
}
```

The test declaration produces two independently identifiable test variants:

```text
messages propagate [alice-to-bob]
messages propagate [bob-to-alice]
```

Each variant receives isolated browser actors. The receiver event checkpoint is established before the sender action, so an immediately delivered WebSocket message cannot be lost merely because the `select` statement executes afterward.

`select` observes events from the receiver's event journal rather than racing executable child blocks. The `clean_browser` guards are installed before the guarded body begins and remain active until its structured scope exits.

The `ChatMessage` declaration is a reusable typed matcher, not a concrete record. Extra fields are accepted because the pattern explicitly uses `subset`.

Every construct remains visible in typed HIR, serializable plans, runtime events, traces, editor services, DAP, test discovery, and machine-readable WebTest interfaces.

## 2. Design principles

Milestone H follows six architectural rules.

### 2.1 Model test concepts directly

Recurring testing concepts receive explicit semantics rather than being encoded through arbitrary user code.

WebTest should prefer:

```webtest
cases [...]
```

over an ordinary runtime loop when each item represents an independently reportable test execution.

It should prefer:

```webtest
actor alice browser
```

over exposing raw `BrowserContext` construction mechanics.

It should prefer:

```webtest
select receiver since mark within 5s {
    ...
}
```

over asking authors to manually create event listeners, promises, timers, and cancellation logic.

### 2.2 Preserve constrained execution

Milestone H does not make WebTest a general-purpose asynchronous programming language.

There are no detached tasks, arbitrary callbacks, dynamically registered handlers, unrestricted channels, shared mutable variables, or user-created threads.

Reactive execution remains structured and owned by a parent plan scope.

### 2.3 Keep compile-time identity ahead of runtime behavior

Tests and variants must be discoverable before execution.

Patterns must be statically resolvable.

Actor references must resolve to semantic resource IDs.

Event sources and event payload types must be known during analysis.

A test runner must never need to execute arbitrary project code merely to discover which tests exist.

### 2.4 Preserve typed machine-readable semantics

A machine consumer must be able to discover:

```text
available pattern forms
case parameters and variant identities
actors and their resource types
event sources and event payload types
guard signatures
verdict kinds
select result types
check aggregation behavior
```

without parsing human documentation or terminal messages.

### 2.5 Keep reactive observation distinct from computation racing

Milestone E:

```webtest
race {
    computation_a
    computation_b
}
```

means:

> Start sibling computations. The first successful computation wins.

Milestone H:

```webtest
select actor within 5s {
    when event_a(x) { ... }
    when event_b(y) { ... }
}
```

means:

> Observe an ordered event stream. Execute the branch associated with the first eligible event.

These concepts may share cancellation/deadline infrastructure but never share ambiguous language semantics.

### 2.6 Make lost-event behavior explicit

Reactive tests are unreliable if an event can arrive between the action that causes it and the installation of a listener.

WebTest therefore provides explicit event checkpoints and bounded journals. It must never rely on timing luck such as "the listener will probably be installed before the response arrives."

## 3. Scope

Milestone H includes:

* statically discoverable data-driven test variants;
* stable `TestVariantId`;
* typed reusable `pattern` declarations;
* a bounded pattern algebra for literals, types, records, lists, regular expressions, ranges, alternatives, negation, optional fields, and containment;
* pattern-aware structured assertion diffs;
* `check {}` accumulated assertion scopes;
* explicit `pass`, `fail`, `skip`, and `inconclusive` verdicts;
* `assume ... else ...`;
* first-class `BrowserActor` resources;
* actor-scoped browser execution;
* explicit actor isolation and ownership rules;
* browser-event normalization;
* actor-bound event journals;
* event checkpoints;
* `select ... within ...` reactive event selection;
* typed `when` alternatives;
* explicit `timeout` alternatives;
* reusable event guards;
* lexically scoped guard activation;
* structured cancellation when a guard produces a terminating verdict;
* source-mapped actor/event/variant/check/verdict identities;
* trace and reporter support for all new constructs;
* test explorer support for variants;
* DAP support for actors, event bindings, guards, and selections;
* C.5 machine-description support for all new static semantic concepts;
* native/WASM analysis and plan parity for portable Milestone H constructs.

## 4. Non-goals

Milestone H does not add:

* arbitrary event-handler callbacks;
* arbitrary user-defined streams or channels;
* detached background work;
* shared mutable DSL variables;
* actor-to-actor in-memory message passing;
* distributed actors;
* remote browser workers;
* multi-machine test orchestration;
* arbitrary JavaScript/Python execution;
* TTCN-3 compatibility;
* Robot Framework keyword compatibility;
* dynamic runtime generation of undiscoverable test variants;
* CSV/database/network-driven test enumeration;
* property-based random test generation;
* automatic LLM-authored tests;
* automatic test healing;
* dynamic alteration of test source;
* arbitrary DOM event subscriptions;
* general WebSocket client APIs independent of a browser actor;
* replay of an entire prior browser session;
* actor migration between browser processes;
* user-defined scheduler policies;
* event handlers that mutate browser/application state while executing concurrently in the background;
* unrestricted recursive patterns;
* arbitrary predicate functions embedded inside patterns;
* pattern capture bindings;
* visual snapshot matching;
* distributed event ordering.

Future milestones may extend the event-source and pattern systems without changing the core ownership, typing, identity, and bounded-observation rules introduced here.

## 5. Relationship to Milestone E structured execution

### 5.1 `race` remains unchanged

Milestone E `race` semantics remain normative:

```webtest
race {
    sequence {
        expect text("Dashboard").visible
        provide "dashboard"
    }

    sequence {
        expect text("Verify your email").visible
        provide "verification"
    }
}
```

Every direct race child starts as a task.

The first successfully completing child wins.

Failed children do not win while another child can still succeed.

The runtime cancels losing children and awaits their teardown.

If every branch fails, failures are aggregated in stable branch order.

Milestone H does not change any of these semantics.

### 5.2 `select` does not execute alternatives speculatively

For:

```webtest
select alice within 5s {
    when console.error(e) {
        fail e.message
    }

    when websocket.received_json(m) {
        provide m.value
    }
}
```

the branch bodies do not run until their event alternative is selected.

Conceptually:

```text
race

start A ───────────────┐
start B ────────success├─ cancel A
start C ─────failure───┘


select

observe event stream
        |
        +-- console.log        -> no matching alternative
        |
        +-- websocket message  -> choose branch
                                    |
                                    +-- execute branch body
```

`race` operates over child computation completion.

`select` operates over typed event arrival.

### 5.3 Shared scheduler substrate

`select`, guards, and actor event observation reuse Milestone E:

```text
cancellation tokens
deadlines
task paths
resource scopes
event identities
monotonic clocks
bounded cleanup
execution events
trace ownership
```

but add an explicit reactive-event subsystem rather than representing browser event subscriptions as user-visible spawned tasks.

## 6. Test variants

### 6.1 Surface syntax

Version 1 uses explicit labeled cases:

```webtest
test "invalid login"
cases [
    case "unknown-user" {
        email: "missing@example.com",
        password: "correct",
        message: "Unknown user",
    },

    case "wrong-password" {
        email: "alice@example.com",
        password: "wrong",
        message: "Incorrect password",
    },
]
as input {
    browser {
        open "/login"
        fill label("Email") with input.email
        fill label("Password") with input.password
        click role("button", name: "Sign in")
        expect text(input.message).visible
    }
}
```

A case label is required in version 1.

Labels must be unique within the test declaration.

The case binding name after `as` is lexical and follows ordinary binding-shadowing rules.

### 6.2 Compile-time expansion

Cases are not a runtime loop.

Analysis expands one declaration into independently executable variant plans:

```text
TestDeclaration
    |
    +-- Variant "unknown-user"
    |
    +-- Variant "wrong-password"
```

Each variant receives:

```text
TestDeclarationId
TestVariantId
case label
case value
source origin
variant plan
variant fixture/resource ownership
```

A failing case does not collapse the other cases into the same test result.

### 6.3 Case values

Case values must be statically evaluable, deterministic, transferable values.

Allowed inputs include:

```text
primitive literals
lists
records
pure deterministic constant expressions
eligible imported pure values/functions whose result can be evaluated during analysis
```

Case enumeration may not depend on:

```text
browser state
provider calls
filesystem reads
process execution
network access
application bridge execution
current wall-clock time
randomness
environment variables not represented as resolved project inputs
```

The workspace must know every variant without running the application.

### 6.4 Case type unification

All cases in one declaration must produce one compatible case type.

For:

```webtest
cases [
    case "a" { email: "a@example.com", admin: false },
    case "b" { email: "b@example.com", admin: true },
]
as user
```

the inferred type is:

```text
{
    email: String,
    admin: Bool,
}
```

A structurally incompatible row is a static error:

```webtest
case "broken" {
    email: 42,
    admin: false,
}
```

produces a diagnostic equivalent to:

```text
case `broken` is incompatible with the case schema

field `email`
expected String
found Int
```

The diagnostic points to both the incompatible value and the case declaration establishing the expected field type where useful.

### 6.5 Stable variant identity

`TestVariantId` derives from:

```text
workspace/project identity
module identity
test declaration semantic identity
explicit case label
```

It does not derive from:

```text
case ordinal
runtime execution order
case field values
hash-map ordering
wall-clock values
secret contents
display formatting
```

Therefore reordering:

```text
"unknown-user"
"wrong-password"
```

does not change either variant ID.

Renaming a case label intentionally changes variant identity.

Duplicate labels are static errors.

### 6.6 Selection and filtering

CLI selection supports variant identity:

```sh
webtest test --test "invalid login"
webtest test --variant "unknown-user"
webtest test --test-id <id>
webtest test --variant-id <id>
```

Exact flag names may follow the existing CLI conventions, but declaration-level and variant-level filtering are both required.

Selecting the parent test declaration runs all currently discoverable variants.

### 6.7 Reporting

Human output distinguishes declaration and variant:

```text
invalid login
  PASS unknown-user
  FAIL wrong-password
```

JSON/event output carries both:

```text
test_declaration_id
test_variant_id
case_label
```

Case values are not automatically emitted in full. Reporters may include bounded redacted summaries under the existing evidence policy.

### 6.8 Test explorer

Milestone F's test tree becomes:

```text
workspace
  -> module/file
      -> test declaration
          -> test variant
```

A declaration without cases has one implicit default variant for execution identity, but editors may visually collapse it and display only the declaration.

Variant nodes are directly runnable and debuggable.

### 6.9 Variants and fixtures

Each variant is a distinct test execution for `test` fixture lifetime.

Therefore:

```text
test-scoped fixture
```

is acquired once per variant execution unless retry ownership requires reacquisition.

File, worker, and suite fixture lifetime semantics remain Milestone F semantics.

### 6.10 Variants and retry

Retry does not create new `TestVariantId`s.

Attempts are identified separately by Milestone E `AttemptId`.

Conceptually:

```text
TestVariantId = "wrong-password"

AttemptId 1
AttemptId 2
AttemptId 3
```

rather than three different test variants.

## 7. Pattern declarations

### 7.1 Purpose

A `pattern` describes acceptable values.

It is not a value constructor and does not create runtime application data.

For:

```webtest
pattern User(email: String) = subset {
    id: Int,
    email: email,
    active: true,
    created_at: any,
}
```

this:

```webtest
User("alice@example.com")
```

describes acceptable values containing at least:

```text
id             an Int
email          exactly "alice@example.com"
active         exactly true
created_at     any present value
```

### 7.2 Declaration syntax

Patterns may be private or exported after Milestone F module rules:

```webtest
export pattern User(email: String) = subset {
    id: Int,
    email: email,
    active: Bool,
}
```

Parameters require declared types at exported boundaries.

Pattern references participate in definition, references, rename, completion, hover, signatures, module resolution, and semantic tokens.

### 7.3 Pattern purity

Patterns are declarative match descriptions.

They cannot:

```text
perform browser operations
call providers
read files
launch processes
call app.*
acquire resources
observe events
mutate bindings
emit verdicts
```

Pattern parameters are values, not callbacks or effects.

The pattern dependency graph must be acyclic in version 1.

### 7.4 Core pattern forms

Version 1 supports the following semantic pattern forms:

```text
Any
Literal
Type
ExactRecord
SubsetRecord
ExactList
Each
Contains
OptionalField
AbsentField
Regex
Range
OneOf
AllOf
Not
PatternReference
```

Surface syntax is intentionally compact.

### 7.5 Literal patterns

A literal in pattern position matches equality:

```webtest
pattern ActiveUser = subset {
    active: true,
    role: "user",
}
```

The normal typed equality rules apply.

No implicit string/number/Boolean coercion occurs.

### 7.6 Type patterns

A type in pattern position matches a value belonging to that runtime type:

```webtest
pattern User = subset {
    id: Int,
    email: String,
    active: Bool,
}
```

For `Json`, this performs the corresponding structural/type check without converting the matched value into a lexical binding.

### 7.7 `any`

`any` matches any value.

Within a record field:

```webtest
id: any
```

requires the field to exist but places no constraint on its value.

`any` also matches `null`.

### 7.8 Absent and optional fields

`absent` is valid only as a record-field matcher:

```webtest
password: absent
```

and requires that the field not exist.

`optional(pattern)` permits absence but validates the field when present:

```webtest
nickname: optional(String)
```

This differs from:

```webtest
nickname: any
```

which requires the field to exist.

### 7.9 Record matching

Milestone H makes record-match policy explicit.

`subset` permits unspecified extra fields:

```webtest
pattern User = subset {
    id: Int,
    email: String,
}
```

matches:

```json
{
  "id": 5,
  "email": "a@example.com",
  "created_at": "...",
  "admin": false
}
```

`exact` rejects unspecified extra fields:

```webtest
pattern Point = exact {
    x: Int,
    y: Int,
}
```

Any existing Milestone C anonymous `matches { ... }` behavior must retain compatibility. If the existing bare-record matcher already has defined exact/subset behavior, that behavior remains unchanged; H's explicit `subset` and `exact` forms remove ambiguity for newly authored reusable patterns.

### 7.10 Exact list patterns

A list pattern:

```webtest
["draft", "published"]
```

matches exactly two elements in that order using nested pattern semantics.

### 7.11 `each`

```webtest
each(subset {
    id: Int,
    name: String,
})
```

matches a list only when every element matches the supplied pattern.

An empty list satisfies `each(...)`.

### 7.12 `contains`

For lists:

```webtest
contains(subset {
    role: "admin",
})
```

requires at least one matching item.

For strings:

```webtest
contains("welcome")
```

uses the existing string containment semantics.

Analyzer resolution distinguishes string and collection forms from context.

### 7.13 Regular expressions

```webtest
regex("^[a-f0-9-]{36}$")
```

matches strings only.

Version 1 requires the regex source to be statically known so invalid syntax is a static diagnostic.

Regex behavior, Unicode mode, anchoring, and engine limits are documented and deterministic across native/WASM builds.

Catastrophic/unbounded regular-expression execution must be prevented through an engine or limits suitable for untrusted test source.

### 7.14 Ranges

```webtest
range(200, 299)
```

matches ordered scalar types compatible with both bounds.

Inclusive bounds are the version 1 default.

Supported types include at least:

```text
Int
Float
StatusCode
Duration
```

where type rules permit.

### 7.15 Alternatives

```webtest
one_of(
    "queued",
    "running",
    "complete",
)
```

succeeds if one alternative matches.

Failure evidence records bounded failure reasons for the alternatives rather than one opaque Boolean.

### 7.16 Conjunction

```webtest
all_of(
    String,
    regex("^[a-z0-9_-]+$"),
)
```

requires every child matcher to succeed.

### 7.17 Negation

```webtest
not("deleted")
```

succeeds only when the nested pattern does not match.

Failure evidence identifies the unexpectedly matched nested pattern.

### 7.18 Pattern references

Patterns may invoke other patterns:

```webtest
pattern UserId = all_of(
    String,
    regex("^usr_[a-z0-9]+$"),
)

pattern User(email: String) = subset {
    id: UserId,
    email: email,
}
```

Pattern-reference resolution is static.

Recursive pattern declarations are rejected in version 1.

### 7.19 Pattern parameters

Parameters are immutable pure values.

For:

```webtest
pattern ChatMessage(from: String, body: String) = subset {
    from: from,
    body: body,
}
```

the field matchers refer to the parameter values, not to pattern-local captures.

Pattern parameters may be ordinary transferable scalar/record/list values when matcher semantics support them.

Resource values such as:

```text
BrowserActor
BrowserPage
TempDirectory
Process handles
EventCursor
```

cannot be pattern parameters.

### 7.20 Pattern matching as expression

Milestone H permits:

```webtest
value matches Pattern(...)
```

as a typed Boolean expression where a Boolean is needed:

```webtest
if message.value matches ChatMessage("alice", "hello") {
    ...
}
```

and preserves assertion syntax:

```webtest
expect message.value matches ChatMessage("alice", "hello")
```

The analyzer resolves both through the same pattern representation.

The Boolean expression returns only a Boolean at language level.

The assertion form additionally preserves structured match evidence for reporting.

### 7.21 Pattern diff paths

Pattern failure evidence is path-aware:

```text
expected ChatMessage("alice", "hello")

$.from
  expected "alice"
  actual   "bob"

$.body
  expected "hello"
  actual   "Hello"
```

Record evidence distinguishes:

```text
missing required field
unexpected field under exact matching
field value mismatch
field expected absent
field optional but malformed
```

List evidence distinguishes:

```text
wrong length
element mismatch
no element satisfying contains
element failing each
```

Evidence remains bounded and redacted before reporter/editor/trace publication.

## 8. Accumulated checks

### 8.1 Surface

Add:

```webtest
check {
    expect page.title == "Account"
    expect text("Alice").visible
    expect text("Premium").visible
    expect url().path == "/account"
}
```

`check` runs its sequential child scope while accumulating recoverable assertion failures.

### 8.2 Failure continuation

The following failures are accumulated and do not immediately stop the `check` scope:

```text
AssertionFailure
explicit `fail`
pattern mismatch
ordinary expect-state mismatch
```

The following terminate the scope immediately:

```text
StaticError
ProviderFailure
InfrastructureError
InternalError
cancellation
timeout of the enclosing scope
browser disconnection
resource acquisition failure
```

A failed action is not converted into a soft assertion.

For example:

```webtest
check {
    click role("button", name: "Missing")
    expect text("Done").visible
}
```

does not continue after the failed click.

### 8.3 Final result

If all recoverable assertions succeed, the `check` node succeeds.

If one or more recoverable assertions fail, the node returns one ordered aggregate assertion failure.

Ordering is stable source/plan child order rather than asynchronous completion order.

### 8.4 Nested checks

Nested `check` scopes are allowed.

An inner aggregate counts as one child failure in the outer scope while preserving its nested structured failures.

Reporters may flatten for presentation only if nested source identity is retained.

### 8.5 Interaction with `parallel`

`check` and `parallel` have distinct semantics.

```webtest
parallel {
    ...
}
```

runs computations concurrently.

```webtest
check {
    ...
}
```

runs sequentially while accumulating eligible assertion failures.

A `check` may appear inside a parallel branch.

A `parallel` may appear inside a check, but its aggregate failure is recoverable only if it consists solely of assertion-class failures. Infrastructure/internal failure retains Milestone E cancellation behavior.

### 8.6 Plan node

Introduce:

```text
Check {
    children,
    continuation_policy,
}
```

The continuation policy is versioned and explicit. It is not inferred dynamically from human-readable failure strings.

## 9. Semantic verdict model

### 9.1 Verdicts

Milestone H introduces semantic test verdicts:

```text
Pass
Fail
Skipped
Inconclusive
```

Execution/environment errors remain distinct from semantic verdicts:

```text
InfrastructureError
InternalError
StaticError
```

The test result model therefore distinguishes:

```text
TestResult
    SemanticVerdict
or
    ExecutionError
```

rather than treating every unsuccessful execution as a generic failure.

### 9.2 Normal completion

Reaching the end of a test variant normally produces:

```text
Pass
```

No explicit `pass` statement is required.

### 9.3 `pass`

```webtest
pass
```

terminates the current test variant successfully.

All owned teardown still runs.

`pass` is useful when a selected event semantically establishes the test's success before the remainder of the lexical test body.

### 9.4 `fail`

```webtest
fail "application emitted an invalid event"
```

terminates with semantic `Fail`.

It produces an assertion-class failure with its precise source origin.

Within `check`, explicit `fail` is accumulated according to check semantics.

### 9.5 `skip`

```webtest
skip "feature is not part of this deployment"
```

terminates the test variant as `Skipped`.

Already acquired resources are torn down.

Trace/event output records any operations that occurred before the skip; reporters must not imply that the test necessarily executed zero operations.

Authors should prefer early skip decisions when possible.

### 9.6 `inconclusive`

```webtest
inconclusive "external prerequisite could not be established"
```

means the test was attempted but the intended proposition could not be meaningfully decided.

This is distinct from:

```text
Pass
Fail
Skipped
InfrastructureError
```

Example:

```text
Skipped
    this test was intentionally not applicable/executed

Inconclusive
    execution occurred, but a semantic prerequisite prevented
    a meaningful pass/fail decision

InfrastructureError
    WebTest could not reliably execute the test environment
```

### 9.7 `assume`

Add:

```webtest
assume feature.enabled
    else inconclusive "premium checkout is disabled"
```

and:

```webtest
assume platform.supported
    else skip "unsupported platform"
```

The condition must be `Bool`.

The `else` arm in version 1 must produce `skip` or `inconclusive`.

`assume` does not silently transform a false condition into Pass.

### 9.8 Verdict precedence and aggregation

Within one variant:

```text
Internal/Infrastructure execution error
    remains an execution error

Fail
    remains Fail even if cleanup later succeeds

Inconclusive
    remains Inconclusive unless teardown produces an execution error

Skipped
    remains Skipped unless teardown produces an execution error

Pass
    requires successful teardown
```

If teardown itself suffers an infrastructure/internal failure, the semantic verdict and cleanup failure are both retained, but the externally visible execution status is Error because WebTest cannot claim a clean semantic completion.

### 9.9 Suite exit behavior

Default CLI suite behavior:

```text
Pass           success
Skipped        does not fail suite
Fail           fails suite
Inconclusive   fails suite
Infrastructure fails suite
Internal       fails suite
```

A future/configurable policy may permit inconclusive results without failing CI, but the default is conservative.

Machine reporters always preserve the original verdict regardless of aggregate exit policy.

### 9.10 JUnit projection

JUnit adapters map:

```text
Pass           normal testcase
Fail           failure
Skipped        skipped
Inconclusive   skipped + webtest verdict metadata
ExecutionError error
```

The JSON/event formats retain full WebTest semantics and are authoritative.

## 10. Browser actors

### 10.1 Actor declaration

Add:

```webtest
actor alice browser
actor bob browser
```

A browser actor is a first-class owned test resource with type:

```text
BrowserActor
```

It represents one independent browser participant.

### 10.2 Runtime ownership

Each actor owns at minimum:

```text
BrowserContext
default Page
cookie/storage namespace
event journal
actor identity
actor resource scope
```

Conceptually:

```text
TestVariant
  |
  +-- implicit/default browser actor
  |
  +-- Actor alice
  |     |
  |     +-- BrowserContext A
  |           |
  |           +-- default Page A
  |
  +-- Actor bob
        |
        +-- BrowserContext B
              |
              +-- default Page B
```

### 10.3 Isolation

Distinct actors do not share by default:

```text
cookies
localStorage
sessionStorage
IndexedDB
page state
navigation history
service-worker-controlled storage where browser context isolation applies
default page
event cursor/journal
```

They may still communicate through the application under test, shared server state, or external systems because that is usually the behavior being tested.

### 10.4 Existing `browser {}` compatibility

Existing tests remain valid.

The ordinary:

```webtest
browser {
    ...
}
```

operates on the test variant's implicit default browser actor.

Explicit actors are additional contexts, not a replacement for the existing browser model.

### 10.5 Actor-scoped browser block

```webtest
alice {
    open "/login"
    fill label("Email") with "alice@example.com"
}
```

selects:

```text
Browser capability
actor = alice
default page = alice.default_page
```

for the lexical body.

The block does not clone or transfer the page.

### 10.6 Actor identity

Each actor declaration receives:

```text
ActorId
```

derived from:

```text
test/fixture semantic scope
declaration origin
actor binding identity
```

Runtime events additionally contain the enclosing:

```text
TestVariantId
AttemptId
```

where applicable.

### 10.7 Actor type restrictions

`BrowserActor` is:

```text
non-transferable
non-serializable as a runtime value
non-comparable by user equality
resource-owned
capability-bearing
```

An actor may be passed to language constructs explicitly designed to accept actor resources, such as:

```text
checkpoint
guard activation
event selection
eligible helper/guard parameters
```

It may not cross a server/browser value-transfer boundary or appear in an emitted literal plan value.

### 10.8 Actor lifecycle

Actor acquisition lowers to explicit Milestone E resource ownership.

On:

```text
success
failure
skip
inconclusive
timeout
race loss
debug disconnect
runner shutdown
```

the actor context is closed exactly once if acquisition completed.

Teardown failures are aggregated under existing resource-scope rules.

### 10.9 Actors and retries

An actor declared inside a retry scope is reacquired for each attempt.

An actor declared outside a retry survives inner retry attempts.

The plan/resource tree makes this distinction visible.

### 10.10 Actors and parallel access

Concurrent use of the same actor is unsafe by default because browser operations on one page are order-sensitive.

Static analysis therefore rejects overlapping exclusive actor use:

```webtest
parallel {
    alice {
        click text("A")
    }

    alice {
        click text("B")
    }
}
```

unless a future explicit concurrency-safe multi-page construct proves independent resource ownership.

Different actors may be used concurrently:

```webtest
parallel {
    alice {
        ...
    }

    bob {
        ...
    }
}
```

### 10.11 Actor configuration

Milestone H version 1 actors inherit the resolved browser configuration.

Per-actor browser engine/version selection is not included.

Future configuration may add explicit actor-specific viewport/device/storage presets without changing actor ownership semantics.

Authentication is not built into `actor`.

An actor represents a browser participant, not a user-account database record.

Authentication is performed through ordinary browser operations, fixtures, application-provider calls, or future explicit reusable project abstractions.

## 11. Reactive browser event model

### 11.1 Normalized events

`browser-cdp` does not expose raw CDP events directly to the DSL.

The protocol-neutral `browser` layer defines normalized typed reactive events.

Initial event families are:

```text
ConsoleEvent
NetworkRequestEvent
NetworkResponseEvent
WebSocketFrameEvent
WebSocketJsonEvent
NavigationEvent
```

The exact CDP messages used to implement them remain backend details.

### 11.2 Console events

At minimum:

```webtest
when console.message(e) { ... }
when console.error(e) { ... }
```

`ConsoleEvent` includes bounded/redacted fields such as:

```text
level
message
source/url when available
line/column when available
timestamp/event identity
```

`console.error` is a filtered event source rather than a separately implemented CDP path.

### 11.3 Network events

At minimum:

```webtest
when network.request(r) { ... }
when network.response(r) { ... }
```

Request metadata includes:

```text
method
url
resource kind when available
bounded/redacted headers under policy
```

Response metadata includes:

```text
url
status
bounded/redacted headers
resource kind when available
request correlation identity
```

Response bodies are not captured by default merely because a network event source is active.

### 11.4 WebSocket events

At minimum:

```webtest
when websocket.received(frame) { ... }
when websocket.sent(frame) { ... }
when websocket.received_json(message) { ... }
when websocket.sent_json(message) { ... }
```

Raw frame summaries expose:

```text
url/connection identity
text or bounded bytes according to frame kind
direction
timestamp/event identity
```

JSON event forms are emitted only when the eligible text frame parses as JSON within configured limits.

The JSON form includes:

```text
value: Json
```

along with bounded connection metadata.

Invalid JSON does not cause the WebSocket connection or `select` itself to fail; it simply does not produce the derived JSON event.

### 11.5 Navigation events

At minimum:

```webtest
when navigation(n) { ... }
```

with:

```text
from_url when known
to_url
navigation kind when known
same-document flag
```

Ordinary deterministic navigation assertions should still use existing browser operations where appropriate. Reactive navigation is intended for behavior whose timing/source is itself part of the test.

### 11.6 Event source schemas

Every event source has a static schema:

```text
EventSourceSchema {
    name,
    actor_requirement,
    payload_type,
    documentation,
    capture_requirements,
    redaction_policy,
}
```

Editor completion, hover, C.5 `describe`, WASM analysis, and runtime use the same schemas.

There is no editor-only list of event source names.

## 12. Actor event journals

### 12.1 Purpose

Browser events can occur faster than the `.webtest` statement sequence reaches a later assertion.

Every explicit actor therefore owns a bounded normalized event journal.

The journal allows WebTest to observe:

```text
checkpoint
stimulus
event arrives immediately
later select
```

without losing the event.

### 12.2 Journal entries

Conceptually:

```text
ReactiveEvent {
    reactive_schema_version,
    event_id,
    actor_id,
    actor_sequence,
    runtime_event_sequence,
    timestamp,
    source_kind,
    payload,
}
```

`actor_sequence` is a monotonic order assigned by WebTest after the browser adapter normalizes the event.

### 12.3 Event ordering

Within one actor, WebTest provides one total observed order:

```text
actor_sequence
```

This is the runtime order in which the browser adapter made normalized events visible to WebTest.

It does not claim a stronger physical or distributed causality guarantee.

Across separate actors there is no semantic total-order guarantee beyond the general runtime event collector's observed event sequence.

Version 1 `select` operates against one actor at a time.

### 12.4 Journal activation

The runtime does not need to retain every possible browser protocol event for every actor indefinitely.

It activates the normalized event families required by:

```text
active guards
active/selectable event checkpoints
current or future plan nodes known to require event history
trace capture policy
```

Because the plan is known before execution, the runtime may establish required observation before an actor reaches operations that can trigger relevant events.

### 12.5 Bounds

Configuration includes documented limits, for example:

```toml
[reactive]
max_events_per_actor = 10000
max_event_bytes_per_actor = 8388608
max_single_event_bytes = 262144
```

Actual default values are determined by implementation measurement and documented.

Limits are not silently lossy for active semantic consumers.

### 12.6 Overflow

If an event needed by an active checkpoint/select/guard is discarded because a configured semantic buffer limit is exceeded, the affected operation fails explicitly:

```text
ReactiveEventOverflow
```

WebTest must not convert:

```text
"we lost events"
```

into:

```text
"the expected event never happened"
```

because those are different test conclusions.

Trace-only evidence may use separate lossy policies if truncation is explicit.

## 13. Event checkpoints

### 13.1 Surface

Add:

```webtest
let mark = checkpoint(receiver)
```

The result type is conceptually:

```text
EventCursor<receiver>
```

represented internally through an actor-bound `EventCursor` resource type.

### 13.2 Purpose

A checkpoint establishes the causal observation boundary before a stimulus:

```webtest
let mark = checkpoint(receiver)

sender {
    click role("button", name: "Send")
}

select receiver since mark within 5s {
    ...
}
```

If the expected event arrives immediately during the sender click, it remains eligible for the later `select`.

### 13.3 Browser-protocol barrier

`checkpoint(actor)` is not merely:

```text
read current integer counter
```

without synchronization.

The browser backend establishes a bounded protocol barrier sufficient to ensure that events preceding the completed checkpoint operation on that actor/session have been drained into the normalized event subsystem according to the backend's documented ordering model.

The exact CDP command used to establish this barrier is implementation-specific.

If the backend cannot establish the required ordering guarantee, checkpoint creation fails explicitly rather than claiming a stronger boundary.

### 13.4 Cursor restrictions

An event cursor is:

```text
actor-bound
execution-bound
attempt/resource-generation-bound
non-transferable
non-serializable as a value
not comparable by user code
```

It cannot be used with another actor:

```webtest
let mark = checkpoint(alice)

select bob since mark within 5s {
    ...
}
```

Static analysis rejects this.

A cursor cannot outlive actor teardown or cross into another retry generation.

### 13.5 Default select cursor

For:

```webtest
select receiver within 5s {
    ...
}
```

the runtime establishes an implicit checkpoint immediately before observation begins.

This form is appropriate when the event of interest is expected only after entering the select.

When an earlier operation can cause the event, authors should use an explicit checkpoint.

Editor/documentation/agent-facing descriptions should make that distinction clear.

## 14. Reactive `select`

### 14.1 Surface

```webtest
select receiver since mark within 5s {
    when websocket.received_json(message)
        if message.value matches ChatMessage("alice", "hello") {
        provide message.value
    }

    when console.error(error) {
        fail error.message
    }

    timeout {
        fail "expected message did not arrive"
    }
}
```

Without an explicit cursor:

```webtest
select receiver within 5s {
    ...
}
```

uses the implicit select-entry checkpoint.

### 14.2 Actor target

Version 1 `select` observes exactly one actor.

This gives the runtime and type system an unambiguous event journal:

```text
select <BrowserActor> ...
```

Multi-actor selection can be expressed through structured composition where needed and may become a future extension only after cross-actor ordering semantics are explicitly specified.

### 14.3 Duration

`within <Duration>` is required in version 1.

There is no unbounded `select`.

This is deliberate.

Reactive test waits must have visible temporal bounds.

The effective deadline is the earliest of:

```text
select duration
enclosing timeout
test deadline
runner cancellation
```

### 14.4 Alternatives

Each `when` contains:

```text
event source
event binding
optional Boolean filter
branch body
```

Example:

```webtest
when network.response(r)
    if r.url.path == "/api/payment" && r.status == 200 {
    ...
}
```

The event binding's type comes from `EventSourceSchema`.

### 14.5 Event scanning

Given an explicit checkpoint:

1. begin at the event immediately after the cursor;
2. inspect journal events in `actor_sequence` order;
3. determine which alternatives can consume that event source;
4. evaluate alternatives for that event in source-code order;
5. evaluate each eligible `if` filter;
6. choose the first alternative whose source and filter match;
7. execute only that branch body;
8. complete the `select` according to that body's result.

Events that match no alternative/filter are skipped for this select.

The journal itself is not globally consumed; another independently scoped select with its own cursor may observe the same retained event.

### 14.6 Same-event alternative ordering

If two alternatives could match the same event:

```webtest
when network.response(r)
    if r.status >= 400 {
    ...
}

when network.response(r)
    if r.status >= 500 {
    ...
}
```

source order determines priority.

The analyzer may warn when an earlier alternative statically subsumes a later alternative where it can prove that fact.

### 14.7 Filter semantics

The optional filter must be a pure `Bool` expression.

It may use:

```text
event binding
visible transferable lexical bindings
pure helpers
pattern matching
ordinary pure operators
```

It may not:

```text
drive the browser
call providers
sleep
acquire resources
emit verdicts
mutate state
```

This prevents event selection from running arbitrary effectful code merely to decide whether an event qualifies.

### 14.8 Branch semantics

After an alternative is selected, its body is an ordinary structured WebTest scope.

It may perform allowed operations according to normal capabilities and ownership.

If it reaches the end successfully, the `select` succeeds.

If it executes `provide`, the `select` may produce a value.

If it emits a verdict, normal verdict semantics apply.

### 14.9 Result typing

A bound `select` result requires statically compatible branch results.

Example:

```webtest
let response = select receiver within 5s {
    when network.response(r)
        if r.status == 200 {
        provide r
    }

    timeout {
        fail "response did not arrive"
    }
}
```

A terminating `fail`, `pass`, `skip`, or `inconclusive` branch has bottom/terminating control-flow type and does not need to produce the result type.

Two successful value-producing branches must unify to a compatible result type.

### 14.10 Timeout branch

A `timeout` branch is optional.

If present, it executes when the effective select deadline expires before a matching event branch is selected.

If omitted, timeout produces a structured:

```text
SelectionTimeout
```

test failure containing:

```text
actor
duration
checkpoint/cursor identity
event source alternatives
bounded summary of relevant observed-but-unmatched events
source origin
```

### 14.11 Timeout is not `race`

The timeout alternative is part of select's temporal semantics.

It is not compiled as a user-visible `race` against a sleep task.

The scheduler may reuse internal deadline machinery but the plan retains an explicit `Select` node and timeout branch identity.

### 14.12 Cancellation

Cancellation while selecting immediately stops further selection, releases journal retention required only by that selection, and follows normal structured cleanup.

Cancellation is not reported as `SelectionTimeout`.

### 14.13 Event source failure

If the browser adapter can no longer reliably provide the required event source because of:

```text
browser disconnect
target destruction
protocol failure
event buffer overflow
adapter invariant violation
```

the select fails with the appropriate infrastructure/internal failure.

It does not wait until timeout and falsely claim that no matching application event occurred.

## 15. Guards

### 15.1 Purpose

A guard expresses an invariant that should remain true throughout a structured scope.

Examples:

```text
no console errors
no HTTP 5xx responses
no unexpected navigation
no unhandled application error event
```

These invariants should not need to be repeated as assertions after every browser action.

### 15.2 Guard declaration

Reusable guards may be declared and exported:

```webtest
export guard clean_browser(actor: BrowserActor) {
    when actor.console.error(e) {
        fail "console error: {e.message}"
    }

    when actor.network.response(r)
        if r.status >= 500 {
        fail "unexpected server error: {r.status} {r.url}"
    }
}
```

A guard declaration contains one or more event alternatives.

### 15.3 Guard parameters

Guard parameters have explicit types at exported boundaries.

Version 1 guards may accept:

```text
BrowserActor
transferable pure values used by filters/messages
```

They may not accept arbitrary callbacks or mutable references.

### 15.4 Guard activation

Use a lexically structured activation:

```webtest
with guards [
    clean_browser(alice),
    clean_browser(bob),
] {
    ...
}
```

All guard subscriptions are established before the body begins.

They remain active until:

```text
body completion
guard termination
cancellation
enclosing failure
enclosing timeout
```

and are then deterministically removed.

### 15.5 Guard bodies

Version 1 guard branches are intentionally constrained.

They may:

```text
evaluate pure expressions
use the event binding
format bounded messages
emit fail
emit inconclusive
attach supported bounded evidence
```

They may not:

```text
click/fill/navigate
call providers
acquire fixtures/resources
start concurrency
declare actors
perform arbitrary browser actions
```

A guard is an observer/invariant, not a background automation callback.

### 15.6 Guard failure

If:

```webtest
with guards [clean_browser(alice)] {
    ...
}
```

observes a matching guard failure event, the runtime:

1. records the guard trigger and event;
2. requests cancellation of the guarded body with reason `guard_failed`;
3. awaits body/resource cleanup;
4. returns the guard's semantic failure as primary unless a higher-severity infrastructure/internal cleanup condition applies.

### 15.7 Multiple near-simultaneous guard failures

The first terminating guard trigger according to normalized runtime event order becomes primary.

Additional guard violations already observed before cancellation completes are retained as bounded secondary evidence.

### 15.8 Guard success

A guard does not need an explicit successful completion event.

If the guarded body finishes without a terminating guard branch, guard deactivation succeeds.

### 15.9 Guard reuse

Guard declarations participate in Milestone F:

```text
imports
exports
definition
references
rename
completion
signature help
hover
workspace symbols
```

The guard dependency graph is acyclic.

Guards cannot invoke arbitrary other guards dynamically. Static composition through declared activation is permitted.

## 16. Patterns, guards, and event selection together

The three mechanisms solve distinct problems:

```text
Pattern
    Is this value acceptable?

Select
    Which relevant event happened first?

Guard
    Did any forbidden event happen during this scope?
```

Example:

```webtest
pattern CreatedUser(email: String) = subset {
    id: String,
    email: email,
}

guard no_errors(actor: BrowserActor) {
    when actor.console.error(e) {
        fail e.message
    }
}

test "signup emits user event" {
    actor user browser

    let mark = checkpoint(user)

    with guards [no_errors(user)] {
        user {
            open "/signup"
            fill label("Email") with "alice@example.com"
            click role("button", name: "Create account")
        }

        select user since mark within 5s {
            when websocket.received_json(event)
                if event.value matches CreatedUser("alice@example.com") {
                pass
            }

            timeout {
                fail "creation event was not observed"
            }
        }
    }
}
```

The pattern does not subscribe to anything.

The select does not globally police console errors.

The guard does not define the successful event.

Each construct has one semantic responsibility.

## 17. Versioned plan model

Milestone H extends the plan with explicit semantic nodes/data:

```text
TestDeclarationPlan {
    declaration_id,
    variants,
}

TestVariantPlan {
    variant_id,
    case_label,
    case_binding,
    case_value,
    child,
}

PatternDefinition {
    pattern_id,
    parameters,
    matcher,
}

Check {
    children,
    continuation_policy,
}

Verdict {
    kind,
    message,
}

Assume {
    condition,
    false_verdict,
}

ActorScope {
    actor_id,
    browser_configuration,
    acquire,
    body/lifetime,
    teardown,
}

EventCheckpoint {
    actor_id,
    cursor_binding,
}

Select {
    actor_id,
    cursor,
    timeout,
    alternatives,
    timeout_branch,
    result_type,
}

SelectAlternative {
    alternative_id,
    event_source,
    event_binding,
    pure_filter,
    child,
}

GuardDefinition {
    guard_id,
    parameters,
    alternatives,
}

GuardScope {
    activations,
    child,
}
```

The exact Rust representation may normalize some declarations separately from executable plan nodes, but the serialized plan must preserve these semantic distinctions.

## 18. Pattern plan representation

Patterns compile to a serializable matcher representation rather than closures:

```text
Matcher =
    Any
    Literal(Value)
    Type(TypeSchema)
    Record {
        policy: Exact | Subset,
        fields: [...]
    }
    ExactList([...])
    Each(Box<Matcher>)
    Contains(Box<Matcher>)
    Optional(Box<Matcher>)
    Absent
    Regex(CompiledRegexSpec)
    Range { lower, upper }
    OneOf([...])
    AllOf([...])
    Not(Box<Matcher>)
    Reference {
        pattern_id,
        bound_arguments,
    }
```

Pattern plan construction is deterministic.

No host-language function pointer or runtime closure appears in emitted plans.

## 19. Scheduler and reactive runtime

### 19.1 Structured ownership tree

The Milestone E execution tree expands conceptually:

```text
Execution
  -> TestVariant task
      -> actor resources
      -> guard scopes
      |    -> event subscriptions
      -> control-node tasks
      |    -> sequence
      |    -> parallel
      |    -> race
      |    -> retry
      |    -> timeout
      |    -> select
      -> browser/provider operations
```

Guard/event observer machinery is owned by an enclosing structured scope.

Nothing survives its owner.

### 19.2 Reactive subsystem

The runtime owns a protocol-neutral reactive subsystem:

```text
Browser backend events
        |
        v
normalization
        |
        v
redaction metadata classification
        |
        v
Actor event journal
        |
        +---- active Select
        |
        +---- active GuardScope
        |
        +---- trace/evidence projection
```

Reporter consumption is not part of event-selection semantics.

A slow reporter cannot delay whether a guard or `select` observes an event.

### 19.3 Subscriptions before stimulus

For `with guards`:

```text
install subscriptions
establish activation cursor
then start child body
```

This ordering is normative.

For explicit `checkpoint(actor)`:

```text
establish backend barrier
record cursor
return cursor
```

before later test statements execute.

### 19.4 Backpressure

Browser-protocol read loops must not block indefinitely on:

```text
trace writer
reporter
editor observation IPC
guard consumer
select consumer
```

The reactive journal owns bounded buffers and explicit overflow failure semantics.

### 19.5 Event processing cost

Pure `when ... if ...` filters must be bounded.

They cannot call arbitrary effectful code.

Regex/pattern operations use existing pattern resource limits.

A test cannot install millions of unbounded dynamic handlers because all select/guard alternatives are statically present in the plan.

## 20. Static analysis

Analysis validates at minimum:

### 20.1 Cases

* duplicate case labels;
* incompatible case record/value types;
* non-static case expressions;
* non-transferable case values;
* references to unknown case fields;
* variant IDs/collisions;
* secret-sensitive values where plan-emission policy prohibits them.

### 20.2 Patterns

* unknown patterns;
* duplicate parameters;
* argument arity/types;
* recursive pattern-reference cycles;
* invalid `absent` placement;
* invalid `optional` placement;
* matcher/value type incompatibility;
* invalid regex literals;
* invalid range bound types/order where statically knowable;
* invalid collection matcher target;
* inaccessible/imported pattern declarations.

### 20.3 Checks/verdicts

* invalid verdict syntax;
* non-string verdict messages where strings are required;
* invalid `assume` conditions;
* invalid `assume` else verdicts;
* illegal use of a value after a guaranteed terminating verdict;
* unreachable statements where the existing diagnostic policy reports them.

### 20.4 Actors

* unknown actor references;
* duplicate actor declarations in the same scope;
* actor use outside lifetime;
* actor transfer across forbidden boundaries;
* actor use inside `server`;
* parallel conflicting exclusive access;
* event cursor bound to the wrong actor;
* event cursor escaping actor/retry lifetime.

### 20.5 Select

* missing/non-positive duration;
* unknown event sources;
* event source unsupported by target actor/backend feature set where statically known;
* invalid event-binding use;
* effectful `when` filters;
* incompatible branch result types;
* duplicate timeout branch;
* multiple structurally unreachable alternatives where provable;
* cursor/actor mismatch.

### 20.6 Guards

* unknown guard declarations;
* guard argument mismatches;
* effectful guard filters;
* forbidden effectful guard branch operations;
* cyclic guard declaration dependencies;
* guard references to actors outside scope;
* incompatible event source/actor types.

Diagnostics use C.5 stable machine-readable codes and structured semantic details.

## 21. Suggested diagnostic codes

Milestone H adds stable diagnostic families conceptually equivalent to:

```text
duplicate_case_label
case_type_mismatch
case_not_static
case_value_not_transferable

unknown_pattern
pattern_argument_mismatch
pattern_cycle
invalid_pattern_context
invalid_absent_pattern
invalid_regex
invalid_range_pattern
pattern_type_mismatch

unknown_actor
duplicate_actor
actor_out_of_scope
actor_capability_mismatch
actor_concurrent_access
actor_cursor_mismatch

unknown_event_source
event_filter_not_pure
select_timeout_required
select_result_type_mismatch
duplicate_select_timeout
select_alternative_unreachable

unknown_guard
guard_argument_mismatch
guard_effect_not_allowed
guard_cycle

invalid_assume_condition
invalid_assume_verdict
unreachable_after_verdict
```

Runtime codes include at minimum:

```text
selection_timeout
reactive_event_overflow
event_source_unavailable
event_checkpoint_failed
guard_failed
```

Human wording may evolve independently of these stable semantic identities.

## 22. Runtime events and observations

Milestone E's versioned event envelope gains optional identity fields or payload metadata for:

```text
test_declaration_id
test_variant_id
case_label
actor_id
event_cursor_id
select_alternative_id
guard_id
guard_activation_id
check_id
verdict
```

Required new event kinds conceptually include:

```text
VariantStarted
VariantFinished

ActorAcquired
ActorReleased

CheckpointCreated

SelectStarted
ReactiveEventMatched
SelectAlternativeStarted
SelectFinished
SelectTimedOut

GuardActivated
GuardTriggered
GuardDeactivated

CheckStarted
CheckFailureRecorded
CheckFinished

VerdictEmitted
```

The runtime does not need to publish every unmatched browser event as a top-level execution event.

Trace/evidence policy may retain bounded unmatched reactive-event summaries where useful.

## 23. Failure evidence

### 23.1 Selection timeout

A timeout should explain what WebTest observed.

Example human output:

```text
message was not delivered within 5s

waiting for:
  websocket.received_json
    where value matches ChatMessage("alice", "hello")

observed since checkpoint:
  websocket.received_json
    {"type":"presence","user":"charlie"}

  websocket.received_json
    {"from":"alice","body":"Hello"}

nearest structural mismatch:
  $.body
    expected "hello"
    actual   "Hello"
```

Evidence is bounded.

The runtime may compute a nearest pattern mismatch only using deterministic semantic rules, not an LLM heuristic.

### 23.2 Guard failure

A guard failure preserves:

```text
guard declaration/call origin
actor
matched event source
event value summary
filter result
failure/verdict origin
currently executing guarded node
```

### 23.3 Check failure

A check aggregate preserves each underlying assertion with:

```text
source range
matcher
expected
actual
diff
actor where applicable
```

### 23.4 Pattern failure

Pattern evidence retains matcher paths and named pattern identity rather than expanding everything into an unreadable anonymous tree.

## 24. Trace artifact behavior

Milestone E trace format is extended rather than replaced.

The trace manifest declares the Milestone H schema/features used.

The viewer can show:

```text
declaration
  -> variant

variant timeline
  -> actor lanes
  -> operations
  -> checkpoints
  -> select waiting intervals
  -> matched events
  -> guard activation intervals
  -> guard triggers
  -> retries
  -> verdict
```

For a two-actor test, the viewer should make participant separation visible.

Example conceptual layout:

```text
time ─────────────────────────────────────>

sender    open ─ fill ─ click
                        |
receiver  open ─ checkpoint ────── websocket event
                       |                 |
guard     active ================= console clean
select                 waiting ==========> match
```

The viewer consumes the trace schema only.

It does not rerun pattern logic against a live application.

## 25. DAP behavior

### 25.1 Variants

A debug launch targets one concrete `TestVariantId`.

Launching the declaration without selecting a variant either:

```text
runs variants according to normal run semantics
```

or prompts through the editor adapter if supported; DAP itself receives explicit execution identities.

### 25.2 Actor frames

Actor-scoped operations display actor metadata:

```text
actor: sender
actor: receiver
```

DAP does not expose raw CDP object IDs as user variables.

### 25.3 Event bindings

When stopped inside:

```webtest
when websocket.received_json(message) {
    ...
}
```

DAP scopes expose the bounded/redacted typed `message`.

### 25.4 Select waiting state

Pausing during select displays:

```text
target actor
cursor/checkpoint
remaining deadline
event alternatives
bounded counts of observed unmatched events
```

It does not invent a JavaScript stack frame for the browser's event loop.

### 25.5 Guards

When a guard triggers and exception/verdict breakpoints are enabled, DAP may stop before applying the terminating verdict.

The stack/source chain contains:

```text
test variant
guard activation call
guard declaration
when alternative
verdict statement
```

### 25.6 Checks

Exception breakpoints may stop on each failed assertion inside `check`, but continuing allows the configured check accumulation semantics to proceed.

DAP must not accidentally turn soft assertions into fatal assertions merely because a debugger is attached.

## 26. Editor services

Milestone F editor services expand for H syntax and semantics.

### 26.1 Test discovery

Variant nodes are first-class test items with stable IDs.

### 26.2 Completion

Completion includes:

```text
case fields
pattern names
pattern parameters
pattern built-ins
actor names
event sources valid for select/guard actor
event payload fields
guard names
verdict keywords
```

Event completion is actor/type-aware.

### 26.3 Signature help

Signature help applies to:

```text
pattern references
guard activations
pattern built-ins
```

### 26.4 Hover

Hover may show:

```text
Pattern<User-like matcher>
BrowserActor
EventCursor<receiver>
WebSocketJsonEvent
guard signature
TestVariantId/case label
```

and relevant current-revision runtime evidence.

### 26.5 Navigation and rename

Definition/references/rename cover:

```text
patterns
pattern parameters
guards
guard parameters
actors
case binding
case fields where structural identity exists
```

Actor rename must update:

```text
actor blocks
checkpoint calls
select targets
guard activations
```

semantically rather than through text replacement.

### 26.6 Code actions

Candidate actions include:

```text
add missing pattern argument
replace misspelled pattern
replace unknown case field
replace unknown event source
qualify/import guard or pattern
insert explicit checkpoint when a diagnostic proves
a select is observing only events after an earlier stimulus
```

The final item should be offered only when semantic control-flow analysis can justify it; WebTest must not heuristically claim that every post-action select is wrong.

## 27. C.5 machine description and agent-facing semantics

`webtest describe --reporter json` expands from shared semantic metadata to expose:

```text
variant syntax/features
pattern built-ins and signatures
semantic verdicts
BrowserActor type/capabilities
event source schemas
guard capabilities/restrictions
select/check control semantics
```

A machine consumer can determine that:

```text
websocket.received_json
    payload = WebSocketJsonEvent

console.error
    payload = ConsoleEvent
```

without reading prose documentation.

Machine diagnostics expose structured candidate information for:

```text
unknown pattern
unknown event source
unknown actor
unknown case field
pattern argument mismatch
guard argument mismatch
```

C.5's rule remains: WebTest describes semantics; an external agent decides what source edit to make.

Milestone H adds no LLM SDK.

## 28. WASM and portable analysis

WASM can:

```text
parse H syntax
lower H HIR
expand static variants
assign stable variant IDs
type-check patterns
type-check actors
type-check select/guard event schemas
compile H plan nodes
produce diagnostics
perform completion/navigation/refactoring
describe H static capabilities
```

WASM cannot:

```text
create BrowserActors
establish checkpoints
consume browser events
execute select
run guards against live Chrome
```

Plans containing these operations compile successfully with appropriate native capability requirements.

Native and WASM variant IDs, pattern plans, event source resolution, diagnostics, and static plan structure must match for identical portable inputs.

## 29. Security and privacy

### 29.1 Actor isolation

WebTest never uses the user's personal Chrome profile.

Each actor uses an isolated test browser context under the existing browser security policy.

### 29.2 Reactive event values

Internal runtime matching may need access to event values before redaction so the authored test can assert against actual application data.

However:

```text
reporters
trace persistence
DAP
LSP observations
C.5 machine output
human diagnostics
```

receive redacted/bounded projections according to the existing secret policies.

Redaction must not alter runtime matching semantics.

### 29.3 Network data

Activating `network.response` does not imply capture of arbitrary response bodies.

Sensitive headers continue to follow redaction policy.

### 29.4 WebSocket data

WebSocket frame content is available to the test only when the test explicitly uses the corresponding event source or capture policy requires it.

Persisted evidence is redacted and bounded.

### 29.5 Case data

Case labels are considered non-secret metadata and may appear in:

```text
test explorer
CLI output
JUnit
trace indexes
CI logs
```

Authors must not place passwords/tokens/secrets in case labels.

Case values obey normal secret/redaction/plan-emission rules.

### 29.6 Pattern arguments

Pattern values marked secret by their source/schema remain secret when pattern failures are rendered.

A diff must never reveal a secret merely because it appeared as an expected pattern argument.

## 30. Configuration

Milestone H may add bounded reactive configuration:

```toml
[reactive]
max_events_per_actor = 10000
max_event_bytes_per_actor = 8388608
max_single_event_bytes = 262144
max_unmatched_evidence = 20
```

and verdict policy:

```toml
[verdict]
fail_on_inconclusive = true
```

Defaults must be documented and participate in project/plan identity when they affect semantics.

Configuration does not permit:

```text
unbounded event journals
unbounded selects
ignoring reactive overflow
turning infrastructure errors into passes
```

## 31. Architecture and crate responsibilities

* `syntax` remains the only parser/CST implementation and adds AST views for cases, patterns, checks, verdicts, actors, checkpoints, select alternatives, guards, and guard activation.
* `hir` owns stable `TestVariantId`, `PatternId`, `ActorId`, `GuardId`, alternative IDs, typed case bindings, pattern matcher HIR, actor references, and origin chains.
* `analysis` owns case expansion, variant identity, pattern/type resolution, actor lifetime/exclusivity analysis, event-source resolution, select-result typing, guard-effect checking, diagnostics, and deterministic plan construction.
* `provider` remains unchanged except where event-source metadata later becomes a general capability; H version 1 does not require arbitrary provider event streams.
* `plan` owns serializable variant, matcher, actor, checkpoint, select, guard, check, assume, and verdict nodes/data.
* `browser` owns protocol-neutral `BrowserActor` runtime contracts, normalized reactive event types, event-source schemas, and checkpoint/barrier semantics.
* `browser-cdp` implements actor contexts, protocol barriers, normalized console/network/WebSocket/navigation events, bounded subscriptions, and backend-specific cancellation.
* `runtime` owns actor resource scopes, event journals, select scanning/waiting, guard activation, check aggregation, verdict resolution, and interaction with Milestone E cancellation/deadlines.
* `observation` owns revision-bound structured H failure/evidence DTOs.
* `reporter` renders variant/actor/select/guard/check/verdict events without recomputing semantics.
* `editor` exposes H semantic DTOs, test variants, completion/navigation/actions, and current-revision runtime evidence.
* `lsp` remains protocol conversion only.
* `dap` maps H runtime identities/scopes/events into DAP without implementing H scheduling.
* `wasm` exposes portable static H semantics only.
* `app` composes CLI filtering, configuration, reporters, browser capabilities, and plan execution.
* `editors/vscode` maps variant test items and semantic DTOs to host APIs without parsing H syntax.

No TypeScript or adapter implementation may recreate pattern, case, actor, guard, or event-selection semantics.

## 32. Delivery slices

### Slice 1 — variant identity

Implement:

```text
case syntax
case type checking
compile-time variant expansion
TestVariantId
CLI/report/test-discovery identity
```

before adding new runtime concurrency behavior.

### Slice 2 — pattern core

Implement:

```text
pattern declarations
pattern references
Any/Literal/Type
exact/subset records
lists
optional/absent
structured diffs
```

through parser → HIR → analysis → plan → runtime → editor/WASM.

### Slice 3 — pattern algebra

Add:

```text
each
contains
regex
range
one_of
all_of
not
```

with deterministic diagnostics and resource limits.

### Slice 4 — semantic verdicts and checks

Implement:

```text
check
pass
fail
skip
inconclusive
assume
```

and update reporters/exit status/JUnit/DAP.

### Slice 5 — browser actors

Implement:

```text
BrowserActor
actor declaration
actor-scoped blocks
context isolation
resource ownership
parallel exclusivity analysis
actor-aware events/traces
```

without reactive select initially.

### Slice 6 — normalized browser events

Implement protocol-neutral:

```text
console
network
WebSocket
navigation
```

events and actor journals under deterministic fake-backend tests.

### Slice 7 — checkpoints

Implement actor-bound protocol barriers, event cursors, overflow retention, trace identity, cancellation, and cursor lifetime checks.

### Slice 8 — reactive select

Implement:

```text
select actor since cursor within duration
when
pure filters
timeout branch
result typing
structured timeout evidence
```

using Milestone E deadlines/cancellation.

### Slice 9 — guards

Implement:

```text
guard declarations
with guards
pre-body subscription installation
guard failure cancellation
guard trace/debug behavior
```

without allowing effectful background callbacks.

### Slice 10 — editor/agent/portable completion

Complete:

```text
test explorer variants
completion
hover
definition/references/rename
code actions
DAP
C.5 describe output
native/WASM parity
trace viewer
examples/docs
```

Every slice includes all applicable parser/HIR/analysis/plan/runtime/editor tests. No temporary second semantic implementation is allowed to accelerate an adapter.

## 33. Reference examples

### 33.1 Data-driven form validation

```webtest
test "signup rejects invalid email"
cases [
    case "missing-at" {
        email: "alice.example.com",
        message: "Enter a valid email",
    },

    case "missing-domain" {
        email: "alice@",
        message: "Enter a valid email",
    },

    case "empty" {
        email: "",
        message: "Email is required",
    },
]
as input {
    browser {
        open "/signup"
        fill label("Email") with input.email
        click role("button", name: "Continue")
        expect text(input.message).visible
    }
}
```

### 33.2 Flexible API response pattern

```webtest
pattern User(email: String) = subset {
    id: all_of(
        String,
        regex("^usr_[a-z0-9]+$"),
    ),
    email: email,
    roles: each(String),
    deleted_at: optional(one_of(Null, String)),
    password: absent,
}

test "created user is returned" {
    server {
        let response = http.post("/users", json: {
            email: "alice@example.com",
        })

        expect response.status == 201
        expect response.json matches User("alice@example.com")
    }
}
```

### 33.3 Accumulated UI assertions

```webtest
test "account page contains expected information" {
    browser {
        open "/account"

        check {
            expect text("Alice Example").visible
            expect text("alice@example.com").visible
            expect text("Premium").visible
            expect url().path == "/account"
        }
    }
}
```

### 33.4 Explicit prerequisite

```webtest
test "premium checkout" {
    server {
        let feature = app.feature_enabled(name: "premium_checkout")
    }

    assume feature
        else inconclusive "premium checkout is not enabled"

    browser {
        ...
    }
}
```

### 33.5 Two independent users

```webtest
test "presence is isolated by account" {
    actor alice browser
    actor bob browser

    alice {
        open "/login"
        fill label("Email") with "alice@example.com"
        click role("button", name: "Sign in")
    }

    bob {
        open "/login"
        fill label("Email") with "bob@example.com"
        click role("button", name: "Sign in")
    }

    parallel {
        alice {
            expect text("Signed in as Alice").visible
        }

        bob {
            expect text("Signed in as Bob").visible
        }
    }
}
```

### 33.6 Robust post-action event observation

```webtest
test "save request completes" {
    actor user browser

    user {
        open "/settings"
    }

    let mark = checkpoint(user)

    user {
        click role("button", name: "Save")
    }

    select user since mark within 5s {
        when network.response(r)
            if r.url.path == "/api/settings" &&
               r.status >= 200 &&
               r.status < 300 {
            pass
        }

        when network.response(r)
            if r.url.path == "/api/settings" &&
               r.status >= 400 {
            fail "settings request failed with {r.status}"
        }

        timeout {
            fail "settings request did not complete"
        }
    }
}
```

### 33.7 Persistent browser invariant

```webtest
guard no_browser_errors(actor: BrowserActor) {
    when actor.console.error(e) {
        fail "browser console error: {e.message}"
    }
}

test "checkout completes cleanly" {
    actor shopper browser

    with guards [no_browser_errors(shopper)] {
        shopper {
            open "/cart"
            click role("button", name: "Checkout")
            fill label("Name") with "Alice"
            click role("button", name: "Place order")
            expect text("Order confirmed").visible
        }
    }
}
```

### 33.8 `race` and `select` coexistence

```webtest
test "login resolves to one expected flow" {
    browser {
        open "/login"
        fill label("Email") with "alice@example.com"
        click role("button", name: "Sign in")

        let destination = race {
            sequence {
                expect text("Dashboard").visible
                provide "dashboard"
            }

            sequence {
                expect text("Verify your email").visible
                provide "verification"
            }
        }

        expect one_of("dashboard", "verification") matches destination
    }
}
```

This remains computation racing.

By contrast:

```webtest
select user within 5s {
    when console.error(e) { ... }
    when websocket.received_json(m) { ... }
}
```

remains reactive event selection.

Neither is syntactic sugar for the other.

## 34. Testing requirements

Required coverage includes all earlier milestone quality gates plus the following.

### 34.1 Parser/CST

Cover:

```text
valid/invalid cases declarations
duplicate/missing case labels
malformed case records
pattern declarations
all matcher forms
check/verdict/assume
actor declarations/blocks
checkpoint calls
select/cursor/within/when/timeout
guard declarations
with guards
half-typed/recovered forms
Unicode identifiers/strings
parser non-progress regressions
```

### 34.2 Variant analysis

Cover:

```text
stable TestVariantId under case reorder
identity change under label rename
same declaration label collisions
case schema inference
incompatible rows
static-evaluation restrictions
test explorer hierarchy
CLI exact variant filtering
fixture lifetime per variant
retry AttemptId vs TestVariantId
```

### 34.3 Pattern analysis

Cover every pattern type against:

```text
typed scalar values
records
Json
lists
nested patterns
optional/absent fields
exact/subset semantics
regex
ranges
one_of/all_of/not
imported/exported patterns
cycle detection
invalid argument types
```

### 34.4 Pattern runtime

Golden tests cover path-aware structured diffs for:

```text
missing fields
unexpected fields
wrong types
wrong literals
nested record mismatch
list length/order
each failures
contains failures
regex mismatch
range mismatch
alternative mismatch
negation mismatch
secret redaction
bounded output
```

### 34.5 Check semantics

Test:

```text
zero failures
one failure
multiple failures
nested checks
explicit fail
provider failure
browser action failure
infrastructure failure
parallel aggregate interaction
timeout interaction
source ordering
```

Property tests verify that only the specified recoverable failure classes continue.

### 34.6 Verdicts

Test:

```text
normal Pass
explicit Pass
Fail
Skipped
Inconclusive
assume true
assume false -> skip
assume false -> inconclusive
teardown after each verdict
teardown failure precedence
suite exit status
JUnit projection
JSON/event preservation
DAP exception/verdict behavior
```

### 34.7 Actor isolation

Real Chrome tests prove:

```text
cookies do not leak
localStorage does not leak
sessionStorage does not leak
navigation/page state does not leak
actors may operate concurrently
same-actor conflicting parallel use is rejected
contexts close exactly once
cancellation closes all actor contexts
```

### 34.8 Event normalization

Fake and real-browser tests cover:

```text
console.log/error
HTTP request/response metadata
redirects
WebSocket send/receive
valid/invalid WebSocket JSON
navigation
Unicode payloads
oversized payloads
redaction
browser target close
CDP disconnect
```

### 34.9 Checkpoints

Deterministic tests prove:

```text
events before checkpoint are not selected
events after checkpoint are selectable
an immediate event during the stimulus is not lost
cursor cannot target another actor
cursor cannot survive actor teardown
cursor cannot cross retry generation
backend barrier failure is explicit
```

A stress fixture should deliberately cause the expected event immediately after the triggering browser action to detect listener-installation races.

### 34.10 Select

Model/fake-clock tests cover:

```text
first eligible event
unmatched events
filtered events
two alternatives matching same event
source-order priority
successful result binding
incompatible branch results
explicit timeout branch
implicit SelectionTimeout
enclosing timeout winning
cancellation
browser disconnect
event overflow
bounded unmatched evidence
```

### 34.11 Guards

Tests cover:

```text
subscription before guarded body
no trigger
console trigger
network trigger
filtered trigger
multiple guards
near-simultaneous triggers
guard cancellation of body
body failure before guard trigger
guard trigger before body failure
cleanup
forbidden effectful guard bodies
import/export/rename
```

### 34.12 Concurrency composition

Stress/model tests cover combinations:

```text
parallel actors
race containing actor scopes
retry around select
select inside retry
timeout around guard scope
check inside actor
guard around parallel
variant + fixture + retry + actor
cancellation during checkpoint
cancellation during select branch
```

Property tests continue to assert the Milestone E invariant:

```text
no runtime task/resource/subscription survives its owning structured scope
```

### 34.13 Event buffer stress

Run deterministic seeded stress tests with:

```text
high console volume
high network volume
high WebSocket volume
slow select consumer
active guards
trace writer enabled/disabled
reporter backpressure
```

Verify:

```text
no CDP read deadlock
bounded memory
explicit semantic overflow
terminal events retained
cleanup after overflow
```

Stress tests print the deterministic seed on failure.

### 34.14 Editor/WASM/DAP

Parity and protocol tests cover:

```text
variant discovery IDs
pattern completion/signatures/hover
actor completion/rename
event-source completion/payload fields
guard definition/references
select diagnostics
Unicode positions
native/WASM plan parity
DAP actor/event scopes
DAP variant targeting
guard/verdict exception stops
stale observation rejection
```

## 35. Performance and boundedness

Add benchmark fixtures covering:

```text
hundreds of variants
hundreds of reusable patterns
nested pattern compositions
multi-actor plans
large but bounded reactive event streams
workspace references to guards/patterns
```

The following must remain bounded and measurable:

```text
variant expansion memory
pattern plan size
pattern matching runtime
pattern diff size
event journal memory
select scan cost
guard filter cost
trace event volume
editor completion latency
```

Case expansion may increase executable test count substantially, but it must not require duplicating unrelated syntax/HIR data per variant.

Pattern references should share immutable matcher structure in compiler/runtime representations where safe rather than recursively cloning large matcher trees for every assertion.

Before final implementation, record benchmark baselines and make major regressions visible in CI.

## 36. Compatibility

### 36.1 Existing tests

Every valid pre-H test must retain its behavior unless it relied on previously undocumented matcher semantics that H explicitly formalizes.

No explicit actor is required for existing browser tests.

### 36.2 Existing `race`

`race` semantics do not change.

### 36.3 Existing assertions

Existing equality/state/assertion behavior remains valid.

Reusable patterns extend matching; they do not turn ordinary equality into pattern matching.

### 36.4 Plans

Milestone H increments the plan format as required.

Older runtimes reject plans containing unknown H node kinds.

They do not best-effort interpret them.

### 36.5 Events/traces

Schema versions are incremented where new required meanings are introduced.

Older trace viewers may show generic unknown-event entries only if the trace compatibility contract explicitly permits it; they must never misrepresent a select or guard as an ordinary browser action.

## 37. Documentation requirements

Documentation must clearly explain:

```text
cases are test expansion, not loops
patterns are matchers, not constructors
actors are browser contexts, not application users
race races computations
select waits for events
check accumulates assertions
guards enforce scoped invariants
skip differs from inconclusive
checkpoint prevents lost-event races
```

At least one guide should compare:

```webtest
race { ... }
```

with:

```webtest
select actor within ... { ... }
```

because confusing them would produce fragile tests.

At least one guide should explain why:

```webtest
let mark = checkpoint(receiver)
trigger_action()
select receiver since mark ...
```

is preferable when an action can emit the event before the test reaches the `select`.

Agent-facing documentation should use the same terminology and schemas exposed by `webtest describe`; no separate "LLM dialect" is introduced.

## 38. Acceptance criteria

Milestone H is complete only when all of the following hold.

1. A test declaration with two explicitly labeled cases is discovered as two stable variants, can run/debug either variant independently, and preserves its `TestVariantId` when the cases are reordered.

2. Case values are statically typed and variant enumeration requires no browser, provider, application, filesystem, network, or arbitrary runtime execution.

3. Reusable imported/exported patterns match typed and JSON values through a deterministic serializable matcher representation with structured path-aware diffs.

4. `subset` and `exact` record patterns, optional/absent fields, collection matchers, regexes, ranges, alternatives, conjunction, and negation behave identically across native analysis/runtime fixtures and portable static WASM plan fixtures where applicable.

5. `check {}` executes all eligible sequential assertions, aggregates their failures in stable order, and still terminates immediately for provider/infrastructure/internal/cancellation conditions according to this specification.

6. `pass`, `fail`, `skip`, and `inconclusive` are distinct structured outcomes; `assume` can produce skip/inconclusive without converting the result into a generic assertion failure.

7. Two declared browser actors receive isolated browser contexts and can execute concurrently without cookie/storage/page-state leakage.

8. Static analysis rejects concurrent conflicting use of one exclusively owned actor while permitting independent actors in parallel branches.

9. `checkpoint(actor)` establishes a tested backend ordering barrier and returns an actor-bound cursor that cannot escape actor/retry lifetime or be used with another actor.

10. An event emitted immediately after a triggering browser operation but before the source reaches the later `select` is still observed when the test uses a pre-action checkpoint.

11. `select actor since cursor within duration` examines normalized actor events in deterministic actor-event order, applies alternatives/filter priority exactly as specified, executes only the selected branch, and never behaves as speculative `race`.

12. Select timeout, reactive event overflow, browser disconnect, and cancellation remain distinguishable structured outcomes.

13. Reusable guards are fully installed before a guarded body begins, terminate/cancel the guarded scope on a matching failure event, never outlive that scope, and cannot execute forbidden effectful background actions.

14. `race {}` continues to pass all Milestone E conformance tests unchanged and remains a distinct plan/runtime node from `Select`.

15. Event journals remain bounded under stress, do not deadlock the browser protocol read loop, and fail explicitly rather than silently dropping events required by active semantic consumers.

16. Traces can reconstruct variant identity, actor lanes, checkpoints, select waiting/matches, guards, checks, verdicts, retries, cancellations, and source origins without executing project code.

17. DAP can debug a concrete variant, identify actor context, inspect selected event bindings, stop on guard/verdict failures, and continue through accumulated checks without changing runtime semantics.

18. Editor services discover variants and provide completion/navigation/rename/hover for cases, patterns, actors, guards, and event sources entirely from shared Rust semantic services.

19. `webtest describe` exposes Milestone H pattern, actor, event-source, guard, select, check, and verdict semantics in versioned machine-readable form without an agent-specific implementation.

20. WASM parses, analyzes, discovers variants, type-checks patterns/actors/selects/guards, and produces the same portable H plans/diagnostics/semantic IDs as native analysis while clearly refusing native runtime capabilities.

21. Redaction prevents secrets from leaking through case summaries, pattern diffs, WebSocket/network evidence, guards, traces, DAP, LSP observations, and machine-readable output without altering the actual runtime values used to decide test semantics.

22. No LLM SDK, arbitrary callback runtime, detached task system, second parser, second type checker, second locator implementation, or raw CDP event model is introduced.

23. Full workspace, browser, provider, bridge, structured-concurrency, trace, LSP, DAP, VS Code/Cursor, native/WASM parity, packaging, and security quality gates pass.

The roadmap acceptance statement is thereby satisfied: WebTest can model independently reportable test data, reusable acceptable-value structures, multiple isolated browser participants, persistent invariants, and bounded asynchronous event behavior while retaining deterministic plans, explicit resource ownership, static analysis, structured diagnostics, and one shared language/runtime implementation.

## 39. Long-term implication

After Milestone H, the core WebTest abstraction is no longer merely:

```text
drive a browser
perform actions
make assertions
```

It becomes:

```text
declare test variants
establish application state
create isolated participants
apply stimuli
observe typed state and events
match values against reusable specifications
enforce invariants over time
resolve an explicit semantic verdict
```

The complete conceptual pipeline is:

```text
.webtest source
      |
      v
lossless CST
      |
      v
typed HIR
      |
      +---- declarations
      |       |
      |       +-- functions
      |       +-- fixtures
      |       +-- patterns
      |       +-- guards
      |       +-- test variants
      |
      v
static analysis
      |
      +-- types
      +-- capabilities
      +-- actor/resource ownership
      +-- event-source schemas
      +-- pattern checking
      +-- variant identity
      |
      v
serializable TestPlan
      |
      +-- Sequence / Parallel / Race
      +-- Retry / Timeout / ResourceScope
      +-- Check
      +-- ActorScope
      +-- EventCheckpoint
      +-- Select
      +-- GuardScope
      +-- Verdict
      |
      v
structured runtime
      |
      +---- typed providers / app bridge
      |
      +---- browser actors
      |       |
      |       +-- semantic browser operations
      |       +-- normalized event journals
      |
      +---- scheduler
      |       |
      |       +-- structured computations
      |       +-- deadlines/cancellation
      |       +-- reactive selection
      |       +-- scoped guards
      |
      v
versioned semantic events
      |
      +-- terminal reporters
      +-- traces
      +-- LSP observations
      +-- DAP
      +-- test explorer
      +-- machine/agent consumers
```

The language remains intentionally constrained.

The runtime, rather than the test author, owns the difficult mechanics of:

```text
browser-context isolation
cancellation
cleanup
retry ownership
event subscription
event buffering
lost-event prevention
deadline handling
failure aggregation
pattern diffs
guard lifetime
variant identity
source mapping
```

That constraint is a feature.

A human or machine author states what the test means; WebTest owns the operational semantics required to execute that meaning robustly.
