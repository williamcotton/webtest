# Milestone C.5 — Semantic Inspection and Machine Feedback

## 0. Status and dependencies

This specification inserts Milestone C.5 between [`milestone-c.md`](./milestone-c.md) and [`milestone-d.md`](./milestone-d.md). It depends on the semantic locator, browser evidence, and source-mapped runtime foundations from [`milestone-b.md`](./milestone-b.md), plus the typed values, provider schemas, capability analysis, structured observations, and versioned machine output established by [`milestone-c.md`](./milestone-c.md).

Milestone C.5 makes WebTest's existing semantics directly discoverable and consumable by external tools. A human, editor, script, or LLM agent should be able to inspect an unfamiliar page, discover valid WebTest vocabulary and semantic locators, author a test, statically check it, execute it, and diagnose or repair a failure without bypassing WebTest through raw CDP, Playwright, DOM scraping, or a second language implementation.

It does not depend on the application bridge in Milestone D. When Milestone D later adds statically known `app.*` functions, those functions participate in the same discovery interfaces introduced here.

**Implementation status (2026-08-26): implemented for the installed language, built-in providers, inspection, machine feedback, and project bootstrapping.** The shared Rust implementation exposes versioned description/search, semantic page inspection, deterministic validated locator candidates, structured static/runtime details and repair hints, bounded/redacted CLI and event output, editor/LSP/DAP transport, WASM static parity, and the deterministic `examples/semantic-discovery` acceptance fixture. `webtest init` creates a non-overwriting, statically checkable Protocol 1 starter project and installs the canonical agent skill plus its Claude compatibility link. The skill routes application-bridge work into the installed `app`, `app.schema`, `app.protocol`, `app.pseudocode`, and project-operation descriptions; those topics expose transport selection, runner-owned command semantics, the complete schema handshake, and the starter dispatch shape without duplicating the protocol in the skill. `inspect` remains an explicitly unsupported native capability in WASM. Protocol 1 project-provider schemas do not currently carry source examples, so project operation leaves explicitly omit examples instead of fabricating application-specific values; validated schema-supplied examples remain future work.

[`future-functionality.md`](./future-functionality.md) places this milestone after C and before D.

## 1. Outcome

An external client with no private knowledge of WebTest internals can discover the semantic surface of a running page and use ordinary WebTest commands as a closed authoring and repair loop:

```sh
webtest init .
webtest describe --reporter json
webtest describe locator.role --reporter json
webtest inspect /login --reporter json
webtest check tests/login.webtest --reporter json
webtest test tests/login.webtest --reporter json
```

The first description call returns a compact index of the installed language and project-visible providers. The second returns a self-contained reference for `role(...)`, including canonical syntax, typed parameters, result type, context and capability restrictions, semantic constraints, and canonical source examples. A client is not expected to know WebTest syntax before entering this loop.

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
* a static, queryable `webtest describe` language-reference API for discovering author-facing grammar, construct syntax and semantics, and the project/provider capability surface;
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

## 7. Static language and project description

`describe` is a machine-readable language reference and project-schema query API, not merely a feature inventory. Returning that `race`, `click`, or `role` exists is insufficient: an unfamiliar client must be able to learn how to write the construct, where it is legal, what it returns, and which semantic rules govern it.

`describe` is static. It does not launch Chrome or execute tests, and it describes the exact installed language and resolved project inputs rather than a roadmap milestone or remotely hosted manual.

### 7.1 Query model

Add:

```sh
webtest describe [QUERY] [--project PATH] --reporter human|json
webtest describe --search <TERMS> [--project PATH] --reporter human|json
```

Project discovery otherwise starts at the current directory using the ordinary nearest-root rules. Core `language`, `grammar`, locator, operation, assertion, type, and capability queries succeed even when no project root exists. In that case project-derived sections are absent and any project-discovery/configuration problem is returned separately; the reference for the installed language is not held hostage by `webtest.toml`. `QUERY` is a stable semantic identifier or category prefix. The supported hierarchy begins with:

```text
language
grammar
declaration.<name>
scope.<name>
statement.<name>
locator.<name>
browser.<operation>
assertion.<name>
type.<name>
capability.<name>
provider.<name>
provider.<name>.<operation>
```

Later milestones extend the same hierarchy with identifiers such as `control.race`, `pattern`, `pattern.subset`, `event.websocket`, and `event.websocket.received_json` only when those constructs are implemented. A unique unqualified name such as `role` or `click` may be accepted as a convenience alias. Ambiguous names produce a structured diagnostic containing the canonical candidate identifiers; they never resolve according to hidden precedence.

Exact hierarchical lookup alone has a bootstrap problem: an unfamiliar client may know that it needs to wait for a WebSocket message without knowing the words `select` or `received_json`. `--search` therefore performs a bounded deterministic lexical search over identifiers, names, summaries, declared use-case terms, parameter/result types, capabilities, and contexts. For example:

```sh
webtest describe --search "wait websocket message" --reporter json
```

Search results contain canonical query identifiers, compact syntax, one-line summaries, provenance, and the fields that matched. Search is local and schema-backed; it does not call a model, use an opaque embedding service, or invent constructs. Ranking and token normalization are specified and compatibility-tested. Exact identifier/name matches outrank authored keywords, results are bounded per provenance class, and project-supplied keyword spam cannot displace exact installed-language results. A result is only a discovery candidate, so a client drills into its canonical identifier before authoring source.

With no query, `describe` returns a compact index suitable for deciding what to query next:

```json
{
  "kind": "description_index",
  "description_schema_version": 1,
  "language_version": "<installed-version>",
  "project": {
    "root": "<normalized-project-root>",
    "configuration_revision": "<revision>"
  },
  "search_supported": true,
  "categories": {
    "grammar": ["language", "grammar"],
    "declarations": ["declaration.test"],
    "scopes": ["scope.server", "scope.browser"],
    "statements": ["statement.let"],
    "browser_operations": [
      "browser.open",
      "browser.evaluate",
      "browser.click",
      "browser.fill",
      "browser.type",
      "browser.press",
      "browser.check",
      "browser.uncheck",
      "browser.select",
      "browser.hover",
      "browser.wait.locator",
      "browser.wait.url"
    ],
    "assertions": [
      "assertion.locator_state",
      "assertion.url",
      "assertion.value"
    ],
    "locators": [
      "locator.id",
      "locator.role",
      "locator.label",
      "locator.text",
      "locator.placeholder",
      "locator.test_id",
      "locator.css",
      "locator.xpath"
    ],
    "types": [
      "type.Null",
      "type.Bool",
      "type.Int",
      "type.Float",
      "type.String",
      "type.Duration",
      "type.Url",
      "type.Json",
      "type.List",
      "type.Option",
      "type.Record",
      "type.StatusCode",
      "type.Headers",
      "type.Bytes",
      "type.Response",
      "type.ProcessResult",
      "type.FilePath",
      "type.TempDirectory",
      "type.Locator",
      "type.BrowserPage"
    ],
    "capabilities": [
      "capability.Pure",
      "capability.Server",
      "capability.Browser",
      "capability.Test"
    ],
    "providers": ["provider.fs", "provider.http", "provider.process"]
  }
}
```

The concrete index reflects the resolved project and installed binary. It is deterministically ordered, bounded, and contains canonical query identifiers rather than complete details. Querying a category such as `provider.http` or a future `event.websocket` returns a bounded category description plus the identifiers of its children. A category may also expose composition syntax shared by those children—for example, a future `pattern` response includes declaration syntax, the `matches` expression form, and matcher children rather than returning matcher names alone. Querying a leaf returns a self-contained construct description; a client need not retain the top-level response to interpret it.

### 7.2 Stable author-facing grammar

`webtest describe language --reporter json` returns the general composition rules needed to assemble a source file. This is a stable author-facing grammar, not a serialization of parser functions, Rowan kinds, recovery states, or every precedence production.

For the C.5 language it includes at least:

```json
{
  "kind": "language_description",
  "description_schema_version": 1,
  "language_version": "<installed-version>",
  "grammar": {
    "source_file": "<test_declaration>*",
    "test_declaration": "test <StringLiteral> <flow_block>",
    "flow_block": "{ <flow_statement>* }",
    "flow_statement": "<let_binding> | <server_block> | <browser_block> | <value_assertion> | <expression_statement>",
    "let_binding": "let <Identifier> [: <Type>] = <expression>",
    "server_block": "server { <server_statement>* }",
    "server_statement": "<let_binding> | <value_assertion> | <expression_statement>",
    "browser_block": "browser { <browser_statement>* }",
    "browser_statement": "<let_binding> | <browser_operation> | <browser_assertion> | <value_assertion> | <expression_statement>",
    "value_assertion": "expect <expression>",
    "provider_call": "<provider>.<operation>(<argument_list>?)",
    "argument_list": "<expression> (, <expression>)* (, <name>: <expression>)*",
    "locator_expression": "<locator>(<argument_list>?)"
  },
  "lexical_forms": {
    "Identifier": "ASCII letter or underscore followed by ASCII letters, digits, underscores, or hyphens",
    "StringLiteral": "double-quoted text using the installed language's documented escape table",
    "Duration": "positive integer followed by ms, s, or m",
    "LineComment": "// followed by text through the end of the line"
  },
  "composition": [
    "top-level declarations are tests",
    "server and browser are capability scopes inside a test flow",
    "a binding is visible only after its declaration in the enclosing sequential flow",
    "a transferable server value may be referenced by a later browser block"
  ],
  "examples": [
    {
      "name": "minimal browser test",
      "source": "test \"home is visible\" {\n    browser {\n        open \"/\"\n        expect text(\"Home\").visible\n    }\n}",
      "source_kind": "source_file",
      "prerequisites": ["configured browser base URL for a relative URL"]
    },
    {
      "name": "server value used by the browser",
      "source": "test \"created user signs in\" {\n    server {\n        let response = http.post(\"/api/test/users\", json: { email: \"alice@example.com\" })\n        let user: { id: Int, email: String } = response.json\n    }\n    browser {\n        fill label(\"Email\") with user.email\n    }\n}",
      "source_kind": "source_file",
      "prerequisites": ["configured HTTP base URL for a relative URL"]
    }
  ]
}
```

Grammar rule identifiers are stable within a description schema version and can be referenced by construct descriptions. The installed escape table, operator precedence/associativity, literal forms, reserved words, type forms, and comment forms are required structured portions of the language response; they are omitted from the illustrative object above only for brevity. The full lossless/error-recovery grammar remains an implementation detail.

`webtest describe grammar` may return only the `grammar`, precedence, literal, and type-form portions of the same language description when a smaller response is desired.

### 7.3 Construct description schema

Every public construct has one canonical description. Conceptually:

```text
ConstructDescription {
    description_schema_version,
    language_version,
    id,
    name,
    kind,
    syntax,
    syntax_forms,
    summary,
    search_terms,
    parameters,
    returns,
    produces_value,
    result_rule,
    requires_capabilities,
    allowed_contexts,
    effects,
    failure_modes,
    constraints,
    guidance,
    examples,
    related,
    availability,
    provenance,
}

ParameterDescription {
    name,
    type,
    required,
    position,
    named,
    default,
    secret,
    syntax_role,
    grammar_rule,
}

SyntaxForm {
    id,
    elements: [Literal | Slot | RuleReference | Optional | Repeat | Choice],
}

ConstraintDescription {
    code,
    phase,
    subject,
    summary,
    details,
}

SourceExample {
    name,
    source,
    source_kind,
    enclosing_context,
    prerequisites,
}
```

Fields that do not apply are absent rather than populated with vague strings. Types use the same structured `TypeSchema` representation as analysis and provider calls; the compact strings below are illustrative JSON projections. `syntax_role` distinguishes call arguments from statement operands, bodies, clauses, and bound variables. `grammar_rule` distinguishes, for example, a string-valued expression from the string-literal-only slot accepted by the current locator grammar. `syntax` is the compact author/LLM rendering, while `syntax_forms` is the normative machine composition model linking literal tokens and grammar references to named parameter slots. Constructs with materially different forms expose multiple named forms instead of compressing alternatives into an ambiguous display string.

An expression or call has `returns`; an effect-only statement sets `produces_value` to false instead of inventing a `Unit` type that the language does not define. A conditional or branch-dependent value uses `result_rule` instead of inventing one fixed return type. Constraints and guidance have stable codes plus bounded explanations, allowing tools to branch on the code without scraping English text. `source_kind` distinguishes a complete source file from a declaration, block, statement, expression, locator, matcher, or other grammar fragment. `enclosing_context` identifies the exact grammar slot used to validate a fragment, not merely a broad capability scope; for example, a locator-only snippet can be validated as `browser.click.target` even when locators are not general standalone expressions.

Every leaf description includes at least:

* canonical display syntax and structured syntax forms with named slots;
* a typed schema for each argument, operand, clause, or body;
* a return type, an explicit no-value statement result, or a result-unification rule;
* required capability and legal source contexts;
* semantic constraints, effects, and failure behavior relevant to correct use;
* canonical source examples.

Every installed public leaf description provides at least two canonical examples: a minimal fragment and a composed use in its legal context. Additional examples are required when they communicate optional forms, temporal ordering, ownership, result unification, or other behavior not apparent from the syntax form. The current Protocol 1 manifest and shared `OperationSchema` do not define source examples, so configured project-provider leaves return no examples and a coded guidance entry explains why. A future external example field must pass WebTest's parser and static validation before it can appear here; the description service never fabricates plausible application values or presents an invalid example as canonical.

For example, `webtest describe locator.role --reporter json` returns a leaf shaped like:

```json
{
  "kind": "construct_description",
  "description_schema_version": 1,
  "language_version": "<installed-version>",
  "id": "locator.role",
  "name": "role",
  "construct_kind": "locator",
  "syntax": "role(<role>, name: <String>?)",
  "syntax_forms": [
    {
      "id": "default",
      "elements": [
        { "kind": "literal", "value": "role(" },
        { "kind": "slot", "parameter": "role" },
        {
          "kind": "optional",
          "elements": [
            { "kind": "literal", "value": ", name: " },
            { "kind": "slot", "parameter": "name" }
          ]
        },
        { "kind": "literal", "value": ")" }
      ]
    }
  ],
  "summary": "Locate an element by accessibility role and optional accessible name.",
  "search_terms": ["accessibility", "accessible name", "button", "control"],
  "parameters": [
    {
      "name": "role",
      "type": "String",
      "required": true,
      "position": 0,
      "named": false,
      "syntax_role": "argument",
      "grammar_rule": "StringLiteral"
    },
    {
      "name": "name",
      "type": "String",
      "required": false,
      "named": true,
      "syntax_role": "argument",
      "grammar_rule": "StringLiteral"
    }
  ],
  "returns": "Locator",
  "requires_capabilities": ["Browser"],
  "allowed_contexts": ["scope.browser"],
  "failure_modes": [
    "locator_not_found",
    "locator_ambiguous"
  ],
  "constraints": [
    {
      "code": "exact_accessible_match",
      "phase": "runtime",
      "subject": "role,name",
      "summary": "Role and name use exact case-sensitive matching after documented whitespace normalization."
    },
    {
      "code": "singular_consumer_requires_unique_match",
      "phase": "runtime",
      "subject": "locator result",
      "summary": "An operation that requires a present target rejects multiple matches; hidden and detached state checks may succeed with no match."
    }
  ],
  "examples": [
    {
      "name": "role only",
      "source": "role(\"button\")",
      "source_kind": "locator_fragment",
      "enclosing_context": "browser.click.target"
    },
    {
      "name": "role and accessible name",
      "source": "click role(\"button\", name: \"Sign in\")",
      "source_kind": "statement_fragment",
      "enclosing_context": "scope.browser"
    }
  ],
  "related": ["browser.click", "assertion.locator_state"],
  "availability": { "analysis": true, "runtime_requires": ["native_browser"] },
  "provenance": { "kind": "core", "content_trust": "installed" }
}
```

Statement-like constructs use the same schema. `webtest describe browser.click --reporter json` includes its target operand and the context restriction that makes a server-side click invalid:

```json
{
  "kind": "construct_description",
  "description_schema_version": 1,
  "language_version": "<installed-version>",
  "id": "browser.click",
  "name": "click",
  "construct_kind": "browser_operation",
  "syntax": "click <target>",
  "syntax_forms": [
    {
      "id": "default",
      "elements": [
        { "kind": "literal", "value": "click " },
        { "kind": "slot", "parameter": "target" }
      ]
    }
  ],
  "summary": "Wait for one visible, enabled, stable, unobscured element and activate it with pointer input.",
  "search_terms": ["activate", "button", "pointer", "press control"],
  "parameters": [
    {
      "name": "target",
      "type": "Locator",
      "required": true,
      "position": 0,
      "named": false,
      "syntax_role": "operand",
      "grammar_rule": "locator_expression"
    }
  ],
  "produces_value": false,
  "requires_capabilities": ["Browser"],
  "allowed_contexts": ["scope.browser"],
  "effects": ["browser_pointer_input", "page_may_navigate"],
  "failure_modes": [
    "locator_not_found",
    "locator_ambiguous",
    "locator_invalid",
    "element_detached",
    "element_not_visible",
    "element_unstable",
    "element_disabled",
    "element_obscured",
    "action_timeout",
    "browser_disconnected"
  ],
  "constraints": [
    {
      "code": "unique_target_before_deadline",
      "phase": "runtime",
      "subject": "target",
      "summary": "The locator must resolve to exactly one visible, enabled, stable target before the operation deadline."
    }
  ],
  "examples": [
    {
      "name": "click a named button",
      "source": "click role(\"button\", name: \"Save\")",
      "source_kind": "statement_fragment",
      "enclosing_context": "scope.browser"
    },
    {
      "name": "click an exact text target",
      "source": "click text(\"Continue\")",
      "source_kind": "statement_fragment",
      "enclosing_context": "scope.browser"
    }
  ],
  "related": ["locator.role", "locator.text"],
  "availability": { "analysis": true, "runtime_requires": ["native_browser"] },
  "provenance": { "kind": "core", "content_trust": "installed" }
}
```

`allowed_contexts` and `requires_capabilities` are compiler facts, not documentation-only advice. They are sufficient to determine statically that this is illegal:

```webtest
server {
    click role("button", name: "Save")
}
```

`guidance` is reserved for bounded authoring advice that explains when a valid construct should be used. Later actor/event constructs can therefore state temporal or ownership patterns such as taking a checkpoint before an action that may immediately emit the event being observed. Guidance does not add semantics that are absent from HIR, analysis, or the relevant runtime contract.

### 7.4 Provider and project-defined operations

Provider operation details are emitted from the same `ProviderSchema`, `OperationSchema`, `ParameterSchema`, and `TypeSchema` values used by static analysis. A provider leaf looks like the same function-call schema used for core callable constructs:

```json
{
  "kind": "construct_description",
  "description_schema_version": 1,
  "language_version": "<installed-version>",
  "id": "provider.http.get",
  "name": "http.get",
  "construct_kind": "provider_operation",
  "syntax": "http.get(<url>, query: <Record>?, headers: <Record>?, timeout: <Duration>?)",
  "syntax_forms": [
    {
      "id": "call",
      "elements": [
        { "kind": "literal", "value": "http.get(" },
        { "kind": "slot", "parameter": "url" },
        {
          "kind": "repeat",
          "separator": ", ",
          "elements": [{ "kind": "slot", "parameter_group": "named_arguments" }]
        },
        { "kind": "literal", "value": ")" }
      ]
    }
  ],
  "summary": "Send an HTTP GET request.",
  "search_terms": ["request", "fetch", "REST", "JSON", "server setup"],
  "parameters": [
    {
      "name": "url",
      "type": "String",
      "required": true,
      "position": 0,
      "named": false,
      "syntax_role": "argument",
      "grammar_rule": "expression"
    },
    {
      "name": "query",
      "type": "Record",
      "required": false,
      "named": true,
      "syntax_role": "argument",
      "grammar_rule": "expression"
    },
    {
      "name": "headers",
      "type": "Record",
      "required": false,
      "named": true,
      "syntax_role": "argument",
      "grammar_rule": "expression"
    },
    {
      "name": "timeout",
      "type": "Duration",
      "required": false,
      "named": true,
      "syntax_role": "argument",
      "grammar_rule": "expression"
    }
  ],
  "returns": "Response<Json>",
  "requires_capabilities": ["Server"],
  "allowed_contexts": ["scope.server"],
  "effects": ["network_request"],
  "failure_modes": [
    "http_transport",
    "response_too_large",
    "provider_invalid_argument",
    "provider_unavailable"
  ],
  "constraints": [
    {
      "code": "relative_url_requires_base_url",
      "phase": "configuration",
      "subject": "url",
      "summary": "A relative URL requires the applicable configured base URL."
    },
    {
      "code": "http_status_is_data",
      "phase": "runtime",
      "subject": "result.status",
      "summary": "An HTTP 4xx or 5xx status is returned as data until an assertion rejects it."
    }
  ],
  "examples": [
    {
      "name": "GET a resource",
      "source": "let response = http.get(\"/api/users\")",
      "source_kind": "statement_fragment",
      "enclosing_context": "scope.server",
      "prerequisites": ["configured HTTP base URL for a relative URL"]
    },
    {
      "name": "GET with query and timeout",
      "source": "let response = http.get(\"/api/users\", query: { active: true }, timeout: 5s)",
      "source_kind": "statement_fragment",
      "enclosing_context": "scope.server",
      "prerequisites": ["configured HTTP base URL for a relative URL"]
    }
  ],
  "availability": { "analysis": true, "runtime_requires": ["native_http"] },
  "provenance": { "kind": "built_in_provider", "content_trust": "installed" }
}
```

The concrete operation description includes every registered parameter, including secret/redaction metadata and supported bodies such as `json`, `text`, `bytes`, or `form`; the shortened example above demonstrates the shape rather than replacing the provider schema.

`provider.app` and leaves such as `provider.app.create_user` participate through Milestone D's implemented offline manifest. Their offline schema supplies documentation, parameters, result types, capability, redaction, defaults, and retry-safety metadata, and canonical syntax is projected from that schema. The current manifest has no source-example field, so these project leaves omit examples rather than inventing values. The application does not need to be running for static discovery.

Every description field carries provenance sufficient to distinguish installed core metadata, built-in provider metadata, project configuration, and externally supplied application/provider schemas. Project-supplied summaries and parameter documentation are untrusted data: they are length-bounded, stripped of disallowed control characters, emitted as plain strings rather than executable markup, and marked `content_trust: "project_supplied"`. WebTest does not present them as compiler rules or agent instructions. Syntax, parameters, types, capability, redaction, and availability continue to come from validated schema fields rather than prose. If a future provider schema supplies examples, each example must be parsed and statically validated against its declared schema/context before appearing in canonical `examples`; rejected content must produce schema/configuration diagnostics rather than being quietly presented as valid source.

Availability is separate from existence. A construct can be statically known while requiring native execution, configuration, a provider connection, or a host capability that is unavailable in the current environment. Structured `availability`, `runtime_requires`, configuration prerequisites, schema identity, and provider provenance prevent a client from treating “described” as “ready to execute.” `describe` remains static and may report requirements; it does not probe the network or start the provider merely to claim live availability.

### 7.5 One reference, shared by every adapter

`describe` must not maintain a CLI-owned grammar table or a handwritten duplicate provider registry. Protocol-neutral description services compose:

* stable author-facing grammar metadata owned beside the one parser and typed AST;
* construct identities and syntax-independent semantics used by HIR and analysis;
* capability/context rules used by analysis;
* locator, browser-operation, assertion, and state registries used by plan lowering and runtime;
* configured provider schemas and project inputs.

Some summaries, syntax templates, guidance, and examples are necessarily authored documentation. They live in one shared Rust reference model, are versioned with the language, and are consumed by CLI, editor, DAP, and WASM adapters. They never become adapter-local semantic tables.

Every canonical example is tested through the ordinary lexer, lossless parser, typed AST, HIR, and analysis path in an appropriate enclosing source context. Every advertised context/capability restriction has a matching analysis test. Index entries, category children, construct identities, plan-lowerable operations, provider schemas, and leaf descriptions are checked for bidirectional completeness so an implemented public construct cannot silently disappear from discovery and an unavailable roadmap construct cannot be advertised.

### 7.6 Versioning, bounds, and query failures

Every response includes `description_schema_version` and `language_version`. Project-sensitive responses also identify the configuration/schema revision from which they were derived. Semantic schema changes follow the machine-output compatibility policy; documentation-only wording may change without pretending that the installed language changed, while syntax, parameter, result, constraint, capability, or context changes participate in language/reference compatibility.

Responses are deterministically ordered and bounded. Top-level and category responses provide child query identifiers instead of recursively embedding the entire manual. Leaf descriptions are self-contained. Truncation is explicit, including which collection was truncated and which narrower query retrieves the omitted details.

Unknown and ambiguous queries return the shared machine-diagnostic envelope with stable `description_unknown_query` and `description_ambiguous_query` codes, the requested query, and bounded exact/prefix/edit-distance candidates. These command diagnostics have no invented source range. Human output renders the same reference data concisely; clients using JSON never parse terminal prose.

### 7.7 Limits of description

`describe` explains what the installed language can express; it does not prove that a construct is appropriate for a particular behavioral requirement, that a copied example is safe to execute in the caller's environment, or that an inspected page will remain unchanged. Search relevance is advisory, canonical examples demonstrate language use rather than project intent, and static availability requirements are not live health checks.

The closed loop remains essential: `describe` supports a good first attempt, `check` validates the actual source and returns structured diagnostics/reference queries, and `test` validates runtime behavior. A valid program can still assert the wrong business behavior. C.5 does not claim that reference completeness makes autonomous synthesis infallible.

## 8. Machine-readable diagnostics

### 8.1 Shared diagnostic representation

Static analysis already produces semantic diagnostics. Milestone C.5 makes their machine contract explicit.

Conceptually:

```text
MachineDiagnostic {
    code,
    severity,
    message,
    source?,
    related,
    semantic_details,
    repair_hints,
    reference_queries,
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

When a diagnostic concerns a describable construct, type, provider operation, grammar rule, or legal-context rule, `reference_queries` contains bounded canonical `describe` identifiers such as `browser.click`, `locator.role`, or `provider.http.get`. A client can therefore move from a failed first attempt directly to the authoritative installed reference instead of guessing which help topic applies. References are advisory links, not source edits, and use the same identities returned by the description index.

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
* description summaries, guidance, defaults, and examples pass through the same schema-aware redaction before emission; a redacted example is revalidated or omitted rather than labeled canonical when redaction makes it invalid;
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

Description limits are separate from inspection limits so a configured provider cannot turn a leaf query or search into an accidental full manual or prompt-sized payload:

```toml
[description]
max_category_children = 200
max_search_results = 20
max_summary_bytes = 1024
max_guidance_entries = 16
max_examples = 4
max_example_bytes = 4096
```

The implementation enforces documented hard ceilings above project configuration. The minimum permitted `max_examples` still allows the two required canonical examples for an installed public leaf. Search applies bounds per provenance class before the aggregate limit so project-supplied entries cannot crowd out installed-language matches.

If a limit is reached, output contains structured truncation metadata. It must not silently appear complete.

## 14. Editor, DAP, and WASM behavior

The protocol-neutral editor layer may expose the same structured diagnostic details and repair hints to adapters.

LSP may render repair hints as related diagnostic information or narrowly safe quick-fix candidates when the editor protocol supports them. The Rust semantic result remains authoritative. This milestone does not require the general completion/navigation feature set reserved for Milestone F.

DAP continues to use the ordinary runtime. Failure stops may display structured semantic details and candidate locators without independently querying the DOM in the VS Code extension.

WASM can:

* produce static machine diagnostics;
* expose the same index, category, leaf, and deterministic search description DTOs available from its supplied project inputs;
* serialize repair hints for static errors.

WASM cannot execute `webtest inspect` because it has no native Chrome capability. It returns the same explicit unsupported/native-capability result used for other host-only behavior.

No TypeScript extension code implements inspection, locator ranking, or repair semantics.

## 15. Architecture and crate responsibilities

* `syntax` gains no new `.webtest` syntax, but exposes the canonical author-facing grammar, lexical forms, syntax forms, and validated source examples needed by the shared reference model.
* `hir` is unchanged except where existing construct identity and semantic metadata must become externally representable.
* `analysis` composes protocol-neutral description queries from syntax/HIR registries, capability/context rules, provider schemas, and explicit project inputs; it also exposes structured diagnostic details, reference queries, and deterministic correction candidates.
* `provider` exposes project-visible schemas, provenance, availability requirements, and bounded untrusted documentation through the shared description DTO rather than a second CLI-specific registry.
* `browser` owns `PageInspection`, `InspectableElement`, semantic element state, and locator-candidate contracts independent of CDP.
* `browser-cdp` implements semantic inspection using the same accessibility/DOM/actionability primitives as normal locator execution.
* `runtime` owns browser inspection lifecycle, bounded secondary inspection on failure, cancellation, and conversion into revision-aware observations.
* `reporter` owns human/JSON/JSONL projections of shared DTOs but does not derive semantic candidates itself.
* `app` composes `inspect` and hierarchical/queryable `describe` CLI commands using shared services.
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

1. `webtest describe --reporter json` exposes the grammar, browser-operation, locator, and provider query identifiers needed for the task without assuming prior WebTest syntax knowledge;
2. exact lookup and lexical search lead to self-contained `browser.open`, `browser.fill`, `browser.click`, `assertion.locator_state`, `locator.label`, and `locator.role` descriptions with structured syntax, context/capability facts, and canonical examples;
3. every returned example or syntax form used by the fixture parses and analyzes through its declared enclosing context, while an intentionally illegal server-side browser action produces a diagnostic linked back to its canonical reference query;
4. `webtest inspect /login --reporter json` exposes enough semantic information to identify valid unique locators for the login controls;
5. those emitted locator expressions parse, analyze, and execute using the normal WebTest pipeline;
6. a deliberately incorrect button name produces `locator_not_found`;
7. the structured failure identifies the actual semantic button as a bounded repair candidate;
8. applying that candidate to the test causes the corrected flow to pass.

A documented external-agent evaluation may additionally give a fresh agent only:

```text
the application URL
a behavioral testing requirement
the WebTest executable
permission to edit .webtest files
```

and allow it to use:

```text
webtest init
webtest describe
webtest inspect
webtest check
webtest test
```

Success of a specific nondeterministic model is evaluation evidence, not a CI compatibility gate and not a dependency of WebTest.

The evaluation should prohibit direct Playwright/CDP/browser-MCP use so that it measures the sufficiency of WebTest's own authoring surface.

## 17. Delivery slices

1. Define versioned language index/category/construct/search, syntax-form, provenance/availability, `PageInspection`, `InspectableElement`, locator-candidate, diagnostic-detail, and repair-hint DTOs with golden serialization tests.
2. Add protocol-neutral browser inspection traits and fake-browser fixtures.
3. Implement semantic element inspection in `browser-cdp` using existing locator/actionability primitives.
4. Implement deterministic preferred/alternate locator generation and verify every emitted locator by normal resolution.
5. Add `webtest inspect` with human and JSON reporters, isolated lifecycle, limits, and redaction.
6. Add hierarchical `webtest describe`, deterministic lexical search, author-facing grammar, typed construct descriptions, and validated examples from shared language/provider metadata without introducing a duplicate semantic registry.
7. Normalize machine-readable static diagnostic details, stable diagnostic codes, and canonical description-reference queries.
8. Add runtime locator/actionability/option repair hints and bounded secondary semantic inspection on failure.
9. Surface shared details through JSON/JSONL reporters, editor services, LSP, DAP, and WASM where their capabilities permit.
10. Add the semantic-discovery acceptance fixture, deterministic client harness, documentation, and optional external-agent evaluation instructions.

Every slice uses shared semantic DTOs. No slice may implement an agent-specific parser, browser driver, or test evaluator.

## 18. Testing requirements

Required coverage includes:

* deterministic serialization and compatibility golden tests for every new machine schema;
* top-level index, category-prefix, exact-leaf, unique-alias, ambiguous-alias, unknown-query, and lexical-search golden tests;
* completeness tests proving every public grammar/locator/browser/assertion/provider construct is indexed and described exactly once, and no unavailable roadmap construct is advertised;
* tests composing every structured syntax form and parsing/analyzing every canonical example in its declared enclosing context;
* tests proving described parameter types, result behavior, capabilities, contexts, constraints, availability, and provider schema identity agree with analysis and plan lowering;
* tests proving static diagnostics include valid bounded `reference_queries` where an authoritative description exists;
* bounds, sanitization, provenance, and instruction-like-content tests for untrusted project/provider documentation and examples;
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
3. `webtest describe --reporter json` returns a compact installed-language/project index, and exact, category, alias, and lexical-search queries return bounded self-contained grammar or construct references with structured syntax forms, typed parameters/results, contexts, capabilities, coded constraints/guidance, and provenance/availability from shared semantic metadata rather than a CLI-only registry. Installed leaves include validated canonical examples; current project-provider leaves explicitly omit them because their schema has no example field.
4. `check` and `test` expose stable source revision/range, diagnostic codes, semantic details, and repair hints in versioned machine-readable form without requiring clients to parse human messages.
5. A deliberately incorrect semantic locator produces a structured runtime failure containing a valid nearby locator candidate when one can be determined, while WebTest never silently changes or heals the authored test.
6. Ambiguous, disabled, obscured, missing-option, type, member, and provider failures preserve distinct structured semantics rather than collapsing into generic repair text.
7. Inspection and repair output obey deterministic bounds and redaction rules and never expose password values, cookies, browser storage, configured secrets, or raw privileged browser identifiers.
8. The deterministic semantic-discovery fixture proves that a client with no prior WebTest syntax knowledge, using only `describe`, `inspect`, `check`, `test`, and ordinary file editing, has enough information to discover the relevant vocabulary, author the reference login test, follow diagnostic reference queries, and repair it.
9. Project/provider-authored description content is explicitly untrusted, bounded, sanitized, provenance-marked, and unable to override compiler-derived syntax, types, contexts, capabilities, availability, or redaction semantics.
10. No LLM SDK, agent protocol, second parser, second locator implementation, CDP-specific public DTO, new DSL syntax, or automatic source mutation is introduced.
11. Existing Milestone B and C behavior, plan serialization, editor/DAP behavior, and the future Milestone D provider architecture remain compatible, and the complete workspace/browser/editor/WASM quality gates pass.

The inserted roadmap acceptance statement is thereby satisfied: an external tool can discover a running page, understand the WebTest surface available to it, author and statically validate a semantic WebTest, execute it, and repair a representative failure using only WebTest's versioned machine-readable interfaces.
