# Milestone D.9.1 — Residual Runtime and Browser Semantic Hardening

## 0. Status and relationship to Milestone D.8

**Status: implemented (scope re-audited on `main` at
`323cb829599e1d416f1fc6c8eed30f1a9f849ee0`; implementation completed 2026-09-01).**

**Implementation status (2026-09-01):** Shared locator resolution is passive; action preparation
owns scrolling, post-scroll remeasurement, and the physical-pointer gate used by click, hover,
check, fill, type, and press. Runtime integer operations are checked and all numeric consumers use
one exact integer-aware relation. Provider type compatibility preserves optional presence, plan
format 3 carries statically optional member access, and runtime returns `Null` only for that plan
fact. Native/portable descriptions and plans, real-Chrome behavior, release-profile overflow, and
the existing structured failure/source-mapping contracts are covered by regression tests.

This milestone contains four corrections that remain after the completed D.8 series. It adds no
language syntax or new product capability.

The earlier draft also repeated two findings that are already implemented:

- [`milestone-d-8-4.md`](./milestone-d-8-4.md) owns and implements the absolute per-test deadline
  and operation-budget rules;
- [`milestone-d-8-5.md`](./milestone-d-8-5.md) owns and implements per-test capabilities, plan
  format 2, and lazy browser allocation.

Those are ordinary regression dependencies, not D.9.1 work. D.9.1 does not restate their designs,
acceptance cases, or delivery order.

The browser decomposition in [`milestone-d-8.md`](./milestone-d-8.md) intentionally preserved the
then-current locator and input behavior. In particular, it documented that unique locator
resolution scrolls before measurement and required action behavior to remain compatible during the
refactor. The decomposition made the remaining browser corrections easier to isolate; it did not
implement them. D.8.6 and D.8.7 address unrelated provider-builder and artifact-I/O findings.

## 1. Remaining outcome

After D.9.1, the current sequential runtime has these four additional invariants:

```text
passive locator observation
  -> never scrolls, focuses, clicks, or dispatches input

action preparation
  -> scrolls only after unique resolution, then remeasures
  -> proves the target receives pointer input before any physical click

integer evaluation
  -> checked i64 arithmetic and exact integer-aware comparison

statically optional record access
  -> present value or Null, never an internal missing-member failure
```

The exhaustive residual scope is:

| Remaining finding | Correction | Principal proof |
|---|---|---|
| Passive locator resolution scrolls | make shared resolution observational; move scrolling into action preparation | waits, assertions, evidence, and inspection leave scroll/focus state unchanged |
| `fill`, `type`, and `press` click without the existing obscuration gate | centralize the physical-pointer requirement | an overlay blocks all three before mouse or keyboard input is sent |
| Integer arithmetic and comparison are unchecked or lossy | checked `i64` operations plus one exact numeric relation | overflow is structured and adjacent integers above 2^53 remain distinct |
| Optional record typing and runtime access disagree | coherent `Option<T>` compatibility plus one plan-level optional-member bit | every legal omitted optional member evaluates to `Null` |

Failures retain the D.8 taxonomy. Integer overflow and an obscured target are ordinary test
failures. Browser transport/provider failures remain infrastructure failures. A required member
missing from a supposedly well-typed value remains a provider-contract or internal-invariant
failure; only a statically optional member may turn absence into `Null`.

## 2. Verified current baseline

The four rows above are present in the current code:

- `crates/browser-cdp/src/page/locator.rs` puts `scrollIntoView` in the one resolver used by
  locator waits, actions, evidence, and inspection;
- `crates/browser-cdp/src/page/actions.rs` requires `!obscured` for `click`, `hover`, and `check`,
  but `fill`, `type`, and `press` all call the same physical click afterward;
- `crates/runtime/src/evaluation.rs` directly negates/adds/subtracts/multiplies `i64` values and
  converts numeric comparisons to `f64`;
- `webtest_provider::Type::accepts` does not accept plain `T` as `Option<T>`, does not reject an
  actual optional field where a required record field is expected, and `PlanExpr::Member` cannot
  tell whether a missing field was statically optional.

Typed JSON decoding already materializes omitted optional fields as `Value::Null`, and header
lookup already returns `Null` for an absent name. The remaining optional-member bug is observable
with annotated record literals and provider records that preserve omission.

## 3. Passive observation and active action preparation

### 3.1 One passive resolver

The shared locator resolver must only locate and measure. Its evaluated JavaScript may read DOM,
style, layout, accessible-name, hit-test, and candidate facts, but it must not call
`scrollIntoView`, focus or click an element, mutate DOM state, or dispatch events.

The passive resolver remains the only locator implementation. These consumers use it without an
action-preparation side effect:

- locator state waits and assertions;
- failure evidence and candidate capture;
- semantic inspection and candidate validation.

An offscreen element may satisfy the existing attached/visible rules. Observing it must not move
the viewport merely to make it actionable.

### 3.2 Action-owned scrolling and remeasurement

`Page::perform` may prepare the page for an action. Its CDP implementation must:

1. passively resolve exactly one target;
2. reject the existing missing/ambiguous/invisible and non-hit-test action-specific failures;
3. scroll that target into view;
4. passively resolve again and use post-scroll facts for stability, hit testing, and input;
5. preserve bounded polling and the current final-failure precedence.

The exact private helper names are not part of the contract. A small action-only scroll helper plus
the existing resolver is sufficient; do not add a second locator evaluator or expose CDP details
through the protocol-neutral `browser` crate.

If scrolling changes layout, exposes a sticky overlay, detaches the target, or changes locator
cardinality, the post-scroll result wins. No physical input may use a pre-scroll rectangle.

### 3.3 One physical-pointer requirement

Use one named action predicate for the existing actions that send or depend on physical pointer
input:

| Action | Requires unobscured hit target |
|---|---|
| `click`, `hover`, `check` | yes |
| `fill`, `type`, `press` | yes |
| `select` | no; its current implementation does not click |

Apply the predicate to every post-scroll stability sample. For `fill`, `type`, and `press`, all
actionability checks complete before the first mouse, key, or text command. A stable overlay yields
the existing `element_obscured` failure and sends no focus click, select-all, backspace, key event,
or inserted text.

This corrects the current physical-input rule only. It does not add a general Playwright-style
actionability model, focus verification, retries, trial actions, or overlay dismissal.

## 4. Checked and exact numeric semantics

### 4.1 Integer overflow

Use `checked_neg`, `checked_add`, `checked_sub`, and `checked_mul` for current `Int` operations.
Overflow returns:

```rust
StepError::Evaluation(EvaluationFailure {
    code: "integer_overflow",
    message: <bounded operator-specific message>,
})
```

It must never panic, wrap, become an internal failure, or depend on the build profile. Existing
division, division-by-zero, float arithmetic, and `Int`-to-`Float` result rules remain unchanged.

### 4.2 One numeric relation

Equality, inequality, ordered expressions, ordered assertions, and list containment share the
same numeric relation:

| Operands | Required behavior |
|---|---|
| `Int`, `Int` | exact `i64` equality/order |
| `Float`, `Float` | normal IEEE equality and partial order |
| `Int`, `Float` in either order | compare their mathematical values without first rounding the integer to `f64` |

This must cover the full `i64` range, finite fractional floats, infinities, signed zero, and NaN.
NaN is unequal to every value and ordered relations involving NaN are false, following the
existing partial-order shape; D.9.1 does not add a separate unordered-comparison failure.

Required boundary facts include:

```text
9007199254740992 < 9007199254740993
9007199254740993 != 9007199254740992.0
i64::MAX < 9223372036854775808.0
```

The last line is an evaluator test fact, not new DSL cast or oversized-integer syntax. The private
comparison API may change as needed, but invalid nonnumeric operands in a forged plan remain an
internal invariant rather than being conflated with unordered floating-point values.

## 5. Optional record semantics

### 5.1 Type compatibility

Define these rules once in `webtest_provider::Type::accepts`:

```text
Option<T> accepts Null
Option<T> accepts U when T accepts U
Option<T> accepts Option<U> when T accepts U
T does not accept Option<U> merely because T accepts U
```

Record compatibility also accounts for presence:

- an expected required field must exist in the actual record, must not be optional there, and must
  have a compatible type;
- an expected optional field may be absent;
- when an expected optional field is present, its value type must be compatible;
- extra actual fields keep the existing structural-record behavior.

Thus a record with `name?: String` cannot satisfy `{ name: String }`, while a present required
`name: String` can satisfy `{ name?: String }`.

### 5.2 Add only the missing plan fact

Do not normalize every record map or provider wire value by inserting `Null`; omission and explicit
JSON `null` can have different provider-boundary meanings.

Instead, add the smallest fact the evaluator lacks to `PlanExpr::Member`, equivalent to:

```rust
Member {
    receiver: Box<PlanExpr>,
    member: String,
    missing_is_null: bool,
}
```

Analysis sets the bit from the statically known member type. Runtime returns `Value::Null` only
when the member is absent and the bit is true. A missing required member remains internal. The
existing missing `response.text`/`response.json` path remains the structured
`response_decode_failed` evaluation failure; it does not need another plan enum case.

This serialized field changes the plan contract, so increment the implemented D.8.5 plan format
from 2 to 3 and update deterministic native/WASM fixtures. Reject unsupported older envelopes as
the current decoder already does; do not guess optionality from an untyped runtime value. DAP uses
the shared plan and evaluator without reconstructing source semantics.

Required language behavior is:

```webtest
let literal: { required: String, optional?: String } = {
    required: "hello",
}
expect literal.optional == null

let present: Option<String> = "hello"
expect present == "hello"
```

Equivalent provider records and typed JSON values have the same member-access result. Omission of
a required provider result field is still a provider contract violation, not an optional value.

## 6. Implementation boundaries

- `browser-cdp` removes scrolling from shared resolution, owns action-only scrolling and
  post-scroll remeasurement, and centralizes the physical-pointer predicate.
- `provider` owns `Option<T>` and structural-record compatibility.
- `analysis` lowers the optional-member bit and updates the installed description facts/examples.
- `plan` serializes the bit and advances the plan format to 3.
- `runtime` implements checked arithmetic, the shared numeric relation, and optional member access.
- `browser`, `editor`, `lsp`, `dap`, `wasm`, and app reporters remain thin consumers; change shared
  DTO/schema snapshots only where the plan or existing structured evaluation failure requires it.

No syntax, AST, or HIR change is expected. `webtest describe` must explain checked `Int` overflow,
exact numeric comparison, optional assignment/member behavior, passive locator observation, and
the existing obscured-target failure without advertising broader roadmap functionality.

## 7. Required regression tests

Browser coverage must prove:

1. a passive wait/assertion on an offscreen locator leaves scroll position, focus, and a scroll
   event counter unchanged;
2. evidence and semantic inspection do not scroll;
3. an offscreen action scrolls, remeasures, and succeeds;
4. a sticky overlay introduced by scrolling is detected from the post-scroll snapshot;
5. obscured `fill`, `type`, and `press` each send no mouse/keyboard/text input;
6. removing an overlay during bounded polling still permits the action;
7. existing click/hover/check/select and failure-precedence behavior remains green.

Numeric coverage must prove:

8. all four `i64` overflow boundaries return `integer_overflow` without unwinding in debug and
   release profiles;
9. equality and ordering around 2^53 and both `i64` boundaries are exact;
10. mixed comparisons cover fractional values, infinities, signed zero, and NaN;
11. expression, assertion, and list-containment paths agree.

Optional coverage must prove:

12. `Option<T>` accepts `Null`, compatible `T`, and compatible `Option<U>`, while required `T`
    rejects `Option<U>`;
13. a required expected record field rejects an actual optional field;
14. omitted and present optional members work for annotated literals, provider results, nested
    records/lists, and typed JSON;
15. a missing required member remains internal and unavailable response decoding retains
    `response_decode_failed`;
16. plan format 3 records the member bit deterministically and native/WASM plan and description
    outputs remain equivalent.

Use focused unit/fake coverage for resolver construction, action requirements, evaluator edges,
and plan lowering. Use random-loopback real Chrome coverage for scroll, layout, hit testing, and
input, skipping only when Chrome or loopback sockets are genuinely unavailable. The completed
D.8.4/D.8.5 suites remain workspace regressions but are not duplicated as D.9.1 acceptance work.

## 8. Explicit non-goals

D.9.1 does not add modules, functions, control flow, retries, traces, parallelism, timeout blocks,
new browser actions/locators, generalized actionability, cross-browser support, arbitrary-precision
or decimal numbers, casts, nullable narrowing, optional chaining/defaulting, or another provider,
locator, runtime, or adapter implementation.

It also does not revisit D.8 deadlines, capability allocation, provider-builder ordering, artifact
persistence, cleanup policy, accessible-name completeness, raw-byte redaction, or repository/CI
polish.

## 9. Verification

```sh
cargo test -p webtest-provider -p webtest-plan -p webtest-analysis -p webtest-runtime
cargo test -p webtest-browser -p webtest-browser-cdp
cargo test -p webtest-editor -p webtest-dap -p webtest-wasm -p webtest
cargo test --release -p webtest-runtime evaluation
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p webtest-wasm --target wasm32-unknown-unknown
```

Run the WASM check when the target is installed.

## 10. Acceptance criteria

1. Passive locator consumers do not prepare or mutate the page; actions scroll only after unique
   resolution and use post-scroll measurements.
2. Every action that physically clicks or relies on a click for focus rejects an obscured target
   before sending mouse, keyboard, or text input.
3. Integer overflow is a structured test failure in every build profile and numeric relations
   preserve exact integer distinctions across the full `i64` range.
4. `Option<T>` compatibility and record presence are sound, and every statically legal optional
   member access returns its value or `Null`.
5. Plan format 3, native/WASM parity, source mappings, structured failure classes, DAP behavior,
   and deterministic event/step ordering remain intact.
6. `webtest describe` exposes the corrected contracts, and no D.8 work or unimplemented roadmap
   feature is duplicated or advertised.
