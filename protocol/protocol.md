# WebTest App Bridge Protocol 1

This document is normative for protocol state, framing, authentication, correlation, calls,
cancellation, and shutdown. [`schema.json`](./schema.json) is normative for JSON message shapes;
[`types.json`](./types.json) is normative for transferable schema/value types.

## Compatibility

Protocol versions are positive integers. A peer advertises every version it implements and the
runner selects the highest version shared by both peers. Version 1 requires unknown object fields
to be ignored. A new optional field is therefore compatible with version 1; renaming or removing a
field, changing its meaning, or changing framing requires a new version.

An SDK publishes its name/version, minimum and maximum protocol version, generated schema revision,
and supported transports. The tested release matrix is:

| Implementation | Version | Protocol range | Generated schema revision | Transports |
| --- | --- | --- | --- | --- |
| WebTest runner | 0.1.0 | 1–1 | 1 | Unix socket, Windows named pipe, loopback TCP, stdio |
| `@webtest/node` | 0.1.0 | 1–1 | 1 | Unix socket, Windows named pipe, loopback TCP, stdio |
| `webtest-app-bridge` Ruby | 0.1.0 | 1–1 | 1 | Unix socket, loopback TCP, stdio |
| no-SDK reference executable | 0.1.0 | 1–1 | 1 | Unix socket, loopback TCP, stdio |

## Framing and limits

Each frame is one UTF-8 JSON object followed by LF. A sender JSON-escapes newlines inside strings.
The default maximum encoded object size is 1,048,576 bytes. Invalid UTF-8, invalid JSON, a non-object,
an unknown message type, or an oversized frame is a protocol failure and closes the connection.
Socket bytes and stdio stdout contain frames only; logs use stderr.

Values are additionally bounded to 32 levels, 1,048,576 bytes per string, 10,000 items per
collection, 1,024 pending calls, and 64 KiB of captured stderr by default. Runners may lower these
limits and communicate the selected frame limit in `hello_ok`.

## Connection state

```text
Connected -> AwaitHello -> Ready -> Draining -> Closed
                 |           |
                 +-> Closing  +-> Closing on protocol/transport failure
```

The bridge sends `hello` first. No other message is accepted in `AwaitHello`. Authentication uses a
new 256-bit `WEBTEST_TOKEN` for each owned run and a timing-resistant comparison. Negotiation selects
the highest common version. Failure sends `hello_error` with `authentication_failed`,
`unsupported_protocol`, or `expected_hello`, then closes. Tokens never enter diagnostics or logs.

After `hello_ok`, the runner sends `describe` and verifies that the canonical live schema hash equals
the offline planned hash before dispatching a call. A mismatch is `app_schema_drift`; dynamic fallback
is forbidden.

In `Ready`, request IDs are unsigned integers unique among in-flight requests. Results may arrive out
of order. Duplicate IDs, unknown IDs, multiple terminal responses, or a terminal response of the
wrong kind are protocol failures. `event` messages are ordered per call and do not terminate it.
`ping` receives `pong` with the same ID.

`cancel` is cooperative. A bridge that supports cancellation cancels its host task/context and sends
exactly one terminal `error` or `result`. `shutdown` moves both peers to `Draining`: new calls are
rejected, in-flight calls receive a bounded grace period, the bridge sends `shutdown_ok`, and closes.
EOF fails every pending call as infrastructure failure.

## Schema identity

`functions` is encoded as canonical JSON by recursively sorting every object key, preserving array
order, using UTF-8 without insignificant whitespace, and using ordinary JSON scalar encoding. The
BLAKE3 digest of those bytes is rendered as `blake3:<lowercase hex>`. Documentation participates in
the version-1 hash. Required/optional/default/secret, retry safety, aliases, validation/display
metadata, parameter/result types, and field names are semantic and also participate.

Documentation is plain UTF-8 text, contains no control characters other than tab/newline, and is at
most 1,024 bytes per entry. Defaults must validate against their parameter type and may appear only
on optional parameters.

## Calls and failures

`call.arguments` is an object validated by the bridge SDK and again by the runner. `result.value` is
validated against the declared result type by both peers. Application exceptions become `error`
with a stable application-owned code, bounded human message, retryability fact, and transferable
data. Stack traces are omitted unless separately enabled and remain bounded/redacted.

Application `error` messages are provider/test failures. Authentication, negotiation, schema drift,
malformed/oversized frames, invalid results, transport loss, and process death are infrastructure
failures. Secret parameters are redacted before events, DAP scopes, observations, or reports are
constructed. Operation `retry_safe` and an individual error's `retryable` are independent facts.

## Discovery and transports

For runner-managed applications, WebTest creates a restricted endpoint, generates a token, and
launches the configured executable without a shell with `WEBTEST_BRIDGE`, `WEBTEST_TOKEN`, and
`WEBTEST_PROTOCOL=1`. Applications connect outward. Unix endpoints use mode 0600 inside a 0700
directory; TCP listeners bind only `127.0.0.1`; Windows uses a per-run named pipe. Owned child
processes use bounded health/bridge readiness deadlines and are killed/reaped on success, failure,
timeout, cancellation, or adapter disconnect.

Persistent stdio bridges receive the same token/version environment, emit `hello` on stdout, and use
stdin/stdout for frames. The `command` compatibility adapter receives one JSON document on stdin and
returns `{ "value": ... }` or `{ "error": ... }`. The declarative `http` adapter maps only explicitly
configured functions to endpoints and never creates or infers a public control route.
