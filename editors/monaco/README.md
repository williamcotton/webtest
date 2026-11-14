# @webtest/editor

A dedicated worker hosts the canonical Rust WebTest workspace compiled to WASM.
TypeScript/JavaScript only transports DTOs and maps Monaco providers. No browser
LSP server or JavaScript language implementation is used. The package does not
execute plans, launch Chrome, or access native providers.

```js
import { createEditor } from '@webtest/editor';
import { registerWebTest } from '@webtest/editor/monaco';
const editor = createEditor();
await editor.openWorkspace({
  configuration: { root: '/workspace' },
  sources: [{ path: '/workspace/example.webtest', text: 'test "hello" {}' }],
});
const adapter = registerWebTest(monaco, editor);
// Create Monaco file:///workspace/... models after registration.
// On application teardown:
adapter.dispose();
await editor.dispose();
```

Direct browser use imports `dist/index.js` and `dist/monaco.js` from self-hosted
files. Bundlers must preserve the module worker and the relative WASM asset URL.
For explicit asset routing or worker factories, pass
`{ worker: new Worker(workerURL, { type: 'module' }), wasmURL }` to `createEditor`.
No CDN is required. Serve WASM as `application/wasm` and modules as JavaScript.
Use `script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'`
(or the equivalent allowed asset origin). Monaco itself may require fonts/styles
and blob workers; the integration fixture demonstrates a self-hosted policy.
The sole worker fetch loads the explicitly supplied WASM asset; host sources,
configuration and schemas are never fetched or transmitted by the package.

One editor owns one workspace. Separate clients/workers provide independent
workspaces. For multiple Monaco adapters, provide disjoint `ownsModel` predicates.
Close retains a supplied file for imports, while `removeFile` removes it. Model
edits are applied in source order with monotonic versions. Register only models
that belong to the supplied root. Schema replacement accepts offline manifest
text or null; invalid schemas evict previous semantic authority.

`query` accepts canonical query kinds: `diagnostics`, `compile`, `compile_variant`,
`format`, `completions`, `signature`, `hover`, `definition`, `references`, `rename`,
`document_symbols`, `workspace_symbols`, `folding`, `selection`, `inlay_hints`,
`code_actions`, `semantic_tokens`, `discovery`, and `project_description`.
Position queries take `{position:{line,character}}`; code actions take `range`.
Both coordinates are zero-based UTF-16, including surrogate validation. References
also take `include_declaration` and `limit`; bounded workspace queries take `limit`.
Rename takes `name`, workspace symbols take `query`, compile_variant takes the
exact `variant` ID supplied by discovery. `describe({query})` or `describe({search})`
returns shared language/provider descriptions. `format` returns canonical full text.

Requests use `{protocol:1,id,method,workspaceRevision?,params}` and responses carry
`{protocol:1,id,ok,revision,result}` or a typed `error`. `query` results contain
`{document_version,value}`. Source DTOs retain canonical byte origins and add
UTF-16 `range`/`position` projections; nested ByteRange values contain their web
projection under `range`. Compiled plans remain byte-for-byte canonical plan DTOs
with native source revisions and required host capabilities. They always report
`executable_on_wasm:false`. `execute` returns `capability_unavailable`.

Optional `workspaceRevision` rejects stale requests. Cancel with `AbortSignal`;
queued work is skipped, and late results are discarded by the client. An executing
single-threaded Rust query finishes before the next worker message. There is no
claim of preemptive WASM interruption. The worker yields between requests and
retains at most 256 queued requests. Envelopes are capped at 80 MiB, documents at
8 MiB, source text per workspace at 64 MiB, source/alias counts at 16,384. Rust
also bounds configurations, schemas and individual semantic queries. Error codes
include `invalid_request`, `invalid_params`, `invalid_position`, `stale_version`,
`stale_revision`, `unknown_document`, `invalid_inputs`, `query_failed`,
`workspace_closed`, `limit_exceeded`, `unsupported_protocol`, `cancelled`,
`internal_error`, `worker_crashed`, and `disposed`. A fatal worker must be recreated;
the host retains and reopens its explicit inputs. Error notifications omit stacks.

Semantic token types: keyword, string, comment, function, variable, property, type,
parameter, namespace, fixture, test. Modifiers: declaration, readonly, deprecated.
The host chooses themes. No independent tokenizer is registered.

Build with the Rust WASM target and the wasm-bindgen CLI version in Cargo.lock:
`npm ci && npm run build && npm test && npm run test:browser`.
The browser suite uses the native WebTest runner and real Chrome, not another
browser runtime. The parity corpus compares exact serialized native Rust and
real WASM DTOs with no path/range/identity exceptions. API 1, worker protocol 1,
plan 10; incompatible worker protocols fail explicitly.

For a local direct-browser example, build the package, serve this directory with
`python3 -m http.server --bind 127.0.0.1 8000`, then open
`http://127.0.0.1:8000/examples/direct-browser.html`. The bundler example shows
explicit asset URLs; configure the bundler to retain the worker's module imports.
The browser suite tests both direct and bundled client modules under CSP.
