# Trace artifact core

`webtest-trace` writes and validates a deterministic archive of native observation
records. It depends on shared observations and source identity, never the runtime,
compiler, browser backend, project configuration, or reporters. It performs no
filesystem access, extraction, network requests, or project execution.

This is the artifact foundation for Milestone E. It is not yet exposed by
`webtest test` or `webtest trace`. CLI capture/retention settings, capture-budget
truncation events, atomic disk publication, and the viewer remain pending.

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

The manifest versions the trace format separately from the native event schema.
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
inclusive. Exceeding one returns a typed error and no output archive. This initial
API does not drop semantic events or synthesize a trace truncation event; adapter
capture-budget policy must be added before public CLI export. `write` returns a
complete byte buffer so a future adapter can publish it atomically.
