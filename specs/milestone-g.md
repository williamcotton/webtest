# Milestone G — Portable Editor and Distribution

## 0. Status and dependencies

This specification expands Milestone G in [`future-functionality.md`](./future-functionality.md). It depends on the C.5 static description, diagnostic-detail, and repair-hint DTOs, the shared workspace/editor services from [`milestone-f.md`](./milestone-f.md), and the stable plan/event/protocol formats established by earlier milestones.

Milestone G makes the same Rust language intelligence available in Monaco/browser hosts and turns all native/editor components into reproducible release artifacts. It does not fork language semantics for the web.

## 1. Outcome

Users can:

- install signed/checksummed native `webtest` builds on supported macOS, Linux, and Windows systems;
- install the Cursor/VS Code extension from a compatible marketplace or VSIX;
- embed `@webtest/editor` in a browser application and receive the same static description, diagnostics/repair hints, formatting, semantic tokens, completion, hover, symbols, test identities, and plan output as native analysis;
- run the official CI container with a matching managed Chrome;
- verify artifact versions, checksums, provenance, and protocol compatibility.

Native, Cursor/VS Code, and Monaco agree on source interpretation because they call the same Rust syntax/HIR/analysis/editor implementations.

## 2. Scope

Milestone G includes:

- a complete WASM facade over portable workspace/editor services;
- portable C.5 language/project description and machine-diagnostic/repair DTOs;
- a versioned request/response DTO protocol for a Web Worker;
- an `@webtest/editor` npm package with WASM, worker, declarations, and Monaco adapter;
- native/WASM parity fixtures and browser-package integration tests;
- release automation for native binaries, checksums, signatures/provenance, and installers;
- VSIX/marketplace publishing and verified native-binary discovery/installation;
- an official container with `webtest` and matching Chrome for Testing;
- compatibility manifests across CLI machine DTOs, plan, events/traces, bridge protocol, WASM API, and extension.

## 3. Non-goals

This milestone does not run Chrome, `webtest inspect`, processes, filesystem providers, application bridges, or arbitrary network requests inside browser WASM. It does not host a browser LSP server, add a JavaScript parser/type checker, embed Chromium in the Rust executable, implement remote workers, provide a cloud editor, or guarantee offline execution of native test plans in a webpage.

## 4. Portable capability boundary

The portable graph contains only deterministic language/editor components:

```text
text -> syntax -> hir -> analysis -> format -> plan DTOs -> editor DTOs
                                                   |
                                                   v
                                                wasm facade
```

Native filesystem discovery, process launch, sockets, CDP, browser management, app-bridge transport, trace serving, and terminal reporting remain outside the WASM dependency graph.

Every operation that depends only on supplied source/configuration/schema inputs remains portable: parsing, analysis, static description, structured diagnostics and repair hints, formatting, semantic queries, test discovery, and plan generation. These are projections of the same Rust services used by native CLI/LSP adapters. `webtest inspect` remains native because semantic page inspection requires a live browser; WASM reports that capability boundary explicitly rather than approximating the page in TypeScript.

WASM may compile operations requiring native capabilities. The resulting plan/summary marks requirements such as:

```text
Browser
Http
Process
Filesystem
AppBridge
LocalIpc
```

Attempting to execute them in the portable package returns `capability_unavailable`; it never silently simulates them.

## 5. WASM API

### 5.1 Lifecycle

The package exposes an asynchronous workspace API conceptually equivalent to:

```ts
interface WebTestEditor {
  openWorkspace(input: WorkspaceInput): Promise<WorkspaceResult>;
  updateConfiguration(config: ProjectConfigurationInput): Promise<void>;
  updateProviderSchema(uri: string, text: string): Promise<void>;

  openDocument(uri: string, text: string, version: number): Promise<void>;
  updateDocument(uri: string, edits: TextEdit[], version: number): Promise<void>;
  closeDocument(uri: string): Promise<void>;
  removeFile(uri: string): Promise<void>;

  analyze(uri: string): Promise<AnalysisResult>;
  describeWorkspace(): Promise<ProjectDescription>;
  diagnostics(uri: string): Promise<Diagnostic[]>;
  format(uri: string): Promise<TextEdit[]>;
  semanticTokens(uri: string): Promise<SemanticToken[]>;
  completion(uri: string, position: Position): Promise<CompletionList>;
  signatureHelp(uri: string, position: Position): Promise<SignatureHelp | null>;
  hover(uri: string, position: Position): Promise<Hover | null>;
  documentSymbols(uri: string): Promise<DocumentSymbol[]>;
  definition(uri: string, position: Position): Promise<LocationLink[]>;
  references(uri: string, position: Position): Promise<Location[]>;
  rename(uri: string, position: Position, name: string): Promise<WorkspaceEdit>;
  codeActions(uri: string, range: Range): Promise<CodeAction[]>;
  inlayHints(uri: string, range: Range): Promise<InlayHint[]>;
  discoverTests(): Promise<TestItem[]>;
  compileTestPlan(testVariantId: string): Promise<CompiledPlan>;

  dispose(): Promise<void>;
}
```

The actual API can batch related calls, but every operation maps to the same protocol-neutral Rust editor/query service used by native adapters. `AnalysisResult` is a stable semantic DTO, not a Rowan tree or serialized Rust implementation detail. `Diagnostic` preserves the C.5 stable code, source revision/range, semantic details, and bounded repair hints; `ProjectDescription` is the same bounded description query projected by `webtest describe` from the inputs available to the host.

### 5.2 Positions and revisions

Public web DTO positions use zero-based UTF-16 line/character coordinates compatible with Monaco. The WASM facade is the only conversion boundary to internal UTF-8 byte ranges. Every document update carries a monotonically increasing host version. Responses include the workspace revision/document version they describe so stale results can be discarded.

Invalid/out-of-order edits return typed errors without mutating document state. Unicode parity fixtures cover surrogate pairs, combining characters, CRLF, and invalid boundary requests.

### 5.3 Serialization

DTOs are versioned, plain structured-clone-safe values. C.5 description/diagnostic/repair versions and F declaration/variant identities retain the same meaning across native and WASM adapters. Do not expose `wasm_bindgen` internals, Rust pointers, Rowan nodes, or mutable analysis objects. Large token/plan payloads may use typed arrays where measured, but their encoding is documented and versioned.

Panics are caught at the facade where possible and returned as `internal_error` with a correlation ID; production payloads do not contain Rust backtraces unless explicitly enabled.

## 6. Worker protocol

Run WASM in a dedicated Web Worker. The UI thread never performs parsing/type checking synchronously.

### 6.1 Envelope

```ts
type Request = {
  protocol: 1;
  id: number;
  method: string;
  workspaceRevision?: number;
  params: unknown;
};

type Response =
  | { protocol: 1; id: number; ok: true; revision: number; result: unknown }
  | { protocol: 1; id: number; ok: false; revision?: number; error: EditorError };
```

Notifications cover diagnostics/test-tree changes and worker readiness. Cancellation uses an explicit request ID. Unknown methods/versions and malformed payloads produce typed errors.

### 6.2 Scheduling

Document changes are applied in order. Read requests use immutable analysis snapshots. Expensive stale/cancelled work does not publish. The worker yields between bounded request batches so editing remains responsive even without WASM threads.

The protocol sets maximum document/workspace/message sizes appropriate for an embeddable editor and reports limit errors. The worker makes no ambient network requests and accesses only files/config/schemas supplied by the host.

## 7. `@webtest/editor` package

The package layout is:

```text
@webtest/editor/
├── package.json
├── dist/
│   ├── index.js
│   ├── index.d.ts
│   ├── worker.js
│   ├── webtest_bg.wasm
│   └── monaco.js
├── README.md
├── LICENSE
└── THIRD_PARTY_NOTICES
```

Publish ESM with documented bundler and direct-browser integration. If a separate CommonJS wrapper is necessary, it must call the same implementation. Package exports distinguish core worker APIs from optional Monaco helpers.

The package declares compatible Rust compiler/editor DTO and worker-protocol versions. WASM initialization accepts explicit asset URLs/factories so CSP-conscious hosts can self-host files; it does not require a CDN.

## 8. Monaco adapter

The adapter registers:

- `.webtest` language identity and basic language configuration;
- diagnostics markers;
- document/range formatting;
- semantic tokens;
- completion and signature help;
- hover;
- symbols, folding, and selection ranges;
- definition, references, and rename;
- code actions and inlay hints.

It mirrors Monaco model create/change/dispose events into worker document lifecycle and translates DTOs to Monaco types. It does not tokenize/parse source, resolve names, inspect provider schemas, or recreate LSP logic in TypeScript.

The adapter supports multiple models/workspaces, disposes registrations/listeners cleanly, coalesces rapid edits without dropping versions, and avoids applying stale markers/edits. Host applications control themes; semantic token categories are documented but not hard-coded to one visual style.

## 9. Native/WASM parity

One fixture corpus runs through native editor services and the WASM worker. Compare normalized DTOs for:

```text
parse/static diagnostics and exact ranges
static language/provider/project description
diagnostic semantic details and repair hints
format edits/result
semantic tokens
completion/signature/hover
symbols/folding/selection
definition/references/rename/code actions/inlay hints
test declaration/variant discovery and identities
compiled plan structure, IDs, source revisions, provider schema hashes, and required host capabilities
```

Allow differences only for explicitly native-only metadata such as canonical filesystem paths. Every exception is documented in the fixture harness. A new portable feature is incomplete until parity coverage exists.

Browser integration tests instantiate the real worker and Monaco adapter in a browser, edit Unicode/malformed source, observe diagnostics/completion, apply formatting/rename, and compile a plan.

## 10. Native release artifacts

### 10.1 Platform matrix

Tier-1 release targets initially include:

```text
macOS x86_64 and arm64
Linux x86_64 and arm64 on a documented glibc baseline
Windows x86_64
```

Additional targets require CI and smoke coverage. Release filenames include product version, target triple, and archive format. Each archive contains `webtest`, license/notices, README/install notes, and completion/manpage assets where available.

### 10.2 Integrity and provenance

Every release publishes:

- SHA-256 checksums in a signed manifest;
- build provenance/attestation tied to source commit and workflow;
- an SBOM and third-party license notices;
- exact Rust/toolchain lock information;
- the tested Chrome for Testing version/checksum metadata;
- protocol/format compatibility versions.

Builds use locked dependencies and reproducible settings where supported. Release automation never uploads artifacts from an untrusted pull-request context.

### 10.3 Installers

Provide release archives first, then shell/PowerShell installers, Homebrew, and appropriate Windows package-manager manifests. Installers:

- select an exact supported platform artifact;
- verify the signed manifest/checksum before installation;
- write only to the documented install prefix;
- support a pinned version;
- never alter shell startup files without explicit consent;
- do not install Chrome silently.

`webtest --version --json` reports product, target, inspection/description/diagnostic/repair, plan/event/trace/bridge versions, and build commit for support tooling.

## 11. Cursor/VS Code distribution

Publish the existing TypeScript adapter as a universal, architecture-neutral VSIX; do not place every platform binary inside it. Binary resolution is:

```text
webtest.serverPath setting
workspace target/debug/webtest for extension development
PATH
extension-managed verified release binary
```

Add an explicit **WebTest: Install/Update Server** command. With user confirmation, it resolves the extension-compatible release manifest, downloads the current platform binary into extension global storage, verifies signature/checksum, and atomically activates it. There is no silent download during activation. Unsupported platforms receive actionable manual-install instructions.

The run command still routes synchronized buffers through LSP; debugging still spawns `webtest dap` and defaults to headed Chrome. Extension-local command IDs remain distinct from server-advertised command IDs. Marketplace metadata declares compatibility and links to release/security documentation.

CI packages the VSIX, inspects contents for accidental `node_modules`/secrets/platform binaries, installs it in a clean compatible VS Code/Cursor harness, activates a `.webtest` document, exercises diagnostics/format/tokens/run/debug, and verifies disposal/restart behavior.

## 12. Official CI container

Publish a pinned non-root image containing:

```text
webtest release binary
matching Chrome for Testing
required Linux runtime libraries/fonts
documented cache/artifact directories
```

The image does not add `--no-sandbox` by default. It declares an unprivileged user, uses a minimal base, records SBOM/provenance, supports common CI volume/workspace layouts, and exposes no daemon port.

Container tags include immutable product versions; `latest` is optional and never used in reproducibility examples. A smoke test runs browser examples in the built image on every release.

## 13. Compatibility and versioning

Publish a machine-readable release compatibility manifest:

```json
{
  "webtest": "1.0.0",
  "plan_format": 1,
  "event_schema": 1,
  "trace_format": 1,
  "observation_ipc": 1,
  "inspection_schema": 1,
  "description_schema": 1,
  "diagnostic_schema": 1,
  "repair_hint_schema": 1,
  "app_bridge_protocol": [1],
  "wasm_editor_api": 1,
  "worker_protocol": 1,
  "vscode_extension": ">=0.3.0 <0.4.0",
  "chrome_for_testing": "<pinned-version>"
}
```

Readers reject unsupported major formats with actionable errors. Migration tools may be added separately; adapters never guess compatibility. The npm package, extension, SDKs, and native product can release independently only while declaring tested ranges.

## 14. Security and privacy

- WASM/worker code has no ambient filesystem, process, socket, or network authority.
- The package avoids `eval`/dynamic code generation and documents required CSP directives.
- Host-provided URIs/text/config/schema are size-bounded and treated as untrusted input.
- Panics and malformed messages cannot escape as uncontrolled worker crashes without an error notification.
- Download/install flows require HTTPS, verified signed metadata/checksums, atomic replacement, and safe archive extraction.
- Release archives reject path traversal/symlink escape during managed installation.
- Extension telemetry is off unless separately specified and consented; source/test content is never transmitted by default.
- Containers run non-root and keep browser profiles/artifacts isolated.
- Secrets remain redacted in plans/events/traces/editor DTOs across native and WASM builds.

## 15. Architecture and ownership

- `wasm` is a DTO/lifecycle facade over `analysis`, `format`, `plan`, and `editor`; it contains no parallel semantics.
- Shared semantic/editor DTO definitions, including C.5 description/diagnostic/repair shapes and F test identities, should live where native/WASM adapters can serialize them consistently without pulling in LSP/Monaco types.
- Worker and Monaco TypeScript are transport/host adapters only.
- Release tooling consumes locked Cargo/npm outputs and canonical compatibility manifests.
- The VSIX locates/manages a verified native product but never implements syntax/runtime behavior.
- Container packaging composes release artifacts and managed Chrome; it does not compile a second product variant with different semantics.

## 16. Delivery slices

1. Stabilize/version shared semantic/editor DTOs, including static description and repair details, and complete the WASM facade method surface.
2. Implement UTF-16/revision-safe worker protocol, cancellation, limits, and typed errors.
3. Package core `@webtest/editor` assets/declarations and bundler/CSP examples.
4. Implement Monaco providers and browser integration tests.
5. Build native/WASM parity corpus and make it a required CI gate.
6. Add cross-platform native release builds, checksums, provenance, SBOMs, and compatibility manifest.
7. Add installers/package-manager automation and the official CI container.
8. Publish VSIX/marketplace artifacts and verified extension-managed server installation.
9. Run end-to-end clean-environment release rehearsals and publish support/security documentation.

## 17. Testing requirements

Required coverage includes:

- WASM facade DTO/version/error and lifecycle tests;
- UTF-16 edit/position, stale response, cancellation, multi-document/workspace, size-limit, and worker-crash recovery tests;
- native/WASM parity for every portable semantic/editor/query result, including static description, diagnostic details, repair hints, and declaration/variant identities;
- real-browser Monaco adapter tests for changes, diagnostics, completion, formatting, rename, symbols, and plan compilation;
- npm package export/type/bundler/CSP/install smoke tests;
- clean cross-platform native archive execution and `--version --json` tests;
- checksum/signature/provenance/SBOM and malicious-archive extraction tests;
- extension binary resolution/download verification, activation, LSP, DAP, and upgrade/rollback tests;
- container non-root/browser execution and artifact ownership tests;
- compatibility rejection tests across every versioned protocol/format.

Release candidates must pass the full Rust workspace, Chrome/browser conformance, bridge SDK conformance, LSP/DAP, trace, benchmark, native/WASM parity, npm, VSIX, installer, and container matrices.

## 18. Acceptance criteria

Milestone G is complete only when:

1. Native release artifacts install and run on every tier-1 platform with verified integrity/provenance.
2. `@webtest/editor` works in a real browser/Monaco host without a browser LSP server or JavaScript language implementation.
3. Native and WASM fixture results agree for every portable static description, diagnostic/repair detail, edit, token, editor feature, test declaration/variant item, and compiled plan field.
4. Native-only operations compile in WASM with explicit capability requirements and cannot execute there.
5. A clean Cursor/VS Code installation can acquire or locate a verified compatible server, activate, run, and debug without bundled multi-platform binaries.
6. The official non-root CI container runs the example browser suite using the release-matched Chrome.
7. Compatibility metadata and rejection behavior prevent silent protocol/format skew.

The roadmap acceptance statement is thereby satisfied: native, Cursor/VS Code, and Monaco experiences agree on syntax, static description, formatting, structured diagnostics/repair hints, semantic tokens, editor intelligence, test identities, and compiled plans.
