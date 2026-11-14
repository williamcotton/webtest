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

When this repository is the editor workspace, select **Debug chat (headed)** in
Run and Debug and press F5. That launch targets the chat file explicitly and pauses
at entry, even if `launch.json` or another example is the active tab. The current
extension discovers tests using the active WebTest document's nearest project, so
the chat provider schema is used when other example projects are open.

Debug pauses count toward deadlines. This demo allows ten minutes per test for
manual stepping; its event selections and UI waits still have five-second limits.

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
