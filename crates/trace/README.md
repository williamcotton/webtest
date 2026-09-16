# Trace artifact core

`webtest-trace` writes and validates a deterministic archive of native observation
records. It depends on shared observations and source identity, never the runtime,
compiler, browser backend, project configuration, or reporters. It performs no
filesystem access, extraction, network requests, or project execution.

This is the artifact foundation for Milestone E. `webtest trace <artifact>`
validates an existing archive and serves the shared viewer on a random loopback
port; `--output <file>` writes standalone HTML without overwriting an existing
file. Budgeted capture records explicit omissions of optional content. The native
app exports completed file runs under configured capture/retention
policy with atomic non-overwriting publication; the core remains filesystem-free.

## Container version 1

The uncompressed binary container has this exact layout:

1. Fifteen magic bytes: `WEBTEST-TRACE` followed by `00 01`.
2. A little-endian unsigned 32-bit entry count.
3. For each entry: an unsigned byte kind (`0` means regular file), a little-endian
   unsigned 32-bit UTF-8 name length, a little-endian unsigned 64-bit content
   length, name bytes, and content bytes.

There is no padding or trailing data. Writers sort entries lexically by their
normalized relative names. Readers accept any entry order but reject duplicate
names. No compression, links, directories, permissions, executable entries, or
timestamps are represented. Unknown entry kinds are rejected. Lengths and counts
are checked before allocation; supplied archive bytes also have an independent
bound. This small container framing is independent of execution representation.

Logical members are `manifest.json`, `events.jsonl`, optional
`sources/<64 lowercase hexadecimal revision digits>`, and optional
`attachments/<64 lowercase hexadecimal BLAKE3 digest digits>`. The reader rejects
absolute paths, traversal, empty components, backslashes, drive/URI prefixes,
control characters, unsupported members, and any unreferenced member. It never
turns even a validated name into a host path.

## Manifest and journal

Manifest format 2 versions the trace separately from the native event schema and
container framing. Readers also accept format 1 without capture reports.
It carries compiler/browser/provider versions, project identity, a configuration
fingerprint (no raw configuration), capture policy, execution membership, source
mapping, and byte lengths/BLAKE3 checksums for every other member. A checksum
verifies consistency, not the identity of the producer. Unknown manifest fields
or unsupported versions are rejected rather than guessed.

The journal contains the unchanged `RecordedEvent` JSON envelopes, one per line
with a final newline. Shared `ReplayJournal` validation checks native schema,
immutable identity, and source/context agreement. The trace additionally requires
ordered unique sequences and a terminal per execution. An omitted interval is
accepted only when exactly covered by the terminal's typed journal-overflow fact.
There is no implied ordering across distinct executions. A journal exhausted
before its start record remains readable when its reserved terminal explicitly
accounts for that loss.

Source mappings use `(ExecutionId, FileId)` because separate executions may reuse
a local file ID. Optional source content must match its declared revision and
the source ranges must be valid UTF-8 boundaries. Captured content is deduplicated
by revision/digest. Attachment mappings use the immutable attachment-event
identity and must agree with its acknowledged length and digest. Historical host
paths remain in the unchanged journal as data; consumers resolve captured content
only through these manifest mappings. They must never open a path from a journal
payload.

## Capture and bounds

Callers explicitly select source and attachment inclusion. Source text is exact,
opt-in content; it may contain source literals. Callers supply already-redacted
journal and attachment bytes, and the writer preserves those bytes rather than
reinterpreting upstream values or collecting files itself. Omitted captures have
explicit absent manifest references, not dangling links. Included captures must
be complete and checksum-valid.

Independent bounds cover archive bytes, total member bytes, each member,
manifest bytes, event-record bytes, member count, and event count. All limits are
inclusive. `write` is strict: exceeding any bound returns a typed error and no
output archive. `write_budgeted` can omit whole optional source/attachment members
to meet archive-byte, total-byte, per-member-byte, and member-count bounds. It
validates supplied captures before selection, including captures ultimately omitted.
The journal is never shortened or rewritten. Mandatory journal/metadata limits,
including the initial manifest size, still fail without producing an archive.

Budgeted manifests include a `capture_report` with the four capture limits and
typed `truncated` export facts carrying each omitted member's path, length, and
digest. These facts belong to the artifact, not the runtime journal: their sorted
vector positions never borrow execution IDs or event sequences. An empty report
means nothing was truncated; absent content under a disabled capture policy has
no truncation fact. Readers verify declared retained sizes, require an exact
omission reference for every budget-dropped capture, and reject duplicate,
orphaned, contradictory, or version-1 capture reports.

Selection visits largest members first with lexical path tie-breaking, dropping
members until the capture fits. Shared content is charged once and dropped for
every reference together with one truncation fact. This deterministic policy does
not promise optimal packing. Metadata and framing, including the truncation facts,
count toward their respective limits. Input content stays borrowed until selection
finishes. Both APIs return a complete byte buffer for atomic adapter publication.

## Viewer

`render_html` consumes only a `VerifiedTrace`. Its inclusive output-byte limit
covers escaped text and base64 evidence; an overrun returns no partial document.
The script-free HTML contains the per-execution timeline, explicit parent-scope
links, attempt/operation identity, all typed facts, included source text, and
verified evidence. It retains complete recorded JSON in expandable details.
Policy-disabled capture, budget-truncated content, and missing parent facts stay
distinct and explicit. The viewer displays the capture report separately from the
unchanged runtime timeline. Wall time is shown in
the original fact; the timeline uses recorded elapsed time, never wall-clock
sorting or a fabricated cross-execution order.

Untrusted strings are escaped, DOM/text evidence is rendered as text, and only
PNG bytes are used for image previews. Captured bytes may be downloaded through
an embedded data URL; historical host paths never become links. There are no
scripts, external assets, frames, forms, or network requests from the page. A
restrictive content-security policy applies to both served and static output.
The app owns bounded regular-file input, atomic non-overwriting HTML output,
the random loopback listener and unguessable URL, request validation, connection
limits/timeouts, and shutdown. It does not invoke project discovery or execution.

## Native test-run export

The app accepts `[trace]` in `webtest.toml`:

```toml
[trace]
mode = "retain-on-failure" # off (default) | on | retain-on-failure
sources = false           # exact analyzed source; opt in explicitly
attachments = true        # acknowledged evidence under existing [evidence] settings
max_bytes = 104857600      # 1..104857600, per archive including metadata
```

After application cleanup, the app publishes one archive per executed file to
`[artifacts].directory/traces/<execution_id>.webtest-trace`. Retain-on-failure
considers the whole invocation, including static failures and application teardown.
Static or pre-admission startup/identity failures have no fabricated journal.
Reports retain all test outcomes and separately identify published paths or
infrastructure export errors. Passing retention-off runs create no trace directory.
Native report schema 12 includes `traces` and adapter `trace_export` envelopes;
these do not consume native journal sequences.

The app uses the compiled source snapshot and only live acknowledged evidence in
the runtime artifact directory, rejects non-regular/link inputs and escaping
storage, and verifies digests before capture. Evidence staging has a separate
100 MiB hard limit per file export; exceeding it fails export without publication.
Optional content below that staging limit can be dropped by the capture budget.
Publication uses a same-directory temporary file, sync, and non-overwriting commit.
Existing traces are never deleted by retention or overwritten by later runs.

Metadata contains the resolved project-configuration digest and provider schema
identities (`schema:<hash>`). Browser version is currently unknown (`null`).
Console/network capture controls and separate trace screenshot policy remain
pending; screenshot/DOM capture still follows `[evidence]`.
