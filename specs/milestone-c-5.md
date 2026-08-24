# Milestone C.5 — Semantic Inspection and Machine Feedback

## 0. Status and dependencies

This specification inserts Milestone C.5 between [`milestone-c.md`](./milestone-c.md) and [`milestone-d.md`](./milestone-d.md). It depends on the semantic locator, browser evidence, and source-mapped runtime foundations from [`milestone-b.md`](./milestone-b.md), plus the typed values, provider schemas, capability analysis, structured observations, and versioned machine output established by [`milestone-c.md`](./milestone-c.md).

Milestone C.5 makes WebTest's existing semantics directly discoverable and consumable by external tools. A human, editor, script, or LLM agent should be able to inspect an unfamiliar page, discover valid WebTest vocabulary and semantic locators, author a test, statically check it, execute it, and diagnose or repair a failure without bypassing WebTest through raw CDP, Playwright, DOM scraping, or a second language implementation.

It does not depend on the application bridge in Milestone D. When Milestone D later adds statically known `app.*` functions, those functions participate in the same discovery interfaces introduced here.

**Implementation status (2026-08-23): proposed.**

[`future-functionality.md`](./future-functionality.md) must be updated to place this milestone after C and before D.

## 1. Outcome

An external client with no private knowledge of WebTest internals can discover the semantic surface of a running page and use ordinary WebTest commands as a closed authoring and repair loop:

```sh
webtest inspect /login --reporter json
webtest check tests/login.webtest --reporter json
webtest test tests/login.webtest --reporter json
```

For a page containing an email field, password field, and sign-in button, `inspect` returns bounded structured information including valid WebTest locator expressions:

```json
{
  "kind": "inspection",
  "inspection_schema_version": 1,
  "page": {
    "url": "http://127.0.0.1:3000/login",
    "title": "Sign in"
  },
  "elements": [
    {
      "role": "textbox",
      "name": "Email",
      "visible": true,
      "enabled": true,
      "editable": true,
      "supported_actions": ["fill", "type", "press"],
      "preferred_locator": {
        "source": "label(\"Email\")"
      }
    },
    {
      "role": "button",
      "name": "Sign in",
      "visible": true,
      "enabled": true,
      "supported_actions": ["click", "hover"],
      "preferred_locator": {
        "source": "role(\"button\", name: \"Sign in\")"
      }
    }
  ]
}
```

The client can then author normal WebTest:

```webtest
test "user signs in" {
    browser {
        open "/login"
        fill label("Email") with "alice@example.com"
        fill label("Password") with "secret"
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
```

If the client instead writes:

```webtest
click role("button", name: "Log in")
```

and the page contains only a `"Sign in"` button, the failure remains `locator_not_found`, but its structured evidence includes bounded repair candidates:

```json
{
  "code": "locator_not_found",
  "requested": {
    "source": "role(\"button\", name: \"Log in\")"
  },
  "repair_hints": [
    {
      "kind": "locator_candidate",
      "replacement": {
        "source": "role(\"button\", name: \"Sign in\")"
      },
      "reason": "same accessible role with a nearby accessible name"
    }
  ]
}
```

WebTest does not silently substitute the candidate. The authored source remains authoritative.

## 2. Architectural rule

Machine consumption is another adapter over shared WebTest semantics, not a parallel implementation of WebTest.

```text
                    WebTest semantic core
                            |
          +-----------------+-----------------+
          |                 |                 |
          v                 v                 v
      human CLI        editor/LSP/DAP     machine JSON
          |                 |                 |
          +-----------------+-----------------+
                            |
                            v
                    external consumer
                    human / script / agent
```

The same locator resolver, provider schemas, type checker, diagnostics, runtime failures, redaction rules, and source identities power all adapters.

No agent-facing component may:

* parse `.webtest` independently;
* implement its own locator semantics;
* infer provider signatures from prose;
* reinterpret test failures from formatted terminal strings;
* drive Chrome directly to obtain information WebTest itself can expose;
* silently rewrite source or alter runtime behavior.

Human-readable messages and machine-readable output are projections of common typed DTOs.

The architectural goal is not "LLM integration." It is that every semantic operation useful to an author is discoverable and every failure useful for repair has a stable machine-readable representation.

## 3. Scope

Milestone C.5 includes:

* a protocol-neutral semantic page-inspection model;
* `webtest inspect` for one-shot inspection of a running page;
* a static `webtest describe` command for discovering the WebTest/project capability surface;
* preferred and alternate WebTest locator candidates derived from the shared locator semantics;
* operation/action metadata for inspected elements;
* stable machine-readable representations of static diagnostics and runtime failures;
* structured repair hints for locator, actionability, option, URL, and selected static-analysis failures;
* deterministic, bounded candidate ordering;
* source revision and source-range identity in machine diagnostics;
* bounded page state attached to relevant runtime failures;
* explicit machine-output schema versioning;
* shared redaction, privacy, and output-size limits;
* a deterministic acceptance fixture proving that WebTest alone exposes enough information to author and repair a basic browser test.

Existing `check`, `test`, `build`, JSON/JSONL reporting, editor services, and runtime observations are extended rather than replaced.

## 4. Non-goals

This milestone does not:

* embed or call an LLM;
* depend on OpenAI, Anthropic, MCP, or another agent protocol;
* define autonomous test generation;
* add a persistent interactive browser-agent session;
* implement arbitrary crawling or site exploration;
* inspect application source code to infer browser behavior;
* implement `app.*` or any Milestone D bridge protocol;
* add patterns, data-driven cases, named actors, event selection, guards, or explicit verdict syntax;
* add Milestone E structured concurrency;
* add Milestone F modules, reusable fixtures, or the full completion/navigation surface;
* automatically heal or persist modifications to failing tests;
* synthesize structural CSS or XPath selectors as "robust" locator replacements;
* require screenshot or vision-model interpretation for normal semantic discovery;
* expose cookies, browser storage, passwords, tokens, or other secret browser state;
* change the semantics of an existing `.webtest` program.

The output is intended to make test authoring and repair tractable for machines and humans. WebTest remains the authority on whether a test is valid and what it means.

## 5. Semantic page inspection

### 5.1 Inspection snapshot

Introduce a protocol-neutral inspection DTO owned outside `browser-cdp`, conceptually:

```text
PageInspection {
    inspection_schema_version,
    snapshot_id,
    browser_version,
    page,
    elements,
    truncation,
}

PageSummary {
    url,
    title,
}

InspectableElement {
    role,
    accessible_name,
    label,
    placeholder,
    test_id,
    dom_id,
    states,
    supported_actions,
    preferred_locator,
    alternate_locators,
}

ElementStates {
    visible,
    enabled,
    editable,
    checked,
    selected,
}
```

Fields that do not apply are absent rather than populated with invented empty values.

`snapshot_id` identifies one observed browser state within the command invocation. It is diagnostic identity only. It is not stable across browser runs and must never be emitted into `.webtest` source.

Raw CDP node IDs, backend node IDs, object IDs, execution-context IDs, and browser-process handles do not appear in the portable inspection model.

### 5.2 Inspection semantics

Inspection uses the same browser and accessibility semantics as normal WebTest execution.

Accessible role and name are computed through the same browser abstraction used by `role(...)`. Label relationships use the same rules as `label(...)`. Visibility, enabled state, editability, checked state, and other actionability facts use the same semantic predicates used by actions and assertions.

An element reported as:

```json
{
  "preferred_locator": {
    "source": "role(\"button\", name: \"Sign in\")"
  },
  "supported_actions": ["click"]
}
```

must be resolvable by the normal WebTest locator engine in that same snapshot and must satisfy the semantic preconditions implied by the reported action set.

Inspection is not a second approximate DOM model.

### 5.3 Inspection population

By default, inspection reports:

* visible interactive controls;
* visible elements with meaningful accessibility roles/names;
* labeled form controls;
* visible user-facing text nodes that can produce a useful `text(...)` locator;
* configured test-ID elements when they represent useful interaction/assertion targets.

It does not dump the complete DOM by default.

Hidden implementation nodes, scripts, style nodes, framework bookkeeping, raw attributes without WebTest meaning, and anonymous layout containers are omitted unless needed as bounded evidence for an ambiguity or actionability failure.

The output is intended to answer:

```text
What can I meaningfully interact with or assert against here?
How would I express that using WebTest?
What operations are valid on that target?
```

rather than:

```text
What is every node in this HTML document?
```

### 5.4 Locator candidates

An inspected element may have multiple valid locator expressions:

```text
label("Email")
role("textbox", name: "Email")
test_id("login-email")
id("email")
placeholder("you@example.com")
```

The inspection model distinguishes:

```text
preferred_locator
alternate_locators
```

Every emitted locator candidate must:

1. use an existing WebTest locator kind;
2. be valid under current project configuration;
3. resolve to exactly one element in the observed snapshot;
4. round-trip through the canonical WebTest locator renderer;
5. carry enough semantic metadata to explain why it was selected.

WebTest never fabricates a locator string that has not been validated against the snapshot.

### 5.5 Preferred locator policy

Version 1 uses deterministic semantic preference rules.

For form controls, prefer a unique associated label when available:

```webtest
label("Email")
```

For named interactive/accessibility elements, prefer unique role plus accessible name:

```webtest
role("button", name: "Sign in")
```

An explicit unique configured test ID is a strong alternate and may become preferred when no stronger user-facing semantic relationship exists:

```webtest
test_id("checkout-submit")
```

Placeholder and DOM ID locators may be emitted when unique:

```webtest
placeholder("Search")
id("submit")
```

Exact user-facing text may be used when it identifies the intended assertion/action target:

```webtest
text("Welcome")
```

CSS and XPath remain supported author escape hatches but are never synthesized automatically by semantic inspection.

The exact ordering is part of the inspection schema version. Changing preference semantics in a way that can change generated preferred locators requires an inspection-schema compatibility decision rather than an undocumented heuristic change.

### 5.6 Candidate stability

The same page state, managed Chrome version, WebTest version, configuration, and inspection limits must produce the same candidate ordering and canonical locator source.

Document order may break otherwise equivalent candidate ties. Randomness, hash-map iteration order, browser object identity, timing races, and model-generated ranking are forbidden.

A preferred locator describes the current observed state. It is not a guarantee that the application will never change.

## 6. `webtest inspect`

Add:

```sh
webtest inspect [url] [--headed] [--reporter human|json]
```

An absolute URL is used directly. A relative URL resolves against `browser.base_url` exactly as `open`.

If no URL is supplied, `browser.base_url` is used when configured. If neither an argument nor a configured base URL exists, inspection fails as a CLI/configuration error.

The command:

1. resolves project configuration;
2. resolves Chrome using the normal browser-selection rules;
3. launches or reuses the normal browser host for the invocation;
4. creates a fresh isolated browser context;
5. opens the requested URL using normal navigation semantics;
6. waits only according to the normal bounded navigation policy;
7. collects one semantic inspection snapshot;
8. renders the snapshot;
9. closes the context through the ordinary lifecycle path.

Inspection does not reuse authentication state from another test or browser profile.

`--headed` changes visibility only; it does not change inspection semantics.

Human output is concise and author-oriented:

```text
http://127.0.0.1:3000/login — Sign in

fillable
  label("Email")                 textbox "Email"
  label("Password")              textbox "Password"

clickable
  role("button", name: "Sign in")  button "Sign in"
```

JSON exposes the complete bounded DTO.

## 7. Static capability discovery

Add:

```sh
webtest describe [paths...] --reporter human|json
```

`describe` is static. It does not launch Chrome or execute tests.

It reports the WebTest surface available for the resolved project:

```text
compiler/language version
machine schema versions
locator kinds
browser operations
assertion matcher families
core types
execution capabilities
configured built-in provider schemas
project-visible provider operations
relevant configuration-derived capabilities
```

Example machine output:

```json
{
  "kind": "description",
  "description_schema_version": 1,
  "language": {
    "locators": [
      "id",
      "role",
      "label",
      "text",
      "placeholder",
      "test_id",
      "css",
      "xpath"
    ],
    "browser_operations": [
      "open",
      "click",
      "fill",
      "type",
      "press",
      "check",
      "uncheck",
      "select",
      "hover",
      "wait",
      "expect"
    ]
  },
  "providers": {
    "http": {
      "operations": {
        "get": {},
        "post": {}
      }
    }
  }
}
```

Provider operation details are emitted from the same `ProviderSchema`, `OperationSchema`, `ParameterSchema`, and `TypeSchema` values used by static analysis.

`describe` must not maintain a handwritten duplicate registry of the grammar or providers.

When Milestone D is implemented, the configured offline `app` manifest participates naturally:

```json
{
  "providers": {
    "app": {
      "operations": {
        "create_user": {
          "parameters": {
            "email": "String"
          },
          "returns": {
            "id": "Int",
            "email": "String"
          }
        }
      }
    }
  }
}
```

The application does not need to be running for such static discovery.

## 8. Machine-readable diagnostics

### 8.1 Shared diagnostic representation

Static analysis already produces semantic diagnostics. Milestone C.5 makes their machine contract explicit.

Conceptually:

```text
MachineDiagnostic {
    code,
    severity,
    message,
    source,
    related,
    semantic_details,
    repair_hints,
}

SourceIdentity {
    path,
    source_revision,
    byte_range,
    start_line,
    start_column,
    end_line,
    end_column,
}
```

Byte ranges are canonical for source mutation. Native line/column values use the documented CLI coordinate convention. LSP continues translating through the shared UTF-8/UTF-16 coordinate machinery and does not redefine the stored range.

Diagnostics preserve structured semantic data when known:

```text
expected type
actual type
unknown name
known names
provider
operation
argument
capability
binding
matcher
```

Clients must not have to parse these facts back out of an English message.

### 8.2 Stable diagnostic codes

Machine-consumable failures use stable codes.

Existing browser codes remain distinct:

```text
locator_not_found
locator_ambiguous
locator_invalid
element_detached
element_not_visible
element_unstable
element_disabled
element_obscured
element_not_editable
option_not_found
action_timeout
navigation_timeout
browser_disconnected
```

Static diagnostics likewise receive stable identifiers for externally useful classes such as:

```text
unknown_name
unknown_member
unknown_provider
unknown_operation
unknown_argument
missing_argument
duplicate_argument
type_mismatch
capability_mismatch
non_transferable_value
```

Human wording may improve without requiring clients to change behavior.

Renaming or changing the semantic meaning of a stable code is a compatibility change.

## 9. Structured repair hints

### 9.1 General model

Diagnostics and runtime observations may contain bounded typed repair hints:

```text
RepairHint {
    kind,
    replacement,
    source_range,
    reason,
    evidence,
}
```

Initial hint kinds include:

```text
locator_candidate
name_candidate
member_candidate
argument_candidate
option_candidate
```

A repair hint is advisory evidence, not an instruction that WebTest applies automatically.

### 9.2 Locator-not-found repair

When a semantic locator fails, WebTest may inspect the current page and return nearby semantic candidates.

For:

```webtest
click role("button", name: "Log in")
```

against:

```text
button "Sign in"
```

the failure may contain:

```json
{
  "code": "locator_not_found",
  "repair_hints": [
    {
      "kind": "locator_candidate",
      "replacement": {
        "source": "role(\"button\", name: \"Sign in\")"
      },
      "reason": "same accessible role with a nearby accessible name"
    }
  ]
}
```

Candidate search is deterministic and bounded.

Ranking may consider, in order:

* operation compatibility;
* exact semantic role;
* exact locator-kind-relevant metadata;
* normalized accessible-name similarity;
* visibility/actionability;
* deterministic document order.

Approximate name similarity is used only to rank exact candidate locators already observed on the page. WebTest never changes the matching semantics of the authored locator itself.

Implementation-specific floating-point similarity scores are not a public compatibility contract.

### 9.3 Ambiguous locators

For:

```webtest
click text("Save")
```

that matches multiple elements, `locator_ambiguous` includes bounded candidates and, where possible, stronger unique locators:

```json
{
  "code": "locator_ambiguous",
  "repair_hints": [
    {
      "kind": "locator_candidate",
      "replacement": {
        "source": "role(\"button\", name: \"Save\")"
      }
    }
  ]
}
```

If no unique stronger locator exists, WebTest reports ambiguity rather than inventing structural selectors.

### 9.4 Actionability failures

Actionability failures expose facts rather than pretending that changing the locator necessarily fixes the problem.

For an obscured button:

```json
{
  "code": "element_obscured",
  "semantic_details": {
    "target": {
      "source": "role(\"button\", name: \"Continue\")"
    },
    "visible": true,
    "enabled": true,
    "receives_pointer_input": false,
    "obstruction": {
      "role": "dialog",
      "name": "Cookie preferences"
    }
  }
}
```

When the obstruction itself has a valid locator, it may be included as related semantic evidence.

### 9.5 Options and static names

`option_not_found` includes a bounded list of currently available option labels/values.

Static unknown-name/member/provider/operation diagnostics may include deterministic edit-distance candidates derived from the actual semantic symbol table:

```json
{
  "code": "unknown_member",
  "semantic_details": {
    "requested": "emial",
    "receiver_type": "{ id: Int, email: String }"
  },
  "repair_hints": [
    {
      "kind": "member_candidate",
      "replacement": "email"
    }
  ]
}
```

This does not pull Milestone F completion into C.5. It only exposes bounded correction candidates for an already-invalid authored name.

## 10. Runtime observations for repair

Relevant runtime failures preserve both the original failure and a bounded semantic snapshot of the state in which it occurred.

Browser failure observations may include:

```text
current URL
document title
requested locator
resolved/nearby candidates
actionability facts
available select options
relevant console errors
semantic repair hints
artifact references
```

The original failure remains primary. Inspection or repair-hint generation is best-effort.

If secondary inspection fails:

```text
locator_not_found
    secondary:
        semantic inspection unavailable: browser target closed
```

must remain a locator failure rather than being replaced by the inspection failure.

Successful actions continue to emit ordinary step events. Machine event output may include bounded semantic effects already known to the runtime, such as a changed URL or document title, but C.5 does not introduce continuous DOM mutation recording.

## 11. Machine schema compatibility

Machine-facing DTOs introduced by this milestone are explicitly versioned:

```text
inspection_schema_version
description_schema_version
diagnostic_schema_version
repair_hint_schema_version
```

The existing reporter/event envelope retains its own schema version.

A version defines:

* required fields;
* optional fields;
* stable enum/code meanings;
* source-coordinate semantics;
* locator-source canonicalization expectations;
* truncation markers;
* compatibility behavior for unknown fields.

Consumers must ignore documented unknown optional fields.

Removing a field, changing an existing field's type, changing a stable code's meaning, or changing locator candidate semantics incompatibly requires the affected schema version to increment.

Human-readable text is not a compatibility interface.

Golden JSON fixtures cover every machine DTO.

## 12. Determinism and identity

Machine output must be reproducible where the observed application state is reproducible.

Deterministic inputs include:

```text
WebTest/compiler version
managed Chrome version
project configuration
provider schemas
source revisions
page state
inspection limits
```

Given equivalent inputs:

* semantic element ordering is deterministic;
* locator preference ordering is deterministic;
* rendered locator source is canonical;
* repair-hint ordering is deterministic;
* source ranges and revisions are stable;
* truncation occurs at deterministic boundaries.

Browser-internal object identity, memory addresses, random IDs, hash iteration order, and wall-clock timing do not participate in semantic ordering.

Execution IDs and snapshot IDs may vary between runs and are clearly marked as execution identity rather than semantic identity.

## 13. Security, privacy, and bounds

Semantic inspection is potentially sensitive because visible pages may contain user data.

The following rules are mandatory:

* password/control secret values are never emitted;
* ordinary form-control values are omitted by default;
* cookies, authorization headers, local storage, session storage, IndexedDB, browser credentials, and service-worker state are not part of inspection;
* configured redaction occurs before DTO construction;
* text, accessible names, labels, placeholders, URLs, candidate sets, and collections are bounded;
* URL query parameters configured as sensitive are redacted;
* generated repair hints never reintroduce a value already classified as secret;
* raw DOM is not emitted as part of the semantic inspection DTO;
* screenshots remain separately controlled artifacts;
* machine reporters cannot bypass normal evidence limits.

Extend redaction configuration where needed:

```toml
[redaction]
headers = ["authorization", "cookie", "set-cookie"]
json_fields = ["password", "token", "secret"]
query_params = ["token", "code", "key"]
```

Inspection limits are explicit:

```toml
[inspection]
max_elements = 500
max_candidates_per_element = 4
max_text_bytes = 256
include_hidden = false
```

If a limit is reached, output contains structured truncation metadata. It must not silently appear complete.

## 14. Editor, DAP, and WASM behavior

The protocol-neutral editor layer may expose the same structured diagnostic details and repair hints to adapters.

LSP may render repair hints as related diagnostic information or narrowly safe quick-fix candidates when the editor protocol supports them. The Rust semantic result remains authoritative. This milestone does not require the general completion/navigation feature set reserved for Milestone F.

DAP continues to use the ordinary runtime. Failure stops may display structured semantic details and candidate locators without independently querying the DOM in the VS Code extension.

WASM can:

* produce static machine diagnostics;
* expose static language/provider description DTOs available from its supplied project inputs;
* serialize repair hints for static errors.

WASM cannot execute `webtest inspect` because it has no native Chrome capability. It returns the same explicit unsupported/native-capability result used for other host-only behavior.

No TypeScript extension code implements inspection, locator ranking, or repair semantics.

## 15. Architecture and crate responsibilities

* `syntax` is unchanged by this milestone. No new `.webtest` syntax is required.
* `hir` is unchanged except where existing semantic metadata must become externally representable.
* `analysis` exposes structured semantic diagnostic details and deterministic correction candidates from existing symbol/type/provider information.
* `provider` exposes project-visible schemas through a protocol-neutral description DTO rather than a second CLI-specific registry.
* `browser` owns `PageInspection`, `InspectableElement`, semantic element state, and locator-candidate contracts independent of CDP.
* `browser-cdp` implements semantic inspection using the same accessibility/DOM/actionability primitives as normal locator execution.
* `runtime` owns browser inspection lifecycle, bounded secondary inspection on failure, cancellation, and conversion into revision-aware observations.
* `reporter` owns human/JSON/JSONL projections of shared DTOs but does not derive semantic candidates itself.
* `app` composes `inspect` and `describe` CLI commands using shared services.
* `editor`, `lsp`, `dap`, and `wasm` transport shared DTOs only.

The browser abstraction evolves conceptually to include:

```text
BrowserHost
  -> BrowserSession
      -> BrowserContext
          -> Page
              -> inspect() -> PageInspection
```

`browser-cdp` may use CDP-specific data internally, but CDP identifiers and raw protocol shapes terminate at that adapter boundary.

`webtest inspect` is an orchestration command, not a hidden `.webtest` program. It does not introduce an `Inspect` `TestPlan` node merely to reuse execution machinery.

## 16. Agent-readiness acceptance fixture

Add a small deterministic example application, such as:

```text
examples/semantic-discovery/
```

The fixture contains at least:

```text
/login
/dashboard
```

with:

* an email field;
* a password field;
* a sign-in button;
* valid and invalid login behavior;
* one navigation;
* one visible post-login assertion target.

The normative acceptance harness does not call an LLM.

It proves mechanically that:

1. `webtest inspect /login --reporter json` exposes enough semantic information to identify valid unique locators for the login controls;
2. those emitted locator expressions parse, analyze, and execute using the normal WebTest pipeline;
3. a deliberately incorrect button name produces `locator_not_found`;
4. the structured failure identifies the actual semantic button as a bounded repair candidate;
5. applying that candidate to the test causes the corrected flow to pass.

A documented external-agent evaluation may additionally give a fresh agent only:

```text
the application URL
a behavioral testing requirement
the WebTest executable
permission to edit .webtest files
```

and allow it to use:

```text
webtest describe
webtest inspect
webtest check
webtest test
```

Success of a specific nondeterministic model is evaluation evidence, not a CI compatibility gate and not a dependency of WebTest.

The evaluation should prohibit direct Playwright/CDP/browser-MCP use so that it measures the sufficiency of WebTest's own authoring surface.

## 17. Delivery slices

1. Define versioned `PageInspection`, `InspectableElement`, locator-candidate, diagnostic-detail, and repair-hint DTOs with golden serialization tests.
2. Add protocol-neutral browser inspection traits and fake-browser fixtures.
3. Implement semantic element inspection in `browser-cdp` using existing locator/actionability primitives.
4. Implement deterministic preferred/alternate locator generation and verify every emitted locator by normal resolution.
5. Add `webtest inspect` with human and JSON reporters, isolated lifecycle, limits, and redaction.
6. Add `webtest describe` from shared language/provider metadata without introducing a duplicate semantic registry.
7. Normalize machine-readable static diagnostic details and stable diagnostic codes.
8. Add runtime locator/actionability/option repair hints and bounded secondary semantic inspection on failure.
9. Surface shared details through JSON/JSONL reporters, editor services, LSP, DAP, and WASM where their capabilities permit.
10. Add the semantic-discovery acceptance fixture, deterministic client harness, documentation, and optional external-agent evaluation instructions.

Every slice uses shared semantic DTOs. No slice may implement an agent-specific parser, browser driver, or test evaluator.

## 18. Testing requirements

Required coverage includes:

* deterministic serialization and compatibility golden tests for every new machine schema;
* fake-browser inspection tests for element roles, names, states, supported actions, ordering, truncation, and absence;
* real-Chrome inspection tests for accessibility roles/names, labels, placeholders, test IDs, IDs, exact text, shadow/iframe limitations, hidden elements, disabled elements, checked controls, and Unicode;
* tests proving every emitted locator candidate parses and resolves through the ordinary locator implementation;
* tests proving preferred locators are unique in the observed snapshot;
* tests proving CSS and XPath are never synthesized by inspection;
* deterministic ordering tests independent of map/set iteration;
* `locator_not_found` candidate tests for same-role nearby names;
* `locator_ambiguous` tests that prefer stronger unique semantic candidates when available;
* actionability-failure tests preserving obstruction/state facts without falsely claiming a locator replacement fixes the failure;
* `option_not_found` tests with bounded available-option candidates;
* static typo tests for bindings, members, providers, operations, and named arguments;
* redaction tests for passwords, configured query parameters, provider secrets, and nested diagnostic details;
* output-limit tests proving truncation is explicit;
* inspection lifecycle tests proving contexts, pages, and Chrome processes are cleaned after success, cancellation, navigation failure, and reporter failure;
* source-revision and byte-range tests for machine diagnostics;
* native/WASM parity tests for static description and diagnostic DTOs;
* LSP UTF-16 conversion tests proving the portable diagnostic range remains canonical;
* DAP tests proving semantic failure details are visible without exposing secrets;
* end-to-end acceptance tests against `examples/semantic-discovery`;
* full workspace, browser, protocol, extension, and WASM quality gates.

Real-browser fixtures use random loopback ports and the pinned managed Chrome version.

## 19. Acceptance criteria

Milestone C.5 is complete only when:

1. `webtest inspect /login --reporter json` returns a bounded semantic snapshot containing valid, unique WebTest locator expressions for the reference login form without requiring raw DOM, CSS, XPath, screenshots, or application source inspection.
2. Every emitted preferred locator is validated through the same locator resolver used by ordinary tests and produces deterministic canonical source for an equivalent page state.
3. `webtest describe --reporter json` exposes the project-visible WebTest locator, operation, type, capability, and provider surface from shared semantic metadata rather than a CLI-only registry.
4. `check` and `test` expose stable source revision/range, diagnostic codes, semantic details, and repair hints in versioned machine-readable form without requiring clients to parse human messages.
5. A deliberately incorrect semantic locator produces a structured runtime failure containing a valid nearby locator candidate when one can be determined, while WebTest never silently changes or heals the authored test.
6. Ambiguous, disabled, obscured, missing-option, type, member, and provider failures preserve distinct structured semantics rather than collapsing into generic repair text.
7. Inspection and repair output obey deterministic bounds and redaction rules and never expose password values, cookies, browser storage, configured secrets, or raw privileged browser identifiers.
8. The deterministic semantic-discovery fixture proves that a client using only `describe`, `inspect`, `check`, `test`, and ordinary file editing has enough information to author and repair the reference login test.
9. No LLM SDK, agent protocol, second parser, second locator implementation, CDP-specific public DTO, new DSL syntax, or automatic source mutation is introduced.
10. Existing Milestone B and C behavior, plan serialization, editor/DAP behavior, and the future Milestone D provider architecture remain compatible, and the complete workspace/browser/editor/WASM quality gates pass.

The inserted roadmap acceptance statement is thereby satisfied: an external tool can discover a running page, understand the WebTest surface available to it, author and statically validate a semantic WebTest, execute it, and repair a representative failure using only WebTest's versioned machine-readable interfaces.
