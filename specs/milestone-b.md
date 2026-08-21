# Milestone B — Useful Browser Testing

**Status: implemented.** 

## 0. Status and dependencies

The canonical syntax-to-runtime path supports the complete language surface below; Chrome execution uses isolated per-test contexts, physical CDP input, bounded actionability polling, and failure evidence. The acceptance coverage lives in the syntax, HIR, plan, runtime, browser-CDP, editor, DAP, project, and reporter test suites.

This specification expands Milestone B in [`future-functionality.md`](./future-functionality.md). It depends on the project, browser-management, reporting, CDP-lifecycle, and packaged-editor guarantees in [`milestone-a.md`](./milestone-a.md).

Milestone B turns the demonstrated browser slice into a dependable form-and-navigation testing tool. Browser semantics remain protocol-neutral; CDP is one implementation of those semantics.

## 1. Outcome

Users can express common web flows with semantic locators, input actions, deterministic waits, state assertions, isolated browser contexts, and failure screenshots:

```webtest
test "user signs in" {
    browser {
        open "/login"
        fill label("Email") with "alice@example.com"
        fill label("Password") with "secret"
        click role("button", name: "Sign in")
        expect text("Welcome, Alice").visible within 5s
        expect url("/dashboard")
    }
}
```

No manual sleeps or CSS selectors are required for normal application flows.

## 2. Scope

Milestone B includes:

- role, label, text, placeholder, test-ID, CSS, and XPath locators;
- click, fill, type, press, check, uncheck, select, and hover actions;
- locator, URL, navigation, and bounded wait operations;
- actionability checks and auto-waiting;
- browser-process reuse with a fresh isolated context per test;
- viewport and base-URL configuration needed for repeatable tests;
- screenshots and bounded core evidence on failure;
- source-mapped runtime observations and DAP behavior for all new steps.

## 3. Non-goals

This milestone does not add general bindings or a static type system, `server {}`, application bridges, request mocking, multi-browser support, visual snapshot approval, parallel tests, retry/race blocks, modules, or a trace viewer. CSS and XPath are escape hatches, not the recommended authoring model.

## 4. Language surface

### 4.1 Locators

The shared locator model becomes:

```rust
enum Locator {
    Id(String),
    Role { role: String, name: Option<String> },
    Label(String),
    Text(String),
    Placeholder(String),
    TestId(String),
    Css(String),
    XPath(String),
}
```

Illustrative syntax:

```webtest
id("submit")
role("button", name: "Submit")
label("Email")
text("Welcome")
placeholder("Search products")
test_id("cart-count")
css("main > form button.primary")
xpath("//button[@type='submit']")
```

Locator string arguments retain their decoded semantic value and precise token/source range. The formatter preserves the established canonical call style. Half-typed locators recover without consuming the enclosing browser block.

### 4.2 Actions and waits

```webtest
open "/settings"
click role("button", name: "Edit profile")
fill label("Display name") with "Alice"
type label("Biography") with "hello"
press label("Search") key "Enter"
check label("Email notifications")
uncheck label("SMS notifications")
select label("Timezone") option "America/Chicago"
hover text("Account")

expect role("dialog", name: "Profile").visible
expect label("Email notifications").checked
expect text("Saved").visible within 5s
wait url("/settings")
```

Exact punctuation may evolve during grammar work, but the semantic operations and source mappings are normative. `wait` is an explicit runtime operation. `expect ... within` is an assertion with an overridden deadline; neither compiles to an arbitrary sleep.

### 4.3 Configuration

Milestone B extends the Milestone A configuration:

```toml
[browser]
base_url = "http://127.0.0.1:3000"
viewport = { width = 1440, height = 900 }
test_id_attribute = "data-testid"

[timeouts]
action = "5s"
assertion = "5s"
navigation = "30s"

[evidence]
screenshot = "on-failure"
dom_snapshot = "on-failure"
max_dom_bytes = 1048576
```

Relative URLs resolve against `browser.base_url`. Missing base URLs are static/configuration errors for relative `open` operations. Timeouts must be positive and bounded by the enclosing test deadline.

## 5. Locator semantics

All locators resolve to a structured result:

```text
NoMatch { evidence }
OneMatch { element }
ManyMatches { count, bounded_candidates }
```

Actions and singular assertions require exactly one match unless an operation explicitly defines collection behavior. There is no implicit "first match" fallback.

### 5.1 Matching rules

- `id(value)` matches the exact DOM `id` value.
- `role(role, name)` uses the accessibility role and, when supplied, computed accessible name. Role and name comparison are exact after documented whitespace normalization; matching is case-sensitive in version 1.
- `label(value)` resolves the form control associated by `for`, nested labeling, or accessible labeling semantics. It returns the control, not the label node.
- `text(value)` matches rendered user-facing text after collapsing HTML whitespace and trimming edges. It does not match script/style content. Prefer the smallest actionable element whose normalized text satisfies the exact value.
- `placeholder(value)` matches the exact placeholder attribute on supported controls.
- `test_id(value)` matches the configured test-ID attribute exactly.
- `css(value)` uses DOM query-selector semantics and reports invalid selectors as structured locator errors.
- `xpath(value)` evaluates against the document and accepts element results only.

Accessibility behavior must be tested against the pinned managed Chrome version. Candidate evidence is bounded and redacted before it reaches observations or reporters.

## 6. Actionability and auto-waiting

### 6.1 Common algorithm

Actions poll until their deadline using a monotonic clock:

1. resolve the locator;
2. require exactly one attached element;
3. evaluate operation-specific actionability;
4. scroll the element into view if needed;
5. re-resolve or validate identity after layout changes;
6. calculate an interaction point when applicable;
7. dispatch the operation through the browser backend;
8. observe immediate navigation/page lifecycle signals without adding an unbounded implicit wait.

Polling uses bounded backoff and is cancellation-aware. Transient zero/multiple matches may retry until the deadline. On timeout, the final structured failure records every actionability condition observed in bounded summary form.

### 6.2 Conditions

`click`, `hover`, and pointer-based actions require the element to be attached, visible, stable across consecutive layout samples, enabled, and able to receive pointer input at the chosen point. They use CDP `Input` events rather than JavaScript `element.click()`.

`fill` requires an editable input, textarea, or contenteditable target. It focuses, replaces existing content, inserts the requested value, and produces the browser events expected from user input. `type` appends keystrokes without clearing. Password values are secret evidence by default.

`press` validates a documented key/chord grammar and dispatches key-down/up events. `check`/`uncheck` require a checkbox-like control and become no-ops when already in the requested state. `select` requires a select-like control and diagnoses missing or ambiguous option values. Every action preserves a provider-neutral request/response shape in `browser`.

### 6.3 Failures

Keep distinct error codes for:

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

Locator/actionability failures are test failures with source-mapped observations. Disconnects, crashes, and malformed CDP messages remain infrastructure failures.

## 7. Assertions and waits

Required locator states are `visible`, `hidden`, `attached`, `detached`, `enabled`, `disabled`, `checked`, and `unchecked`. Positive and negative state assertions poll until their deadline. A hidden assertion succeeds if the element is absent or present but not rendered; a detached assertion requires absence from the DOM.

`expect url(value)` and `wait url(value)` compare the normalized current URL to an expected absolute URL or a relative value resolved against `browser.base_url`. They use bounded polling and preserve expected/actual URLs on failure. General URL member expressions such as `url().path` arrive with the expression/type system in Milestone C.

Every assertion is a distinct HIR and `TestPlan` node. Runtime evidence includes the final state, elapsed duration, locator result, and bounded candidate details. Assertions never become opaque callbacks in the CDP crate.

## 8. Browser process, context, and page lifecycle

Reuse one Chrome process within a `webtest test` invocation, but create a fresh isolated browser context for every test. Each test starts with one page and receives isolated cookies, cache, local/session storage, permissions, and service-worker state to the degree supported by CDP.

The browser abstraction should evolve toward:

```text
BrowserHost
  -> BrowserSession
      -> BrowserContext
          -> Page
```

`runtime` owns semantic lifetimes; `browser-cdp` maps them to CDP targets/context IDs. Test completion or cancellation closes pages and context before the next test. A failed cleanup taints the shared process; the runtime discards it and launches a fresh browser for subsequent tests.

Milestone B remains sequential, so process reuse must not imply parallel context scheduling.

## 9. Evidence and artifacts

On a browser action/assertion failure, capture when available:

- a viewport screenshot;
- current URL and document title;
- bounded locator candidates and actionability facts;
- a bounded DOM or accessibility snapshot;
- relevant console errors observed during the step.

Evidence capture is best-effort and must not replace the original failure. Capture failures are attached as secondary facts. Artifact names derive from stable project/test/step IDs plus execution identity, never raw test names alone. Paths remain inside the configured artifact directory.

Screenshots are PNG and are not yet visual baselines. DOM/accessibility content follows configured size limits and redaction rules.

## 10. Architecture and lowering

Every feature follows the canonical vertical path:

```text
tokens/grammar -> typed AST -> HIR -> analysis -> TestPlan
               -> browser traits -> runtime -> browser-cdp
               -> events/observations -> editor/LSP/DAP/reporters
```

`plan` should represent browser operations explicitly, for example:

```text
Navigate { url, wait_policy }
Click { locator, timeout }
Fill { locator, value, timeout }
Press { locator, key, timeout }
Check { locator, checked, timeout }
Select { locator, option, timeout }
Hover { locator, timeout }
WaitForLocator { locator, state, timeout }
WaitForUrl { predicate, timeout }
AssertLocator { locator, matcher, timeout }
AssertUrl { matcher, timeout }
```

Each node carries deterministic `StepId`, `SyntaxOrigin`, and `SourceRevision`. `browser-cdp` does not receive syntax nodes and editor/LSP code does not inspect locator source text.

## 11. DAP and editor behavior

Semantic tokens cover new keywords and locator/action names from shared CST tokens. Static diagnostics detect malformed locator signatures and invalid literal configuration where possible.

Every executable action, wait, and assertion is a valid breakpoint target. DAP pauses before the operation, leaving the previous page state inspectable. The stopped frame shows the normalized operation and locator without exposing secret fill values. Continue and step use the same `RunControl` path as existing operations.

Runtime observations are revision-bound and underline the smallest useful range: usually the locator for resolution/actionability failure, the expected value for mismatch, or the URL expression for navigation failure.

## 12. Delivery slices

1. Add semantic locator syntax/AST/HIR/plan variants and fake-browser tests.
2. Implement one structured locator resolver per semantic locator in `browser-cdp`.
3. Add cancellable auto-wait primitives and locator-state assertions.
4. Replace JavaScript click with CDP input-based actionability.
5. Add fill/type/press, then check/uncheck/select/hover.
6. Add URL/navigation waits and page-state assertions.
7. Introduce per-test isolated contexts and safe browser-process reuse.
8. Add screenshots/evidence, reporter links, editor observations, and DAP coverage.
9. Expand examples into representative form, navigation, ambiguity, timeout, and failure cases.

Each slice must be usable end to end before the next action family is added.

## 13. Testing requirements

Required coverage includes:

- lossless valid, invalid, and half-typed syntax for every locator/action/assertion;
- exact AST/HIR/plan source ranges and deterministic step IDs;
- fake-browser runtime tests for transient absence, ambiguity, timeout, cancellation, and success;
- browser conformance tests shared by all future backends;
- real-Chrome tests for accessibility names, labels, whitespace, shadow/iframe limitations, scrolling, overlays, stability, editing, key chords, controls, and navigation;
- isolated-context tests proving cookies/storage do not leak between tests;
- artifact naming, redaction, size-limit, and capture-failure tests;
- observation clearing after a successful rerun;
- Unicode UTF-8/LSP UTF-16/DAP line mapping for new operations;
- headed DAP breakpoint tests before fill, click, wait, and assertion steps.

Real browser fixtures bind random loopback ports. Time-dependent tests use injected clocks in core runtime logic wherever possible.

## 14. Acceptance criteria

Milestone B is complete only when:

1. The reference sign-in flow runs without sleeps or CSS selectors.
2. Clicks use physical CDP input after all documented actionability checks.
3. Missing, ambiguous, obscured, disabled, unstable, and timed-out elements produce distinct source-mapped failures with bounded evidence.
4. Two sequential tests sharing a Chrome process cannot observe each other's cookies or storage.
5. Failed browser steps produce deterministic screenshot/evidence artifacts and links in human/JSON results.
6. All new steps can be paused before execution in headed DAP sessions.
7. Browser conformance, workspace, LSP/DAP, and extension compilation tests pass.

The roadmap acceptance statement is thereby satisfied: common form and navigation flows are reliable without manual sleeps or CSS selectors.
