# Milestone I — Complete Browser Workflows

## 0. Status and dependencies

**Implementation status: proposed.**

This specification follows [`milestone-h.md`](./milestone-h.md) and closes the remaining browser-workflow and CI-selection gaps required to migrate ordinary Chromium Playwright Test suites without falling back to Playwright.

It depends on:

- Milestone B actionability, semantic locators, isolated contexts, browser evidence, and physical input;
- Milestone C typed expressions, values, capability checking, and server/browser transfer rules;
- Milestone C.5 semantic inspection, stable diagnostic details, repair hints, and `webtest describe`;
- Milestone E structured concurrency, resource scopes, cancellation, retries, test-level jobs, events, and traces;
- Milestone F modules, helpers, fixtures, workspace identity, and editor intelligence;
- Milestone G native/WASM parity and portable editor delivery;
- Milestone H test variants, browser actors, event journals, checkpoints, reactive `select`, guards, checks, and semantic verdicts.

The currently implemented slice remains the baseline described in the repository guidelines. In particular, the current plan and browser traits have primitive locators, one default page per context, the Milestone B action set, and void browser evaluation. Composable locators, explicit page/frame resources, browser-state artifacts, downloads, dialog policies, network routing, sharding, and repeat execution are not implemented merely because this document specifies them.

Milestone I is a vertical product milestone, not a list of adapter methods. Every feature in this document must travel through the one syntax/CST/AST/HIR/analysis/plan/runtime/browser/event/editor path and must be discoverable through the shared description surface.

## 1. Outcome

After Milestone I, WebTest can express a production-style Chromium workflow involving reusable authentication, a mocked backend response, a composed semantic locator, a popup, a cross-origin payment frame, an upload, a dialog, and a download without JavaScript orchestration or Playwright:

```webtest
import { admin_auth } from "./support/auth.webtest"

test "admin exports a paid invoice" tags ["billing", "smoke"] {
    actor admin browser state admin_auth

    with routes on admin {
        route products when request(r)
            if r.method == "GET" && r.url.path == "/api/products" {
            fulfill {
                status: 200,
                json: [
                    { id: "plan-pro", name: "Pro" },
                ],
            }
        }
    } {
        admin {
            open "/billing"

            let pro_row = role("row").filter(has_text: "Pro")
            click pro_row.role("button", name: "Purchase")
        }

        let popup_mark = checkpoint(admin)

        admin {
            click role("link", name: "Open payment details")
        }

        let payment = select admin since popup_mark within 5s {
            when page.opened(opened)
                if opened.url.path == "/payment" {
                provide opened.page
            }

            timeout {
                fail "payment popup did not open"
            }
        }

        page payment {
            frame role("iframe", name: "Payment") {
                fill label("Card number") with "4242 4242 4242 4242"
                upload label("Purchase order") files [
                    project_file("fixtures/purchase-order.pdf"),
                ]
                click role("button", name: "Pay")
            }
        }

        let download_mark = checkpoint(admin)

        with dialog confirm(message: "Export this invoice?")
            on admin
            respond accept
            within 5s {
            admin {
                click role("button", name: "Export PDF")
            }
        }

        let pending = select admin since download_mark within 10s {
            when download.started(download) {
                provide download
            }

            timeout {
                fail "invoice download did not start"
            }
        }

        let invoice = await download pending within 30s
        expect invoice.suggested_name == "invoice.pdf"
        expect products.calls == 1
    }
}
```

The example is illustrative surface syntax. Grammar work may refine punctuation where necessary, but the following semantics are normative:

- locator composition remains symbolic and strict until consumed by an action/assertion;
- pages, frames, downloads, routes, and browser state have typed identities and explicit lifetimes;
- popup and download observation uses Milestone H checkpoints so an event between stimulus and `select` is retained;
- dialog handling and network routing are installed before the child body begins;
- state application occurs when an actor context is acquired, before its default page can run application code;
- every operation, decision, resource, artifact, and count remains source-mapped in the plan, event stream, trace, editor, DAP, and machine output;
- test selection and shard membership are resolved from static declaration/variant metadata before execution.

The product boundary for this milestone is:

> WebTest can migrate representative Chromium production E2E tests that use authentication state, refined locators, pages/frames, uploads/downloads/dialogs, and deterministic HTTP routing, and can distribute those tests predictably across CI shards.

## 2. Design principles

### 2.1 Complete workflows through shared semantics

Milestone I adds semantic browser operations and resources to the protocol-neutral `browser` crate. It does not expose a CDP-shaped DSL, put browser logic in TypeScript, or add page-specific parsers/evaluators in adapters.

The path remains:

```text
source -> syntax -> typed AST -> HIR -> analysis -> TestPlan
       -> runtime -> browser traits -> browser-cdp
       -> events/observations/traces -> editor/LSP/DAP/reporters/WASM DTOs
```

### 2.2 Preserve strict locator intent

Composition gives authors deliberate ways to refine an ambiguous locator. It does not weaken strictness.

```webtest
click role("button", name: "Delete")
```

continues to fail if three buttons match. Authors may state the intended relationship:

```webtest
let row = role("row").filter(has_text: "William")
click row.role("button", name: "Delete")
```

or make an explicit ordinal choice:

```webtest
click role("button", name: "Delete").nth(1)
```

There is never an implicit first-match fallback.

### 2.3 Model browser objects as owned resources

`Page`, `BrowserState`, `Download`, and route/dialog scopes are not ordinary serializable records and are not raw browser handles. They have typed semantic IDs, runtime generations, capability requirements, source origins, and structured teardown.

Raw target IDs, frame IDs, request IDs, object IDs, and CDP sessions never enter source, HIR, portable plans, reports, or editor DTOs.

### 2.4 Arm asynchronous behavior before stimulus

Milestone I uses two explicit mechanisms:

- retain non-blocking popup/download events through Milestone H actor checkpoints and journals;
- install blocking/intercepting dialog and route policies before the controlled child starts.

No implementation may translate these forms into "click, then start listening".

### 2.5 Make persisted state explicit and reviewable

Browser state contains credentials. Capturing, applying, persisting, updating, redacting, expiring, and deleting it must therefore be explicit.

Ordinary `webtest test` execution may consume a named state artifact but never silently create or overwrite one. Named state updates use an explicit command/workflow and atomic replacement.

### 2.6 Keep selection deterministic and separate from scheduling

Discovery, variant expansion, tag/filter evaluation, shard assignment, and repeat expansion occur before jobs are scheduled. `--jobs` changes concurrency, not membership. Completion timing never changes which variants belong to a shard or how final results are ordered.

### 2.7 Prefer bounded structured behavior over callbacks

Network routes and dialog policies are constrained plan data plus pure expressions. They are not general asynchronous callbacks.

Route decisions cannot call providers, drive the browser, spawn work, or wait on arbitrary events. Dialog responses cannot execute application automation while the page is blocked.

## 3. Scope

Milestone I includes:

- first-class symbolic `Locator` values usable in bindings and typed helpers;
- descendant/scoped locator composition;
- `filter` with `has`, `has_not`, `has_text`, and `has_not_text` constraints;
- explicit zero-based `nth`, `first`, and `last` refinement;
- page resources, new pages, popup discovery, page selection/scopes, page close, and page lifecycle events;
- nested same-origin and cross-origin frame targeting through locator-driven frame scopes;
- focus, blur, element/page scrolling, drag-and-drop, and file-input upload;
- typed value-producing browser evaluation as a constrained escape hatch;
- download start/completion/failure resources and artifact integration;
- preinstalled alert/confirm/prompt policies with explicit responses;
- in-memory browser-state capture/application and fixture integration;
- named versioned browser-state artifacts with an explicit update workflow;
- cookies, local storage, default-page session storage, and IndexedDB state where the backend advertises support;
- actor-scoped HTTP request routing with pure matching, abort, request modification, deterministic fulfillment, and invocation counts;
- route decisions projected into Milestone H passive network events and Milestone E traces;
- statically declared test/case tags;
- one shared filter-expression model for CLI/editor/machine selection;
- deterministic `--shard <index>/<total>` assignment;
- bounded repeat execution with identity distinct from retries;
- complete source mapping, diagnostics, observations, reporters, trace, DAP, editor services, `describe`, inspection, and portable static plan support for the new features.

## 4. Non-goals

Milestone I does not add:

- Firefox or WebKit backends;
- browser/device project matrices;
- visual screenshot baseline matching or approval;
- geolocation, locale, timezone, color-scheme, permission, offline, CPU, or network emulation profiles;
- fake clocks or time virtualization;
- browser video recording;
- HAR recording/replay;
- upstream response-body modification after a real server response;
- WebSocket interception or WebSocket mocking;
- service-worker behavior that the active backend cannot intercept with a documented guarantee;
- arbitrary browser-protocol commands in the DSL;
- arbitrary DOM event subscriptions;
- raw pointer-coordinate scripting, unrestricted clipboard access, or a general input-device API;
- arbitrary effectful route/dialog callbacks;
- dynamic route creation from runtime loops or provider results after a route scope starts;
- test discovery by executing project/application code;
- changed-tests-only selection from version-control history;
- distributed scheduling, a hosted grid, or cross-machine fixture sharing;
- automatic login-state refresh during an ordinary test run;
- automatic test healing;
- a Playwright compatibility layer or a second runtime backend.

Typed `evaluate` remains an explicit escape hatch. It is not a general host-language extension system and does not permit Node.js, filesystem, process, or browser-protocol access outside the page execution realm.

## 5. Relationship to earlier milestones

### 5.1 Milestone B actionability remains normative

Every action added by I uses B's deadline, cancellation, repeated resolution, strictness, stability, visibility, enabledness, hit-testing, scrolling, evidence, and error-class conventions where those conditions apply.

Locator composition is evaluated as one locator query on every poll. Implementations must not resolve the parent once, retain a stale element handle, and then query descendants after application rerendering.

### 5.2 Milestone E owns resource and cancellation semantics

Pages, downloads, route scopes, dialog scopes, captured state operations, and named-state production lower through explicit resource/control nodes. Cancellation tears them down under the existing bounded cleanup deadline.

Retry and repeat remain different:

- `Retry` reruns a failed child according to retry safety and records `AttemptId`;
- repeat execution intentionally runs the selected variant multiple times and records `IterationId` even when prior iterations passed.

### 5.3 Milestone F owns reusable setup

Browser state composes with fixture lifetimes. A worker- or suite-scoped fixture may log in once, capture immutable `BrowserState`, and provide that state to test-scoped actors. It must not share the live bootstrap actor/page across tests.

Named browser-state declarations are workspace declarations resolved through the same module graph, symbol, documentation, and invalidation system as other reusable declarations. They do not introduce an adapter-owned registry.

### 5.4 Milestone H actors and event journals remain normative

Each page belongs to one actor/context. Page and download events are normalized actor events and use H checkpoints/selects for race-free observation.

I extends the initial H event-source catalog with:

```text
PageOpenedEvent
PageClosedEvent
DownloadStartedEvent
DownloadCompletedEvent
DownloadFailedEvent
DialogObservedEvent
NetworkRouteDecisionEvent
```

`select` semantics do not change. `race` semantics do not change. Dialog policy and network routing are explicit structured scopes, not special meanings of either construct.

### 5.5 Passive observation and active network control remain distinct

H answers:

> What request or response did the browser observe?

I additionally answers:

> Which preinstalled route handled this request, and did it continue, modify, abort, or fulfill it?

The same normalized request identity correlates passive request/response events, route decisions, assertions, and trace entries without exposing backend request IDs.

## 6. Composable locator model

### 6.1 Symbolic locator values

`Locator` becomes a pure symbolic browser-domain type. Constructing or composing a locator does not query the page.

```webtest
let table = test_id("users")
let william = table.role("row").filter(has_text: "William")
let delete = william.role("button", name: "Delete")

click delete
```

A locator may be:

- bound to a local;
- passed to or returned from a typed helper;
- placed in a statically typed record when the record remains in the browser capability domain;
- consumed by browser actions, assertions, frame selection, and inspection queries.

A locator is not a live element, is not transferable into `server {}`, and does not carry a backend handle.

### 6.2 Plan representation

The plan evolves from a flat primitive enum to a recursive serializable expression:

```text
LocatorExpr =
    Primitive {
        kind,
        arguments,
    }
    Descendant {
        scope,
        target,
    }
    Filter {
        base,
        has,
        has_not,
        has_text,
        has_not_text,
    }
    Index {
        base,
        selection,
    }

IndexSelection = First | Last | ZeroBased(Int)
```

Arguments remain typed `PlanExpr`s where the language already permits expression-valued locator arguments. The runtime evaluates those expressions before each complete locator resolution attempt. The plan contains no DOM node, selector-engine closure, or syntax node.

### 6.3 Descendant composition

Member-style locator construction scopes the right locator to descendants of each left candidate:

```webtest
role("dialog", name: "Preferences")
    .role("button", name: "Save")

test_id("users")
    .text("William")
```

The right side searches descendants and does not include the scope element itself. Composition may be nested to arbitrary statically bounded depth. Implementations must cap pathological plan/query depth through documented analysis limits.

Each primitive retains its existing semantic matching rules. Composition changes the search root, not the meaning of role, label, text, test ID, CSS, or XPath.

CSS and XPath remain escape hatches. Their behavior is relative to the composed root. They do not acquire privileged cross-origin frame access.

### 6.4 Filters

Required filters are:

```webtest
role("row").filter(has_text: "William")
role("listitem").filter(has: role("button", name: "Buy"))
role("listitem").filter(has_not: text("Sold out"))
role("row").filter(has_not_text: "Archived")
```

For each base candidate:

- `has` succeeds when the relative locator has at least one descendant match;
- `has_not` succeeds when the relative locator has no descendant match;
- `has_text` compares normalized rendered subtree text using the documented text normalization rules;
- `has_not_text` negates that comparison.

Filter-relative locators may not escape to an ancestor, another page, or another frame. A filter is pure and cannot contain actions, provider calls, waits, or evaluation.

Multiple supplied filter fields are conjunctive. Exact punctuation may be refined, but the conjunction and relative-root semantics are normative.

### 6.5 Explicit ordinal refinement

Required forms are:

```webtest
role("row").first()
role("row").last()
role("row").nth(2)
```

`nth` is zero-based. Its argument must be a statically non-negative integer in version 1. Candidate order is deterministic DOM order within the selected page/frame and composed roots.

An out-of-range ordinal produces `NoMatch` and may retry until the operation deadline because the collection can change. `first`/`last`/`nth` are deliberate author intent, so the final locator is singular even when the unrefined base has many matches.

The editor and `describe` surface must clearly state that ordinal locators are more brittle than semantic refinement. WebTest may suggest a semantic filter when inspection has evidence, but it must not reject a deliberate ordinal choice or silently rewrite it.

### 6.6 Strictness and auto-waiting

For every poll:

1. evaluate typed locator arguments;
2. resolve primitive and composed roots against the current document/frame state;
3. apply filters;
4. apply explicit ordinal selection;
5. classify the final result as zero, one, or many;
6. apply operation-specific B actionability or assertion semantics.

Intermediate multiplicity is allowed because filters and descendant composition may refine it. Final singular consumers still require exactly one match.

Transient ambiguity, absence, frame replacement, or candidate changes may retry until the operation deadline. Evidence records the final full locator tree and bounded facts from prior distinct states without retaining unbounded polling history.

### 6.7 Shadow DOM and frame boundaries

Semantic locator behavior through open shadow roots must be one documented cross-backend contract and covered by the browser conformance suite. Closed shadow roots are not pierced.

Frame boundaries are never crossed implicitly. A locator in a page/frame scope searches that scope only. Authors use the frame construct in section 8 to enter an iframe document.

### 6.8 Origins and diagnostics

Every primitive, composition edge, filter field, and ordinal operation retains a `SyntaxOrigin`. Failures underline the smallest useful range:

- the base locator when it finds no candidates;
- the descendant locator when a scope exists but the target does not;
- the relevant filter when it removes all candidates;
- the ordinal argument when it is out of range;
- the complete locator when multiple composition paths remain ambiguous.

Structured evidence includes bounded candidate ancestry so a trace or inspector can explain which parent/row/dialog scoped a candidate.

## 7. Pages and popups

### 7.1 `Page` resource

Milestone I introduces a first-class runtime type:

```text
Page<ActorId>
```

A page is:

```text
actor-bound
context-bound
execution/resource-generation-bound
non-transferable to server capability
non-serializable as a value
exclusively mutable by default
source-mapped and traceable
```

Every page has a semantic `PageId`. Backend target/session IDs remain private.

### 7.2 Default-page compatibility

Existing:

```webtest
browser {
    open "/"
}
```

continues to use the implicit default actor and its default page.

Existing Milestone H:

```webtest
admin {
    open "/"
}
```

continues to use `admin.default_page`.

No page declaration is required for existing tests.

### 7.3 Creating and using pages

Required semantics are represented by forms conceptually equivalent to:

```webtest
let docs = new page on admin

page docs {
    open "/documentation"
    expect role("heading", name: "Documentation").visible
}

close page docs
```

`new page` acquires an owned page under the actor resource scope. It does not replace the actor's default page unless an explicit future operation says so.

`page <binding> {}` selects the page capability for its lexical body. Nested `browser {}` or actor blocks must not ambiguously override it; analysis reports conflicting page-target scopes.

`close page` is idempotent only when the same WebTest cleanup path observes a page already closed by prior explicit close. Using a closed page is a structured `page_closed` test failure. Closing the actor context remains responsible for any page not explicitly closed.

### 7.4 Popup observation

A popup is an ordinary new page with opener metadata. It is observed through the actor event journal:

```webtest
let mark = checkpoint(shopper)

shopper {
    click role("link", name: "Open receipt")
}

let receipt = select shopper since mark within 5s {
    when page.opened(opened)
        if opened.url.path == "/receipt" {
        provide opened.page
    }
}
```

`PageOpenedEvent` includes bounded typed metadata:

```text
page: Page<ActorId>
page_id
opener_page_id when known
initial/current URL
creation kind: explicit | popup | target-created
```

The event's serialized reporter/trace projection contains page identity and metadata, never a live resource handle. If the page closes before selection, the event remains observable but later page use fails as `page_closed`.

The checkpoint must be established before the click or other stimulus when an immediately opened page matters. I does not add a second `wait_for_popup` orchestration mechanism with different lost-event behavior.

### 7.5 Page lifecycle and navigation

Page lifecycle is explicit:

```text
Created -> Available -> Closing -> Closed
```

Browser/context failure can transition all owned pages to unavailable. Application-initiated page close is distinct from whole-browser disconnect.

Page navigation, URL assertions, evidence, downloads, dialogs, and events carry `PageId` in addition to `ActorId`. Traces render separate page lanes nested under an actor lane.

### 7.6 Concurrent pages

Different pages owned by the same actor may execute concurrently after the backend passes multi-page conformance tests. The analyzer still rejects:

- overlapping exclusive use of the same page;
- actor-wide context mutation concurrently with page operations when the operation cannot be proven safe;
- route/dialog/state scope changes that conflict with an active sibling operation;
- use after page or actor lifetime.

Actor-wide cookies and storage remain shared among its pages because they share one browser context. Separate pages are not separate authenticated actors.

### 7.7 Page failures

Required structured failures include:

```text
page_closed
page_not_owned_by_actor
page_concurrent_access
page_creation_failed
page_close_failed
page_event_unavailable
```

A target closed by application behavior is generally a test failure for an operation targeting that page. Browser-process crash, protocol disconnect, malformed messages, and backend invariant violations remain infrastructure/internal failures.

## 8. Frames

### 8.1 Locator-driven frame scope

Frames use semantic iframe locators rather than backend frame IDs:

```webtest
frame role("iframe", name: "Payment") {
    fill label("Card number") with "4242 4242 4242 4242"
    click role("button", name: "Pay")
}
```

The locator is evaluated in the current page/frame scope and must resolve to exactly one frame-owning element. The body runs against that frame document.

Frame scope may nest:

```webtest
frame test_id("outer-widget") {
    frame role("iframe", name: "Card details") {
        fill label("Security code") with "123"
    }
}
```

### 8.2 Frame locators, not stale frame handles

A frame scope retains a symbolic `FrameLocator` chain and semantic `FrameScopeId`; it does not expose a portable live frame object.

Each child browser operation resolves or validates the frame chain under its own deadline. If an application replaces an iframe during rerender, a later operation may resolve the replacement when it still satisfies the authored frame locator. An operation already dispatching into a detached frame fails or retries only according to its documented idempotency/actionability stage.

### 8.3 Cross-origin frames

Same-origin and cross-origin frames have the same DSL surface. `browser-cdp` uses the appropriate target/session mechanics and never bypasses origin security by interpolating evaluated JavaScript from the parent page.

If the backend cannot provide a required locator/action capability in an out-of-process iframe, it reports `unsupported_frame_capability` before claiming the operation succeeded. CSS/XPath stay relative to the selected frame document.

### 8.4 Frame readiness and navigation

Entering a frame scope waits, within the effective action deadline, for:

1. exactly one iframe element;
2. an attached content frame;
3. a usable document/session for the required browser capability.

It does not add an unbounded load wait. URL/navigation assertions inside the frame target the frame URL where explicitly documented; existing page URL operations continue to mean the selected page's main-frame URL. The type/description surface distinguishes `page_url()` from any new `frame_url()` accessor.

### 8.5 Frame errors and evidence

Required failures include:

```text
frame_not_found
frame_ambiguous
frame_element_invalid
frame_detached
frame_navigation_failed
frame_session_unavailable
unsupported_frame_capability
```

Evidence records the outer page, full frame-locator chain, last known frame URL where policy permits, and bounded locator candidates. The source range normally underlines the frame locator that failed, not the entire nested block.

## 9. Additional browser interactions

### 9.1 Focus and blur

Required forms:

```webtest
focus label("Search")
blur label("Search")
```

`focus` repeatedly resolves one attached focusable target and verifies that it becomes the active element. Visibility/scroll requirements follow the backend-neutral focus contract and are documented separately from pointer actionability.

`blur` resolves exactly one target and removes focus when it is active. It succeeds as a no-op when the unique target is not focused, but it never ignores ambiguity or invalid target type.

Both operations produce the browser events expected from the documented user-level semantics. They do not invoke author-supplied JavaScript.

### 9.2 Scrolling

Required forms are conceptually:

```webtest
scroll role("heading", name: "Invoices") into_view
scroll page by { x: 0, y: 600 }
```

Element scrolling resolves exactly one attached target and uses the nearest documented alignment that makes it visible. Page scrolling dispatches bounded wheel/scroll input against the selected page/frame.

Coordinates and deltas must be finite and within documented bounds. Scroll completion means the input and resulting browser scroll state have settled according to the browser contract; it does not imply application-specific lazy content has finished loading. Authors assert the resulting state normally.

### 9.3 Drag and drop

Required form:

```webtest
drag test_id("card-1") to test_id("done-column")
```

The source and target locators each require one attached, visible, stable, unobscured pointer target. The backend dispatches a documented physical pointer sequence and supports ordinary HTML drag/drop behavior without calling application handlers directly.

The operation retains distinct source and target origins/evidence. Failure states identify whether source resolution, target resolution, pickup, movement, drop, or post-dispatch browser transport failed.

### 9.4 Typed browser evaluation

Existing void evaluation remains compatible:

```webtest
evaluate "window.bootstrap && window.bootstrap();"
```

Milestone I adds typed value-producing evaluation with structured arguments:

```webtest
let title = evaluate<String>(
    "(args) => document.querySelector(args.selector)?.textContent ?? ''",
    args: { selector: "title" },
)
```

The script is explicit source text. Arguments are JSON-serialized through protocol arguments; they are never string-interpolated into code. The result is decoded through the shared typed value/JSON decoding model and must satisfy its declared transferable type.

Evaluation:

- runs in the currently selected page/frame execution realm;
- is an opaque browser effect to static analysis;
- is retry-unsafe by default unless the author encloses it in an explicitly justified policy supported by Milestone E;
- has a deadline and cancellation context;
- preserves script/result decoding/source origins;
- redacts arguments/results according to their typed secret metadata;
- cannot return DOM nodes, functions, promises that outlive the deadline, browser handles, or arbitrary cyclic objects.

This escape hatch does not justify adding core browser workflows only through JavaScript.

### 9.5 Action plan variants

The plan adds explicit operations conceptually equivalent to:

```text
Focus { locator, timeout }
Blur { locator, timeout }
ScrollIntoView { locator, alignment, timeout }
ScrollBy { target, delta, timeout }
DragAndDrop { source, target, timeout }
UploadFiles { locator, files, timeout }
EvaluateTyped { script, arguments, result_type, timeout }
```

Every operation remains distinct for static capability analysis, DAP stepping, retry policy, event reporting, and trace rendering.

## 10. File uploads

### 10.1 Surface and file values

Required upload syntax is conceptually:

```webtest
upload label("Purchase order") files [
    project_file("fixtures/purchase-order.pdf"),
    project_file("fixtures/terms.txt", mime_type: "text/plain"),
]
```

`project_file(...)` creates a typed `UploadFile` descriptor. It does not read bytes during parsing or static analysis.

Version 1 file sources are:

```text
project-relative file
completed WebTest download/artifact explicitly accepted as an upload source
bounded in-memory bytes/text when constructed through an explicit typed form
```

Arbitrary ambient absolute paths, home-directory expansion, shell expansion, and URL fetching are not allowed in upload expressions.

### 10.2 Target semantics

Upload resolves exactly one file input control, including a control reached through its semantic label. It verifies that the target is an `<input type="file">` or backend-neutral equivalent and enforces single/multiple-file compatibility.

A hidden file input may be set because ordinary styled upload controls intentionally hide the native input. This is a documented exception to pointer visibility actionability, not a general way to bypass locator strictness or attachment checks.

The browser backend sets file payloads through a browser protocol mechanism. It must not inject user paths into JavaScript. The expected input/change events are produced according to the documented browser contract.

### 10.3 Path and content validation

At runtime the app composition layer resolves project files against the canonical project root and configured upload roots. It rejects:

```text
path traversal
symlink escape
missing/non-regular files
unreadable files
files larger than configured limits
too many files
invalid declared names/MIME types
single-file input receiving multiple files
```

The plan records a normalized project-relative descriptor, expected metadata when known, origin, and capability requirement. It does not embed local absolute paths in portable plan output.

File bytes and paths are secret evidence by default. Traces record bounded metadata and digest according to policy, not contents unless explicitly configured.

### 10.4 Upload failures

Required codes include:

```text
upload_target_invalid
upload_multiple_not_allowed
upload_file_missing
upload_file_unreadable
upload_file_outside_root
upload_file_too_large
upload_too_many_files
upload_failed
```

Project path/configuration problems are configuration failures. A page that rejects or replaces the input during dispatch is a source-mapped test failure. Browser disconnect remains infrastructure failure.

## 11. Downloads

### 11.1 Download resource and events

Milestone I introduces:

```text
Download<ActorId>
DownloadedFile
ArtifactRef
```

The actor journal emits:

```text
download.started
download.completed
download.failed
```

`DownloadStartedEvent` carries a non-transferable `Download` resource plus bounded metadata:

```text
download_id
actor_id
page_id when known
source URL under redaction policy
suggested filename
declared MIME type when available
```

### 11.2 Race-free capture

Authors checkpoint before the triggering operation:

```webtest
let mark = checkpoint(user)

user {
    click role("button", name: "Download report")
}

let pending = select user since mark within 5s {
    when download.started(download) {
        provide download
    }
}
```

The runtime establishes download acceptance/capture for every actor whose known plan may observe downloads before it executes a possible stimulus. It must not enable downloads only after `download.started` has already been missed.

### 11.3 Completion

Required completion form:

```webtest
let file = await download pending within 30s

expect file.suggested_name == "report.pdf"
expect file.size > 0
```

Completion returns typed metadata:

```text
DownloadedFile {
    download_id,
    suggested_name,
    media_type,
    size,
    sha256,
    artifact: ArtifactRef,
}
```

The completed bytes live under the runner-owned artifact directory. Source cannot choose an arbitrary destination path. An explicit artifact-retention operation may assign a normalized artifact-relative logical name, but cannot escape the artifact root or overwrite another execution's artifact.

An `ArtifactRef` may be passed only to operations/providers that explicitly accept it. It is not silently converted to an ambient filesystem path.

### 11.4 Failure and cleanup

Cancellation, actor teardown, test timeout, page closure, and browser disconnect cancel or finalize owned downloads according to browser capability. Partial files are either removed or retained under an explicitly marked partial-artifact policy; they are never reported as successful completed downloads.

Required codes include:

```text
download_timeout
download_cancelled
download_failed
download_page_closed
download_artifact_failed
download_too_large
```

Application/browser download cancellation is a test failure. Inability to write the configured artifact directory or browser-process failure is infrastructure/configuration as appropriate.

## 12. Dialog handling

### 12.1 Why dialogs use a structured scope

JavaScript alert, confirm, and prompt dialogs can block the page before an ordinary later statement runs. A passive `select` after the triggering click is therefore insufficient as the only response mechanism.

Milestone I adds a preinstalled dialog policy scope:

```webtest
with dialog confirm(message: "Delete this order?")
    on user
    respond accept
    within 5s {
    user {
        click role("button", name: "Delete")
    }
}
```

Prompt response:

```webtest
with dialog prompt(message: "Reason")
    on user
    respond accept with "duplicate"
    within 5s {
    user {
        click role("button", name: "Cancel order")
    }
}
```

Exact line breaks/delimiters may evolve, but actor targeting, matcher, response, finite deadline, pre-body installation, and structured child ownership are normative.

### 12.2 Matching and response

Dialog kind is one of:

```text
alert
confirm
prompt
before_unload where backend policy permits
```

The message matcher uses typed string/pattern semantics rather than backend-specific dialog text fields. Responses are:

```text
accept
dismiss
accept with <String>  # prompt only
```

Response expressions are evaluated before the child body starts. They may reference transferable lexical values but cannot call providers or perform effects while the page is blocked.

### 12.3 Scope semantics

The runtime:

1. installs the dialog handler on the actor/context;
2. establishes a backend acknowledgement/barrier;
3. starts the child body;
4. immediately responds to the first matching dialog;
5. records the normalized dialog event and response decision;
6. waits for both child completion and the required dialog match within the effective deadline;
7. removes the handler during structured teardown.

The version 1 scope expects exactly one matching dialog. Sequential or nested scopes express multiple expected dialogs. A later extension may add explicit cardinality without changing this default.

An unexpected dialog is dismissed by the configured safe default and fails the owning operation/scope as `unexpected_dialog`; it is never silently accepted. If no explicit scope is active, the same safe default prevents the browser from hanging and produces a source-mapped failure at the operation during which the dialog appeared.

### 12.4 Interaction with H journals

Every observed dialog also produces bounded `DialogObservedEvent` metadata for trace/evidence/guards. Passive observation cannot respond to a blocking dialog and does not replace the policy scope.

Guard branches remain observational and cannot accept/dismiss dialogs. This preserves H's prohibition on effectful background callbacks.

### 12.5 Dialog failures

Required codes include:

```text
dialog_timeout
dialog_kind_mismatch
dialog_message_mismatch
unexpected_dialog
dialog_response_failed
dialog_handler_unavailable
dialog_scope_conflict
```

Evidence contains expected/actual kind and bounded/redacted message, actor/page, currently running node, and whether a safe default response was sent.

## 13. Browser state and authentication

### 13.1 State is a browser resource, not an application user

Milestone I introduces the opaque typed value:

```text
BrowserState
```

It may contain backend-neutral projections of:

```text
cookies
local storage by origin
default-page session storage by origin
IndexedDB by origin when requested/supported
state-format/backend metadata
```

It does not contain a live page/context and does not imply an application account. Application users remain values created through fixtures/providers/app bridge or ordinary browser flows.

`BrowserState` is immutable after capture, shareable when its producing fixture lifetime permits, non-comparable in user code, non-transferable into browser JavaScript/server providers, secret by default, and serializable only through the dedicated state-artifact codec.

### 13.2 Capture

In-memory state capture is conceptually:

```webtest
let state = capture browser_state from bootstrap {
    cookies: true,
    local_storage: true,
    session_storage: true,
    indexed_db: true,
}
```

Capture establishes a bounded browser-protocol barrier and snapshots the actor context consistently according to the documented backend contract. It does not race arbitrary in-flight storage writes without acknowledgement.

Cookies are context-wide. Local storage and IndexedDB are keyed by origin. Session storage is captured only from the actor's selected/default page and is restored only for the recorded origin before that page's first application script; WebTest does not pretend that session storage is naturally shared across every page in a context.

Unsupported requested components fail explicitly. The backend must not silently omit IndexedDB or session storage while labeling the result complete.

### 13.3 Actor initialization

State is applied when an actor is acquired:

```webtest
actor admin browser state state
```

or when referencing a named state declaration:

```webtest
actor admin browser state admin_auth
```

Application occurs before the default page can navigate or execute application code. Loading state into an already active actor is not supported in version 1 because partial cookie/storage mutation would create unclear isolation and retry semantics.

State application validates backend/features, origin records, cookie fields, expiry policy, size bounds, and artifact integrity before the test body begins.

### 13.4 Fixture integration

A reusable per-run authentication fixture may be expressed conceptually as:

```webtest
export fixture authenticated_admin() -> BrowserState scope worker {
    setup {
        server {
            let login = app.create_login_link(email: "admin@example.com")
        }

        actor bootstrap browser

        bootstrap {
            open login.url
            expect url("/admin")
        }

        provide capture browser_state from bootstrap {
            cookies: true,
            local_storage: true,
            indexed_db: true,
        }
    }
}
```

Tests use the immutable state to acquire fresh test-scoped actors. They never share the bootstrap context:

```webtest
let auth = use authenticated_admin()
actor admin browser state auth
```

Fixture retry/acquisition/teardown behavior remains F/E behavior. A worker fixture is per scheduler worker, including within each independently invoked CI shard.

### 13.5 Named persisted state

For explicit cross-run persistence, modules may declare a named producer:

```webtest
export browser_state admin_auth {
    setup {
        server {
            let login = app.create_login_link(email: "admin@example.com")
        }

        actor bootstrap browser

        bootstrap {
            open login.url
            expect url("/admin")
        }

        provide capture browser_state from bootstrap {
            cookies: true,
            local_storage: true,
            indexed_db: true,
        }
    }
}
```

The exact declaration keyword may be refined with Milestone F grammar, but the declaration has:

```text
stable BrowserStateDeclarationId
module/export identity
typed setup plan
declared capture components/origins
deterministic artifact location under configured state root
optional freshness policy
documentation and source origins
```

`webtest state update admin_auth` runs the producer plan, performs normal cleanup, writes a versioned temporary artifact, verifies it, and atomically replaces the named artifact only after success. A failure leaves the previous artifact unchanged.

`webtest state list` reports declaration/artifact/freshness metadata without printing cookies or storage values. `webtest state clean <name>` removes only the resolved named artifact after normal CLI target validation. Exact CLI spelling follows the app's command conventions and is documented by command help.

Ordinary `check`, `build`, editor analysis, and WASM analysis use declaration and supplied artifact metadata without executing the producer. Ordinary `test` consumes the existing artifact and fails clearly if it is missing, corrupt, incompatible, or expired according to policy.

### 13.6 State artifact format and safety

The artifact is a versioned WebTest format, conceptually:

```text
BrowserStateEnvelope {
    schema_version,
    declaration_id,
    browser_backend,
    feature_set,
    captured_at,
    origins,
    payload,
    checksum,
}
```

Artifacts are stored under a configured project-relative state root with user-private permissions where supported. They contain authentication secrets in recoverable form unless a future external secret-store integration is configured. Documentation and `init` guidance must tell users to ignore them in version control.

State payloads are never included in source-emitting builds, traces, reporters, JUnit, LSP diagnostics, DAP variables, machine descriptions, or logs. Those surfaces expose only declaration identity, component names, timestamps/freshness, redacted origins where allowed, and checksum/fingerprint metadata.

### 13.7 State lifetime and retries

- Applying immutable input state to a new actor does not mutate the input.
- An actor declared inside retry gets a new context initialized from the same input state for every attempt.
- Capturing state inside a failed/cancelled attempt does not publish a named artifact.
- A named state update is one structured execution with normal actor/provider teardown.
- Suite/file/worker fixtures may share immutable state values but not mutable actor contexts.
- Shards do not coordinate named-state updates; update commands are separate from sharded test execution.

### 13.8 State failures

Required codes include:

```text
browser_state_missing
browser_state_corrupt
browser_state_incompatible
browser_state_expired
browser_state_capture_failed
browser_state_apply_failed
browser_state_component_unsupported
browser_state_origin_invalid
browser_state_too_large
browser_state_update_not_authorized
```

Missing/corrupt/incompatible persisted input is configuration/infrastructure, not an application assertion failure. A state producer's authored login assertion can still fail as an ordinary test-plan failure during explicit update.

## 14. Network interception and mocking

### 14.1 Actor-scoped route policy

Routing is installed on a browser actor/context so it covers requests from that actor's pages and frames according to the advertised backend capability.

Conceptual surface:

```webtest
with routes on shopper {
    route products when request(r)
        if r.method == "GET" && r.url.path == "/api/products" {
        fulfill {
            status: 200,
            headers: { "content-type": "application/json" },
            json: [
                { id: "basic", name: "Basic" },
                { id: "pro", name: "Pro" },
            ],
        }
    }

    route images when request(r)
        if r.resource_type == "image" {
        abort "blocked_by_client"
    }
} {
    shopper {
        open "/products"
        expect text("Pro").visible
    }

    expect products.calls == 1
    expect images.aborted >= 1
}
```

Route names are stable semantic bindings within the scope and receive `NetworkRouteId`s.

### 14.2 Installation ordering

Route scope execution is:

1. evaluate/capture transferable lexical values used by route policies;
2. install every route and required request interception on the actor;
3. establish a backend acknowledgement/barrier;
4. begin the child body;
5. make decisions for matching requests while the body executes;
6. await child and bounded in-flight route decisions;
7. freeze final route counters;
8. remove interception/routes during teardown.

If installation is partial or the backend cannot guarantee declared coverage, the child does not start.

### 14.3 Structured request matcher

Route matching uses the same normalized URL/request facts as H network observation:

```text
method
structured URL: scheme/host/port/path/query
bounded/redacted headers where explicitly allowed
resource type
navigation/document flag
page/frame identity where available
redirect-hop metadata
```

The `if` expression is pure, typed, and bounded. It may use request fields, captured transferable values, pure helpers, and H patterns. It may not call providers, drive the browser, acquire resources, wait, emit verdicts, mutate state, or run arbitrary evaluation.

Request bodies are not captured merely to evaluate an unrelated route. A route that explicitly matches or modifies a body activates bounded body capture and must declare/obey size and redaction policy.

### 14.4 Route ordering

For each request:

1. inspect routes in source order;
2. choose the first route whose source kind and pure filter match;
3. execute exactly one terminal route decision;
4. if no route matches, continue the request unchanged.

There is no hidden "most specific" ranking. Source order is visible in the plan and trace. The analyzer may warn when an earlier route provably subsumes a later route.

Redirect hops are independently normalized and routed. A modification or fulfillment does not implicitly carry across a later redirect request.

### 14.5 Continue and request modification

Required semantics:

```webtest
continue

continue with {
    method: "POST",
    url: "https://example.test/api/replacement",
    headers: {
        "x-webtest": "fixture",
    },
    body: "...",
}
```

All fields are optional; omitted fields preserve the original request. Header behavior distinguishes replace/remove/add operations in the typed plan rather than relying on ambiguous duplicate maps.

Modification values are pure typed expressions evaluated for the matched request. URL validity, forbidden schemes, header syntax, body size, and method/body compatibility are validated before the backend resumes the request.

### 14.6 Abort

Required form:

```webtest
abort "blocked_by_client"
```

Abort reasons come from a documented protocol-neutral enum. Unknown arbitrary backend error strings are static errors. The resulting passive network event identifies the request as intentionally aborted by a route and correlates the route identity.

### 14.7 Deterministic fulfillment

Required forms support typed response data:

```webtest
fulfill {
    status: 201,
    headers: { "content-type": "application/json" },
    json: { id: "ord_123", status: "created" },
}

fulfill {
    status: 200,
    media_type: "text/plain",
    body: "ok",
}
```

Exactly one of typed `json`, text/binary `body`, or an accepted artifact/file source is supplied. JSON uses the existing deterministic typed value encoder. WebTest does not silently add application-specific headers, CORS policy, latency, or cookies beyond explicitly documented defaults.

Fulfillment is bounded by configured header/body limits. A response generated from request data remains deterministic with respect to the captured lexical values and normalized request; effectful calls are forbidden.

### 14.8 Route handles and counts

Each named route exposes a typed read-only `RouteHandle` during its owning scope:

```text
calls
continued
modified
aborted
fulfilled
failed
last_request summary under evidence policy
```

Counts increment at documented points:

- `calls` when the route is selected for a request;
- terminal action count when the backend acknowledges that decision;
- `failed` when a selected decision cannot be applied.

Counts are safe to assert after the relevant application wait/response and before route-scope exit. They are frozen for trace/reporting at teardown. A handle cannot escape its scope or cross actor/retry generations.

### 14.9 Interaction with passive events and traces

H `network.request` and `network.response` remain normalized observations. They gain optional fields such as:

```text
route_id
route_name
route_decision
mocked
original URL/method metadata under redaction policy
```

A fulfilled response produces a normalized response event with the authored status/headers and `mocked = true`. An aborted request produces a terminal request-failure/aborted fact, not a fabricated HTTP response.

Route decision events are emitted before the corresponding resumed/aborted/fulfilled network completion. Correlation uses WebTest semantic request identity, not raw CDP IDs.

### 14.10 Service workers and coverage

Deterministic routing cannot claim coverage for requests hidden behind an uncontrolled service worker.

Before entering a route scope, the backend must either:

- advertise and prove interception coverage for the declared request classes; or
- apply an explicit project policy that blocks service workers for routed actors; or
- reject activation with `network_route_coverage_unavailable`.

Silently missing a request and later reporting a route count of zero is not acceptable when the backend knew it could not observe that request class.

### 14.11 Teardown and cancellation

No request may remain paused after the route scope exits. On normal exit, all selected decisions are acknowledged or reported failed before teardown completes. On cancellation/timeout/browser shutdown, pending intercepted requests follow one documented safe cancellation policy and cleanup remains bounded.

Route teardown failure is retained alongside the primary child failure using E resource aggregation. A slow reporter or trace writer cannot block request interception.

### 14.12 Network-control failures

Required codes include:

```text
network_route_unknown
network_route_duplicate
network_route_unreachable
network_route_filter_not_pure
network_route_scope_conflict
network_route_coverage_unavailable
network_route_install_failed
network_route_decision_failed
network_route_body_too_large
network_route_invalid_header
network_route_invalid_url
network_route_unsupported_action
```

An authored count/assertion mismatch is a test failure. Backend interception installation, protocol failure, or unavailable declared coverage is infrastructure/capability failure.

## 15. Tags, filtering, sharding, and repeat execution

### 15.1 Static tags

Test declarations may define tags:

```webtest
test "checkout succeeds" tags ["checkout", "smoke"] {
    ...
}
```

Cases may add variant-specific tags:

```webtest
test "card behavior" tags ["checkout"]
cases [
    case "visa" tags ["smoke"] { brand: "visa" },
    case "expired" tags ["negative"] { brand: "visa", expired: true },
]
as card {
    ...
}
```

Effective variant tags are the deterministic set union of declaration tags and case tags. Duplicate tags are normalized/deduplicated with a diagnostic according to the language's warning policy.

Tags are static non-secret metadata and follow a documented canonical grammar such as:

```text
[a-z0-9][a-z0-9._-]*
```

They are included in discovery, editor test items, build manifests, events, traces, and machine reports. Tags are not runtime bindings.

### 15.2 Shared selection expression

CLI, editor, and machine clients use one structured selection model:

```text
SelectionExpr =
    All
    And([...])
    Or([...])
    Not(child)
    Tag(glob)
    TestName(glob)
    VariantLabel(glob)
    Path(glob)
    TestDeclarationId(id)
    TestVariantId(id)
```

The CLI may accept a bounded textual form such as:

```sh
webtest test --filter 'tag("smoke") && !tag("slow") && path("tests/checkout/**")'
```

The parser/evaluator for this expression is a shared Rust selection component consumed by `app`, editor services, and portable DTO conversion. The extension must not implement a second filter parser.

Globs, escaping, case sensitivity, path normalization, and Unicode comparison are documented and platform-independent. Invalid filters fail before any fixture, application process, browser, or test starts.

Existing exact declaration/variant/ID selection from H composes with this model rather than bypassing it.

### 15.3 Selection order

The runner performs:

```text
project discovery
-> static analysis
-> test declaration discovery
-> case/variant expansion
-> exact ID/name restrictions
-> tag/filter expression
-> deterministic shard assignment
-> repeat-instance expansion
-> scheduling with --jobs
```

Every selected variant and its transitive module/helper/fixture/pattern/guard/state dependencies must be statically valid before execution. A static error in an independent unselected declaration is still returned by `webtest check` and editor diagnostics but does not block an otherwise valid targeted `webtest test` run. File/module/import errors that make discovery or dependency resolution unsound do block selection. CLI and editor use the same dependency-aware rule; filters cannot hide malformed selected dependencies.

### 15.4 Deterministic sharding

Required CLI form:

```sh
webtest test --shard 3/10
```

Shard indices are one-based; total must be positive and within a documented maximum; index must be in `1..=total`.

Assignment uses a versioned algorithm over stable `TestVariantId`, conceptually:

```text
u64(blake3("webtest-shard-v1" || TestVariantId)) % total
```

The selected variant belongs to shard `remainder + 1`.

Consequences:

- discovery order and filesystem enumeration do not affect membership;
- `--jobs` and runtime completion timing do not affect membership;
- adding an unrelated variant does not move existing variants for the same shard total;
- changing shard total intentionally recomputes membership;
- a shard may be empty;
- all repeat iterations and retry attempts for one variant stay on the same shard.

The exact hash encoding is normative once shipped and receives a schema/version identifier. Implementations must test identical membership on every platform and in native/WASM selection DTOs.

### 15.5 Fixture semantics across shards

Each shard invocation is an independent suite execution:

- suite fixtures are once per invoked shard, not once globally across machines;
- worker fixtures are once per scheduler worker in that shard;
- file fixtures acquire only when that shard has an assigned dependent variant;
- test fixtures remain per variant/attempt according to E/F;
- no fixture state is implicitly shared across shard processes or hosts.

Suites requiring a globally unique setup must use an external provider/application primitive designed for distributed coordination; Milestone I does not invent cross-machine locks.

### 15.6 Repeat execution

Required CLI form is conceptually:

```sh
webtest test --repeat 20
```

The count must be positive and bounded. Repeat expansion gives each execution:

```text
same TestDeclarationId
same TestVariantId
distinct IterationId
retry AttemptIds nested within that iteration
```

Repeat is not retry. A passing earlier iteration does not suppress later iterations, and a failure does not automatically stop remaining iterations unless fail-fast policy explicitly does so.

A variant that fails any completed repeat iteration fails the repeated aggregate, even if other iterations pass. Retry-based fail-then-pass remains one iteration with its Milestone E attempt history; repeat does not reinterpret a failed iteration merely because another independent iteration passed.

### 15.7 Selection manifest and reporting

Before execution, the runner creates a versioned `ExecutionSelectionManifest` containing:

```text
workspace/config/source/provider fingerprints
normalized selection expression
shard index/total/algorithm version
repeat count
ordered assigned TestVariantIds
effective tags
deterministic seed policy
```

Human concise output may summarize this metadata. JSON/events/traces retain it exactly. A list/dry-run command exposes selected variants and shard membership without acquiring fixtures or launching the app/browser.

JUnit case names remain stable and add bounded iteration labels only when repeat count exceeds one. Retry attempts do not become duplicate unrelated test cases.

### 15.8 Invalid selection behavior

Required failures include:

```text
invalid_tag
duplicate_tag
invalid_filter_expression
unknown_test_selection
unknown_variant_selection
invalid_shard
invalid_repeat_count
empty_selection
selection_manifest_mismatch
```

Selection errors are configuration/usage errors with the stable CLI exit class. They are not test failures and do not start runtime resources. An empty selection is an error by default; an explicit command option may permit it for CI matrix jobs and must be represented in the selection manifest.

## 16. Versioned semantic and plan model

### 16.1 New semantic identities

Milestone I adds stable or execution-scoped typed identities as appropriate:

```text
PageId
FrameScopeId
DownloadId
DialogScopeId
NetworkRouteId
NetworkRouteScopeId
BrowserStateDeclarationId
BrowserStateGenerationId
IterationId
ExecutionSelectionManifestId
```

Declaration identities derive from workspace/module/declaration semantics. Plan-node identities derive from source identity and stable child path. Runtime resource-generation identities additionally distinguish retry/iteration/acquisition generations.

No identity derives solely from a display name, raw backend ID, pointer address, filesystem enumeration order, or completion timestamp.

### 16.2 Plan data

Milestone I extends the versioned plan conceptually with:

```text
LocatorExpr

PageCreate {
    actor_id,
    page_binding,
    options,
}

PageScope {
    actor_id,
    page,
    child,
}

PageClose {
    page,
}

FrameScope {
    page,
    parent_frame_scope,
    frame_locator,
    child,
}

Focus / Blur / ScrollIntoView / ScrollBy / DragAndDrop
UploadFiles
EvaluateTyped

AwaitDownload {
    download,
    timeout,
    result_binding,
}

DialogScope {
    actor_id,
    kind,
    message_matcher,
    response,
    timeout,
    child,
}

CaptureBrowserState {
    actor_id,
    components,
    result_binding,
}

ActorScope {
    actor_id,
    initial_state,
    acquire,
    child,
    teardown,
}

NetworkRouteScope {
    actor_id,
    routes,
    child,
}

NetworkRoute {
    route_id,
    request_binding,
    pure_filter,
    decision,
    counter_binding,
}
```

Named state producers live in declaration/project plan data and compile to ordinary explicit setup/resource/capture/teardown nodes when the update command selects them.

Suite selection is not hidden inside each `TestPlan`. The app builds a versioned execution manifest over independently compiled `TestVariantPlan`s:

```text
ExecutionSelectionManifest {
    identity/fingerprints,
    selector,
    shard,
    repeat_count,
    selected_variants,
}
```

### 16.3 Plan serialization rules

Plans may contain:

```text
semantic resource references
project-relative upload descriptors
named browser-state identity/checksum metadata
pure route match/decision expressions
typed dialog matcher/response
required host capabilities
source origins and source revision
```

Plans never contain:

```text
live browser/page/frame/download/request handles
raw CDP IDs/messages/method names
browser-state secret payloads
ambient absolute local paths
host-language callbacks/closures
open files/sockets/processes
runtime counters
```

Emitted plans with project files or named state inputs carry integrity/fingerprint metadata sufficient for the native host to detect drift before execution.

### 16.4 Required host capabilities

Portable plans declare granular requirements such as:

```text
browser.multiple_pages
browser.frames
browser.cross_origin_frames
browser.file_upload
browser.downloads
browser.dialogs
browser.state.cookies
browser.state.local_storage
browser.state.session_storage
browser.state.indexed_db
browser.network_interception
browser.network_fulfill
host.project_files
host.artifact_store
```

Capability absence is reported before executing the affected node/scope. A backend must not advertise a broad `browser` capability and then silently ignore required subfeatures.

### 16.5 Determinism

For identical source, project inputs, provider schemas, named-state metadata, and feature configuration:

- locator plan trees are byte-for-byte stable;
- route order/IDs and pure decision plans are stable;
- state declarations and component requirements are stable;
- variant tags and shard membership are stable;
- page/frame/download/dialog source identities are stable;
- native and WASM portable plans agree.

Runtime page/download generations, timestamps, request sequences, and iteration executions remain runtime facts and do not perturb compile-time plan identity.

## 17. Static analysis

### 17.1 Locators

Analysis validates:

- locator values remain in browser-capable contexts;
- member composition names/signatures are valid;
- `has`/`has_not` receive relative locators;
- text filter fields receive strings;
- `nth` is a non-negative bounded integer;
- locator recursion/depth stays within limits;
- a locator/filter cannot capture non-transferable runtime resources;
- frame locators are eligible to identify frame elements where statically knowable;
- first-class locators cannot cross server/provider boundaries.

### 17.2 Pages and frames

Analysis validates:

- page bindings resolve and belong to the referenced actor;
- page resources do not escape actor/test/retry lifetime;
- closed-page use where statically provable;
- same-page overlapping exclusive access;
- actor-wide operation conflicts with parallel page operations;
- frame scopes appear only inside a selected browser page capability;
- frame resource/capability use does not escape its scope;
- popup/download event resource values are used only in compatible actor/resource generations.

### 17.3 Uploads, downloads, dialogs, and evaluation

Analysis validates:

- upload file descriptors and literal project paths;
- upload file-count/static size metadata limits when knowable;
- `await download` receives a live compatible `Download` resource;
- download/page/actor generation compatibility;
- dialog kinds and response compatibility;
- finite positive dialog/download deadlines;
- no conflicting dialog scopes on one actor;
- typed evaluate script/argument/result signatures;
- evaluation results are supported transferable types;
- evaluated arguments preserve secret metadata.

Filesystem existence/canonicalization remains a project/host input check, not syntax parsing in analysis.

### 17.4 Browser state

Analysis validates:

- named state declarations, exports, imports, uniqueness, and cycles;
- producer setup capability/effects and exactly one provided `BrowserState` on success;
- actor state arguments have type `BrowserState`;
- state is applied only during actor acquisition;
- state values do not enter server capability, ordinary JSON, case labels, reports, or unsupported fixture lifetimes;
- capture component names/options are valid;
- named state declarations do not depend on undiscoverable runtime declaration generation;
- concurrent named-state updates cannot be represented inside ordinary test plans.

Project analysis additionally reports missing/corrupt/incompatible named artifact metadata without reading secret payloads into the semantic database.

### 17.5 Network routes

Analysis validates:

- unique route names/IDs in a scope;
- actor target and actor lifetime;
- pure typed request filters;
- pure typed route response expressions;
- exactly one terminal decision per route;
- structured URL/method/header/body constraints;
- no effectful provider/browser/evaluate/verdict/wait operations in route decisions;
- route handles do not escape their scope;
- conflicting/nested actor-wide route scopes according to the composition policy;
- unsupported capture/action requirements where the configured backend feature set is known;
- statically provable unreachable/subsumed route ordering;
- bounded plan size and route count.

### 17.6 Tags and suite selection

Analysis/workspace discovery validates:

- tag grammar and duplicates;
- case/declaration effective tag sets;
- stable declaration/variant IDs independent of tags unless tag metadata intentionally participates in a selection-manifest fingerprint;
- bounded static filter AST;
- valid exact IDs/names/variant labels;
- shard/repeat numeric ranges;
- identical selection results from native snapshots and portable discovery DTOs.

Changing tags changes selection metadata but does not change `TestVariantId`. Renaming a test/case follows F/H identity rules.

## 18. Diagnostic and failure taxonomy

### 18.1 Suggested static/configuration codes

Milestone I adds stable codes conceptually equivalent to:

```text
locator_composition_invalid
locator_filter_type_mismatch
locator_index_invalid
locator_depth_exceeded
locator_not_transferable

unknown_page
page_actor_mismatch
page_out_of_scope
page_concurrent_access
frame_context_required
frame_locator_invalid

upload_file_descriptor_invalid
upload_file_outside_root
download_resource_mismatch
dialog_response_invalid
dialog_scope_conflict
evaluate_result_not_transferable

unknown_browser_state
duplicate_browser_state
browser_state_cycle
browser_state_apply_context_invalid
browser_state_component_invalid
browser_state_artifact_missing
browser_state_artifact_incompatible

network_route_duplicate
network_route_filter_not_pure
network_route_decision_not_pure
network_route_terminal_action_required
network_route_handle_out_of_scope
network_route_unreachable
network_route_capability_unavailable

invalid_tag
duplicate_tag
invalid_filter_expression
invalid_shard
invalid_repeat_count
```

### 18.2 Runtime test-failure codes

Required test-failure families include:

```text
locator_not_found / locator_ambiguous and existing B actionability codes
locator_index_out_of_range
page_closed
frame_not_found / frame_ambiguous / frame_detached
focus_failed / blur_failed / scroll_failed / drag_failed
upload_target_invalid / upload_failed
download_timeout / download_failed
dialog_timeout / dialog_mismatch / unexpected_dialog
network_route_count_mismatch through ordinary assertion structure
network_route_decision_failed when caused by authored invalid runtime data
typed_evaluation_failed / typed_evaluation_decode_failed
```

### 18.3 Infrastructure/configuration/internal distinctions

Keep distinct:

- missing upload/state input, invalid artifact roots, bad filter/shard flags: configuration/usage;
- expected element/frame/page/dialog/download/route count not observed: test failure;
- browser launch/crash/disconnect, CDP transport, artifact write failure, route-install protocol failure: infrastructure;
- violated ownership, duplicate runtime IDs, impossible scheduler state, malformed internally generated route decision: internal bug.

Do not flatten any class into a generic browser error below the app/reporting layer.

### 18.4 Repair details

Structured diagnostic/observation details may include:

```text
composed locator candidate paths
available frame candidates
open page summaries
valid dialog responses
available state declarations/components
available route fields/actions
nearest tags/tests/variants
normalized shard constraints
```

Repair hints remain bounded, source-mapped, and advisory. WebTest may propose a composed semantic locator from current inspection evidence but never silently updates source or chooses an ordinal.

## 19. Runtime ownership and scheduling

### 19.1 Expanded ownership tree

Conceptually:

```text
Execution
  -> Selection manifest
  -> Variant iteration
      -> retry attempt/resource scopes
          -> browser actor/context
              -> initial browser state application
              -> route/dialog policy scopes
              -> pages
                  -> frame scopes
                  -> operations
                  -> downloads
              -> actor event journal
```

Every child has one owner. No page, frame session, intercepted request, dialog handler, temporary upload payload, download, or state-capture task survives its owning scope.

### 19.2 Runtime resource table

The runtime maintains typed resource entries keyed by semantic/runtime identity and generation. Entries record:

```text
owner task/scope
actor/context/page relationship
acquisition state
capability set
exclusive/shared access state
cancellation state
teardown state/result
redacted debug metadata
```

Browser backends receive opaque runtime references through traits. Plan execution never downcasts to a CDP page/target.

### 19.3 Policy installation barriers

State application, route installation, dialog handler installation, popup/download journal activation, and frame/session acquisition all use explicit acknowledgement or protocol barriers where ordering affects correctness.

If a backend cannot establish the claimed boundary, the operation fails. It must not use a timing delay as a substitute.

### 19.4 Cancellation and cleanup

On cancellation/timeout/fail-fast/debug disconnect:

1. stop scheduling new child operations;
2. cancel active page/frame/browser operations;
3. resolve/block/dismiss active dialogs under safe policy;
4. release or abort paused intercepted requests;
5. cancel/finalize downloads;
6. close pages and actor contexts;
7. remove temporary upload/state files;
8. emit cleanup outcomes before parent completion.

Primary/secondary failure precedence remains Milestone E's typed aggregation. Cleanup never replaces a more useful application failure with an unstructured string.

### 19.5 Retry composition

Resource placement in the plan determines retry behavior:

- a page created inside retry is recreated each attempt;
- a page outside retry survives an inner retry and must be proven safe for repeated child effects;
- a download cannot cross attempt generation;
- route/dialog scopes inside retry reinstall per attempt;
- actor initial state is reapplied when the actor is reacquired;
- route counts are per scope/attempt and aggregate evidence identifies attempt;
- named browser-state artifacts are immutable retry inputs.

### 19.6 Repeat composition

Each repeat iteration is a fresh test execution boundary with normal test isolation. Test-scoped actors/fixtures/pages/routes/downloads never cross iterations. Longer fixture lifetimes follow the configured execution ownership model, but reports/traces associate every operation with `IterationId`.

Repeat iteration scheduling may interleave under `--jobs`, but final aggregate ordering is variant identity then iteration ordinal, not completion time.

## 20. Runtime events and observations

### 20.1 Event envelope fields

Milestone E/H's event envelope gains optional typed identity fields or payload metadata for:

```text
iteration_id
page_id
frame_scope_id
download_id
dialog_scope_id
network_request_id
network_route_id
network_route_scope_id
browser_state_declaration_id
browser_state_generation_id
selection_manifest_id
```

### 20.2 Required event kinds

Conceptually required new kinds include:

```text
SelectionManifestCreated
VariantAssignedToShard
IterationStarted / IterationFinished

PageCreated / PageClosed
PageScopeEntered / PageScopeExited
FrameScopeEntered / FrameScopeExited

UploadStarted / UploadFinished
DownloadStarted / DownloadFinished / DownloadFailed

DialogHandlerInstalled
DialogObserved
DialogResponded
DialogHandlerRemoved

BrowserStateCaptureStarted / BrowserStateCaptured
BrowserStateApplied
BrowserStateArtifactUpdated

NetworkRouteScopeInstalled / NetworkRouteScopeRemoved
NetworkRouteMatched
NetworkRouteDecisionApplied / NetworkRouteDecisionFailed
NetworkRouteCountersFinalized
```

Events are typed immutable facts after they occur. A `NetworkRouteMatched` execution event is distinct from the H reactive `NetworkRequestEvent`, though they share semantic request correlation.

### 20.3 Observations

New observation kinds include:

```text
composed locator refinement failure
page/frame lifecycle failure
upload path/target failure
download timeout/failure/artifact
dialog mismatch/unexpected dialog
state artifact/capture/apply failure
route installation/decision/count evidence
selection/shard/repeat metadata where relevant
```

Observation replacement and source-revision checks remain unchanged. Starting a new run clears prior current observations. A successful rerun removes old runtime diagnostics.

### 20.4 Smallest useful ranges

Examples:

- descendant locator range for a missing nested button;
- filter field for a filter eliminating all rows;
- frame locator for an unavailable frame;
- upload path literal for missing file;
- dialog matcher for kind/message mismatch;
- route filter/decision field for invalid runtime data;
- named state reference for missing/incompatible state;
- shard/filter CLI argument has command diagnostic identity and no fake source range.

## 21. Trace artifact and viewer

### 21.1 Trace extension

Milestone E's trace format is extended, not replaced. The manifest declares the I feature/schema versions and selection manifest.

The trace can reconstruct:

```text
variant -> repeat iteration -> retry attempt
actor -> page -> nested frame scopes
composed locator evaluation and actionability summary
popup/page lifecycle
upload metadata
download start/progress/completion artifact
dialog handler interval, match, and response
route scope interval, request match, decision, and counters
browser-state declaration/application metadata without payload
shard/filter/tag membership
```

### 21.2 Viewer layout

For a multi-page actor, conceptual lanes are:

```text
time ----------------------------------------------------------------->

actor admin
  default page   open -- click popup -- click export -- dialog
  popup page                  open -- frame Payment -- fill -- upload
    frame Payment                        fill -- upload -- click
  network        route active ========================================
  download                                         start ===== complete
```

The viewer links route decisions to normalized request/response events and download events to the retained artifact. It does not rerun route filters, locator matching, or project code.

### 21.3 Capture and redaction

Trace policy may retain:

```text
bounded request/response metadata
mock response bodies only when explicitly enabled and redacted
upload name/size/digest, not bytes by default
download artifact under configured attachment policy
dialog kind/message under redaction policy
state identity/freshness/components, never payload
typed evaluate script/result only according to source/evidence policy
```

Every truncation is explicit. Trace size limits cannot cause semantic route/dialog handling to drop data needed for execution; they affect persisted evidence only.

### 21.4 Safety

Trace readers continue to reject traversal, symlinks, checksum mismatch, oversized entries, malformed schemas, and external-network fetches. Download filenames, response headers, URLs, and dialog messages are untrusted data and must be escaped in the viewer.

## 22. DAP behavior

### 22.1 Frames and scopes

DAP stack/scopes identify:

```text
test variant / iteration / retry attempt
actor
selected page
frame locator chain
route/dialog/resource scope
current operation
```

This browser frame identity is semantic WebTest state, not a JavaScript engine call stack or raw CDP frame ID.

### 22.2 Variables

Variables may expose bounded/redacted metadata for:

```text
Locator symbolic structure
Page URL/title/lifecycle
Frame locator/current URL
Download status/name/size/digest/artifact
Route counts and last bounded request summary
BrowserState declaration/components/freshness
selection manifest/shard/iteration
```

They never expose cookies, storage payloads, upload bytes/absolute paths, auth headers, prompt secrets, live handles, or CDP IDs.

### 22.3 Breakpoints and stepping

Every explicit I plan node is a breakpoint target where safe. DAP can pause before page/frame actions, state capture, route/dialog scope activation, upload, download await, and typed evaluation.

The debugger must not leave a blocking dialog or intercepted request suspended merely to stop inside an internal policy decision. Dialog responses and route decisions execute through bounded runtime policy; a configured exception breakpoint may stop immediately after safe response/resume while retaining the triggering event and source chain.

### 22.4 Disconnect

Debug disconnect cancels the execution and performs all I cleanup: handlers removed, requests released/aborted, downloads finalized, pages/contexts closed, and temporary artifacts removed. DAP must not own alternate cleanup logic.

## 23. Editor and inspection services

### 23.1 Completion and signatures

Shared editor services add context-aware completion/signature help for:

```text
locator composition members
filter fields
first/last/nth
page/frame resources
new browser actions
upload file descriptors
download event/result members
dialog kinds/responses
browser-state declarations/components
normalized request fields and route actions
tags and selection metadata where appropriate
```

Completion uses the same schemas/tables as analysis and `describe`; the LSP/extension owns no lists.

### 23.2 Hover and navigation

Hover can display:

```text
Locator symbolic type and strictness
Page<actor>
FrameScope/page ownership
Download<actor>
BrowserState declaration/components/freshness
NetworkRoute signature/count fields
effective test-variant tags
```

Definition/references/rename cover named browser states and route bindings. Page/download/local locator bindings use ordinary semantic binding navigation. Renaming a route updates assertions/references semantically.

### 23.3 Code actions

Candidate bounded actions include:

- add a semantic `filter` based on validated inspection candidates;
- qualify/import a browser-state declaration;
- replace an unknown route/request field;
- insert a missing finite `within` value;
- correct invalid dialog response for kind;
- replace unknown tag/test/variant with a static candidate.

No action silently adds `.first()` to suppress ambiguity. An ordinal refinement may be offered only as an explicit low-confidence choice distinct from semantic candidates.

### 23.4 Live inspection

`webtest inspect` extends its shared page semantics to include:

```text
page summaries and stable execution-local page identity
frame tree summaries
frame-qualified validated locator candidates
composed locator candidates when they are shorter/more semantic
file-input capability
dialog/download availability metadata where observable
```

Inspection never enables routing, changes state, responds to dialogs, or begins downloads merely to discover capabilities. It remains bounded and read-oriented.

For a candidate inside a frame, inspection returns a canonical frame chain plus inner locator rather than a CSS selector that pretends to cross the frame boundary.

### 23.5 Test explorer and run profiles

Editor test items carry effective tags and stable variant IDs. Run/debug profiles can send a structured selection expression, shard metadata where configured, and repeat count to the CLI/DAP adapter.

The extension does not locally rediscover tests, expand cases, evaluate tags, or compute shard hashes.

## 24. `webtest describe` and machine-facing semantics

Every public I language/type/browser construct must be discoverable through canonical description topics. Expected topic families include conceptually:

```text
type.Locator
locator.composition
locator.filter
locator.nth

type.Page
browser.page.new
browser.page.close
browser.frame
browser.focus
browser.blur
browser.scroll
browser.drag
browser.upload
browser.download
browser.dialog
browser.evaluate

type.BrowserState
browser.state.capture
browser.state.declaration
browser.actor.state

browser.network.route
browser.network.continue
browser.network.abort
browser.network.fulfill
type.RouteHandle

test.tags
```

Each entry supplies applicable:

```text
syntax forms
typed parameters/result
legal contexts
capabilities/effects
strictness/lifetime rules
failure modes
constraints
availability
related topics
canonical examples
```

Configured named state declarations project from the workspace database. Machine description exposes identity, documentation, capture component metadata, and freshness status, never state payload.

CLI-only flag spelling for shard/filter/repeat remains in `webtest test --help`; `describe` documents language semantics and shared static selection metadata rather than duplicating the CLI manual.

Description completeness tests must prove that:

- every new public AST/HIR/plan/browser operation has a topic/index membership;
- types/capabilities/failures agree with analysis/runtime schemas;
- every installed canonical example parses and statically analyzes in its declared context;
- no topic advertises cross-browser, visual, HAR, WebSocket routing, emulation, or other non-goals;
- native/WASM descriptions match for portable inputs.

## 25. WASM and portable analysis

WASM can:

```text
parse and format I syntax
type-check locator composition/resources/routes/state declarations/tags
compile deterministic portable I plans
discover state declarations/test tags/variants
evaluate structured test selection and shard membership
provide completion/hover/navigation/refactoring
describe I static semantics
report required native host capabilities
```

WASM cannot:

```text
read upload files or state artifacts without explicit host inputs
launch/acquire pages or frames
execute evaluation
capture/apply browser state
install routes/dialog handlers
handle downloads
run tests in a browser
```

A WASM host may supply normalized project-file and named-state metadata as explicit inputs. The semantic database must not fetch them through ambient browser APIs.

Native/WASM parity covers plan structure, origins, IDs, diagnostics, tags, selection AST, shard assignment, description DTOs, and required capabilities for identical portable inputs.

## 26. Configuration

Milestone I may extend typed project configuration along these lines:

```toml
[browser]
service_workers = "allow" # allow | block; route scopes validate coverage

[timeouts]
download = "30s"
dialog = "5s"

[uploads]
roots = ["fixtures", "tests/fixtures"]
max_files = 20
max_file_bytes = 52428800
max_total_bytes = 104857600

[downloads]
mode = "on-demand" # deny | on-demand | accept
max_bytes = 104857600
retain = "on-failure"

[browser_state]
directory = ".webtest/state"
max_bytes = 16777216
default_max_age = "24h"

[network]
max_routes_per_actor = 100
max_request_body_bytes = 1048576
max_fulfill_body_bytes = 10485760
max_header_bytes = 65536
```

Exact defaults require implementation measurement and security review. All sizes, counts, durations, roots, and modes are bounded and validated by `project`.

Configuration affecting semantics/capabilities participates in project analysis invalidation and plan/selection fingerprints. Editor/LSP watched-input handling must republish relevant diagnostics after configuration changes without restart.

Command-line precedence follows the app's one existing precedence model. Environment variables are not invented per feature when typed config/CLI input suffices.

## 27. Security and privacy

### 27.1 State credentials

Browser state is credential material. WebTest must:

- create state directories/files with private permissions where supported;
- use atomic write/rename and integrity checks;
- avoid state payloads in plans, traces, diagnostics, DAP, reporters, crash messages, or source bundles;
- redact state origins/metadata according to policy;
- warn when state paths appear likely to be tracked by version control where this can be determined safely;
- never refresh/overwrite named state during ordinary test execution;
- bound artifact size and validate every decoded field;
- document that deletion/revocation may be required after compromise.

### 27.2 Upload and download paths

Resolve paths from explicit canonical roots, reject traversal and symlink escape, and avoid shell interpretation. Download filenames are untrusted hints and never become destination paths without normalization. Artifact paths derive from stable execution/resource identity.

### 27.3 Network routing

Route match/decision values may contain credentials and request/response bodies. Runtime matching uses actual values, but persisted/rendered projections are redacted and bounded.

The backend:

- never binds an interception service to a non-local endpoint;
- validates URL schemes/headers/body sizes;
- prevents route expressions from reading ambient secrets except through explicitly typed captured inputs;
- escapes trace/viewer content;
- does not log raw authorization/cookie headers;
- does not let a slow route observer block the protocol read loop indefinitely.

Mock fulfillment does not bypass WebTest's configured network policy for unrelated requests.

### 27.4 Evaluation

Arguments are protocol-serialized, never interpolated. Results are bounded before decoding. Evaluation failures redact secret arguments and preserve the explicit script source only under source/trace policy.

Evaluation executes with the tested page's privileges and must be treated as project code. It cannot access native runner APIs unless the application itself exposes them in the page.

### 27.5 Dialogs and downloads

Dialog/prompt text may contain secrets and follows redaction. Unexpected dialogs are safely dismissed by default. Downloaded content is untrusted, is never executed by WebTest, and remains inside the artifact root.

### 27.6 Selection metadata

Tags, test names, case labels, and shard manifests are log-visible non-secret metadata. Documentation must tell authors not to place credentials in them. Filter parsing is bounded and cannot execute regexes or code with pathological unbounded behavior.

## 28. Architecture and crate responsibilities

- `text` continues to own file/revision/document identities and byte ranges used by every new origin.
- `syntax` remains the only lexer/parser/lossless CST and adds typed AST views for locator composition, page/frame/actions, uploads/downloads/dialogs, state declarations/capture, route scopes, tags, and related expressions.
- `hir` owns typed symbolic locator/page/frame/download/state/route constructs and IDs, capability/effect annotations, tag metadata, and exact origin chains.
- `analysis` owns locator/resource/type/lifetime/exclusivity checks, named-state declaration resolution, route purity/order/capability checks, tag/filter discovery, deterministic plan construction, descriptions, and selection/shard queries.
- `provider` remains protocol-neutral and does not absorb browser routing. Explicit future provider operations may accept `ArtifactRef` only through a shared typed contract; no provider receives ambient paths implicitly.
- `feedback` owns protocol-neutral I semantic-detail and repair-hint DTOs without rendering policy.
- `plan` owns recursive locator expressions, page/frame/resource operations, uploads/downloads/dialogs/state/routes, required host capabilities, and versioned execution-selection DTOs. It never depends on syntax.
- `browser` owns protocol-neutral page/context/frame/action/upload/download/dialog/state/network-route traits and normalized schemas/errors.
- `browser-cdp` implements Chrome target/frame sessions, physical actions, file protocol mechanics, download behavior, dialog handling, storage capture/application, Fetch/network interception, protocol barriers, and normalized events. CDP types remain private.
- `project` owns typed upload/download/state/network configuration, canonical roots, selection configuration, and configuration diagnostics. `analysis` receives resolved inputs and never reads ambient configuration.
- `runtime` owns resource tables, actor/page/frame/download lifetimes, state application/capture, policy scopes, route counters, cancellation/cleanup, iteration execution, and event correlation.
- `observation` owns revision-bound structured I failures/evidence and artifact references.
- the trace component owns versioned I trace schemas and safe rendering; it never executes route/locator logic.
- `editor` exposes shared I completion/hover/navigation/selection/inspection DTOs and current-revision observations.
- `lsp` remains protocol conversion/watched-input routing only.
- `dap` converts semantic runtime resources/scopes to protocol frames/variables and uses `RunControl`; it does not handle dialogs/routes itself.
- `wasm` exposes portable static I analysis, plans, selection/sharding, and descriptions with explicit host metadata/capabilities.
- `app` composes native files/artifacts/state commands/browser capabilities, parses CLI selection options, schedules the execution manifest, and renders reporters/exit classes.
- `editors/vscode` forwards structured test items/selections and maps shared results; it does not parse locators/routes/tags or compute shards.

No adapter-specific parser, locator evaluator, route matcher, storage codec, state registry, shard algorithm, diagnostic engine, or browser semantic implementation is permitted.

## 29. Delivery slices

Every slice is vertical and updates `webtest describe`, editor services, portable plan DTOs, DAP where executable, trace/events, examples, and tests applicable to its public surface. No slice may land a temporary adapter-only semantic implementation.

### Slice 1 — symbolic composable locators

Implement:

```text
first-class Locator type
recursive locator plan
descendant member composition
has/has_not/has_text/has_not_text
first/last/nth
strict resolution and evidence
inspection/describe/editor support
```

Prove existing primitive locator behavior and B actionability unchanged.

### Slice 2 — page resources and popup events

Implement:

```text
Page type/IDs/resource table
default-page compatibility
new/page scope/close operations
actor-aware page ownership and exclusivity
page.opened/page.closed normalized events
checkpoint/select popup workflow
trace/DAP page lanes
```

Prove immediate popups are not lost after a pre-action checkpoint.

### Slice 3 — frame scopes

Implement:

```text
FrameScope plan/HIR
locator-driven nested frame chains
same-origin frames
out-of-process cross-origin frames
frame re-resolution/lifecycle failures
frame-aware evidence/inspection/DAP
```

Do not implement frames as source-string rewriting or parent-page JavaScript selector tunneling.

### Slice 4 — interaction completion and typed evaluate

Implement:

```text
focus / blur
element/page scroll
drag-and-drop
typed evaluate arguments/results
operation-specific actionability/errors/events
```

Keep each operation explicit in plan/runtime traits.

### Slice 5 — uploads

Implement typed project/artifact upload descriptors, root/path/size validation, file-input semantics, Chrome protocol transfer, redaction, evidence, and frame support.

### Slice 6 — downloads

Implement actor download activation, normalized start/completion/failure events, checkpoint/select capture, `Download`/`DownloadedFile`/`ArtifactRef`, bounded completion, cleanup, trace attachments, and DAP metadata.

### Slice 7 — dialog policy scopes

Implement alert/confirm/prompt matching, pre-body installation barrier, safe unexpected-dialog policy, immediate constrained response, finite deadlines, normalized observations, cancellation, and trace/DAP behavior.

### Slice 8 — in-memory and fixture browser state

Implement backend-neutral state schema, capture/apply barriers, cookies/local storage/session storage/IndexedDB capability reporting, immutable fixture values, actor initialization, retry ownership, and secret redaction.

### Slice 9 — named browser-state artifacts

Implement workspace declarations, artifact metadata inputs, versioned codec, explicit update/list/clean commands, atomic/private storage, freshness/integrity validation, watched invalidation, `describe`, and init/documentation guidance.

### Slice 10 — network route core

Implement actor-scoped interception installation, normalized request matching, pure route filters, source-order choice, continue/abort/fulfill decisions, counters, request correlation, service-worker coverage validation, and structured cleanup.

Start with deterministic static fulfillment, then add request modification. Do not add HAR/WebSocket/upstream-response mutation through hidden shortcuts.

### Slice 11 — suite metadata and deterministic selection

Implement:

```text
declaration/case tags
shared SelectionExpr
exact/list selection manifest
versioned stable shard algorithm
repeat/IterationId
fixture/retry/report semantics
editor/WASM selection parity
```

No browser/application/fixture starts during list, filter validation, or shard computation.

### Slice 12 — migration hardening

Complete:

```text
cross-feature runtime composition
failure evidence and observations
trace viewer
DAP
inspection
editor actions
native/WASM parity
reporter/JUnit/event schemas
security review
performance budgets
production-style migration corpus
documentation and roadmap status
```

The milestone is not complete after browser-CDP methods exist; it completes only after the shared product surfaces and acceptance corpus pass.

## 30. Reference examples

These examples are normative for intent and must become installed canonical examples or equivalent focused fixtures as their slices land.

### 30.1 Refine a repeated row semantically

```webtest
test "remove one team member" {
    browser {
        open "/team"

        let member = role("row").filter(
            has: text("William Cotton"),
            has_not_text: "Owner",
        )

        click member.role("button", name: "Remove")
        expect member.detached
    }
}
```

The row may be one of many. The final button remains strict within the intended row.

### 30.2 Two pages under one actor

```webtest
test "support link opens documentation" {
    actor user browser

    user {
        open "/settings"
    }

    let mark = checkpoint(user)

    user {
        click role("link", name: "Documentation")
    }

    let docs = select user since mark within 5s {
        when page.opened(opened)
            if opened.url.host == "docs.example.test" {
            provide opened.page
        }

        timeout {
            fail "documentation page did not open"
        }
    }

    page docs {
        expect role("heading", name: "Settings").visible
    }

    user {
        expect url("/settings")
    }
}
```

### 30.3 Upload inside a payment frame

```webtest
test "purchase order is attached" {
    browser {
        open "/checkout"

        frame role("iframe", name: "Payment") {
            upload label("Purchase order") files [
                project_file("fixtures/purchase-order.pdf"),
            ]

            expect text("purchase-order.pdf").visible
        }
    }
}
```

### 30.4 Confirm and capture a download

```webtest
test "invoice can be exported" {
    actor user browser

    user {
        open "/invoices/123"
    }

    let mark = checkpoint(user)

    with dialog confirm(message: "Export invoice?")
        on user
        respond accept
        within 5s {
        user {
            click role("button", name: "Export")
        }
    }

    let pending = select user since mark within 5s {
        when download.started(download) {
            provide download
        }
    }

    let invoice = await download pending within 30s

    check {
        expect invoice.suggested_name == "invoice-123.pdf"
        expect invoice.size > 0
    }
}
```

### 30.5 Dynamic per-worker authentication

```webtest
export fixture authenticated_user(email: String) -> BrowserState scope worker {
    setup {
        server {
            let account = app.ensure_user(email: email)
        }

        actor login browser

        login {
            open "/test-login/{account.id}"
            expect text("Signed in").visible
        }

        provide capture browser_state from login {
            cookies: true,
            local_storage: true,
            indexed_db: true,
        }
    }
}

test "authenticated dashboard" {
    let state = use authenticated_user(email: "alice@example.com")
    actor alice browser state state

    alice {
        open "/dashboard"
        expect text("Welcome, Alice").visible
    }
}
```

The state is shared; the live actor is not.

### 30.6 Named persisted authentication

```webtest
export browser_state admin_auth {
    setup {
        server {
            let login = app.create_login_link(email: "admin@example.com")
        }

        actor login browser

        login {
            open login.url
            expect url("/admin")
        }

        provide capture browser_state from login {
            cookies: true,
            local_storage: true,
        }
    }
}
```

Explicit update and use:

```sh
webtest state update admin_auth
webtest test --filter 'tag("admin")'
```

```webtest
test "admin sees audit log" tags ["admin"] {
    actor admin browser state admin_auth

    admin {
        open "/admin/audit"
        expect role("heading", name: "Audit log").visible
    }
}
```

### 30.7 Deterministic HTTP fulfillment and count

```webtest
test "empty catalog is rendered" tags ["catalog", "mocked"] {
    actor shopper browser

    with routes on shopper {
        route catalog when request(r)
            if r.method == "GET" && r.url.path == "/api/catalog" {
            fulfill {
                status: 200,
                json: { items: [] },
            }
        }
    } {
        shopper {
            open "/catalog"
            expect text("No products available").visible
        }

        expect catalog.calls == 1
        expect catalog.fulfilled == 1
    }
}
```

### 30.8 Modify and abort requests

```webtest
test "analytics cannot affect checkout" {
    actor shopper browser

    with routes on shopper {
        route test_header when request(r)
            if r.url.host == "app.example.test" {
            continue with {
                headers: add {
                    "x-webtest-run": "checkout",
                },
            }
        }

        route analytics when request(r)
            if r.url.host == "analytics.example.test" {
            abort "blocked_by_client"
        }
    } {
        shopper {
            open "/checkout"
            expect text("Order summary").visible
        }

        expect analytics.aborted >= 1
    }
}
```

### 30.9 Typed escape hatch

```webtest
test "application build is exposed" {
    browser {
        open "/"

        let build = evaluate<String>(
            "(args) => window[args.namespace].build",
            args: { namespace: "myApplication" },
        )

        expect build matches regex("^[a-f0-9]{40}$")
    }
}
```

### 30.10 CI selection

```sh
webtest test \
  --filter 'tag("smoke") && !tag("slow")' \
  --shard 2/8 \
  --repeat 3 \
  --jobs 4 \
  --reporter json
```

The JSON result identifies selection-manifest version, shard, variant, iteration, and retry attempt separately.

## 31. Testing requirements

Required coverage includes all earlier milestone gates plus the following.

### 31.1 Parser/CST/formatter

Cover valid, invalid, malformed, and half-typed forms for:

```text
locator member composition/filter/first/last/nth
page new/scope/close
frame scope and nesting
focus/blur/scroll/drag/upload/evaluate
download await
dialog scopes and responses
browser-state capture/declaration/actor initialization
route scopes/matchers/decisions
declaration/case tags
```

Every fixture asserts losslessness:

```rust
parse.syntax().text().to_string() == source
```

Recovery always makes progress and retains enclosing block/module structure. Formatter output is idempotent and preserves comments/trivia.

### 31.2 AST/HIR/analysis/plan origins

Golden tests cover:

- exact origin for every locator primitive/composition/filter/index;
- page/frame/resource binding identity/lifetime;
- separate drag source/target ranges;
- upload file and dialog matcher/response ranges;
- route/filter/action/counter origins;
- named state declaration/reference/capture origins;
- tag declaration/case origins;
- deterministic node/resource/declaration/selection IDs;
- native/WASM plan parity.

### 31.3 Locator composition

Fake and real Chrome conformance covers:

```text
multiple parents narrowed by descendant
has / has_not
has_text / has_not_text normalization
nested composition
first / last / nth and dynamic collections
out-of-range retry
final ambiguity
rerender between polls
open shadow roots and closed-shadow limitation
CSS/XPath relative roots
frame boundary prohibition
bounded evidence and redaction
```

Property tests compare optimized resolution against a simple reference semantic model over generated DOM fixtures.

### 31.4 Page lifecycle

Fake and real Chrome tests cover:

```text
new page
immediate popup after checkpoint
multiple popups and source-order selection
page closes before/after selection
explicit close and context cleanup exactly once
default-page compatibility
navigation/event identity per page
same-page parallel rejection
different-page parallel execution
actor close with multiple pages
browser disconnect
```

### 31.5 Frames

Real Chrome fixtures cover:

```text
same-origin iframe
cross-origin/OOPIF iframe
nested frames
ambiguous iframe locator
iframe appears late
iframe replaced during rerender
detach during action
frame navigation
frame-scoped semantic/CSS/XPath locators
upload/download/dialog inside frame where supported
source/evidence ranges
```

Servers bind random loopback ports; cross-origin fixtures use distinct assigned loopback ports/hosts without fixed ports.

### 31.6 Interactions and evaluation

Cover:

```text
focusable/non-focusable elements
blur active/inactive target
scroll containers and page
invalid/huge scroll deltas
drag/drop success, source/target obscured, detach mid-drag
typed evaluation success
argument serialization with quotes/Unicode/secrets
result type mismatch/cycles/oversize
timeout/cancellation/page close
retry-unsafe analysis
```

No test should prove evaluation by unsafe string interpolation.

### 31.7 Uploads

Cover:

```text
single/multiple inputs
hidden labeled input
input replacement
accepted project file
artifact-as-upload where supported
missing/unreadable/oversize file
path traversal and symlink escape
MIME/name metadata
Unicode filenames
secret/redacted path evidence
frame upload
cancellation and temporary cleanup
```

### 31.8 Downloads

Cover:

```text
immediate download after action checkpoint
suggested filename
completed size/digest/artifact
multiple downloads and filtering
download failure/cancel/timeout
page closes during download
oversize policy
partial artifact policy
actor/test cancellation cleanup
trace attachment integrity
malicious filename traversal
```

### 31.9 Dialogs

Cover:

```text
alert accept
confirm accept/dismiss
prompt accept with value/dismiss
message match/mismatch
unexpected dialog safe dismissal
dialog before child reaches next statement
no matching dialog timeout
two dialogs via sequential scopes
conflicting handlers
cancellation/browser disconnect
redaction and trace events
```

Stress tests deliberately emit a dialog synchronously from a click handler to prove preinstallation.

### 31.10 Browser state

Fake codec and real Chrome coverage includes:

```text
cookie capture/apply/isolation
local storage by origin
default-page session storage restore timing
IndexedDB capture/apply where advertised
unsupported component failure
state applied before first application script
fresh context per test/retry/iteration
fixture sharing immutable state without context sharing
named declaration resolution/import/rename
explicit update success and atomic failure preservation
missing/corrupt/checksum mismatch/incompatible/expired artifacts
private permissions and version-control guidance
size limits
payload absence from plans/events/traces/DAP/logs
```

### 31.11 Network routing

Fake backend/model and real Chrome coverage includes:

```text
install-before-child barrier
first source-order match
unmatched continue
continue unchanged
method/URL/header/body modification
abort reasons
JSON/text/binary fulfillment
redirect hops
navigation/subresource/fetch/XHR requests
page/frame/popup requests under one actor
separate actor isolation
route counters and failed decisions
passive request/response correlation
mocked response event metadata
route teardown and paused-request release
timeout/cancellation/browser disconnect
service-worker coverage allow/block/reject behavior
body/header/route-count limits
secret redaction
slow trace/reporter backpressure
```

Pure route matcher tests ensure no effectful HIR/plan form can enter the routing evaluator.

### 31.12 Selection and sharding

Tests cover:

```text
declaration/case effective tags
invalid/duplicate tags
all SelectionExpr nodes and escaping
platform-independent path/glob behavior
selection before fixture/app/browser startup
stable shard golden vectors
independence from discovery order/jobs/completion
no remap when unrelated variants are added for same total
empty shards
invalid index/total
repeat IterationId
retry AttemptId nested in iteration
fail-fast interaction
fixture lifetimes per shard/worker/test
deterministic final ordering
JSON/JUnit/event/trace identity
native/WASM parity
```

Publish shard-algorithm golden vectors as a compatibility fixture before release.

### 31.13 Cross-feature structured execution

Model/stress tests cover combinations:

```text
retry around page/frame actions
parallel independent pages
parallel actors with separate routes
popup selected inside retry
download during route scope
dialog during download-triggering action
state-initialized actors in cases/repeats
fixture state + retry + shard
route scope around page popup/frame
guard observing mocked 5xx response
timeout during state capture/route installation/download
race loser with active page/dialog/route/download resources
debug disconnect during interception/dialog/download
```

Property tests preserve:

```text
no runtime task/resource/handler/intercepted request survives its owner
teardown occurs at most once after successful acquisition
every terminal resource outcome is represented in events
```

### 31.14 Events/traces/reporters/observations

Golden and adversarial tests cover schema versions, event correlation, source revisions, route/page/frame lanes, iteration/attempt identity, redaction, boundedness, checksum/traversal safety, stale observation rejection, successful-rerun clearing, and viewer escaping.

### 31.15 Editor/LSP/DAP/WASM/extension

Protocol/parity tests cover:

```text
all new completion/signature/hover entries
definition/references/rename for state/routes/resources
inspection frame/composed candidates
Unicode UTF-8 -> UTF-16 -> DAP line mappings
DAP page/frame/resource scopes and safe dialog/route stops
test explorer tags and structured selection
native/WASM IDs/plans/diagnostics/descriptions/shards
watched state/config metadata invalidation
VS Code/Cursor compile/package without generated out edits
```

### 31.16 Migration corpus

Maintain a corpus of at least 50 representative tests drawn from at least 10 production-style application patterns, including:

```text
CRUD/search/filtering
authentication/authorization
admin/customer multi-actor flow
popup/OAuth-like handoff
cross-origin payment iframe
file upload
PDF/CSV download
confirm/prompt dialog
network fulfillment/abort/modification
repeated list/card disambiguation
CI filters/shards/repeats
```

The corpus may be repository-owned equivalents rather than copyrighted third-party test source. It must run without Playwright and record any intentionally deferred non-goal instead of silently substituting JavaScript for a core I feature.

## 32. Performance and boundedness

Add benchmarks and stress fixtures for:

```text
deep but allowed locator composition
large repeated candidate sets and filters
many pages/frames per actor
large uploads/downloads within configured bounds
many route definitions and high request volume
browser-state capture/apply across many origins
thousands of tagged variants
large shard totals and repeat counts
trace capture with routes/downloads
```

Measure and bound:

```text
locator plan size/resolution cost
page/frame resource-table growth
route-match latency per request
interception queue memory
state codec time/size
upload/download memory and disk use
selection/shard computation
event/trace volume
editor analysis/completion latency
```

Browser protocol IO must not block indefinitely on trace/report/editor consumers. File transfer should stream where backend APIs permit rather than copy unbounded payloads into multiple buffers.

Before final implementation, record baselines and enforce practical regression thresholds in CI. Exact thresholds must be measured, documented, and platform-aware rather than invented in this proposal.

## 33. Compatibility and versioning

### 33.1 Existing source

Every valid pre-I test retains behavior unless it depended on undocumented behavior explicitly corrected by this milestone.

- primitive locators retain B matching rules;
- uncomposed singular operations remain strict;
- `browser {}` and H actor blocks retain default-page behavior;
- existing void `evaluate` remains valid;
- no route/dialog/state policy is active unless authored/configured;
- without selection options, all discovered variants run according to existing policy;
- `--jobs` does not imply sharding or repeat.

### 33.2 Existing browser backends

Trait evolution may provide default unsupported-capability implementations only to keep test fakes compiling during migration. A production backend must advertise capability precisely. It cannot silently degrade an I plan to a different semantic operation.

The browser conformance suite is capability-gated but normative: every backend claiming a feature passes the same semantic tests.

### 33.3 Plans and builds

I increments plan/build schema versions for recursive locators/resources/routes/state metadata. Older runtimes reject plans using unknown I nodes/capabilities. They do not flatten them to JavaScript or ignore them.

State secret payloads remain external inputs and are never embedded in portable build envelopes.

### 33.4 Events and traces

Event/trace schemas gain versioned optional/required fields. Older viewers may display a safe generic unknown entry only where compatibility rules allow; they must not represent a routed request as an ordinary unmodified request or merge distinct pages/iterations.

### 33.5 Shard algorithm

Once released, `webtest-shard-v1` encoding/hash-to-shard mapping is immutable. A future algorithm uses a new explicit version and cannot change membership silently within the same toolchain contract.

### 33.6 Future browsers

Firefox/WebKit/BiDi backends implement the protocol-neutral I traits/conformance suite. No source construct in this milestone contains `cdp`, `chromium`, target IDs, or Chrome-only request fields merely to ease the first backend.

## 34. Documentation requirements

Documentation must explain clearly:

```text
locators are symbolic queries, not element handles
composition refines; strictness remains
nth is zero-based and deliberate
page is not actor; pages in one actor share context state
frames require explicit scope boundaries
checkpoint must precede popup/download stimulus
dialogs need a preinstalled response policy
routes are active control; network events are passive observation
route handlers are pure constrained decisions, not callbacks
BrowserState contains credentials and is not an application user
state applies only at actor creation
ordinary tests never update named state
repeat is not retry
jobs are not shards
suite fixtures are per shard invocation, not global across CI machines
```

Required guides include:

- refining repeated UI structures without CSS;
- popup/page/frame ownership and event-safe waiting;
- upload/download/dialog workflows;
- per-run fixture auth versus named persisted auth state;
- deterministic network mocking and invocation assertions;
- selecting, sharding, repeating, retrying, and interpreting identities;
- secret/path/body redaction and artifact safety;
- migrating common Playwright page/locator/route/storage-state idioms.

Update [`future-functionality.md`](./future-functionality.md) when this milestone is adopted so network control, complete browser actions/state, and CLI filtering are owned by I rather than left as unassigned umbrella roadmap text.

Every new public feature updates:

```text
webtest describe catalog/completeness tests
.agents/skills/webtest/SKILL.md when authoring guidance changes
initializer skill parity assertions
examples and implementation-status documentation
CLI help for state/selection flags
```

No separate agent-only dialect or Playwright translation table becomes authoritative. Agents and humans consume the same descriptions, diagnostics, and examples.

## 35. Acceptance criteria

Milestone I is complete only when all of the following hold.

1. Locator values can be bound/passed through typed browser helpers, composed by descendant/filter/ordinal operations, and remain symbolic until consumed.

2. Final singular operations retain strict zero/one/many behavior; no implementation or repair path silently selects the first candidate.

3. Descendant, `has`, `has_not`, `has_text`, `has_not_text`, `first`, `last`, and zero-based `nth` pass deterministic fake-DOM and real-Chrome conformance tests with precise origins/evidence.

4. Existing Milestone B locator/actionability tests pass unchanged, and composed actions rerun the complete locator against current DOM/frame state rather than stale handles.

5. One actor can own multiple explicit pages; default-page source remains compatible; page creation/use/close and actor teardown have typed exactly-once ownership.

6. A popup opened immediately by a click is reliably observed by a later H `select` when the source established a pre-action checkpoint.

7. Independent pages may run concurrently only under proven ownership rules, while overlapping mutation of the same page or conflicting actor-wide policy is statically rejected.

8. Locator-driven nested same-origin and cross-origin frame scopes execute ordinary semantic locators/actions without raw frame/CDP identities entering plans or source.

9. Frame replacement/detachment, unavailable cross-origin capability, and page/browser failure remain distinct structured outcomes with the failing frame locator range.

10. Focus, blur, scroll, drag-and-drop, and file upload behave through protocol-neutral browser contracts with operation-specific actionability, cancellation, evidence, and DAP support.

11. Upload paths cannot escape configured project roots through absolute paths, traversal, or symlinks; size/count limits and secret evidence policies are enforced.

12. Typed evaluation serializes arguments safely, decodes bounded transferable results through the shared type system, remains an opaque retry-unsafe effect by default, and does not become the implementation of core I workflows.

13. Download capture is armed before possible stimuli, immediate downloads are not lost, completed files have deterministic bounded artifact metadata, and partial/cancelled downloads cannot be reported successful.

14. Dialog handlers are installed before their child, respond within bounded time, safely dismiss/fail unexpected dialogs, and never become arbitrary effectful callbacks.

15. Immutable `BrowserState` can capture/apply the advertised cookies/storage components, initialize a fresh actor before application code runs, and integrate with fixture lifetime/retry semantics without sharing a live context.

16. Named browser-state declarations are statically discoverable and explicitly updateable through an atomic versioned artifact workflow; ordinary test runs never create/overwrite state and no state payload leaks into plans, traces, reporters, DAP, LSP, or logs.

17. Actor route scopes install before their child and deterministically match normalized structured requests in source order.

18. Continue, request modification, abort, and JSON/text/binary fulfillment operate through protocol-neutral route plans with bounded pure expressions and no provider/browser callbacks.

19. Route invocation/decision counts are typed and assertable, and every active decision correlates with H passive network events and E trace events through WebTest semantic request/route identity.

20. Service-worker/request-class interception gaps cause explicit activation failure or an explicit blocking policy; WebTest never claims a deterministic mock while knowingly unable to observe eligible requests.

21. No intercepted request, dialog handler, page/frame session, download, upload temporary, or state task survives its structured owner after success/failure/timeout/cancellation/race loss/debug disconnect.

22. Declaration/case tags are static, included in shared discovery metadata, and produce deterministic effective variant tag sets without changing `TestVariantId`.

23. CLI, editor, and portable clients evaluate one shared bounded `SelectionExpr`; no extension-owned filter parser/evaluator exists.

24. Shard membership is platform-independent, versioned, stable for a given `TestVariantId`/total, independent of discovery/jobs/completion, and backed by published golden vectors.

25. Repeat executions use distinct `IterationId`s, retries use nested distinct `AttemptId`s, all iterations preserve one stable `TestVariantId`, and aggregate/JUnit/JSON outcomes preserve the difference.

26. Suite/file/worker/test fixture acquisition under filters/shards/jobs/repeats matches the documented ownership model and never assumes cross-shard shared memory.

27. Traces reconstruct selection, iterations/attempts, actor/page/frame lanes, locator refinement, uploads/downloads, dialog handling, state application metadata, route decisions/counters, cleanup, source, and evidence without executing project code.

28. DAP can pause/inspect/step through safe I nodes, identify semantic actor/page/frame/resource scopes, and disconnect with complete cleanup without exposing secret/raw backend state.

29. Editor services and `webtest inspect` provide composed/frame-qualified candidates and shared completion/navigation/hover/actions entirely from Rust semantics.

30. `webtest describe` exposes every public I language/browser/type construct with accurate syntax, parameters/results, contexts, capabilities, effects, failures, constraints, examples, related topics, and availability; installed examples parse/analyze and native/WASM portable descriptions agree.

31. All path/body/header/state/event/trace inputs are bounded, redacted, integrity checked where applicable, and adversarially tested.

32. The 50-test/10-application production-style migration corpus runs on managed Chromium without Playwright and without using evaluation to replace a core I feature.

33. Full workspace formatting, unit/integration/doc tests, clippy, real-Chrome conformance, bridge/provider, structured-concurrency, trace, LSP, DAP, WASM parity, extension compile/package, security, and performance gates pass.

34. No second parser, formatter, semantic model, locator evaluator, route engine, browser-state codec/registry, shard algorithm, CDP-shaped DSL, host callback runtime, or adapter-specific browser behavior is introduced.

The roadmap acceptance statement is thereby satisfied: ordinary production Chromium E2E workflows involving authentication state, refined semantic locators, pages, frames, uploads, downloads, dialogs, deterministic HTTP control, and CI distribution are expressible and reliable within WebTest's one statically analyzable language/runtime architecture.

## 36. Long-term implication

After Milestone I, WebTest's remaining difference from full Playwright-market parity is primarily deliberate breadth rather than a missing Chromium E2E foundation:

```text
cross-browser backends
browser/device project matrices
visual baselines
emulation and fake time
HAR/WebSocket routing
distributed execution
```

The ordinary Chromium workflow is complete enough to center WebTest's stronger product thesis:

```webtest
test "user can cancel order" tags ["orders"] {
    server {
        let user = app.create_user(email: "alice@example.com")
        let order = app.create_order(user_id: user.id)
    }

    let auth = use authenticated_user(user_id: user.id)
    actor customer browser state auth

    with routes on customer {
        route order_request when request(r)
            if r.method == "POST" &&
               r.url.path == "/api/orders/{order.id}/cancel" {
            continue
        }
    } {
        customer {
            open "/orders/{order.id}"
            click role("button", name: "Cancel order")
            expect text("Cancelled").visible
        }

        expect order_request.calls == 1
    }

    server {
        let actual = app.order(id: order.id)
        expect actual.status == "cancelled"
    }
}
```

The example combines statically checked backend state, a typed authenticated browser participant, deterministic browser/network behavior, and backend verification in one source-mapped plan.

The runtime owns the hard operational mechanics:

```text
locator strictness and actionability
page/frame/context ownership
pre-stimulus event retention
dialog/request interception barriers
state isolation and credential handling
file/artifact safety
cancellation and cleanup
route/event correlation
retry/iteration/shard identity
trace/debug evidence
```

The author states intent. WebTest preserves that intent through analysis, execution, observability, editor intelligence, and portable machine interfaces without turning the language into JavaScript or duplicating its semantics in an adapter.
