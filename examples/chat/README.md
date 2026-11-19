# Side by Side: Alice and Bob chat

A working two-person chat application and an advanced WebTest scenario. Alice
and Bob sign into separate isolated browser contexts, exchange real WebSocket
messages, and reload their pages without losing their separate identities.

## Run the browser test

From the repository root, with Node.js 22+ and Chrome available:

```sh
cargo build
npm --prefix examples/chat ci
target/debug/webtest check examples/chat
target/debug/webtest test examples/chat --headed
```

WebTest starts and stops the app itself. Do not start `npm start` separately when
running these tests. Omit `--headed` for headless execution. Two labeled variants
exercise a normal greeting and Unicode/literal HTML text. HTML is rendered as
text, never inserted as markup.

Editor Run actions also stop their application before returning a result. You can
then start a debug session on the same configured port; stop an active session
before starting another execution mode.

The main scenario is [tests/conversation.webtest](tests/conversation.webtest).
Its imported patterns, guard, and actor-aware helper live in
[support/chat.webtest](support/chat.webtest).

The test demonstrates:

1. Typed server-side setup through `app.prepare_room`, an authenticated private
   bridge operation that creates a fresh room and secret credentials per variant.
2. Two `BrowserActor` resources and concurrent sign-in through a reusable helper.
   The server derives each sender from its own HTTP-only session cookie.
3. Explicit receiver checkpoints **before** the other actor clicks Send. A later
   `select` can therefore see an event even if delivery preceded the select step.
4. Pattern-filtered `websocket.received_json` alternatives, a provided event
   payload, precise timeout failures, and checks against the visible transcript.
5. Scoped console/server-error guards installed around both participants.
6. Concurrent reloads that verify cookie isolation, restored identity, and shared
   room history. There are no sleeps or arbitrary browser JavaScript in the test.

`webtest.toml` enables traces with source snapshots. The JSON reporter lists trace
paths; open a conversation trace with `target/debug/webtest trace <path>` to inspect
the actor lanes, provider call, checkpoints, selected events, guards, and verdict.
Use `--reporter json` on `test` to obtain that report. Password fields are marked
secret in the offline bridge schema.

To debug a single variant, open the test in Cursor/VS Code and use its Testing
view, or put breakpoints on the send/select assertions and run Debug Current File.
The runner owns both contexts through cleanup.

The file contains two cases, so a breakpoint on `app.prepare_room` is reached once
for each case. Debug one case from the Testing view to run just that conversation.
In Variables, **Locals** contains authored bindings, **Actors and checkpoints**
shows named browser resources, and **Execution context** identifies the current
case and operation. Expand **Details** or **Internal** to inspect resource IDs.

Open `tests/conversation.webtest` and run **WebTest: Debug Current File**, or use
F5 with WebTest selected as the debugger. No repository `launch.json` is needed.
Enable the extension setting `webtest.debug.stopOnEntry` to pause at entry;
otherwise set an enabled breakpoint before launching. The current extension
discovers tests using the active WebTest document's nearest project, so
the chat provider schema is used when other example projects are open.

With pause-on-entry enabled, execution pauses first on the case-row binding. Use **Step Into
(F11)** to reach `app.prepare_room`, enter nested scopes, and follow helpers.
**Step Over (F10)** runs nested scopes before pausing at the same depth; from the
case binding, it can run through the rest of this deeply nested scenario. Use
**F5** to continue to an enabled breakpoint, and **Shift+F5** to stop and clean up.
On keyboards with media keys, the function keys may also require **Fn**.

If an editor opened from the macOS Dock cannot find Node.js, add this extension
setting to your **user settings**, then reload the window:

```json
"webtest.environment": {
  "PATH": "/opt/homebrew/bin:/usr/local/bin:${env:PATH}"
}
```

Adjust the PATH if Node is installed elsewhere. The override applies to both Run
and Debug, including the Testing view, without tying either action to this test.
Application startup errors appear in the debugger.

Debug pauses count toward deadlines. This demo allows ten minutes per test for
manual stepping; its event selections and UI waits still have five-second limits.

## Language walkthrough

Read [tests/conversation.webtest](tests/conversation.webtest) alongside
[support/chat.webtest](support/chat.webtest). The test reads mostly from top to
bottom: prepare data, sign in two people, exchange messages, then reload. The
extra structure makes ownership, concurrency, and event timing explicit.

### Imports and exports

```webtest
import { Message, healthy, sign_in } from "../support/chat.webtest"
```

The relative path names another WebTest source file. The braces select three
exported declarations: a pattern, a guard, and a function. Their definitions start
with `export` so other files can import them. Importing the support file makes
those definitions available; calling `sign_in` or activating `healthy` is what
performs their work.

### One test, two labeled cases

```webtest
test "Alice and Bob exchange messages in separate browser sessions" cases [
    case "a friendly hello" { greeting: "Hello, Bob!", reply: "Hi, Alice! Good to see you." },
    case "Unicode and literal HTML" { greeting: "Coffee, Bob? ☕ <hello>", reply: "Yes! Tea & cake for Alice 🍰" },
] as conversation {
    // The same test body runs for each row.
}
```

`test` gives the scenario its human-readable name. `cases [...]` supplies two
statically enumerated variants with distinct labels. Each row is a record with
`greeting` and `reply` String fields. `as conversation` binds the current row so
the body can read `conversation.greeting` and `conversation.reply`.

The complete body runs once per row, including room creation and actor creation.
The second row checks Unicode and literal HTML-looking text without duplicating
the scenario. Case data must be deterministic, transferable values with compatible
record shapes; a provider call cannot generate these rows at runtime.

### Typed server setup and immutable bindings

```webtest
server {
    let room = app.prepare_room(label: "Alice & Bob · a private conversation")
}
```

`server` permits server-provider operations. `app.prepare_room` is a project
operation described by the offline bridge schema in
[.webtest/app-schema.json](.webtest/app-schema.json). `label:` is a named argument.
The operation creates a room and credentials; it does not interact with a browser.

`let` creates an immutable binding whose type is inferred. The schema says `room`
is a record containing `path`, `alice`, and `bob`; each person has `name`, `email`,
and `password` String fields. Thus `room.alice.email` is statically checked field
access, rather than an unchecked lookup into arbitrary JSON. Later statements
can use the transferable setup result in browser operations. Password fields
carry the schema's secret metadata for redacted observations.

This is a direct provider call, not a WebTest `fixture` declaration. The runner
starts the configured application, invokes its authenticated bridge, and stops
the application after execution. The room lives in the demo server's memory.

### Actors represent separate browser identities

```webtest
actor alice browser
actor bob browser
```

Each declaration owns an isolated browser context. Alice and Bob have separate
cookies and login state even though they visit the same application. Their
contexts remain available through the enclosing scope and are cleaned up by the
runner.

```webtest
alice {
    fill label("Message") with conversation.greeting
    click role("button", name: "Send message")
}
```

An actor block directs browser operations to that actor's page. Reopening Alice's
page later uses Alice's existing context and cookies; it does not create a third
participant. Actors are owned resources, not ordinary record values. The compiler
rejects concurrent exclusive access to the same actor.

### Typed statement helpers

The support file defines:

```webtest
export fn sign_in(actor: BrowserActor, path: String, email: String, password: String) -> Null {
    actor {
        open path
        fill label("Email") with email
        fill label("Password") with password
        click role("button", name: "Join conversation")
        wait role("button", name: "Send message").enabled within 5s
    }
}
```

`fn` declares a reusable function with typed parameters. `BrowserActor` explicitly
passes the caller's browser resource; the three `String` parameters pass data.
`-> Null` declares that this helper produces no useful result value. Its body
performs browser statements against the supplied actor.

Calling `sign_in(alice, room.path, room.alice.email, room.alice.password)` uses
positional arguments. The helper does not create or replace Alice's browser
identity. Inside the helper, `actor { ... }` refers to its parameter, just as
`alice { ... }` refers to the named actor in the test.

### Browser actions, locators, states, and waits

The helper and scenario use several browser constructs:

- `open path` navigates the actor's page. Relative paths such as `room.path`
  resolve against the configured browser base URL.
- `label("Email")` locates a form control by its label;
  `fill ... with email` enters the supplied String.
- `role("button", name: "Join conversation")` locates a button by role and name;
  `click` activates it.
- `text("Signed in as Alice")` locates visible-content text. `.visible` and
  `.enabled` express the element states being checked.
- `wait ... within 5s` explicitly waits for a state, with a five-second limit.
  `5s` is a Duration literal. The enclosing test deadline still applies.

For example, the sign-in helper waits until sending is enabled before returning.
After selecting a WebSocket event, the test separately waits for its text to
appear in the page. Receiving data and rendering it are distinct observations.
There are no fixed sleeps, CSS selectors, or evaluated JavaScript in this test.

### Expectations and soft check scopes

```webtest
alice {
    check {
        expect text("Signed in as Alice").visible
        expect text("Bob is online").visible
    }
}
```

`expect` asserts a condition. Here the conditions are browser visibility states;
elsewhere the test asserts structural matches on values.

`check` runs its statements sequentially and collects assertion-class failures,
so both visibility checks can report a mismatch. It does not make them parallel
or turn them into explicit five-second waits. A failed check scope still fails
the scenario; it is not a way to ignore failures. Provider, browser-action,
infrastructure, cancellation, timeout, and cleanup failures terminate the check
immediately instead of being accumulated.

### Parallel branches and joining

```webtest
parallel {
    alice {
        sign_in(alice, room.path, room.alice.email, room.alice.password)
    }
    bob {
        sign_in(bob, room.path, room.bob.email, room.bob.password)
    }
}
```

The two direct child blocks run concurrently. Statements inside each branch
remain sequential. The next statement after `parallel` runs after both branches
and their cleanup have finished. Separate actors make these browser operations
safe to run concurrently; two branches could not both act on Alice's page.

The branches read immutable outer data such as `room`. Their local bindings do
not merge back into the parent. The second parallel block reloads both pages,
then checks each participant's restored identity and the other person's message.
Those checks exercise both cookie isolation and shared room history.

### Reusable structural patterns

```webtest
export pattern Message(sender: String, body: String) = subset { "type": "message", id: range(1, 100000), "from": sender, body: body }
```

`pattern` declares a reusable matcher with two typed parameters. A value matching
`Message("Alice", conversation.greeting)` must be a record with:

- `type` exactly equal to `"message"`;
- `id` within the inclusive numeric range 1 through 100000;
- `from` exactly equal to `"Alice"`;
- `body` exactly equal to the current case's greeting.

`subset` allows additional fields. The quoted record keys `"type"` and `"from"`
are field names, while their values are matchers. In `body: body`, the left side
names the field and the right side uses the pattern parameter. `range(...)` is a
built-in matcher, not an array constructor or loop.

`value matches Message(...)` produces a Boolean when used in a predicate.
`expect value matches Message(...)` makes it an assertion and can report a
structured mismatch path. The same declaration therefore filters incoming
events and checks the selected payload.

### Checkpoints prevent missed-event timing bugs

```webtest
let bobBeforeGreeting = checkpoint(bob)
alice {
    fill label("Message") with conversation.greeting
    click role("button", name: "Send message")
}
```

`checkpoint(bob)` establishes a backend barrier and records a cursor into Bob's
ordered event journal. It happens **before Alice sends**, because Bob might
receive the message before execution reaches `select`.

The later `since bobBeforeGreeting` includes events recorded after that cursor,
even if they have already arrived. Without `since`, selection begins observing
at select entry and could miss that earlier delivery. A checkpoint does not send
anything or wait for the desired message. It is bound to its actor and execution,
and cannot be serialized or used as ordinary application data.

### Event selection, predicates, provided values, and timeouts

```webtest
let deliveredToBob = select bob since bobBeforeGreeting within 5s {
    when websocket.received_json(event) if event.value matches Message("Alice", conversation.greeting) {
        provide event.value
    }
    timeout {
        fail "Bob never received Alice's greeting"
    }
}
expect deliveredToBob matches Message("Alice", conversation.greeting)
```

`select bob` scans Bob's journal for a matching event. The `when` clause chooses
the normalized `websocket.received_json` event source and binds its event envelope
to `event`. `event.value` contains the decoded application JSON payload; fields
such as `from` and `body` belong to that payload, not the envelope.

The optional `if` predicate must be true before the arm is selected. Unrelated
JSON events do not satisfy this Message pattern. Selection uses journal order
and executes exactly one selected body; it does not speculatively execute every
alternative. If alternatives match the same event, their source order breaks
the tie.

`provide event.value` supplies the bound select's result, which becomes
`deliveredToBob`. If no arm matches within five seconds, the `timeout` body raises
an explicit failure with a useful explanation. The timeout path terminates, so
it needs no result value. Parent deadlines and cancellation can still end the
selection earlier; event loss or disconnection is not silently treated as a
successful timeout.

The reply uses an unbound `select alice ...`: it asserts inside the selected arm
instead of returning a payload. Both directions then check the corresponding UI.

### Scoped guards watch for errors throughout the conversation

```webtest
export guard healthy(actor: BrowserActor) {
    when actor.console.error(event) {
        fail event.message
    }
    when actor.network.response(response) if response.status >= 500 {
        fail "The chat application returned a server error"
    }
}
```

A `guard` declares reusable event observations. This one fails on a browser
console error or an HTTP response with status 500 or higher. The second clause
uses field access, a numeric comparison, and an `if` predicate to restrict which
responses trigger it. These clauses observe browser events; they do not inspect
the server process's stderr or capture HTTP response bodies.

```webtest
with guards [healthy(alice), healthy(bob)] {
    // Sign in, exchange messages, and reload.
    pass
}
```

`with guards` activates the declarations for the duration of this body, before
its operations begin. Passing each actor installs the same policy for both
participants. A trigger cancels the owned body, awaits its cleanup, and executes
the guard's failure body. Leaving the scope ends the guards. They do not compete
with the explicit selects for a single consumable message.

The final `pass` explicitly ends the variant successfully after owned cleanup.
It does not erase an earlier check failure or turn cleanup failures into success.
`fail "..."` records a deliberate assertion-class failure with the given reason.

### Configuration supplies the environment

[webtest.toml](webtest.toml) completes the example without adding language syntax:
`test_roots` discovers the scenario, `source_roots` includes its support module,
`[app]` owns the Node server, and `[server.app]` supplies the bridge schema and
transport. `[browser]` sets the base URL and viewport. The test's ten-minute
budget supports manual debugging; individual event selections and UI waits still
use their explicit five-second limits. Traces include source snapshots so the
execution can be inspected alongside these declarations.

## Try the app by hand

```sh
npm --prefix examples/chat start
```

Visit <http://127.0.0.1:3110> in two **different browser profiles** (or a normal and
private window). Sign in as `alice@chat.test` and `bob@chat.test`, both with password
`hello-chat`. Two ordinary tabs share cookies; separate profiles reproduce the
test's actor isolation. `PORT=3111 npm --prefix examples/chat start` changes the port.

This is a loopback-only, in-memory demonstration. Restarting clears messages and
sessions. It is not a production authentication or persistence implementation.
The test bridge is activated only by `WEBTEST=1` in the runner's configuration;
normal manual startup exposes no fixture HTTP endpoints. The app honors
`WEBTEST_APP_PORT` when WebTest starts isolated workers.

## App regression check

```sh
npm --prefix examples/chat test
```

This starts the app on an ephemeral loopback port and verifies independent cookie
sessions, presence, bidirectional delivery, sender spoofing resistance, and replay
after reconnection. The `.webtest` scenario adds the real browser/UI proof.
