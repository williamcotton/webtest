# Milestone D.8 — Chrome CDP Backend Decomposition

## 0. Status and dependencies

**Status: implemented (2026-08-30).**

Implementation decomposes the native CDP adapter behind the unchanged
`webtest_browser_cdp::{ChromeHost, find_system_chrome}` facade. Process/profile ownership, generic
wire envelopes, the single bounded correlation actor, session/context/target setup, and the page
evaluation, navigation, locator, action, evidence, redaction, and inspection responsibilities now
have the explicit owners defined below. The focused crate suite increased from the original 13
mixed tests to 36 owner-focused and cross-layer tests, while retaining the available real-Chrome
startup, context isolation, physical input, inspection, evidence, actionability, disconnect,
reaping, and profile-cleanup coverage.

This maintenance milestone follows the implemented browser foundations in
[`milestone-b.md`](./milestone-b.md), the implemented semantic inspection and machine-feedback
work in [`milestone-c-5.md`](./milestone-c-5.md), the implemented application bridge in
[`milestone-d.md`](./milestone-d.md), and the decomposition sequence in
[`milestone-d-5.md`](./milestone-d-5.md), [`milestone-d-6.md`](./milestone-d-6.md), and
[`milestone-d-7.md`](./milestone-d-7.md). It reorganizes the existing direct Chrome DevTools
Protocol backend before later browser-event, typed-protocol, trace, actor, checkpoint, and
cross-browser work from [`future-functionality.md`](./future-functionality.md).

It does not implement those roadmap capabilities.

The current `crates/browser-cdp/src/lib.rs` is 3,042 lines and is the crate's only source file. It
combines:

- the complete public `ChromeHost` facade and system-Chrome discovery;
- owned Chrome launch, startup detection, shutdown, reaping, and temporary-profile cleanup;
- WebSocket connection ownership, bounded command submission, request/response correlation,
  command deadlines, disconnect handling, protocol-envelope decoding, and limited console-error
  collection;
- browser-session, isolated-context, target, flattened CDP session, page, and viewport setup;
- navigation and raw JavaScript evaluation;
- every locator resolver and state predicate;
- pre-action polling, physical mouse/keyboard/text input, check/uncheck, and select behavior;
- screenshot, page-state, DOM, candidate, actionability, and console evidence collection;
- semantic page inspection, locator-candidate validation, deterministic preference ordering,
  redaction, truncation, and inspection DTO construction;
- embedded JavaScript programs for resolution, selection, DOM evidence, and inspection;
- thirteen mixed pure, fake-WebSocket, process, and real-Chrome tests.

All of these responsibilities belong in the native CDP adapter, but they do not belong in one Rust
module. The refactor must make ownership visible without moving CDP types or browser mechanics into
`browser`, `runtime`, `app`, `editor`, `lsp`, or `dap`.

At the time this plan was drafted:

- `cargo test -p webtest-browser-cdp` passed all 13 unit/integration-style tests and its doc tests;
- `cargo clippy -p webtest-browser-cdp --all-targets -- -D warnings` passed;
- the crate exposed only `ChromeHost` and `find_system_chrome` at its root;
- `webtest` was the only production reverse dependency, while `webtest-editor` used the crate as a
  dev-dependency for real-browser vertical coverage.

Several real-Chrome tests intentionally return early when Chrome or a loopback listener is
unavailable. A green result is therefore a starting baseline, not proof that every backend path was
executed. D.8 adds focused characterization before moving high-risk lifecycle, correlation,
JavaScript-generation, redaction, or ordering code.

## 1. Outcome

`crates/browser-cdp/src/lib.rs` becomes a small, stable public facade. It declares private modules
and re-exports the same two root-level public symbols. It contains no process loop, WebSocket actor,
CDP envelope, target/session setup, page operation, embedded JavaScript, evidence policy,
inspection mapping, or mixed test suite.

The intended ownership flow is:

```text
ChromeHost
   |
   +-- discovery.rs -------- explicit/env/system executable lookup
   |
   +-- process.rs ---------- owned Chrome child + temporary profile
   |
   +-- connection.rs ------- one bounded CDP command/correlation actor
   |      |
   |      +-- wire.rs ------ generic CDP envelopes and response helpers
   |
   +-- session.rs ---------- browser session, contexts, targets, page setup
          |
          +-- page/
                 evaluation ---- Runtime.evaluate boundary
                 navigation ---- Page.navigate and URL waits
                 locator ------- resolver scripts, snapshots, state waits
                 actions ------- pre-action polling and physical input
                 evidence ------ best-effort failure evidence
                 inspection ---- semantic inspection and candidates
                 redaction ----- shared bounded/redacted page output
```

There remains exactly one native Chrome path:

```text
BrowserHost::start
  -> locate one executable
  -> launch one owned Chrome process with one fresh profile
  -> connect one CdpConnection to the browser WebSocket
  -> create CdpBrowserSession
      -> optionally create isolated CdpBrowserContext values
      -> create and attach flattened page targets
      -> expose CdpPage only as Box<dyn webtest_browser::Page>
```

The final `lib.rs` should normally remain below 50 lines. This is a design target, not permission to
hide the current file behind a macro, `include!`, generated source, or one replacement module of
similar size. New production modules should normally remain below 500 lines. Embedded JavaScript
counts toward that review target because it is executable browser semantics, not inert data.

The public crate path remains `webtest_browser_cdp::{ChromeHost, find_system_chrome}`. Downstream
crates must not change imports merely because internal ownership improved.

## 2. Research baseline

### 2.1 Current responsibility map

The current file divides approximately as follows:

| Lines | Current responsibility |
|---|---|
| `1–42` | crate documentation, imports, timeouts, correlation sweep, and snapshot identity |
| `44–103` | public `ChromeHost`, builder methods, executable lookup, `BrowserHost`, and `Default` |
| `105–264` | Chrome child launch, `DevToolsActivePort` polling, shutdown, reaping, profile cleanup, and `Drop` fallback |
| `266–527` | CDP connection/envelopes, bounded command actor, correlation, deadlines, incoming events, and console errors |
| `528–674` | browser session/context implementations plus target attachment, domain enablement, and viewport setup |
| `675–1058` | `Page` trait implementation: navigation, evaluation, actions, waits, URL state, evidence, and inspection |
| `1059–1500` | page evaluation, locator resolution, inspection mapping, pre-action polling, physical input, key input, and selection |
| `1501–1580` | locator snapshot/rectangle DTOs and state predicates |
| `1580–1678` | raw inspection DTOs, field bounds, secret replacement, and URL-query redaction |
| `1679–1905` | inspection, resolver, and locator-array JavaScript generation plus attribute validation |
| `1906–2022` | CDP evaluation extraction, UTF-8 truncation, evidence redaction, key parsing, and response-field helpers |
| `2023–2046` | platform-specific system Chrome discovery |
| `2047–3042` | fake CDP helpers and thirteen mixed tests |

These ranges describe the current implementation only. They are not future module boundaries and
must not be preserved mechanically when a more cohesive owner is specified below.

### 2.2 Current public consumers

| Consumer | CDP backend surface used |
|---|---|
| `crates/app/src/commands/test.rs` | constructs `ChromeHost`, applies headed and configured timeout settings, and injects it into the shared runtime |
| `crates/app/src/commands/inspect.rs` | starts `ChromeHost`, creates an isolated context/page, navigates, inspects, and explicitly closes context/session |
| `crates/app/src/commands/lsp.rs` | injects a configured `ChromeHost` into the shared editor/LSP composition |
| `crates/app/src/commands/dap.rs` | injects a headed-by-default `ChromeHost` into DAP composition |
| `crates/app/src/chrome.rs` | uses `find_system_chrome` only after explicit, environment, configuration, and managed-browser resolution |
| `crates/app/tests/protocol.rs` | uses `find_system_chrome` as the final real-browser test fallback |
| `crates/editor` tests | use `ChromeHost::default`, `locate`, and the protocol-neutral browser traits for a real-browser vertical test |

`browser`, `runtime`, `editor`, `lsp`, and `dap` do not depend on CDP implementation types in
production. `app` is the native composition root and is authorized to select this backend.

### 2.3 Current executable resolution and host defaults

`ChromeHost` currently preserves the following behavior:

- `ChromeHost::new(executable)` defaults to headless operation, a 10-second CDP command timeout,
  and a 15-second navigation timeout;
- `with_headed(true)` sets `headless` to false, while `with_headed(false)` restores it to true;
- `with_timeouts(command, navigation)` replaces both values exactly, including zero durations;
- `locate` checks the constructor-supplied path, then `WEBTEST_CHROME_PATH`, then fixed supported
  system locations;
- `locate` returns a path without canonicalizing or validating it; app-level Chrome resolution owns
  the richer explicit/configured/managed precedence and existence validation;
- system lookup checks Google Chrome then Chromium on macOS, four fixed paths on Linux, and no
  candidates on Windows or other targets;
- `start` reports one stable launch message when no executable can be found.

The browser-manager crate remains separate. D.8 must not duplicate its download, cache, version,
checksum, or resolution policy inside `browser-cdp`.

### 2.4 Current Chrome process and profile lifecycle

The current process contract is security-sensitive:

- every launch creates a new `tempfile::TempDir` and passes it as `--user-data-dir`;
- remote debugging binds to `127.0.0.1` and requests a random port with
  `--remote-debugging-port=0`;
- the default personal Chrome profile is never reused;
- `--no-sandbox` is not supplied;
- headless mode adds `--headless=new`, while headed mode omits it;
- startup also passes no-first-run, no-default-browser, no-startup-window, and foreground-speed
  flags needed by the current debug/editor experience;
- stdin/stdout/stderr are null and `kill_on_drop(true)` is enabled;
- startup polls the owned profile's `DevToolsActivePort` every 25 ms for at most 10 seconds;
- a child exit before two complete port-file lines becomes `BrowserCrashed`; malformed file content
  and file I/O remain launch failures;
- the WebSocket URL is derived as `ws://127.0.0.1:<port><path>` from the two file lines;
- explicit shutdown first waits up to two seconds for Chrome to exit after `Browser.close`; if it
  does not, it calls `start_kill` and then performs a bounded wait to reap the child;
- profile removal retries for up to two seconds and treats an already-missing directory as success;
- `Drop` performs best-effort kill and profile cleanup, using the current Tokio runtime when one is
  available and synchronous removal otherwise.

Tokio documents that dropping a child does not normally imply cancellation, that `kill_on_drop`
only requests termination, and that `start_kill` must be followed by `wait`/`try_wait` to avoid a
zombie on Unix. The current explicit shutdown path follows that ownership requirement. The D.8
extraction must keep reaping visible rather than treating `kill_on_drop` as sufficient.

Chrome's current remote-debugging guidance requires a non-default `--user-data-dir` for ordinary
Chrome remote debugging and recommends Chrome for Testing for automation. The existing fresh
profile satisfies the isolation requirement. The refactor must preserve it.

### 2.5 Current CDP command and correlation model

`CdpConnection` owns one WebSocket split into a writer and reader inside one spawned actor. Cloned
connection handles submit work through a bounded Tokio MPSC channel of capacity 32 and receive one
result through a oneshot channel.

For each accepted command, the actor:

1. allocates a nonzero wrapping `u64` request ID;
2. serializes a generic CDP envelope containing method, optional params, and optional flattened
   `sessionId`;
3. inserts one `PendingCommand` keyed by ID before writing;
4. increments the in-flight counter;
5. bounds the WebSocket send by the command's remaining deadline;
6. correlates an incoming response by ID;
7. verifies that its `sessionId` exactly equals the pending command's expected session;
8. returns either `result`, a typed CDP protocol error, or a malformed-protocol error;
9. removes the pending entry and decrements the in-flight count exactly once.

The deadline is created before the bounded sender finishes enqueueing, so queue time is currently
part of the command deadline. A 10 ms sweep removes expired commands and commands whose receiver
has been dropped. A terminal socket read/write error, close frame, binary frame, malformed JSON, or
closed command channel ends the actor and fails every remaining pending command with the same
terminal error. Unknown response IDs are warned and ignored. Out-of-order responses remain valid.

Incoming events are not generally routed. The actor currently retains at most twenty connection-
wide console entries:

- `Runtime.exceptionThrown` contributes a bounded exception text;
- `Runtime.consoleAPICalled` with type `error` contributes the literal `console.error`;
- every other event is ignored.

This is deliberately not the future normalized event system. D.8 isolates the actor and wire
boundary but does not add event subscribers, per-page journals, network events, request
cancellation tokens, protocol barriers, generated method types, or trace capture.

### 2.6 Current browser session, context, target, and close behavior

One `CdpBrowserSession` owns the optional `ChromeProcess`, a clonable `CdpConnection`, and the
navigation timeout.

- `BrowserSession::new_page` creates a target in the default browser context.
- `BrowserSession::new_context` calls `Target.createBrowserContext` and retains the supplied
  viewport and test-ID attribute.
- each isolated context calls `Target.createTarget` with its context ID;
- every page target is attached with `flatten: true` and uses the returned `sessionId` on page
  commands;
- `Page.enable` and `Runtime.enable` run before the page is returned;
- `Emulation.setDeviceMetricsOverride` applies the configured width and height with device scale
  factor 1 and `mobile: false`;
- a context records target IDs but currently uses them only as local bookkeeping and clears them on
  close;
- context close is idempotent and uses `Target.disposeBrowserContext`, which closes the context's
  pages;
- requesting a page after context close returns a structured protocol error;
- session close sends `Browser.close`, then shuts down/reaps the owned process;
- a successful process shutdown converts `BrowserDisconnected` from the graceful close command
  into success, while another graceful CDP error remains an error;
- a process shutdown/profile cleanup error takes precedence over the graceful CDP result.

CDP-specific target IDs, browser-context IDs, session IDs, commands, and raw JSON terminate inside
this crate. Public consumers continue to see only the traits and DTOs from `webtest-browser`.

### 2.7 Current page, navigation, and evaluation behavior

`CdpPage` retains a connection, flattened session ID, navigation timeout, and configured test-ID
attribute.

- `open` sends `Page.navigate`, maps `errorText` to `NavigationFailed`, then polls
  `document.readyState` every 25 ms until `interactive` or `complete`;
- navigation timeout is independent from the per-command timeout and reports the requested URL;
- `wait_for_url` polls `location.href` every 25 ms and requires exact string equality;
- `current_url` requires a by-value string result;
- `evaluate` uses `Runtime.evaluate` with `returnByValue: true`, checks the current `errorText`,
  exception description, then exception text locations, and returns no value through the `Page`
  trait;
- the internal evaluation helper does not enable `awaitPromise` and does not create an isolated
  execution world;
- `click` and `expect_visible` are compatibility wrappers with fixed five-second waits;
- every page command uses the page's exact flattened session ID.

This milestone must not replace ready-state polling with lifecycle events, reinterpret URL
matching, await arbitrary promises, or introduce execution-context tracking while extracting the
code. Those are behavior changes requiring separate design and tests.

### 2.8 Current locator and state semantics

The resolver builds JavaScript for all current protocol-neutral locator variants:

- `Id` uses `document.getElementById`;
- `Role` scans `body *`, computes the current limited implicit-role/accessibility model, and applies
  exact role and optional exact accessible-name equality;
- `Label` applies the same accessible-name helper to form/editable controls;
- `Text` normalizes rendered whitespace, prefers actionable exact-text nodes, and removes ancestor
  matches when a descendant is the useful leaf;
- `Placeholder` matches the exact attribute on input/textarea elements;
- `TestId` validates the configured attribute name and matches its exact value;
- `Css` uses `querySelectorAll`;
- `XPath` iterates ordered XPath results and keeps element nodes.

All authored/configured values are JSON-serialized before entering JavaScript. The test-ID
attribute is additionally restricted to nonempty ASCII alphanumeric, dash, underscore, or colon
characters before being used in selector syntax. No extraction may reintroduce string
interpolation that bypasses those protections.

Resolution currently:

- operates only in the main document; open shadow roots and iframes are explicitly not traversed;
- returns a match count and at most five bounded candidate-evidence entries;
- scrolls a unique match to the center before measuring it;
- derives visibility, disabled, editable, checked, obscured, tag, rectangle, and document index in
  the evaluated script;
- converts script exceptions into `LocatorInvalid` rather than infrastructure failure.

`wait_for_locator` uses a 20 ms exponential backoff capped at 100 ms. Hidden and detached may pass
with zero matches; other states require exactly one matching element. At deadline, ambiguity,
missing, and single-hidden-visible failures retain their distinct variants before the generic
state assertion fallback.

### 2.9 Current action and input semantics

`perform` first runs the existing pre-action polling loop and then dispatches exactly one current
operation:

- click moves to the measured center, sends mouse pressed, then mouse released;
- hover sends only mouse moved;
- fill clicks, selects all using Meta on macOS or Control elsewhere, sends Backspace, then
  `Input.insertText`;
- type clicks and inserts text without clearing;
- press clicks, parses the supported key chord, then sends key down followed by key up;
- check/uncheck avoids input when the current state already matches, otherwise clicks and resolves
  once more to verify the requested checked state;
- select evaluates one script that matches a unique `<select>`, then a unique option by exact value
  or normalized displayed text, assigns its value, and dispatches bubbling `input` then `change`.

The pre-action loop distinguishes missing, ambiguous, invisible, disabled, non-editable,
obscured, detached, and unstable conditions. Stability is two rectangle samples separated by 50 ms
with a less-than-0.25 tolerance for each coordinate/dimension. If the observed failure code changes
before the deadline, the final failure is `ActionTimeout`; otherwise the final specific error is
retained. Poll backoff starts at 20 ms and caps at 100 ms.

Key parsing accepts Alt, Control/Ctrl, Meta/Command, and Shift modifiers plus the current named-key
set or one Unicode scalar. Modifier bits, key/code/text derivation, unsupported chord behavior, and
input command order are compatibility facts for this refactor.

### 2.10 Current evidence and redaction behavior

Evidence capture is deliberately best effort. It never replaces a primary browser failure with a
secondary capture failure.

- screenshots are optional, requested as PNG/from-surface, and base64-decoded;
- page URL and title are evaluated together;
- locator evidence reuses the ordinary resolver and includes its actionability facts/candidates;
- optional DOM capture clones the document, removes input/textarea value content according to the
  current script, serializes it, and truncates to the requested UTF-8 byte bound;
- connection-wide console errors are copied into the evidence;
- every failed subcapture appends bounded descriptive text to `capture_failures`;
- configured query parameter names are redacted case-insensitively in parsed URLs;
- configured concrete secret strings are replaced in URL, title, DOM, console errors, and candidate
  names/text;
- evidence uses `<redacted>`, while semantic inspection display fields currently use
  `[redacted]`;
- UTF-8 truncation has no suffix and backs up to a character boundary.

The replacement token distinction, replacement order, invalid-URL fallback, field coverage, and
bounds are observable and must not drift incidentally during module extraction.

### 2.11 Current semantic inspection behavior

Inspection first applies `InspectionOptions::bounded` from `webtest-browser`, then executes one
main-document inspection script. It omits scripts/styles/templates, password text, ordinary form
values, nonsemantic layout nodes, and hidden nodes unless requested. It returns bounded raw
role/name/label/placeholder/test-ID/DOM-ID/text, current states, action flags, select options,
document index, page URL/title, total count, and the requested prefix of elements.

Rust mapping then:

1. redacts and bounds every potential author-facing field;
2. refuses to use a redacted or truncated value as locator source;
3. proposes candidates in the fixed order label, role, test ID, DOM ID, placeholder, exact text;
4. validates each candidate sequentially through the ordinary resolver;
5. keeps it only when it resolves uniquely to the same document index;
6. drops an element when no candidate validates;
7. uses the first validated candidate as preferred and bounds the remainder;
8. derives supported actions from the returned state flags;
9. caps select options at 50 and records truncation;
10. constructs inspection schema version 1 with a process-global `snapshot-<n>` identity;
11. obtains `Browser.getVersion` best effort, falling back to `unknown` and truncating to 128 bytes;
12. redacts configured URL query parameters and bounds the page URL to 4,096 bytes.

Preference order and candidate validation are part of the current versioned inspection behavior.
Moving validation into concurrent tasks, changing script semantics, sharing a new accessibility
algorithm, or reordering collections would be a behavior change rather than a decomposition.

### 2.12 Known sharp edges that D.8 must expose, not silently redefine

The following current limitations and edge semantics need characterization or explicit deferral:

- the generic CDP envelopes use `serde_json::Value`; normal methods do not yet use generated typed
  bindings from a pinned protocol schema;
- response IDs wrap without an explicit occupied-ID collision check;
- the in-flight test counter excludes commands still waiting in the bounded submission channel;
- console entries are connection-wide, capped at twenty, and are not cleared per page/context/test;
- most CDP events are intentionally ignored, and no observer/event queue exists;
- a malformed incoming message terminates the entire connection;
- target IDs stored by a context are not individually closed or detached;
- default-context pages have no explicit page-close operation before browser shutdown;
- the navigation wait polls `document.readyState` instead of correlating loader/lifecycle events;
- locator, accessible-name, visibility, and inspection semantics are a bounded main-document model,
  not a complete browser accessibility-tree implementation;
- open shadow roots and frames are not traversed;
- resolver and inspection scripts repeat some semantic JavaScript rather than sharing a generated
  browser bundle;
- inspection candidate validation performs sequential reevaluation and can observe a mutating page;
- snapshot IDs are execution identities, not deterministic semantic identities;
- best-effort `Drop` cleanup cannot report profile-removal failure;
- real-Chrome tests can succeed by returning early when their environment is unavailable.

Initial extraction preserves these facts. A safety defect found while characterizing process leaks,
secret exposure, unsafe JavaScript interpolation, correlation cleanup, or use-after-close must not
be enshrined as intended compatibility. Stop the slice, add a failing regression test, and either:

1. make a narrowly reviewed safety correction with an explicit release note; or
2. document and defer it when the correction requires a protocol/event/lifecycle redesign.

Do not quietly mix such a correction into a file move.

### 2.13 Primary references consulted

The current repository code is authoritative for D.8 behavior. The following primary sources were
used to verify external constraints and terminology:

- the [Chrome DevTools Protocol Target domain](https://chromedevtools.github.io/devtools-protocol/tot/Target/)
  documents flattened `sessionId` routing, browser contexts, target creation, and context disposal;
- the [Browser](https://chromedevtools.github.io/devtools-protocol/tot/Browser/),
  [Page](https://chromedevtools.github.io/devtools-protocol/tot/Page/),
  [Runtime](https://chromedevtools.github.io/devtools-protocol/tot/Runtime/),
  [Input](https://chromedevtools.github.io/devtools-protocol/tot/Input/), and
  [Emulation](https://chromedevtools.github.io/devtools-protocol/tot/Emulation/) domain references
  document the current command families used by this adapter;
- Chrome's official
  [remote-debugging security guidance](https://developer.chrome.com/blog/remote-debugging-port)
  confirms the custom-user-data-directory isolation requirement for modern Chrome;
- Tokio's [`Child` documentation](https://docs.rs/tokio/latest/tokio/process/struct.Child.html)
  documents kill-on-drop and the need to reap a force-killed process;
- Tokio's [channel guidance](https://tokio.rs/tokio/tutorial/channels) documents the bounded MPSC
  plus oneshot response pattern and backpressure requirement used by the current actor;
- the Rust Reference's
  [`use` declaration documentation](https://doc.rust-lang.org/reference/items/use-declarations.html)
  confirms that private implementation modules can support stable root-level public re-exports.

The `tot` CDP pages describe the evolving tip-of-tree protocol. D.8 does not infer new support from
their contents; it preserves only the methods and response shapes already exercised by this code.

## 3. Architectural rules

### 3.1 Preserve the crate boundary and dependency direction

All moved behavior remains in `crates/browser-cdp`. This crate continues to implement the
protocol-neutral contracts from `webtest-browser` and may depend on native process, filesystem,
WebSocket, JSON, and async facilities.

The existing dependency direction remains mandatory:

```text
app -> browser-cdp -> browser -> feedback
```

Do not introduce:

- `browser -> browser-cdp`;
- `runtime`, `editor`, `lsp`, `dap`, `analysis`, `plan`, or `provider` dependencies into
  `browser-cdp`;
- CDP request/response/session/target types in `webtest-browser`;
- raw CDP JSON in runtime errors, observations, reporters, DAP, LSP, or WASM;
- browser-manager download/cache/version policy in this crate;
- a second browser abstraction or Playwright dependency.

No new crate is required. A cross-crate move is out of scope unless a separately reviewed change
proves the behavior is protocol-neutral and belongs to the documented owner.

### 3.2 Preserve one CDP connection actor

There remains one actor per browser WebSocket that owns both socket halves and all correlation
state. Page/session/context modules call `CdpConnection::command`; they do not write raw frames,
allocate IDs, hold response senders, or spawn competing readers.

The actor continues to be the only owner of:

- next command ID;
- pending command map;
- writer and reader;
- timeout/cancel sweep;
- terminal connection error fan-out;
- unknown-response handling;
- the current bounded console-error buffer.

Do not extract one task per command, one reader per page, an unbounded event channel, or a generic
actor framework. The current single-owner model is what makes out-of-order correlation and terminal
failure behavior reviewable.

### 3.3 Keep the public facade stable and internals narrow

`lib.rs` privately declares modules and explicitly re-exports:

```rust
pub use discovery::find_system_chrome;
pub use host::ChromeHost;
```

Preserve `ChromeHost`'s derives, constructor, builder methods, `locate`, `Default`, and
`BrowserHost` implementation. Do not make internal modules public to avoid fixing visibility.

Use the narrowest visibility that works:

- `pub` only for the two existing root-level public items and their public methods;
- `pub(crate)` for constructors/types shared by sibling production modules;
- `pub(super)` for page child-module collaboration;
- private for actor state, wire fields, raw inspection DTOs, snapshots, scripts, and leaf helpers.

Public re-export tests must use the root path. Internal tests should live with their owner rather
than broadening production visibility solely for testing.

### 3.4 Organize by lifecycle and browser responsibility

Modules own cohesive reasons to change:

- `discovery` owns fixed system executable candidates;
- `host` owns public configuration and composition of launch + connect + session;
- `process` owns the child and temporary profile from spawn through reap/removal;
- `wire` owns generic CDP serialization/deserialization envelopes and narrow response helpers;
- `connection` owns transport, correlation, deadlines, terminal failure, and current event intake;
- `session` owns browser/context/target/session/page setup and close ordering;
- `page::evaluation` owns `Runtime.evaluate` request/response interpretation;
- `page::navigation` owns navigation and URL polling;
- `page::locator` owns locator JavaScript, snapshots, resolution, and state waits;
- `page::actions` owns pre-action polling and input/selection dispatch;
- `page::redaction` owns page-output secret/query redaction and UTF-8 byte bounds;
- `page::evidence` owns best-effort evidence collection;
- `page::inspection` owns inspection script, raw DTO mapping, candidate validation, and schema DTO
  construction;
- `page/mod.rs` owns `CdpPage` and the protocol-neutral `Page` trait delegation.

Do not add `helpers.rs`, `utils.rs`, `common.rs`, `browser.rs`, `cdp.rs`, or another vague catch-all.
Do not move the entire old file into `backend.rs` or `page.rs`.

### 3.5 Preserve process/resource ownership

Ownership remains linear and visible:

```text
ChromeHost::start
  -> ChromeProcess { Child, TempDir }
  -> CdpBrowserSession { Option<ChromeProcess>, CdpConnection }
       -> CdpBrowserContext { context ID, options }
            -> CdpPage { flattened session ID }
```

Explicit close paths remain primary. `Drop` is only a last-resort safety net. Do not duplicate the
child handle, detach profile ownership, put process state in a global, or let page/context objects
own independent Chrome processes.

Every early return after process spawn must leave the process/profile recoverable through owned
drop behavior. Every explicit session close must attempt both graceful browser close and child
reaping/profile cleanup with the existing precedence.

### 3.6 Preserve protocol-neutral browser semantics

The implementation must continue to consume and return `webtest-browser` types. CDP method names,
session IDs, target IDs, raw response fields, WebSocket frames, and embedded JavaScript remain
private.

There remains one locator implementation in this backend shared by:

- actions;
- locator-state assertions/waits;
- evidence capture;
- semantic inspection candidate validation.

Inspection must not create a second selector evaluator. Evidence must not reconstruct locator
facts independently. App/editor adapters must not inspect the DOM themselves.

### 3.7 Preserve JavaScript safety and exact semantics

Every user-, source-, project-, or page-derived string entering generated JavaScript must continue
to be encoded with `serde_json::to_string` or validated for its exact syntactic position. Never
insert raw locator values, action values, option strings, key text, or test-ID attributes into a
script.

Initial extraction moves embedded programs with minimal textual changes. Do not deduplicate the
current repeated `norm`, implicit-role, accessible-name, or label functions until golden script
tests and real-browser behavior tests can prove equivalence. A shared generated browser script may
be valuable later, but it is a semantic change surface, not a free cleanup.

### 3.8 Preserve bounds, redaction, and deterministic order

The following remain explicit constants or option-driven bounds with focused owners:

- MPSC command capacity 32;
- correlation sweep 10 ms;
- startup 10 seconds, navigation 15 seconds, command 10 seconds, shutdown 2 seconds;
- console errors 20;
- resolver candidate evidence 5;
- inspection limits from `InspectionOptions::bounded`;
- select options 50;
- inspection URL 4,096 bytes;
- browser version 128 bytes;
- exception/protocol text 256 characters;
- requested DOM byte maximum.

Preserve deterministic locator preference and supported-action order. Never introduce hash-map
iteration into author-facing inspection output. Redact before a value can become locator source,
candidate text, machine DTO content, or persisted DOM evidence.

### 3.9 Preserve structured error classification

This crate continues to return `webtest_browser::BrowserError` variants. It must not flatten them to
strings below the app/editor presentation boundary.

Preserve the current distinction among:

- locator/action/assertion failures;
- navigation failures/timeouts;
- command timeouts;
- disconnect/crash/malformed protocol/protocol/launch failures;
- raw evaluation failures.

Method names and current messages used to construct `Protocol`, `Launch`, and
`MalformedProtocol` remain compatible. Runtime/app continue to determine test versus
infrastructure behavior from the shared typed errors.

### 3.10 Refactor before redesign

Each extraction slice first moves coherent code and its tests with minimal signature or control-flow
change. Only after parity may a slice narrow visibility, remove transitional imports, or introduce
a small private constructor needed to preserve field encapsulation.

Do not combine extraction with:

- generated CDP bindings;
- a protocol schema pin/code generator;
- normalized browser events or trace capture;
- command cancellation tokens or protocol barriers;
- page/target auto-attach;
- frame or shadow-root traversal;
- accessibility-tree integration;
- navigation lifecycle redesign;
- action retry/timeout configuration changes;
- browser manager/resolution changes;
- cross-browser/WebDriver BiDi work.

## 4. Scope

This milestone includes:

- characterization tests for the public facade and every high-risk current boundary;
- extraction of system executable discovery;
- extraction of Chrome process/profile ownership and cleanup;
- extraction of generic CDP wire envelopes and response helpers;
- extraction of the bounded WebSocket command/correlation actor;
- extraction of browser session, isolated context, target/session setup, and close behavior;
- creation of a page module family organized around evaluation, navigation, locators, actions,
  evidence, inspection, and redaction;
- movement of embedded JavaScript to its semantic owner without changing accepted locators or page
  behavior;
- focused unit/fake-CDP/real-Chrome test placement;
- exact root-level re-exports for the current public API;
- removal of obsolete imports, transitional wrappers, and unnecessarily broad visibility;
- final focused/downstream/workspace formatting, build, test, and Clippy verification;
- a final implementation-status update to this milestone after completion.

## 5. Non-goals

This milestone does not:

- add or remove a public type, method, trait implementation, or crate dependency;
- change `ChromeHost` defaults, builder precedence, lookup order, or error messages intentionally;
- move managed Chrome behavior from `browser-manager` or app composition into this crate;
- add Chrome executable candidates or Windows registry discovery;
- change Chrome flags, remote-debugging address/port, profile isolation, or sandbox policy;
- alter startup, command, navigation, action, wait, polling, backoff, stability, or shutdown timing;
- add generated typed CDP bindings or a pinned protocol schema;
- add a second WebSocket reader, event bus, network/console normalization, trace, or observer API;
- change request-ID allocation, queue capacity, correlation, timeout, disconnect, or terminal-error
  semantics;
- add target discovery, auto-attach, page-close, frame/session recursion, or worker support;
- change locator matching, accessible-name approximation, visibility, obstruction, stability, or
  state predicates;
- add shadow DOM or iframe support;
- change physical input ordering, supported keys, select matching, or check verification;
- change evidence fields, screenshot/DOM policy, failure tolerance, redaction tokens, URL handling,
  or byte/collection limits;
- change inspection schema version, snapshot identity, candidate preference/validation, action
  ordering, truncation flags, or serialization;
- add retries, general actionability syntax, parallelism, actors, cancellation, checkpoints, guards,
  or any other Milestone E/Future Functionality construct;
- expose CDP IDs/shapes to `browser`, runtime, observations, CLI reports, editor protocols, or WASM;
- split `browser-cdp` into additional crates;
- edit generated editor output or add TypeScript browser semantics.

Any desirable behavior change found during extraction is documented separately unless it is an
explicitly reviewed safety correction under section 2.12.

## 6. Compatibility contract

### 6.1 Root-level public API

Preserve these exact paths and signatures:

```rust
pub struct ChromeHost;

impl ChromeHost {
    pub fn new(executable: Option<PathBuf>) -> Self;
    pub fn with_headed(self, headed: bool) -> Self;
    pub fn with_timeouts(self, command: Duration, navigation: Duration) -> Self;
    pub fn locate(&self) -> Option<PathBuf>;
}

impl Default for ChromeHost;
impl webtest_browser::BrowserHost for ChromeHost;

pub fn find_system_chrome() -> Option<PathBuf>;
```

Preserve `ChromeHost: Clone + Debug + Send + Sync` as currently derived/implied. Keep fields private.
Do not expose internal constructors, processes, connections, sessions, contexts, pages, snapshots,
wire DTOs, or scripts.

### 6.2 Host configuration and executable selection

Preserve:

- headless default and reversible `with_headed` semantics;
- command/navigation default values and exact builder assignment;
- explicit constructor path over environment over system lookup;
- fixed candidate order per supported platform;
- no filesystem validation inside `locate`;
- app-level managed-browser and configuration precedence;
- exact missing-Chrome launch classification/message;
- no browser launch until `BrowserHost::start` is awaited.

### 6.3 Launch, startup, shutdown, and profile cleanup

Preserve:

- a fresh profile for every process;
- local-only random-port remote debugging;
- headed/headless flag behavior and every current nonsecurity startup flag;
- null stdio and no incidental stdout logging;
- `kill_on_drop(true)` plus explicit reap on normal close;
- `DevToolsActivePort` path, two-line readiness condition, polling interval, deadline, and error
  classification;
- early child-exit reporting as `BrowserCrashed` with status;
- graceful `Browser.close` before forced termination;
- shutdown error precedence and disconnect-as-success case;
- idempotent profile take/removal, retry deadline, not-found success, and final path in errors;
- best-effort asynchronous/synchronous `Drop` cleanup without panicking.

### 6.4 Connection, wire, and correlation behavior

Preserve:

- generic JSON request/response envelopes and serde field names;
- request IDs starting at 1 and remaining nonzero after wrapping;
- optional params omission and flattened `sessionId` encoding;
- bounded MPSC capacity and oneshot per command;
- deadline creation before queue admission;
- pending insertion/in-flight increment before socket send;
- bounded writer send and periodic expiration sweep;
- exact ID correlation under out-of-order responses;
- exact response session matching, including `None` versus `Some`;
- CDP error code/message rendering with the original method;
- missing result/error as malformed protocol;
- unknown IDs warned and ignored;
- text-only incoming protocol, ping/pong/frame ignore behavior, and binary rejection;
- malformed JSON/close/read/write/channel termination fan-out to all pending calls;
- cancellation cleanup when a command receiver is dropped;
- the current trace metric and test-only in-flight observability;
- at most twenty current console-error summaries.

### 6.5 Session, context, target, and page setup

Preserve exact command order and parameters:

```text
Target.createBrowserContext                 isolated contexts only
Target.createTarget about:blank             optional browserContextId
Target.attachToTarget flatten=true
Page.enable                                 page sessionId
Runtime.enable                              page sessionId
Emulation.setDeviceMetricsOverride          configured viewport, scale 1, mobile false
```

Preserve default-context `new_page`, isolated-context options, target bookkeeping, page session
identity, context-close idempotence, use-after-close error, `Target.disposeBrowserContext`, and
session close/process ownership.

### 6.6 Navigation and evaluation

Preserve:

- `Page.navigate` response error mapping;
- ready-state polling expression, accepted states, interval, and navigation deadline;
- command timeout remaining independently active inside navigation polling;
- exact URL polling/equality and final `UrlMismatch` values;
- `Runtime.evaluate` with `returnByValue: true`;
- result extraction at `/result/value`;
- evaluation exception lookup precedence and `EvaluationFailed` fields;
- missing/wrong result types as `Protocol { method: "Runtime.evaluate", ... }`;
- no promise awaiting, execution-world selection, or adapter-visible evaluation value.

### 6.7 Locator generation, resolution, and state waits

Preserve every locator's exact matching semantics, safe serialization, main-document scope,
candidate bound, DOM traversal order, whitespace normalization, accessible-name approximation,
scroll behavior, rectangle/state facts, and invalid-selector mapping.

Preserve `state_satisfied` for all eight states, including zero-match hidden/detached behavior.
Preserve wait backoff and the exact failure-precedence rules at deadline.

Golden tests should compare scripts after normalizing only formatting introduced by an intentional
raw-string move. Prefer byte-for-byte script preservation during the extraction.

### 6.8 Actions and physical input

Preserve:

- action-to-required-state checks;
- two-sample rectangle stability and tolerance;
- changing-failure-code conversion to `ActionTimeout`;
- fixed wrapper timeouts for `click` and `expect_visible`;
- mouse move/press/release order and coordinates;
- fill/type/press/check/select subcommand order;
- macOS versus non-macOS select-all modifier;
- key aliases, modifier bits, named keys, Unicode scalar behavior, and invalid chords;
- already-correct check/uncheck no-op;
- post-click checked-state verification;
- select option matching, ambiguous/missing error distinctions, and input/change event order.

### 6.9 Evidence, bounds, and redaction

Preserve:

- best-effort capture returning `PageEvidence` rather than `Result`;
- screenshot request/PNG decode and capture-failure wording;
- page state, locator snapshot, DOM, and console collection order;
- DOM sanitization expression and requested byte maximum;
- resolver reuse for evidence;
- all currently redacted fields and case-insensitive query parameter matching;
- `<redacted>` evidence token versus `[redacted]` inspection token;
- secret replacement before returned evidence;
- valid UTF-8 byte truncation without suffix;
- capture failures remaining secondary and never erasing prior successful fields.

### 6.10 Semantic inspection

Preserve:

- `InspectionOptions::bounded` as the first options operation;
- raw script filtering, field derivation, main-document ordering, returned prefix, and total count;
- omission of password/control values and raw DOM from the inspection DTO;
- redaction/bounds before locator-source eligibility;
- fixed candidate proposal and validation order;
- sequential validation through the ordinary resolver and same-document-index requirement;
- omission of elements with no validated locator;
- preferred/alternate split and configured candidate bound;
- exact supported-action order and state applicability;
- select option cap/truncation behavior;
- schema/kind values and all truncation flags;
- browser-version best-effort fallback;
- URL/title redaction/bounds;
- snapshot ID prefix and process-global atomic allocation;
- no synthesized CSS or XPath candidate.

### 6.11 Error and logging behavior

Preserve every current `BrowserError` variant selected by this backend, method field, relevant
message, locator/value payload, and infrastructure classification as defined by `webtest-browser`.

Tracing remains through `tracing`; no `println!`/`eprintln!` is added to protocol code. App-owned LSP
and DAP stdout remains exclusively framed protocol output.

### 6.12 Downstream parity

After the refactor:

- app `test`, `inspect`, `lsp`, and `dap` construct the root `ChromeHost` without import changes;
- app Chrome resolution still calls root `find_system_chrome` as the final fallback;
- runtime still sees only `BrowserHost`/`BrowserSession`/`BrowserContext`/`Page` trait objects;
- editor and app protocol tests compile without internal CDP imports;
- reporter error codes and structured runtime observations remain unchanged;
- managed browser installation/cache behavior remains solely in `browser-manager` and app;
- no portable/WASM crate gains a native browser dependency.

## 7. Target module layout

```text
crates/browser-cdp/src/
  lib.rs
  discovery.rs
  host.rs
  process.rs
  wire.rs
  connection.rs
  session.rs
  page/
    mod.rs
    evaluation.rs
    navigation.rs
    locator.rs
    actions.rs
    redaction.rs
    evidence.rs
    inspection.rs
  tests/
    mod.rs
    lifecycle.rs
    page_behavior.rs
```

Tests tightly coupled to one private owner may instead use `#[cfg(test)] mod tests` beside that
owner, for example `connection/tests.rs`, `page/locator/tests.rs`, or `page/redaction/tests.rs`.
The root `tests` family is for real-Chrome behavior spanning process, connection, session, and page
ownership. It is not a dumping ground for pure leaf tests.

### 7.1 `lib.rs`

Owns only:

- crate documentation;
- private module declarations;
- explicit public re-exports;
- the root declaration of cross-cutting real-browser tests when needed.

It contains no glob re-export and no implementation.

### 7.2 `discovery.rs`

Owns:

- `find_system_chrome`;
- platform-specific fixed candidate tables;
- focused candidate-order/platform tests that do not mutate unrelated user files or settings.

It does not read project configuration or managed-browser caches.

### 7.3 `host.rs`

Owns:

- public `ChromeHost` storage and methods;
- `Default`;
- `BrowserHost::start` composition;
- default command/navigation timeout constants if they are host configuration facts;
- construction of `CdpBrowserSession` through narrow internal constructors.

It does not contain child-process mechanics, socket actor logic, target setup, or page behavior.

### 7.4 `process.rs`

Owns:

- `ChromeProcess`;
- startup and shutdown grace constants;
- command construction and current Chrome flags;
- fresh temporary profile creation;
- `DevToolsActivePort` readiness polling and WebSocket URL derivation;
- early-exit detection;
- explicit shutdown/reap/profile cleanup;
- best-effort `Drop` cleanup;
- synchronous not-found-tolerant removal fallback.

The child and profile fields remain private. Tests may use narrow `#[cfg(test)]` accessors rather
than production getters.

### 7.5 `wire.rs`

Owns only generic protocol representation and narrow response-shape helpers:

- serializable command envelope;
- deserializable incoming message and CDP error;
- exact serde names/omission behavior;
- required string-field extraction used during target/context setup;
- bounded protocol diagnostic text if shared by event and evaluated-script errors.

It does not own the socket, pending map, timers, browser semantics, or a generated method catalog.

### 7.6 `connection.rs`

Owns:

- `CdpConnection`;
- outgoing/pending command state;
- bounded sender and command method;
- WebSocket connect/split/spawn;
- one actor loop and correlation map;
- command ID allocation;
- write deadline and expiration sweep;
- incoming response validation;
- terminal failure fan-out;
- unknown response warning;
- current bounded console-error intake;
- in-flight test instrumentation;
- fake WebSocket tests.

The connection may depend on `wire`; `wire` never depends on `connection`.

### 7.7 `session.rs`

Owns:

- `CdpBrowserSession` and its `BrowserSession` implementation;
- `CdpBrowserContext` and its `BrowserContext` implementation;
- private constructors that keep fields encapsulated;
- browser-context creation/disposal;
- target creation and flattened attachment;
- Page/Runtime enablement and viewport emulation;
- session/context/page ownership transfer;
- graceful browser close plus process shutdown result precedence.

It constructs `CdpPage` through a narrow constructor. It does not implement page operations.

### 7.8 `page/mod.rs`

Owns:

- private `CdpPage` storage;
- one narrow constructor from an attached session;
- the single `Page` trait implementation;
- delegation from trait methods to the responsible child module;
- the compatibility five-second wrappers for `click` and `expect_visible`.

The trait implementation remains easy to audit against `webtest-browser::Page`. It must not become
a second 1,000-line page implementation.

### 7.9 `page/evaluation.rs`

Owns:

- the one internal `Runtime.evaluate` call path;
- by-value result extraction;
- author-facing raw evaluate error checks;
- creation of structured `Runtime.evaluate` protocol errors for invalid/missing values.

Locator, navigation, actions, evidence, and inspection all reuse this path.

### 7.10 `page/navigation.rs`

Owns:

- `Page.navigate`;
- ready-state polling and navigation deadline;
- current URL evaluation;
- exact URL wait loop and mismatch construction;
- duration-to-saturated-milliseconds conversion if navigation is its only remaining owner.

It does not subscribe to lifecycle events or interpret redirects beyond current behavior.

### 7.11 `page/locator.rs`

Owns:

- `ResolveSnapshot` and `ElementRect`;
- center/state/actionability summary projection;
- locator resolution through the shared evaluation path;
- `state_satisfied` and locator wait loop;
- resolver and locator-array JavaScript generation;
- locator value JSON serialization;
- configured test-ID attribute validation;
- rectangle stability comparison when shared with actions.

`actions`, `evidence`, and `inspection` receive snapshots by calling this owner. They do not parse
locator scripts or raw response JSON themselves.

### 7.12 `page/actions.rs`

Owns:

- action dispatch;
- pre-action polling and changing-failure tracking;
- physical mouse move/click;
- text insertion and select-all;
- key specification/parsing and key down/up dispatch;
- fill/type/press/check/uncheck semantics;
- select script generation, option result interpretation, and DOM events;
- action timeout conversion.

It depends on `locator` and `evaluation` and does not depend on evidence or inspection.

### 7.13 `page/redaction.rs`

Owns reusable page-output safety primitives:

- UTF-8 byte truncation;
- concrete-secret replacement plus truncation;
- case-insensitive configured query-parameter redaction;
- inspection URL bound;
- evidence field traversal for redaction, if keeping that traversal here produces the clearer
  one-way dependency.

It does not select what evidence or inspection fields exist. Those DTO construction decisions stay
with their owners.

### 7.14 `page/evidence.rs`

Owns:

- screenshot request/decoding;
- page URL/title evaluation;
- locator evidence collection through `locator`;
- DOM sanitization script and requested bound;
- console-error copy;
- ordered secondary capture failures;
- final call to shared redaction.

It always returns `PageEvidence` and never changes the primary operation result.

### 7.15 `page/inspection.rs`

Owns:

- process-global snapshot counter;
- raw inspection DTOs;
- bounded/redacted field eligibility;
- inspection JavaScript generation;
- raw response decoding;
- browser-version lookup/fallback;
- candidate proposal, sequential validation, deduplication, preference, and bounds;
- supported-action/state derivation;
- option and truncation flags;
- final `PageInspection` construction.

It depends on `evaluation`, `locator`, and `redaction`. It does not expose raw inspection DTOs or
invent a selector evaluator.

### 7.16 Test ownership

- wire serialization tests live with `wire`;
- correlation/timeout/disconnect/event-pressure tests live with `connection`;
- locator script/state tests live with `page::locator`;
- key/action ordering tests live with `page::actions`;
- redaction/truncation tests live with `page::redaction` and `page::evidence`;
- inspection mapping/order/bounds tests live with `page::inspection`;
- process/session/page integration tests spanning real Chrome live under `src/tests`.

No production item becomes `pub` solely so an external integration test can reach it.

## 8. Internal dependency direction

The intended production dependency graph is:

```text
lib
  -> discovery
  -> host

host
  -> discovery
  -> process
  -> connection
  -> session

process                 wire
  -> std/tokio/tempfile   -> serde/serde_json/browser errors

connection
  -> wire

session
  -> process
  -> connection
  -> wire
  -> page

page/mod
  -> connection
  -> evaluation
  -> navigation
  -> locator
  -> actions
  -> evidence
  -> inspection

navigation -> evaluation
locator    -> evaluation
actions    -> evaluation, locator
evidence   -> evaluation, locator, redaction
inspection -> evaluation, locator, redaction
redaction  -> url + browser DTOs only
```

Forbidden internal edges include:

- `process -> host/session/page`;
- `wire -> connection/session/page`;
- `connection -> process/session/page`;
- `locator -> actions/evidence/inspection`;
- `actions -> evidence/inspection`;
- `evidence -> inspection` or `inspection -> evidence`;
- page children constructing another WebSocket connection;
- leaf modules depending on `lib.rs` re-exports rather than their real owner;
- tests forcing a production dependency cycle.

If Rust privacy makes this graph awkward, add a narrow constructor or `pub(super)` function at the
owner. Do not make modules public, expose fields, add a universal backend context, or merge modules
back into a monolith.

## 9. Current symbol migration map

| Current symbol/responsibility | Target owner |
|---|---|
| `ChromeHost`, its impls, host timeout defaults | `host.rs` |
| `find_system_chrome` and platform candidates | `discovery.rs` |
| `ChromeProcess`, launch/shutdown/cleanup/`Drop` | `process.rs` |
| `STARTUP_TIMEOUT`, `SHUTDOWN_GRACE` | `process.rs` |
| command envelope, incoming envelope, `CdpError` | `wire.rs` |
| response string extraction | `wire.rs` |
| `CdpConnection`, outgoing/pending commands, actor | `connection.rs` |
| `COMMAND_TIMEOUT`, `CORRELATION_SWEEP` | host/connection according to configuration ownership |
| console-event extraction and bounded buffer | `connection.rs` with wire text helper where needed |
| `CdpBrowserSession`, `CdpBrowserContext`, `create_page` | `session.rs` |
| `CdpPage` and `impl Page` | `page/mod.rs` |
| `evaluate_expression`, evaluation-value/error parsing | `page/evaluation.rs` |
| `open`, `current_url`, `wait_for_url` | `page/navigation.rs` |
| `ResolveSnapshot`, `ElementRect`, state predicates | `page/locator.rs` |
| resolver/locator-array expressions and CSS attribute validation | `page/locator.rs` |
| `perform`, pre-action polling, mouse/key/text/select helpers | `page/actions.rs` |
| `KeySpec`, `parse_key` | `page/actions.rs` |
| evidence capture and DOM evidence expression | `page/evidence.rs` |
| UTF-8 truncation, secret/query redaction | `page/redaction.rs` |
| raw inspection DTOs, bounds, inspection expression/mapping | `page/inspection.rs` |
| `NEXT_SNAPSHOT_ID`, inspection URL limit | `page/inspection.rs` / `page/redaction.rs` by policy owner |
| fake WebSocket helper and connection tests | `connection` test module |
| public facade tests | `host`/`discovery` tests using root paths where required |
| real Chrome lifecycle/page tests | `src/tests/lifecycle.rs`, `src/tests/page_behavior.rs` |

This map assigns ownership, not a required sequence of `git mv` operations.

## 10. Interface design

### 10.1 Public facade

The desired root is conceptually:

```rust
//! Direct Chrome DevTools Protocol implementation of the browser abstraction.

mod connection;
mod discovery;
mod host;
mod page;
mod process;
mod session;
mod wire;

pub use discovery::find_system_chrome;
pub use host::ChromeHost;

#[cfg(test)]
mod tests;
```

Use explicit re-exports. Do not `pub use host::*` or make the implementation modules public.

### 10.2 Host composition

`ChromeHost::start` should remain a short composition operation:

```text
locate
  -> ChromeProcess::launch
  -> CdpConnection::connect
  -> CdpBrowserSession::new
  -> Box<dyn BrowserSession>
```

If connection setup fails after process launch, ordinary Rust ownership must drop the process and
profile safely. Do not add a detached cleanup task that loses the original error.

### 10.3 Connection command boundary

Retain one narrow internal method equivalent to:

```rust
async fn command(
    &self,
    method: &str,
    params: Option<serde_json::Value>,
    session_id: Option<&str>,
) -> Result<serde_json::Value, BrowserError>;
```

The generic method is private to this crate. D.8 does not genericize it over request/response types,
return actor handles, or expose event receivers.

### 10.4 Session/page construction

Prefer narrow constructors over visible fields:

```text
CdpBrowserSession::new(process, connection, navigation_timeout)
CdpPage::new(connection, session_id, navigation_timeout, test_id_attribute)
```

`session` retains command order. `page` begins only after target/session/domain/viewport setup has
completed successfully.

### 10.5 Page trait delegation

Keep exactly one `impl Page for CdpPage`. Each method delegates to a specifically named child
operation. Avoid adding a private trait per page responsibility; inherent/module functions are
sufficient for the current single backend.

The delegation layer is responsible for preserving compatibility wrappers and signatures, not for
reimplementing logic.

### 10.6 Shared locator boundary

`page::locator` exposes only the narrow crate-internal operations required by its siblings:

```text
resolve(page, locator) -> ResolveSnapshot
wait_for_locator(page, locator, state, timeout)
locator_array_expression(locator, test_id_attribute) -> String
```

`ResolveSnapshot` fields should remain private where accessors express the actual collaboration.
Do not turn the raw resolver DTO into a protocol-neutral browser type.

### 10.7 JavaScript program ownership

Keep each script next to the Rust code that decodes its result:

- locator scripts with `ResolveSnapshot` decoding;
- select script with option-result decoding;
- inspection script with `RawInspection` decoding;
- DOM evidence script with DOM evidence extraction.

For each program, tests pair the generated source with its expected JSON result shape. This prevents
one side from moving or changing independently.

### 10.8 Test seams

Test seams remain private and deterministic:

- fake loopback WebSocket server for connection and scripted command tests;
- short injected/internal timeout values where current public `with_timeouts` is sufficient;
- private constructors for contexts/pages over a fake connection where no process is needed;
- real Chrome only for behavior that CDP emulation cannot prove.

Do not add public mock traits, a generic dependency-injection container, feature flags, or a second
transport solely for the refactor.

## 11. Delivery slices

Each slice must compile, format, pass focused tests, and leave root imports usable. Avoid a single
3,000-line move followed by delayed repair.

### Slice 1 — Characterize the facade and safety boundaries

Before moving production code, add focused tests for:

- root imports and `ChromeHost` defaults/builders;
- explicit executable lookup precedence that can be tested without ambient project configuration;
- wire serialization field names/omission;
- response session mismatch and missing result/error;
- command queue/deadline/cancel cleanup;
- process shutdown/reap/profile cleanup paths feasible in the current environment;
- JavaScript escaping for every locator value, option string, and test-ID attribute;
- key parsing and locator state predicates;
- redaction/truncation edge cases and inspection candidate ordering.

Record the existing thirteen-test and Clippy baseline in the implementation change.

### Slice 2 — Extract discovery, host, and process ownership

Move system discovery, `ChromeHost`, and `ChromeProcess` into their target modules. Add narrow
session construction but leave connection/session/page behavior otherwise unchanged.

Verify:

- root public paths compile unchanged;
- no flag or timeout changes;
- a connection failure still cleans the process/profile;
- normal and forced shutdown still reap;
- no new stdout/stderr output;
- app Chrome-resolution tests still pass.

### Slice 3 — Extract wire representation and connection actor

Move envelopes/helpers to `wire` and actor/correlation to `connection` without changing its select
loop or control-flow order.

Verify every fake-CDP test plus new coverage for:

- exact serialized envelopes;
- out-of-order and session-specific responses;
- protocol versus malformed-protocol errors;
- unknown IDs/event pressure;
- timeout, dropped receiver, disconnect, close, malformed text, and binary terminal behavior;
- pending/in-flight cleanup and terminal fan-out;
- console-entry cap/order.

### Slice 4 — Extract session, context, and page shell

Move browser/session/context implementations and target/page setup. Introduce `page/mod.rs` with
the single trait implementation while page internals may temporarily remain in a clearly marked
transitional child module.

Verify exact fake-CDP command sequences, params, browser-versus-page session IDs, viewport values,
context isolation, idempotent close, use-after-close, graceful/disconnect close behavior, and real
Chrome no-startup-page/context-isolation coverage.

Remove the transitional child before the milestone is complete.

### Slice 5 — Extract evaluation, navigation, and locators

Move the shared evaluation boundary, navigation/URL loops, resolver DTO/scripts, and state waits.

Verify:

- evaluation result/exception variants;
- navigate error and ready-state transitions/timeouts;
- exact URL waits;
- every locator script and hostile-string escaping;
- test-ID attribute validation;
- match counts, candidates, state predicates, and deadline failure precedence;
- current frame/shadow limitations remain explicit in real-browser tests.

### Slice 6 — Extract actions and physical input

Move pre-action polling, mouse/keyboard/text operations, key parsing, check verification, and select
behavior.

Verify exact CDP command order and parameters with fake responses, plus real-browser form behavior,
distinct current failure variants, candidate bounds, changing-failure timeout, and Unicode input.

### Slice 7 — Extract redaction and evidence

Move shared output-safety primitives and best-effort evidence capture.

Verify every field, bound, token, query-name case rule, screenshot decode, DOM sanitization,
secondary-failure path, and secret sentinel across a debug/serialized complete `PageEvidence`.
Evidence capture must remain non-fallible at the trait boundary.

### Slice 8 — Extract semantic inspection

Move raw DTOs, inspection program, mapping, validation, version lookup, snapshot identity, and
truncation construction.

Verify deterministic element/candidate/action order, redaction-before-source eligibility, exact
same-element validation, candidate/element/option/text limits, all truncation flags, URL redaction,
browser-version fallback, schema version, Unicode, and no CSS/XPath synthesis.

### Slice 9 — Rehome tests and finalize the facade

Move pure/fake tests beside owners and cross-layer real-Chrome tests into the root test family.
Delete transitional wrappers/imports, narrow visibility, and confirm production module sizes.

Run focused downstream and full workspace gates. Update this milestone's status and baseline only
after all acceptance criteria pass.

## 12. Testing requirements

### 12.1 Public facade and discovery

Required coverage includes:

- root-level construction/import of both public items;
- `Clone`, `Debug`, `Default`, `Send`, and `Sync` expectations for `ChromeHost`;
- headless default and both `with_headed` values;
- exact timeout replacement;
- explicit path over environment/system precedence without cross-test environment races;
- fixed platform candidate order;
- no-candidate launch error;
- app explicit/environment/configuration/managed/system precedence remaining unchanged.

### 12.2 Process and profile lifecycle

Required coverage includes:

- headed/headless command arguments;
- `127.0.0.1`, random port, fresh user-data directory, and absence of `--no-sandbox`;
- no startup page;
- two-line `DevToolsActivePort` parsing;
- partial file retry, missing file retry, malformed file, early child exit, and startup timeout where
  deterministic test seams allow;
- graceful exit, forced kill, child reap, and profile removal;
- already-removed profile success;
- cleanup retry exhaustion preserving the profile path in the error;
- drop inside and outside a Tokio runtime without panic;
- process/profile cleanup when WebSocket connect fails;
- forced real-Chrome disconnect followed by reap when Chrome is available.

Tests must never point cleanup at a personal profile, home directory, workspace root, or unresolved
environment variable.

### 12.3 Wire and connection actor

Required coverage includes:

- request JSON with/without params and session ID;
- response/result, CDP error, missing payload, and malformed JSON;
- monotonically allocated nonzero IDs;
- out-of-order concurrent responses;
- exact session match/mismatch for browser and page commands;
- bounded writer send timeout and ordinary response timeout;
- command deadline including queue wait;
- dropped response receiver cleanup;
- disconnect/read/write/close/binary terminal failures;
- terminal error cloned to every pending command;
- unknown/late response IDs ignored without blocking a valid response;
- event pressure not starving correlation;
- in-flight count returns to zero on every terminal path;
- console exception/error intake, bound of twenty, eviction order, and unrelated-event omission;
- dropping all connection senders closes the actor without leaking pending work.

### 12.4 Session, context, and target setup

Required coverage includes:

- default-context page creation;
- isolated browser-context creation and exact ID propagation;
- `about:blank` target creation;
- flattened attachment and returned session ID;
- Page/Runtime enable order;
- viewport width/height/scale/mobile params;
- configured test-ID attribute reaching only the page resolver/inspection behavior;
- page creation failure at each command preserving the primary error;
- context close idempotence and use-after-close error;
- context disposal closing isolated storage/pages;
- session close result precedence and process ownership;
- two contexts not sharing cookie/local-storage state in real Chrome.

### 12.5 Evaluation and navigation

Required coverage includes:

- by-value evaluation request and result extraction;
- `errorText`, exception description, exception text, missing value, and wrong value type;
- user expression safely serialized as a request field rather than interpolated into another script;
- navigation response error;
- loading → interactive and loading → complete polling;
- navigation deadline and saturated millisecond reporting;
- current URL string validation;
- exact URL success/mismatch and query/fragment behavior as currently observed;
- every command carrying the correct page session ID.

### 12.6 Locators and waits

Required coverage includes every locator variant with:

- ordinary ASCII, quotes, backslashes, newlines, Unicode, and script-like hostile values;
- zero/one/multiple match decoding;
- candidate count and bound;
- accessible role/name, labels, normalized exact text, placeholders, test IDs, CSS, and XPath;
- allowed and rejected test-ID attribute names;
- invalid selector/XPath mapping;
- main-document shadow/iframe limitations;
- visible/hidden/attached/detached/enabled/disabled/checked/unchecked predicates;
- ambiguous, missing, invisible, and generic final-state failure precedence;
- wait backoff cap and deadline behavior;
- valid UTF-8 candidate text.

### 12.7 Actions and input

Required coverage includes:

- missing, ambiguous, invisible, disabled, non-editable, obscured, detached, unstable, and changing
  failure states;
- exact rectangle stability threshold;
- mouse center and move/press/release ordering;
- fill select-all/backspace/insert order and platform modifier;
- type without clear;
- all supported named keys, aliases, modifier combinations, single Unicode scalar, invalid duplicate
  main keys, empty key, and unsupported names;
- key down/up fields and optional text;
- check/uncheck no-op and post-click verification;
- select by value and displayed text;
- missing/ambiguous locator and option distinctions;
- input then change dispatch;
- current form-flow success in real Chrome.

### 12.8 Evidence and redaction

Required coverage includes:

- screenshot success, invalid base64, malformed response, and CDP failure;
- page URL/title success and failure;
- locator actionability/candidate evidence and resolver failure;
- DOM value sanitization, maximum bytes, Unicode boundary, and evaluation failure;
- console evidence ordering/bound;
- secret sentinels in URL, title, DOM, exception/console text, candidate names/text, and overlapping
  secret strings;
- case-insensitive configured query names while preserving nonsensitive query pairs;
- parsed and invalid URL behavior;
- empty secret handling;
- all partial failures accumulating without discarding successful evidence;
- no raw secret in the full debug representation returned to runtime.

### 12.9 Semantic inspection

Required coverage includes:

- options clamped by the protocol-neutral browser limits;
- visible/hidden filtering and raw total/returned counts;
- password value/text omission and absence of storage/cookies/raw DOM;
- roles, names, labels, placeholders, test IDs, DOM IDs, exact leaf text, Unicode, and select options;
- candidate order label → role → test ID → ID → placeholder → text;
- candidate validation through the ordinary resolver and exact document index;
- deduplication and configured candidate bound;
- omission when no locator validates;
- redacted/truncated values displayed but never reused as locator source;
- supported action and applicable state ordering for editable/clickable/checkable/selectable/hoverable
  elements;
- option cap 50 and every truncation flag;
- page URL query redaction, title/URL/version bounds, and version fallback;
- schema version/kind and snapshot prefix;
- deterministic repeated output apart from documented snapshot identity/browser version;
- no generated CSS/XPath candidate;
- explicit shadow/iframe limitations;
- emitted locator source parsing and resolving through the normal WebTest pipeline where covered by
  the existing semantic-discovery acceptance tests.

### 12.10 Downstream compatibility

Required downstream coverage includes:

- app Chrome selection/provenance tests;
- app browser error-code conversion for disconnect, crash, malformed protocol, protocol, timeout,
  launch, navigation, and operation failures;
- runtime browser-operation and evidence/observation tests through fake `Page` implementations;
- app `inspect` reporter construction from protocol-neutral `PageInspection`;
- editor real-browser diagnostic publication/clearing when Chrome is available;
- LSP real run and DAP headed breakpoint protocol tests when Chrome is available;
- no CDP types in any public `browser`, runtime, observation, LSP, DAP, or WASM DTO;
- no stdout contamination in LSP/DAP modes.

### 12.11 Real Chrome test policy

Use real Chrome for process startup, target/session behavior, navigation, locator semantics,
physical input, context isolation, screenshots, DOM behavior, inspection, and disconnect/reaping.
Fixtures bind `127.0.0.1:0` and use the assigned port.

Tests may skip only when Chrome or loopback sockets are genuinely unavailable. Pure and fake-CDP
coverage must still test error classification and command sequencing, so an absent browser cannot
turn the whole crate into a false-green no-op. CI with managed Chrome should make the real-browser
path mandatory through its environment configuration.

### 12.12 Verification commands

Every delivery slice runs:

```sh
cargo fmt --all -- --check
cargo test -p webtest-browser-cdp
cargo clippy -p webtest-browser-cdp --all-targets -- -D warnings
```

Slices touching the public facade, errors, sessions, page traits, evidence, or inspection also run:

```sh
cargo test -p webtest-browser
cargo test -p webtest-runtime
cargo test -p webtest-editor
cargo test -p webtest
```

Slices touching app composition or editor/debug protocol behavior additionally run:

```sh
cargo test -p webtest --test cli
cargo test -p webtest --test protocol
```

The final slice runs:

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
target/debug/webtest check examples/plain-html/sign-in.webtest
```

When Chrome is available, the focused browser-cdp, editor, and app protocol tests must exercise
their existing real-browser paths. D.8 does not require extension or WASM changes; if those files
and APIs remain untouched, extension packaging and the portable target are not completion gates.

## 13. Review checkpoints

Review every slice against these questions:

1. Does every moved item have one clear lifecycle or browser-semantic owner?
2. Can all consumers still import `ChromeHost` and `find_system_chrome` from the crate root?
3. Is `app` still the only production reverse dependency and native composition root?
4. Is there still exactly one WebSocket reader/writer actor and one pending correlation map?
5. Did queue capacity, deadline start, sweep cadence, correlation, session validation, or terminal
   fan-out change?
6. Can every process-spawn path still reap Chrome and remove only its owned temporary profile?
7. Are local-only debugging, fresh profile, and no-`--no-sandbox` invariants intact?
8. Did target/context/page creation or close command order, params, or error precedence change?
9. Does every page command use the correct flattened session ID?
10. Is there still one locator resolver reused by actions, waits, evidence, and inspection?
11. Did any embedded JavaScript string or returned raw JSON shape change unintentionally?
12. Is every external string JSON-serialized or syntactically validated before entering JavaScript?
13. Did any wait interval, deadline, backoff, state predicate, failure precedence, or input order
    change?
14. Can any secret now become a locator source or escape through evidence/inspection/capture failure?
15. Did any byte/collection bound, redaction token, candidate order, action order, truncation flag, or
    inspection schema field change?
16. Did a CDP identifier or raw JSON type escape the adapter boundary?
17. Did extraction add a vague helper, universal context, public internal module, dependency cycle,
    or replacement monolith?
18. Did the work accidentally implement or constrain generated bindings, browser events, frames,
    traces, actors, checkpoints, or cross-browser behavior?
19. Are leaf tests beside their owner and real-Chrome tests limited to behavior requiring Chrome?
20. When a real-browser test reports success, is there separate evidence that its essential pure or
    fake-CDP contract cannot be skipped?

Stop and correct the slice if an answer exposes semantic drift, weaker cleanup/redaction, duplicate
protocol ownership, hidden public API change, or premature roadmap architecture.

## 14. Risks and mitigations

### 14.1 Hidden public API breakage

Moving `ChromeHost` can change its root path, derives, auto traits, rustdoc, or trait implementation.
Keep explicit root re-exports and add compile-time facade tests before extraction.

### 14.2 Chrome process or profile leak

Separating host, process, and session can alter drop order or lose the child after a connection
failure. Keep one owned `ChromeProcess`, test failure at every handoff, and explicitly reap after
force-kill.

### 14.3 Correlation race or counter leak

Small actor-loop rewrites can double-remove pending entries, leave counters nonzero, misroute a
session response, or strand commands on disconnect. Move the loop intact first and exhaustively test
each terminal path before cleanup.

### 14.4 Deadline semantic drift

Computing the deadline after queue admission or wrapping only the response receiver would silently
extend timeouts under pressure. Characterize queue time and writer time explicitly and retain one
deadline per outgoing command.

### 14.5 Session or target identity mix-up

Once session setup and page code are siblings, a browser-level command may accidentally carry a page
session ID or vice versa. Scripted fake-CDP tests must assert every method's session field and exact
order.

### 14.6 Locator divergence

Splitting actions, evidence, and inspection can invite separate resolver helpers. Keep resolution
and locator-array generation in `page::locator` and prohibit duplicate selector semantics.

### 14.7 JavaScript injection or escaping regression

Moving raw strings often tempts interpolation cleanup. Golden hostile-string tests must prove JSON
encoding and attribute validation for every external value.

### 14.8 Wait/action behavior drift

Extracted helper calls can move deadline checks, sleeps, second resolution, or final-error selection.
Test exact state sequences with short deterministic timeouts before and after the move.

### 14.9 Input ordering regression

Physical browser behavior depends on command order and fields. Assert fake-CDP command transcripts
and retain real form-flow coverage.

### 14.10 Secret or bound regression

Redaction shared by evidence and inspection may be applied too late or with the wrong replacement
token. Keep field-specific DTO construction with each owner, centralize only proven leaf primitives,
and scan complete returned structures for sentinel secrets.

### 14.11 Inspection compatibility drift

Changing candidate validation concurrency, role/name helpers, or vector assembly can alter preferred
locators and schema output. Preserve sequential evaluation and exact deterministic order with golden
DTO tests.

### 14.12 Module cycles and visibility explosion

Page responsibilities share connection/evaluation/locator facts. Follow section 8 with narrow
`pub(super)` functions and private constructors. Do not expose fields or collapse the page family to
avoid privacy work.

### 14.13 Premature typed-protocol architecture

The roadmap calls for generated typed CDP bindings, but choosing a schema/codegen/API while moving
files combines two large risk surfaces. Keep `wire` deliberately generic and private; generated
bindings get a separate milestone with version and compatibility tests.

### 14.14 Premature event architecture

Console intake may look like an invitation to add subscribers or normalize events. D.8 preserves the
current bounded buffer only. Event backpressure, ordering, loss, barriers, and cancellation require
the later structured design.

### 14.15 False-green real-browser tests

Early returns can hide that Chrome-specific code never ran. Pair each critical real behavior with
pure/fake-CDP tests, make managed-Chrome CI explicit, and report environment skips clearly where the
test harness permits.

### 14.16 Oversized replacement module

Moving most of the file to `page/mod.rs` or `connection.rs` would shorten `lib.rs` without improving
change isolation. Enforce the production module review targets and semantic owners in section 7.

## 15. Acceptance criteria

Milestone D.8 is complete only when:

1. `crates/browser-cdp/src/lib.rs` contains only crate documentation, private module declarations,
   explicit root re-exports, and optional test-module declaration, and normally remains below 50
   lines.
2. Discovery, host composition, process/profile lifecycle, wire envelopes, connection actor,
   session/context/target lifecycle, and every page responsibility have the explicit owners defined
   in this plan.
3. No production module is a renamed replacement monolith, no vague helper module exists, and
   production modules normally remain below the 500-line review target.
4. Existing downstream imports compile unchanged; `ChromeHost` methods/derives/auto traits/defaults
   and `find_system_chrome` behavior remain compatible.
5. Browser-manager and app retain executable-resolution/download/cache composition; no duplicate
   policy appears in `browser-cdp`.
6. Chrome launch retains a fresh owned profile, local-only random-port debugging, current flags,
   null stdio, no default `--no-sandbox`, bounded startup, and exact early-exit/error behavior.
7. Every normal, failure, forced-disconnect, and drop path retains child termination/reaping and
   owned profile cleanup behavior without touching a non-owned directory.
8. There is exactly one bounded CDP connection actor with the current request IDs, queue capacity,
   deadline semantics, session-aware out-of-order correlation, expiration, cancellation cleanup,
   disconnect handling, terminal fan-out, and console bound.
9. Generic wire envelopes remain private and compatible; no generated-binding/event-system work is
   mixed into the refactor.
10. Browser session/context/page creation retains exact Target/Page/Runtime/Emulation command order,
    params, flattened session IDs, viewport settings, isolation, close idempotence, and error
    precedence.
11. Navigation/evaluation retain exact ready-state/URL polling, deadlines, result extraction,
    exception handling, and structured errors.
12. Every current locator/state retains exact matching, safe string serialization, snapshot facts,
    wait behavior, candidate bounds, main-document limitations, and structured failures.
13. Actions retain exact pre-action checks, stability threshold, failure selection, physical input
    order, key behavior, check verification, and select semantics.
14. Evidence remains best effort, bounded, redacted, partial-failure tolerant, and free of sentinel
    secrets across every returned field.
15. Semantic inspection retains schema version, snapshot identity form, raw filtering, deterministic
    candidate/action order, ordinary-resolver validation, source eligibility, redaction, bounds,
    truncation flags, version fallback, and no synthesized CSS/XPath.
16. CDP method names, IDs, sessions, targets, raw JSON, WebSocket frames, and browser-process handles
    remain private to `browser-cdp`; all external behavior still crosses `webtest-browser` traits and
    DTOs.
17. Pure/fake-CDP characterization covers critical behavior even without Chrome, while random-port
    real-Chrome tests cover process, navigation, input, contexts, evidence, inspection, and
    disconnect/reaping when available.
18. App test/inspect/LSP/DAP composition, runtime error/observation behavior, editor real-browser
    behavior, and protocol tests remain compatible with no stdout contamination.
19. No new production dependency, crate, browser backend, parser, locator evaluator, event system,
    generated protocol layer, frame/shadow support, trace, actor, retry, or concurrency capability is
    introduced.
20. Focused browser/browser-cdp/runtime/editor/app suites, app CLI/protocol suites, full workspace
    build/tests/Clippy/format, and representative CLI static checking pass.
21. The checked-in change contains only the intended CDP decomposition, characterization tests, and
    milestone status update; unrelated user work, including other proposed milestone files, remains
    untouched.

The milestone succeeds when a contributor can change Chrome process ownership, command
correlation, navigation, locators, input, evidence, or inspection in its own named module while the
rest of WebTest continues to observe the same protocol-neutral browser behavior.
