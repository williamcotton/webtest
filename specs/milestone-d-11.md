# Milestone D.11 — Semantic Ownership and Typed Failure Identity

## 0. Status and dependencies

**Status: proposed.**

This maintenance milestone follows the implemented application bridge and D-series hardening,
including [`milestone-d-9-1.md`](./milestone-d-9-1.md) and
[`milestone-d-10.md`](./milestone-d-10.md). It precedes the structured execution and observation
work in [`milestone-e.md`](./milestone-e.md).

D.11 adds no DSL syntax, provider operation, browser behavior, project setting, CLI command, editor
feature, execution scheduler, or remote worker. It tightens ownership in the current sequential
architecture before Milestone E adds plan-node identity, concurrent runtime occurrences, expanded
events, traces, and observation IPC.

The current dependency topology is already predominantly one-directional. The remaining problems
are narrower than a redesign:

- `webtest-hir` owns `TestId`, `StepId`, `BindingId`, `UnaryOperator`, and `BinaryOperator`, even
  though plans, runtime state, observations, DAP, and application security checks use them after the
  HIR tree is gone;
- `webtest-provider` owns `Type`, `RecordField`, `Value`, `ResponseValue`, `ProcessResultValue`, and
  `Capability`, even though those are the common language/execution value model used by analysis,
  plans, runtime, DAP, WASM, and provider schemas;
- `webtest-plan` consequently depends on both HIR and provider for types that are not specific to
  either subsystem, and also owns public HIR-to-plan locator conversion helpers;
- runtime failure identity is typed in `BrowserError`, `ProviderError`, and `StepError`, then reduced
  to strings in events and observations and interpreted again by `editor`, `app`, and DAP; and
- some adapter mappings collapse a known failure into a generic code, so two consumers can render
  different diagnostic identities for the same structured runtime error.

These are type-ownership and translation braids. D.11 removes them while preserving the existing
source-to-execution architecture.

## 1. Outcome

After D.11, the relevant topology is:

```text
source identity                  shared semantic model
  webtest-text                     webtest-model
  FileId                           TestId / StepId / BindingId
  SourceRevision                   UnaryOperator / BinaryOperator
  SyntaxOrigin                     Type / RecordField / Value
                                   Capability
         |                                  |
         v                                  v
      syntax -----------------------------> HIR
                                                 \
                                                  \ semantic compilation
                                                   v
                    provider schemas ---------> analysis
                                                   |
                                                   | explicit HIR -> plan lowering
                                                   v
                                                 plan
                                                   |
                                                   | explicit plan -> host conversion
                                                   v
                                                runtime
                                             /             \
                                      provider calls     browser contract
                                             \             /
                                              v           v
                                               observation
                                                   |
                                                   v
                                             editor / app / DAP
```

In dependency terms, where `A -> B` means “A depends on B”:

```text
hir         -> model, syntax, text
provider    -> model
plan        -> model, text
analysis    -> hir, model, plan, provider, syntax, text, feedback
observation -> model, browser, provider, feedback, text
runtime     -> model, plan, provider, browser, observation, feedback, text

plan        -X-> hir
plan        -X-> provider
runtime     -X-> hir
observation -X-> hir
```

There remains one canonical Rust type for each shared semantic concept. HIR, plan, provider, and
browser representations that have different contracts remain different types connected by
explicit, exhaustive conversions.

Runtime failures retain their structured payloads and three-way `FailureClass`. In addition, every
failure that reaches an event, observation, diagnostic, reporter, or debugger carries a typed
`RuntimeFailureCode`. Strings are produced only when a presentation or wire adapter requests the
existing short or namespaced spelling.

## 2. Type-topology baseline

The D.11 scope is based on ownership and change fan-out, not crate size. The relevant current
topology is:

| Concept | Current owner | Current non-owner consumers | D.11 decision |
|---|---|---|---|
| `FileId`, `SourceRevision`, `SyntaxOrigin` | `text` | syntax through adapters | keep in `text` |
| `TestId`, `StepId`, `BindingId` | `hir` | analysis, plan, runtime, observation, DAP, app | move canonical definitions to `model` |
| unary and binary operators | `hir` | analysis, plan, runtime, app | move canonical definitions to `model` |
| `Type`, `RecordField` | `provider` | analysis, plan, runtime, app bridge, WASM | move canonical definitions to `model` |
| `Value` and compound values | `provider` | plan, runtime, observation, DAP, app bridge, app | move canonical definitions to `model` |
| `Capability` | `provider` | analysis, plan, runtime, editor, DAP, WASM, app | move canonical definition to `model` |
| provider schemas/calls/registry/errors | `provider` | analysis, app bridge, runtime, adapters | keep in `provider` |
| HIR locators | `hir` | analysis and current plan conversion helper | keep in `hir` |
| plan locators and expressions | `plan` | analysis, runtime, DAP, WASM, app | keep in `plan` |
| browser locators/actions/errors | `browser` | CDP, runtime, observation, editor, DAP, app | keep in `browser` |
| runtime events/observations/failure DTOs | `observation` | runtime, editor, DAP, app | keep in `observation`; add typed failure code |

The move is justified only where the same type already has the same meaning and serialized shape
across independent layers. Similar-looking types with distinct contracts are not candidates.

## 3. Architectural rules

### 3.1 Preserve one-directional pipeline

Ownership continues to flow in one direction:

```text
source -> syntax -> HIR -> analysis -> plan -> runtime -> observation -> adapters
```

A downstream layer may consume or explicitly convert an upstream representation. It must not parse,
reconstruct, or reinterpret the upstream representation. D.11 does not create a shared “core” tree,
universal locator, universal diagnostic, or object that every layer mutates.

Moving neutral definitions to `model` must remove false dependencies; it is not permission to move
compiler, provider, runtime, editor, or transport behavior into a foundation crate.

### 3.2 Keep the model crate narrow

`webtest-model` owns only already-shared, protocol-neutral semantic values:

```text
identity
  TestId
  StepId
  BindingId

operators
  UnaryOperator
  BinaryOperator

types and values
  Type
  RecordField
  Value
  ResponseValue
  ProcessResultValue
  JSON/value conversion and inherent pure value semantics

execution domains
  Capability
```

The crate may depend on `serde` and `serde_json` plus the Rust standard library. It must not depend
on another WebTest crate, async runtime, filesystem implementation, socket/process implementation,
browser backend, editor protocol, or project configuration.

A type is not admitted merely because two crates use it. Admission requires all of the following:

1. it has one meaning across the consuming layers;
2. consumers require the same identity or serialized representation, not merely a conversion;
3. it has no syntax-tree, compiler-state, runtime-handle, backend, adapter, or project dependency;
4. putting it in `model` removes an otherwise false ownership edge; and
5. its semantics can be tested without host I/O.

`webtest-model` must not grow into `webtest-core`, `common`, or a dumping ground for anything widely
used.

### 3.3 Preserve representation boundaries

These remain deliberately distinct:

```text
ast::Locator -> HirLocator -> plan::Locator -> browser::Locator
HirExpr       -> PlanExpr                  -> Value
HirType       -> Type
BrowserError / ProviderError -> RuntimeFailure -> Diagnostic/report DTO
```

The arrows are explicit translations. They are not duplicate parsers or duplicate semantics.
Combining any row into one universal enum would couple authoring, serialized plan compatibility,
runtime execution, and backend evolution.

### 3.4 Keep structured errors authoritative

`BrowserError`, `ProviderError`, assertion/decode/evaluation failures, `RunError`, cleanup failures,
and `RuntimeFailure` keep their structured payloads. `RuntimeFailureCode` identifies a failure for
machine routing; it does not replace the structured error, its evidence, redacted message, dynamic
application error code, bridge subcode, source origin, or `FailureClass`.

No core or adapter logic may recover semantic identity by matching a returned code string. Code
strings are serialization and presentation values only.

### 3.5 Preserve classification and redaction

D.11 does not reclassify any existing failure. Malformed/invalid source remains static, ordinary
assertion/provider/browser mismatches remain test failures, host/transport failures remain
infrastructure failures, and violated internal invariants remain internal failures.

Redaction occurs before a structured failure, event, observation, debugger value, or report crosses
its existing boundary. A typed code must never contain user text, secret values, filesystem paths,
provider-returned application codes, or backend messages.

### 3.6 Preserve source and execution identity semantics

Moving an ID changes only its Rust owner. D.11 does not change allocation, numeric encoding,
serialization, equality, ordering, source revision behavior, or source ranges. In particular,
`TestId`, `StepId`, and `BindingId` do not become cross-revision or globally stable merely because
they move to `model`.

Milestone E's `PlanNodeId`, execution-scope IDs, attempt IDs, operation-occurrence IDs, and resource
generation IDs remain distinct future concepts. Milestone F remains responsible for
declaration/variant-ready discovery identity.

## 4. Scope

D.11 includes:

- adding `crates/model` / `webtest-model` to the workspace;
- moving the canonical shared IDs, operators, types, values, and capability enum into it without
  changing their public data or Serde representation;
- migrating every production consumer to a direct `webtest-model` dependency when it names those
  types as part of its own logic or API;
- removing production `webtest-hir` dependencies from `plan`, `runtime`, `observation`, and `app`
  where they exist only for the moved definitions;
- removing the production `webtest-provider` dependency from `plan`;
- moving HIR-to-plan locator/state translation into the analysis compiler so `plan` no longer
  depends on HIR;
- introducing a typed, exhaustive runtime failure-code vocabulary at the observation boundary;
- replacing string-valued runtime failure identity in runtime events and observations with the
  typed code;
- replacing string switches in editor, app, DAP, and browser retry bookkeeping with typed matches
  or producer-local typed variants;
- preserving or deliberately normalizing existing outward diagnostic codes as specified below;
- updating crate-level documentation, workspace dependency rules, architecture documentation, and
  direct tests; and
- proving native/WASM, plan, provider-schema, app-bridge, CLI, editor/LSP, and DAP compatibility.

## 5. `webtest-model` contract

### 5.1 Canonical ownership

After extraction, the canonical definitions live only in `webtest-model`. HIR and provider may
temporarily re-export the moved types from their existing public roots for pre-1.0 source
compatibility, but those re-exports must be aliases to the exact same Rust types, never copied
definitions or compatibility wrappers.

Repository production code must use `webtest_model::*` directly for model concepts. Compatibility
re-exports do not authorize a new crate to acquire an HIR or provider dependency merely to import a
model type. They also must not cause `plan`, `runtime`, or `observation` to retain a false dependency.

### 5.2 Type and value behavior

The extraction preserves all implemented semantic behavior, including:

- `Type::accepts`, optional-field presence, structural record compatibility, and numeric
  compatibility;
- `Type::member`, statically optional member behavior, and transferability;
- deterministic `BTreeMap` record/value order;
- `Value::member` behavior for records, responses, process results, and case-insensitive headers;
- JSON-to-value and transferable value-to-JSON conversion;
- value type names and display behavior;
- structured redaction of records, headers, response bodies/JSON, process output, and secrets;
- exact `Int`, `Float`, duration, bytes, path, response, and process-result representations; and
- all existing `Clone`, equality, hash, order, and Serde traits.

Pure inherent behavior may move with its type. Host behavior does not: HTTP, processes, filesystem
containment, temporary-directory ownership, provider dispatch, bridge transport, Chrome, runtime
evaluation, artifacts, and editor presentation remain outside `model`.

### 5.3 Serialization compatibility

Moving a Rust definition across crates does not justify changing its wire shape. The following stay
identical:

- Serde tags, variant names, field names, omission/default rules, and numeric representations;
- plan-format-3 JSON emitted by `webtest build --emit`;
- provider-schema JSON and its canonical schema hash;
- Protocol 1 application manifest and value projections;
- WASM portable compilation output; and
- DAP binding/value shapes.

No plan-format increment is required for a crate-path-only move. If implementation uncovers an
actual serialized-shape change, that change is outside D.11 and must receive its own reviewed
versioning decision rather than being hidden in this refactor.

### 5.4 Target dependency rules

At completion:

| Crate | Required relationship |
|---|---|
| `hir` | depends on `model`, `syntax`, and `text`; owns only HIR tree/source semantics |
| `provider` | depends on `model`; owns schemas, calls, results, errors, registry, and current native feature |
| `plan` | depends on `model` and `text`; has no production HIR, syntax, provider, runtime, or adapter dependency |
| `analysis` | depends directly on `model`, HIR, provider, and plan; owns HIR-to-plan lowering |
| `observation` | depends directly on `model`; has no production HIR dependency |
| `runtime` | depends directly on `model`; has no production HIR dependency |
| `app-bridge` | depends directly on `model` for values/types and on provider for its provider contract |
| `editor`, `DAP`, `WASM`, `app` | depend directly on `model` when they inspect model values/types/capabilities |

Test-only dependencies may cross these boundaries for vertical fixtures, but they do not authorize
the same production edge.

## 6. Explicit lowering and execution conversions

### 6.1 HIR to plan belongs to analysis

The existing `plan::locator_from_hir` and `plan::locator_state_from_hir` helpers make the plan crate
depend on HIR. They move to the analysis compiler alongside the other source-semantic-to-plan
lowering logic.

The conversion remains exhaustive and explicit:

```text
HirLocatorKind::{Id, Role, Label, Text, Placeholder, TestId, Css, XPath}
    -> plan::Locator::{Id, Role, Label, Text, Placeholder, TestId, Css, XPath}

hir::LocatorState
    -> plan::LocatorState
```

Plan does not gain a generic AST/HIR conversion trait, source parser, or dependency on syntax. HIR
does not construct plan nodes. Analysis is the boundary that understands both representations.

### 6.2 Plan to browser belongs to runtime

Runtime retains the explicit conversion from `plan::Locator`, `plan::LocatorState`, and
`plan::BrowserOperation` into the protocol-neutral browser request types. Browser does not depend
on plan, and plan does not depend on browser.

The two conversion boundaries must have exhaustive tests. Adding a future author-facing locator
may legitimately touch syntax, HIR, analysis lowering, plan, runtime conversion, browser, CDP,
inspection, evidence, and authoring reference facts. D.11 treats that as domain propagation to be
made compiler-visible, not as a reason to introduce a universal locator.

### 6.3 Expressions remain staged representations

`HirExpr`, `PlanExpr`, and `Value` remain separate:

```text
HirExpr   what the source construct means
PlanExpr  what pure computation an execution host performs
Value     what data exists while the plan executes
```

Moving shared operators and values to `model` must not collapse these stages or allow runtime to
interpret HIR.

## 7. Typed runtime failure identity

### 7.1 Owner and purpose

`webtest-observation` owns a public, copyable, comparable, hashable, serializable
`RuntimeFailureCode`. Observation is the correct owner because it already owns the protocol-neutral
runtime event/failure/observation DTOs and depends on browser and provider error contracts. Browser
and provider must not depend upward on observation.

The type has a closed variant for every stable runtime failure identity currently emitted by
WebTest. It provides at least these two edge projections:

```rust
impl RuntimeFailureCode {
    pub const fn short_code(self) -> &'static str;
    pub const fn diagnostic_code(self) -> &'static str;
}
```

`short_code` preserves producer/debugger spellings such as `locator_not_found`,
`element_not_visible`, `path_escape`, and `integer_overflow`. `diagnostic_code` preserves or
normalizes the author-facing namespace, such as `runtime.locator_not_found`,
`runtime.locator_not_visible`, `runtime.path_escape`, and `runtime.integer_overflow`.

The exact private module layout and enum variant names are not a wire contract. The returned code
strings are. The Serde representation is the exact `short_code()` string so replacing a current
string field with the typed value does not silently alter that field's JSON shape. Deserialization
accepts only the closed implemented vocabulary; an unknown external string is not converted to a
generic known failure.

### 7.2 Typed mapping points

The current browser projections are preserved exactly:

| Structured browser error | Short code | Diagnostic code | Normal class |
|---|---|---|---|
| `LocatorNotFound` | `locator_not_found` | `runtime.locator_not_found` | test |
| `LocatorAmbiguous` | `locator_ambiguous` | `runtime.locator_ambiguous` | test |
| `LocatorInvalid` | `locator_invalid` | `runtime.locator_invalid` | test |
| `ElementDetached` | `element_detached` | `runtime.element_detached` | test |
| `LocatorNotVisible` | `element_not_visible` | `runtime.locator_not_visible` | test |
| `ElementUnstable` | `element_unstable` | `runtime.element_unstable` | test |
| `ElementDisabled` | `element_disabled` | `runtime.element_disabled` | test |
| `ElementObscured` | `element_obscured` | `runtime.element_obscured` | test |
| `ElementNotEditable` | `element_not_editable` | `runtime.element_not_editable` | test |
| `OptionNotFound` | `option_not_found` | `runtime.option_not_found` | test |
| `OptionAmbiguous` | `option_ambiguous` | `runtime.option_ambiguous` | test |
| `InvalidKey` | `invalid_key` | `runtime.invalid_key` | test |
| `ActionTimeout` | `action_timeout` | `runtime.action_timeout` | test |
| `AssertionFailed` | `assertion_failed` | `runtime.assertion_failed` | test |
| `UrlMismatch` | `url_mismatch` | `runtime.url_mismatch` | test |
| `NavigationFailed` | `navigation_failed` | `runtime.navigation_failed` | infrastructure |
| `NavigationTimeout` | `navigation_timeout` | `runtime.navigation_timeout` | infrastructure |
| `CommandTimeout` | `browser_command_timeout` | `runtime.browser_command_timeout` | infrastructure |
| `BrowserDisconnected` | `browser_disconnected` | `runtime.browser_disconnected` | infrastructure |
| `BrowserCrashed` | `browser_crashed` | `runtime.browser_crashed` | infrastructure |
| `MalformedProtocol` | `browser_malformed_protocol` | `runtime.browser_malformed_protocol` | infrastructure |
| `Protocol` | `browser_protocol` | `runtime.browser_protocol` | infrastructure |
| `Launch` | `browser_launch` | `runtime.browser_launch` | infrastructure |
| `EvaluationFailed` | `evaluation_failed` | `runtime.evaluation_failed` | test |
| `UnsupportedCapability` | `unsupported_browser_capability` | `runtime.unsupported_browser_capability` | infrastructure |

The current provider projections are also preserved:

| Structured provider error | Short code | Diagnostic code | Normal class |
|---|---|---|---|
| `NotRegistered` | `provider_not_registered` | `runtime.provider_not_registered` | infrastructure |
| `UnknownOperation` | `provider_unknown_operation` | `runtime.provider_unknown_operation` | infrastructure |
| `InvalidArgument` | `provider_invalid_argument` | `runtime.provider_invalid_argument` | test |
| `HttpTransport` | `http_transport` | `runtime.http_transport` | infrastructure |
| `ResponseTooLarge` | `response_too_large` | `runtime.response_too_large` | infrastructure |
| `ProcessSpawn` | `process_spawn` | `runtime.process_spawn` | infrastructure |
| `ProcessTimeout` | `process_timeout` | `runtime.process_timeout` | infrastructure |
| `ProcessOutputTooLarge` | `process_output_too_large` | `runtime.process_output_too_large` | infrastructure |
| `Filesystem` | `filesystem` | `runtime.filesystem` | infrastructure |
| `PathEscape` | `path_escape` | `runtime.path_escape` | test |
| `Unavailable` | `provider_unavailable` | `runtime.provider_unavailable` | infrastructure |
| `BridgeHandshake` | `app_bridge_handshake` | `runtime.app_bridge_handshake` | infrastructure |
| `BridgeProtocol` | `app_bridge_protocol` | `runtime.app_bridge_protocol` | infrastructure |
| `BridgeTransport` | `app_bridge_transport` | `runtime.app_bridge_transport` | infrastructure |
| `BridgeProcess` | `app_bridge_process` | `runtime.app_bridge_process` | infrastructure |
| `BridgeSchemaDrift` | `app_schema_drift` | `runtime.app_schema_drift` | infrastructure |
| `BridgeValidation` | `app_bridge_validation` | `runtime.app_bridge_validation` | infrastructure |
| `BridgeTimeout` | `app_bridge_timeout` | `runtime.app_bridge_timeout` | infrastructure |
| `Application` | `app_provider_failure` | `runtime.app_provider_failure` | test |

Runtime-local projections complete the current vocabulary:

| Runtime failure | Short code | Diagnostic code | Normal class |
|---|---|---|---|
| test timeout | `test_timeout` | `runtime.test_timeout` | test |
| value assertion | `assertion_failed` | `runtime.assertion_failed` | test |
| typed JSON decode | `json_decode_failed` | `runtime.json_decode_failed` | test |
| response decode | `response_decode_failed` | `runtime.response_decode_failed` | test |
| division by zero | `division_by_zero` | `runtime.division_by_zero` | test |
| integer overflow | `integer_overflow` | `runtime.integer_overflow` | test |
| internal invariant | `internal_error` | `runtime.internal_error` | internal |
| browser-context cleanup | `cleanup_browser_context_failed` | `runtime.cleanup_browser_context_failed` | cause-derived |
| browser-session cleanup | `cleanup_browser_session_failed` | `runtime.cleanup_browser_session_failed` | cause-derived |
| temporary-directory cleanup | `cleanup_temporary_directory_failed` | `runtime.cleanup_temporary_directory_failed` | cause-derived |

The normal-class column characterizes the path on which the failure is currently produced; it does
not make the code string authoritative for classification. Run-level setup/finalization context
and aggregate-primary selection remain authoritative where applicable. Cleanup demonstrates why
the detailed failure remains primary: the same cleanup identity can be infrastructure or internal
according to its typed cause.

Mappings are exhaustive matches over structured variants:

```text
BrowserError variant       -> RuntimeFailureCode
ProviderError variant      -> RuntimeFailureCode
assertion/decode kind      -> RuntimeFailureCode
typed evaluation kind      -> RuntimeFailureCode
cleanup resource/cause     -> RuntimeFailureCode
test timeout               -> RuntimeFailureCode
internal invariant failure -> RuntimeFailureCode
```

The mapping lives with observation/runtime conversion, not separately in editor and app. Adding a
new `BrowserError`, `ProviderError`, evaluation kind, or cleanup resource must produce a compile
error at its missing runtime-code mapping rather than fall through to a generic string.

`EvaluationFailure.code: &'static str` becomes a typed evaluation-failure kind or directly stores a
constrained typed runtime code. Arbitrary string construction is not permitted. The implemented
evaluation identities include `division_by_zero`, `integer_overflow`, and
`response_decode_failed`.

### 7.3 Events and observations

These current string fields become typed:

```text
ExecutionEvent::ProviderCallFailed.code
ExecutionEvent::CleanupFailed.code
RuntimeObservationKind::BrowserFailure.code
RuntimeObservationKind::ValueFailure.code
```

`StepError`, `RunError`, `RuntimeFailure`, and `CleanupFailure` expose or derive the same typed code.
Detailed error variants and payloads remain intact. `RunError::Multiple` continues to report the
typed code of its already-defined primary failure; D.11 does not change severity or primary-error
selection.

The legacy locator-specific observation variants may remain for source compatibility in D.11, but
their diagnostic identity must use `RuntimeFailureCode` rather than a separately maintained
literal. Milestone E may redesign the observation schema only through its versioned contract.

### 7.4 Dynamic subcodes remain data

These strings are intentionally not variants of the closed runtime failure vocabulary:

- the application-defined `code` in `ProviderError::Application`;
- bridge handshake/protocol subcodes such as authentication or framing details;
- protocol method names, filesystem paths, messages, locators, expected/actual values, and evidence.

They remain redacted structured semantic details beneath the stable top-level identities
`app_provider_failure`, `app_bridge_handshake`, `app_bridge_protocol`, and their corresponding
namespaced diagnostic codes. An application or bridge cannot mint a new top-level WebTest machine
diagnostic by returning a string.

### 7.5 Adapter behavior

The adapter rule is:

```text
structured failure + RuntimeFailureCode
        -> adapter-specific message/details
        -> short_code() or diagnostic_code() at the final wire field
```

Editor, app reporters, DAP, and future IPC may match `RuntimeFailureCode` or the structured failure
variant. They must not match `&str` to rediscover a locator, provider, timeout, cleanup, or
evaluation meaning.

Existing code strings remain compatible except for one intentional normalization: when editor
currently collapses an already-known typed failure to `runtime.provider_failure` or
`runtime.assertion_failed`, it must publish the same specific `diagnostic_code()` used by the CLI.
For example, integer overflow becomes `runtime.integer_overflow` and an application failure becomes
`runtime.app_provider_failure` in both surfaces. This removes adapter disagreement; it does not add
a new failure class. Because C.5 defines stable code meaning as part of the diagnostic compatibility
contract, this normalization increments `DIAGNOSTIC_SCHEMA_VERSION` from 1 to 2.

The established exceptional spelling remains stable:

```text
short:      element_not_visible
diagnostic: runtime.locator_not_visible
```

All other current diagnostic spellings remain the existing explicitly tested values, normally
`runtime.` plus the short code.

### 7.6 Messages, details, repairs, and references

Typed identity does not centralize all presentation. Friendly CLI/editor phrasing may remain in the
adapter when it is genuinely host-specific. Structured semantic facts remain in their current
owners.

However, default authoring-reference routing must not diverge by string fallback. The shared typed
mapping supplies stable default reference facts where they already exist, including application
bridge diagnostics/configuration and value assertions. Locator-specific reference queries may still
be derived from the typed locator evidence because `locator.role`, `locator.label`, and other leaves
depend on the locator variant, not only on the failure code.

Any intentional change to a reference query must be backed by an installed `webtest describe`
topic and exact lookup/search tests. D.11 does not invent a generic reference for a known failure
when no useful topic exists.

## 8. Provider boundary after extraction

After D.11, `webtest-provider` owns:

```text
ProviderName / OperationName
ParameterSchema / OperationSchema / ProviderSchema / provenance
ProviderCall / ProviderResult / CallContext / ProviderError
ServerProvider / ProviderRegistry
current native HTTP, process, and sandboxed-filesystem feature
native provider configuration
```

Provider schemas refer to `webtest_model::Type`, `Value`, and `Capability`; they do not own them.
Analysis still consumes schemas as explicit semantic inputs. Runtime still dispatches through the
registry. App bridge still implements the same provider contract.

Separating the built-in HTTP/process/filesystem implementations into a future
`webtest-provider-native` crate may become useful, but it is not required by D.11. The existing
feature boundary remains acceptable for this pre-E slice. D.11 must not combine the model move with
a provider implementation redesign, new registry abstraction, plugin system, or host-capability
negotiation.

## 9. Compatibility contract

### 9.1 Language and runtime behavior

Parsing, formatting, HIR lowering, static diagnostics, type checking, capability checking, plan
step ordering, provider dispatch, browser actions, assertions, timeouts, cleanup, evidence,
redaction, and test outcomes remain unchanged.

No source program becomes newly valid or invalid. Canonical examples continue to parse and
statically analyze through their declared enclosing context.

### 9.2 Plans and builds

`PLAN_FORMAT_VERSION` remains 3. For identical source and explicit semantic inputs, plan JSON is
structurally identical before and after D.11, including IDs, operators, type/value tags,
capabilities, origins, source revisions, provider schema hashes, and test/step order.

`webtest build --emit` remains deterministic. Literal-secret traversal moves to model imports but
does not change which values are rejected.

### 9.3 Providers and application bridge

Provider schema identity remains derived from the same canonical serialized functions and types.
Offline manifests, live schema comparison, Protocol 1 DTOs, Node/Ruby SDK behavior, compatibility
adapters, and conformance fixtures remain unchanged.

Native provider feature selection stays explicit. Portable builds do not acquire filesystem,
process, socket, HTTP-execution, or Chrome behavior through `model`.

### 9.4 Machine and editor surfaces

Human, concise, JSON, events, and JUnit reporters retain their envelope schema versions, field names,
messages, exit classes, failure classes, redaction, source spans, repair hints, and established code
strings. DAP retains one-based source positions, redacted binding values, and the existing spelling
of each code field, including short-code fields where currently used. LSP retains UTF-16 conversion
and revision-safe diagnostic publication.

The only intended output correction is the removal of adapter-specific generic fallbacks for a
failure that has a known canonical typed code, as described in section 7.5. Tests must enumerate
those corrected cases so this is not an open-ended output rewrite. Diagnostic schema version 2
records the narrower meanings of `runtime.provider_failure` and `runtime.assertion_failed` and the
specific replacement codes. The repair-hint, report-envelope, plan, inspection, description, and
bridge schema versions do not change.

### 9.5 Description and portable parity

D.11 adds no author-facing language, type, capability, provider operation, or browser feature, so
the installed description catalog and canonical WebTest skill require no new topic or guidance.
Existing description completeness, example analysis, configured-provider projection, and
native/WASM parity tests are mandatory regressions.

Crate-ownership documentation must be updated to name `model` as the owner and narrow `hir` and
`provider` accordingly. Historical milestone scopes are not silently rewritten.

## 10. Delivery slices

### Slice 1 — Characterize serialized and diagnostic compatibility

Before moving definitions:

- add or retain golden plan-format-3 coverage for every moved ID/operator/type/value/capability
  shape;
- record representative built-in and app-provider schema hashes;
- enumerate every `BrowserError`, `ProviderError`, evaluation, timeout, cleanup, assertion, decode,
  and internal runtime code projection;
- characterize CLI reporter, editor/LSP, DAP, and WASM output for the same failures; and
- identify every production dependency/import that exists only because of the false owners.

### Slice 2 — Add the model crate and move identities/operators

Create the dependency-free WebTest foundation crate, move the canonical IDs and operators, add
temporary re-exports if retained, migrate direct consumers, and prove numeric/Serde compatibility.

Do not change allocation or plan ordering while moving the definitions.

### Slice 3 — Move types, values, and capabilities

Move the canonical type/value model and its pure inherent behavior. Migrate provider schema types,
analysis, plan, runtime, bridge, DAP, WASM, and app consumers. Move unit tests with the semantic
owner and keep provider integration tests at the provider boundary.

At the end of this slice, `plan` has no provider dependency.

### Slice 4 — Rehome HIR-to-plan conversions

Move locator/state lowering into analysis, migrate exact range and deterministic-plan tests, and
remove the production HIR dependency from `plan`. Remove remaining HIR dependencies from runtime,
observation, and app when they exist only for moved model types.

### Slice 5 — Introduce typed failure codes

Add the exhaustive code type and typed evaluation identity. Migrate `StepError`, `RunError`,
cleanup, `RuntimeFailure`, execution events, and runtime observations without changing detailed
payloads or classification.

### Slice 6 — Migrate adapters and remove string interpretation

Make editor, app reporters, DAP, and producer-local retry bookkeeping use typed identity. Delete
duplicate runtime-code tables and generic string fallbacks. Preserve edge spellings and explicitly
test the intended editor normalization.

### Slice 7 — Finalize architecture and parity

Update crate documentation and repository guidance, audit direct dependencies and compatibility
re-exports, run native/WASM and bridge/provider regressions, and confirm no implementation-only
model or error policy leaked into an adapter.

Each slice must leave the workspace building and tests passing. Compatibility re-exports may bridge
slices, but completion requires internal production consumers to use the canonical crate directly.

## 11. Testing requirements

### 11.1 Model tests

Focused `webtest-model` tests must cover:

- ID/operator Serde and equality/order behavior;
- every `Type` Serde variant and display form;
- type acceptance, optional records, member lookup, and transferability;
- every `Value` Serde variant and member behavior;
- JSON conversion success and rejection of non-JSON value variants;
- deterministic record order; and
- recursive redaction of all value shapes without changing non-secret values.

### 11.2 Dependency tests and review checks

The completed normal dependency graph must prove:

- `model` has no WebTest dependency;
- `plan` has no HIR, syntax, provider, browser, runtime, or adapter dependency;
- runtime and observation have no HIR dependency;
- browser remains independent of browser-cdp and plan;
- provider remains independent of analysis, plan, runtime, editor, and adapters; and
- project remains absent from analysis/model/runtime/browser/provider.

Source inspection must find no duplicate definitions of the moved types and no production import
of them through HIR/provider compatibility paths.

### 11.3 Plan/provider/portable parity

Tests must prove:

- plan-format-3 JSON equality for representative pure, provider, browser, decode, record, optional,
  and assertion operations;
- deterministic `build --emit` output;
- unchanged built-in and application provider schema hashes;
- offline/live app-schema parity and existing Protocol 1 conformance;
- every installed description example still parses and analyzes; and
- native and WASM diagnostics, descriptions, and portable compilation remain equal for portable
  inputs.

### 11.4 Failure-code exhaustiveness

There must be table-driven coverage for every current structured error variant and local runtime
failure kind. Each row asserts:

```text
structured input
short code
diagnostic code
failure class
adapter-visible reference facts where applicable
```

Tests also prove:

- dynamic application/bridge subcodes remain details rather than top-level diagnostic identities;
- diagnostic schema version 2 accompanies the normalized runtime-code vocabulary while unrelated
  schema versions remain unchanged;
- `RunError::Multiple` selects the existing primary before projecting its typed code;
- every event and observation stores or losslessly derives the typed code without string matching;
- editor and CLI publish the same namespaced code for the same failure;
- DAP preserves each established code-field spelling;
- no unknown known-variant fallback becomes `runtime.provider_failure` or
  `runtime.assertion_failed`; and
- messages, evidence, repair hints, paths, and values remain redacted and bounded.

### 11.5 Verification commands

Run at minimum:

```sh
cargo fmt --all -- --check
cargo test -p webtest-model
cargo test -p webtest-provider
cargo test -p webtest-plan
cargo test -p webtest-analysis
cargo test -p webtest-app-bridge
cargo test -p webtest-runtime
cargo test -p webtest-observation
cargo test -p webtest-editor
cargo test -p webtest-lsp
cargo test -p webtest-dap
cargo test -p webtest-wasm
cargo test -p webtest
cargo clippy --workspace --all-targets -- -D warnings
```

When the portable target is installed, also run:

```sh
cargo check -p webtest-wasm --target wasm32-unknown-unknown
```

The full `cargo test --workspace` remains the final regression gate. Real Chrome is required only
for existing backend semantics affected by an implementation regression; D.11 adds no new
real-browser behavior.

## 12. Review checkpoints

Before merging, review must answer yes to all of the following:

1. Does `model` contain only concepts with identical cross-layer meaning?
2. Is every moved concept defined once, with any compatibility path implemented as a re-export?
3. Did the move remove the targeted false normal-dependency edges?
4. Is HIR-to-plan lowering owned by analysis and plan-to-browser conversion owned by runtime?
5. Are HIR, plan, and browser locators still separate representations?
6. Do all core failure-routing decisions use typed variants rather than code strings?
7. Is `FailureClass` still derived from structured failure semantics, not presentation spelling?
8. Are dynamic application/bridge codes still bounded, redacted details?
9. Are plan, schema-hash, reporter, DAP, LSP, and WASM compatibility proven?
10. Did the change avoid pulling any Milestone E scheduler, identity, event, trace, or IPC design
    into the current sequential runtime?

## 13. Risks and mitigations

### 13.1 A generic core crate forms

The name `model` can attract unrelated shared helpers. Enforce the admission rule in section 3.2,
keep its dependency graph at the standard library/Serde level, and reject source, provider,
runtime, backend, configuration, and presentation policy.

### 13.2 Crate moves alter wire formats

Serde ordinarily ignores Rust crate paths, but derive attributes, field defaults, enum order,
number conversion, or map choice can drift during a move. Golden plan/provider/portable fixtures and
schema-hash tests are required before and after extraction.

### 13.3 Compatibility re-exports hide false dependencies

A re-export can preserve source compatibility while allowing internal architecture to remain
misleading. All repository production consumers import directly from `webtest-model`, and the
normal Cargo graph—not only type identity—is an acceptance artifact.

### 13.4 Typed codes become a second error hierarchy

`RuntimeFailureCode` is identity metadata, not a replacement for detailed errors. Keep payload,
classification, redaction, evidence, and display on the structured error path. Do not copy all
error fields into the code enum.

### 13.5 Adapter output drifts

Centralizing code projection can accidentally rename human/JSON/JUnit/DAP fields. Characterize all
surfaces first and restrict intentional correction to known generic fallbacks.

### 13.6 Dynamic provider codes escape into the stable namespace

Application and bridge subcodes are untrusted/dynamic data. Keep them under semantic details and
map their containing structured error to one stable WebTest code.

### 13.7 Representation deduplication increases coupling

Similar locator/expression enums may tempt a broad cleanup. Preserve the explicit staged types and
test both conversion boundaries exhaustively.

### 13.8 Milestone E concepts leak backward

D.11 prepares ownership for E but does not add graph nodes, task trees, cancellation tokens,
attempts, event journals, IPC, or traces. Shared model extraction must not freeze E's future
runtime-occurrence identity or plan topology.

## 14. Non-goals

D.11 does not:

- merge crates or create a general `webtest-core` crate;
- split native HTTP/process/filesystem implementations into a new provider-native crate;
- unify HIR, plan, and browser locators or locator-state enums;
- collapse `HirExpr`, `PlanExpr`, and `Value`;
- redesign provider schemas, registry dispatch, built-in providers, or application bridge Protocol
  1;
- redesign `EditorService`, LSP, DAP, WASM, reporters, or project configuration;
- add a new language type, value, capability, provider operation, locator, failure class, or repair
  mechanism;
- change plan format, schema hash, report-envelope, repair-hint, inspection, description, or bridge
  protocol versions; diagnostic schema version 2 is the one required version change;
- add sequence/parallel/race/retry/timeout syntax or plan nodes;
- add remote execution, worker negotiation, plan submission, event streaming, CLI-to-LSP IPC, or a
  trace format; or
- claim that a shared type's current identity is stable across revisions or distributed hosts when
  its existing contract does not say so.

## 15. Acceptance criteria

Milestone D.11 is complete only when:

1. `webtest-model` is the single canonical owner of the existing shared semantic IDs, operators,
   type/value model, and capability vocabulary.
2. Its dependency surface is limited to the standard library, Serde, and Serde JSON, and its tests
   own the moved pure semantics.
3. `plan` has no production dependency on HIR or provider; runtime and observation have no
   production dependency on HIR.
4. Analysis owns explicit HIR-to-plan lowering, runtime owns explicit plan-to-browser conversion,
   and no universal locator/expression type is introduced.
5. Plan-format-3 output, provider schema hashes, application manifests, bridge conformance, and
   portable compilation remain compatible.
6. Every runtime failure that reaches an event, observation, diagnostic, report, or debugger has an
   exhaustive typed `RuntimeFailureCode` while retaining its structured payload and class.
7. Editor, CLI reporters, and DAP no longer switch on failure-code strings; outward code spellings
   remain compatible except for the enumerated removal of generic adapter fallbacks, which is
   identified by diagnostic schema version 2.
8. Dynamic application and bridge subcodes remain redacted structured details and cannot create a
   top-level WebTest diagnostic identity.
9. Existing `webtest describe` completeness/examples and native/WASM parity pass without
   advertising a new author-facing feature.
10. Repository architecture guidance names the new owner, and all focused plus workspace format,
    test, Clippy, and portable verification gates pass.

The practical completion test is that the normal dependency graph tells the same story as the
product architecture: source identity belongs to `text`, shared semantic values belong to `model`,
provider contracts belong to `provider`, executable representation belongs to `plan`, runtime facts
belong to `observation`, and no adapter reconstructs those meanings from strings.
