# Web Application Testing DSL — Architecture and Vertical Slice Implementation Specification

## 0. Purpose

Implement a new statically analyzable DSL for testing web applications.

The language must support testing both:

* server-side behavior, such as HTTP APIs, processes, fixtures, services, and eventually databases; and
* browser-side behavior through a real headless Chromium instance.

The defining architectural goals are:

1. **One grammar, lexer, parser, and canonical syntax tree.**
2. Formatting, compilation, semantic analysis, LSP, Monaco support, and execution must never use independent parsers.
3. The canonical source representation must be a **lossless CST**.
4. Typed AST wrappers must be views over the CST rather than a separately parsed tree.
5. Semantic analysis and execution planning must lower from that syntax tree.
6. Editor functionality must be implemented as **protocol-independent editor services**.
7. LSP must be a thin adapter over editor services.
8. The native LSP implementation must use **Tower**, preferably `tower-lsp-server`.
9. The portable language implementation should be capable of compiling to WASM for Monaco/browser usage.
10. The native product should ship as **one executable** containing the CLI, compiler, formatter, runtime, test runner, and LSP server.
11. Browser automation should be implemented through a browser abstraction with a **direct Chrome DevTools Protocol backend**.
12. Runtime observations and failures must map back to source code and appear in editor diagnostics.
13. Runtime diagnostics must be tied to a specific source revision so stale execution results are never presented as current facts.
14. The first implementation should be a narrow vertical slice proving the architecture end-to-end rather than attempting to build a complete testing framework immediately.

The working product/binary name in this specification is:

```text
webtest
```

This is a placeholder and should be easy to rename globally.

---

# 1. Product model

Native users install one executable:

```text
webtest
```

It eventually supports:

```text
webtest test
webtest run
webtest check
webtest fmt
webtest build
webtest lsp
webtest repl
webtest trace
webtest browser install
```

The initial vertical slice only needs:

```text
webtest check <file>
webtest fmt <file>
webtest test <file>
webtest lsp
```

The browser/editor build is a second compilation target of the same source tree:

```text
webtest.wasm
```

It should contain as much portable functionality as practical:

```text
lexer
parser
CST
typed AST
HIR
semantic analysis
formatter
TestPlan construction
editor services
```

Browser process launching and native CDP transport remain native-host capabilities.

---

# 2. Architectural principle

The system must follow this invariant:

```text
                        ONE LANGUAGE IMPLEMENTATION

                                  source
                                    |
                                    v
                                  lexer
                                    |
                                    v
                                  parser
                                    |
                                    v
                            lossless canonical CST
                         /          |          \
                        /           |           \
                       v            v            v
                  formatter     typed AST    syntax tools
                                    |
                                    v
                                   HIR
                                    |
                                    v
                              semantic engine
                           /        |         \
                          /         |          \
                         v          v           v
                    diagnostics   editor     TestPlan
                                  services      IR
                                     |           |
                           +---------+--+        |
                           |            |        |
                           v            v        v
                          LSP         Monaco   runtime
                           |            |        |
                           |            |        v
                           |            |     browser/CDP
                           |            |        |
                           +------------+--------+
                                        |
                                        v
                              runtime observations
                                        |
                                        v
                                 editor services
```

There must never be:

```text
compiler parser
formatter parser
LSP parser
Monaco parser
test-runner parser
syntax-highlighter parser
```

There is exactly one parser.

---

# 3. Top-level system architecture

```text
+----------------------------------------------------------------------------------+
|                                WEBTEST PROJECT                                   |
|                                                                                  |
| Native distribution                               Browser/editor distribution    |
|                                                                                  |
| +--------------------------------+                +----------------------------+ |
| |           webtest              |                |       webtest.wasm         | |
| |       ONE EXECUTABLE           |                |                            | |
| |                                |                | parser                     | |
| | check                          |                | CST                        | |
| | fmt                            |                | HIR                        | |
| | test                           |                | semantics                  | |
| | build                          |                | formatter                  | |
| | run                            |                | TestPlan compiler          | |
| | lsp                            |                | editor services            | |
| | trace                          |                |                            | |
| +---------------+----------------+                +-------------+--------------+ |
|                 |                                               |                |
+-----------------+-----------------------------------------------+----------------+
                  |                                               |
                  v                                               v
+----------------------------------------------------------------------------------+
|                                   HOST LAYER                                     |
|                                                                                  |
| Native                                             Browser                       |
|                                                                                  |
| filesystem                                         Monaco models                 |
| processes                                          Web Worker                    |
| stdout/stderr                                      virtual workspace             |
| browser launching                                  JS/TS bridge                  |
| sockets                                            optional remote runner        |
| file watching                                                                    |
| package download                                                                 |
| LSP stdio                                                                       |
+--------------------------------------+-------------------------------------------+
                                       |
                                       v
+----------------------------------------------------------------------------------+
|                              WORKSPACE / DATABASE                                |
|                                                                                  |
| FileId                                                                           |
| SourceRevision                                                                   |
| file contents                                                                    |
| paths / URIs                                                                     |
| project config                                                                   |
| package/module graph                                                             |
| incremental query state                                                          |
+--------------------------------------+-------------------------------------------+
                                       |
                                       v
+----------------------------------------------------------------------------------+
|                                 LANGUAGE CORE                                    |
|                                                                                  |
| lexer -> parser -> CST -> AST facade -> HIR -> semantics                         |
|           |              |                     |                                 |
|           |              |                     +---- TestPlan                    |
|           |              |                     +---- editor queries              |
|           |              |                                                       |
|           +---- formatter + syntax diagnostics                                   |
+--------------------------------------+-------------------------------------------+
                                       |
                    +------------------+------------------+
                    |                                     |
                    v                                     v
+--------------------------------------+    +--------------------------------------+
|          EDITOR SERVICES             |    |             RUNTIME                  |
|                                      |    |                                      |
| diagnostics                          |    | TestPlan executor                    |
| completion                           |    | event stream                         |
| hover                                |    | BrowserHost                          |
| definition                           |    | HttpHost eventually                  |
| references                           |    | ProcessHost eventually               |
| rename                               |    |                                      |
| formatting                           |    | runtime observations                 |
| semantic tokens                      |    |                                      |
+------------------+-------------------+    +------------------+-------------------+
                   |                                           |
            +------+-------+                                   v
            |              |                        +--------------------------+
            v              v                        | Browser abstraction      |
          LSP            Monaco                     |       |                  |
            |                                        |       v                  |
            |                                        | CDP backend              |
            |                                        |       |                  |
            |                                        |       v                  |
            |                                        | Chromium                 |
            |                                        +--------------------------+
            |
            +---------------------+
                                  |
                                  v
                       runtime diagnostics
```

---

# 4. Implementation language

Use **Rust** for the entire core implementation.

Rust should own:

* lexer;
* parser;
* CST;
* typed AST wrappers;
* HIR;
* semantic analysis;
* formatter;
* TestPlan IR;
* execution engine;
* browser abstraction;
* CDP backend;
* observation/event infrastructure;
* editor services;
* LSP server;
* CLI;
* WASM core.

TypeScript should only be used where required for host integration:

* VS Code extension;
* Monaco adapter;
* Web Worker glue;
* generated WASM bindings.

TypeScript must not duplicate parser or semantic logic.

---

# 5. Important dependencies

Use workspace-managed dependencies.

For the language architecture:

```text
rowan
salsa
serde
serde_json
thiserror
tracing
blake3
```

For native asynchronous infrastructure:

```text
tokio
futures
```

For LSP:

```text
tower-lsp-server
tower
```

Use the protocol types re-exported/provided by the chosen Tower LSP package rather than introducing another independently versioned LSP-types dependency unless required.

For CLI:

```text
clap
anyhow
tracing-subscriber
```

For CDP transport:

```text
tokio-tungstenite
url
serde_json
```

For temporary Chrome profiles:

```text
tempfile
```

For WASM later:

```text
wasm-bindgen
serde-wasm-bindgen
```

At the time this specification was written, useful reference releases included:

```text
rowan             0.16.x
salsa             0.28.x
tower-lsp-server  0.23.x
```

Do not scatter dependency versions among individual crates. Define them through:

```toml
[workspace.dependencies]
```

and commit `Cargo.lock`.

---

# 6. Repository structure

Create a Cargo workspace similar to:

```text
webtest/
|
+-- Cargo.toml
+-- Cargo.lock
+-- rust-toolchain.toml
+-- README.md
+-- LICENSE
+-- .gitignore
|
+-- crates/
|   |
|   +-- text/
|   |   +-- Cargo.toml
|   |   +-- src/lib.rs
|   |
|   +-- syntax/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- kind.rs
|   |       +-- lexer.rs
|   |       +-- parser.rs
|   |       +-- grammar.rs
|   |       +-- error.rs
|   |       +-- ast.rs
|   |
|   +-- hir/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- ids.rs
|   |       +-- lower.rs
|   |       +-- expr.rs
|   |       +-- test.rs
|   |
|   +-- analysis/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- db.rs
|   |       +-- workspace.rs
|   |       +-- diagnostics.rs
|   |       +-- semantics.rs
|   |
|   +-- format/
|   |   +-- Cargo.toml
|   |   +-- src/lib.rs
|   |
|   +-- plan/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- lower.rs
|   |       +-- step.rs
|   |
|   +-- observation/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- event.rs
|   |       +-- store.rs
|   |       +-- diagnostic.rs
|   |
|   +-- browser/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- locator.rs
|   |       +-- action.rs
|   |       +-- error.rs
|   |
|   +-- browser-cdp/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- process.rs
|   |       +-- connection.rs
|   |       +-- protocol.rs
|   |       +-- session.rs
|   |
|   +-- runtime/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- runner.rs
|   |       +-- context.rs
|   |       +-- events.rs
|   |
|   +-- editor/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- service.rs
|   |       +-- diagnostics.rs
|   |       +-- formatting.rs
|   |
|   +-- lsp/
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- backend.rs
|   |       +-- convert.rs
|   |       +-- documents.rs
|   |
|   +-- wasm/
|   |   +-- Cargo.toml
|   |   +-- src/lib.rs
|   |
|   +-- app/
|       +-- Cargo.toml
|       +-- src/
|           +-- main.rs
|           +-- cli.rs
|           +-- commands/
|               +-- mod.rs
|               +-- check.rs
|               +-- fmt.rs
|               +-- test.rs
|               +-- lsp.rs
|
+-- editors/
|   +-- vscode/
|       +-- package.json
|       +-- tsconfig.json
|       +-- src/
|           +-- extension.ts
|
+-- examples/
|   +-- minimal/
|       +-- passing.webtest
|       +-- failing.webtest
|
+-- tests/
    +-- fixtures/
    +-- integration/
```

The actual number of crates may evolve, but preserve the dependency boundaries below.

---

# 7. Crate dependency graph

The intended direction is:

```text
                           text
                            |
                            v
                          syntax
                            |
                            v
                           hir
                            |
                            v
                        analysis
                       /   |    \
                      /    |     \
                     v     v      v
                 format   plan   editor
                           |       |
                           v       |
                        runtime    |
                       /      \    |
                      v        v   |
                browser   observation
                   |          |
                   v          |
              browser-cdp     |
                              |
                              +------> editor
                                        |
                                        v
                                       lsp

app depends on:
    analysis
    format
    plan
    runtime
    browser-cdp
    editor
    lsp

wasm depends on:
    syntax
    hir
    analysis
    format
    plan
    editor
```

More strictly:

```text
core crates MUST NOT depend on:
    tower-lsp-server
    VS Code
    Monaco
    tokio process spawning
    native filesystem APIs
    CDP protocol details
```

In particular:

```text
analysis -> lsp
```

is forbidden.

```text
editor -> lsp
```

is forbidden.

```text
browser -> browser-cdp
```

is forbidden if `browser` defines the abstraction and `browser-cdp` implements it.

---

# 8. `text` crate

The `text` crate owns language-independent source identity.

Suggested types:

```rust
pub struct FileId(u32);

pub struct SourceRevision([u8; 32]);

pub struct DocumentVersion(i64);
```

Use Rowan's text range primitives where practical:

```rust
rowan::TextRange
rowan::TextSize
```

`SourceRevision` must be deterministic from source contents.

Use BLAKE3:

```text
SourceRevision = blake3(file bytes)
```

This revision will eventually make cross-process runtime observations safe.

A runtime observation may only be displayed as current if:

```text
observation.source_revision == current_source_revision
```

---

# 9. `syntax` crate

## 9.1 Requirements

The parser must:

* be error tolerant;
* preserve comments;
* preserve whitespace;
* preserve punctuation;
* preserve malformed tokens;
* preserve exact text ranges;
* return a CST even for incomplete input;
* never require semantic validity.

Use Rowan as the lossless tree representation.

The CST is the canonical syntactic source of truth.

---

# 10. Initial vertical-slice DSL grammar

Do not implement the entire future language.

The first slice only needs enough syntax to prove:

```text
source
-> parse
-> CST
-> HIR
-> TestPlan
-> Chromium
-> runtime failure
-> observation
-> LSP diagnostic
-> VS Code underline
```

Support this program:

```text
test "missing button" {
    browser {
        open "http://127.0.0.1:3000"
        click id("does-not-exist")
    }
}
```

Also support a passing version:

```text
test "existing button" {
    browser {
        open "http://127.0.0.1:3000"
        click id("submit")
    }
}
```

A possible initial grammar:

```ebnf
file          = test_decl* EOF ;

test_decl     = "test" STRING block ;

block         = "{" statement* "}" ;

statement     = browser_block ;

browser_block = "browser" "{"
                    browser_statement*
                "}" ;

browser_statement
              = open_statement
              | click_statement ;

open_statement
              = "open" STRING ;

click_statement
              = "click" locator ;

locator       = id_locator ;

id_locator    = "id" "(" STRING ")" ;
```

Optional semicolons are unnecessary initially.

Newlines and spaces are trivia.

Comments may support:

```text
// line comment
```

from the beginning because the CST must prove it is lossless.

---

# 11. Syntax kinds

Define explicit syntax kinds such as:

```rust
pub enum SyntaxKind {
    // trivia
    Whitespace,
    LineComment,

    // tokens
    Ident,
    String,
    LBrace,
    RBrace,
    LParen,
    RParen,

    // keywords
    TestKw,
    BrowserKw,
    OpenKw,
    ClickKw,
    IdKw,

    // nodes
    Root,
    TestDecl,
    Block,
    BrowserBlock,
    OpenStmt,
    ClickStmt,
    IdLocator,

    // recovery
    Error,

    Eof,
}
```

Do not treat keywords as generic identifiers in downstream consumers if distinct token kinds make AST access cleaner.

---

# 12. Typed AST facade

Typed AST types must wrap Rowan syntax nodes.

Example conceptual API:

```rust
pub struct TestDecl {
    syntax: SyntaxNode,
}

pub struct BrowserBlock {
    syntax: SyntaxNode,
}

pub struct OpenStmt {
    syntax: SyntaxNode,
}

pub struct ClickStmt {
    syntax: SyntaxNode,
}

pub struct IdLocator {
    syntax: SyntaxNode,
}
```

Implement:

```rust
rowan::ast::AstNode
```

where appropriate.

AST accessors:

```rust
impl TestDecl {
    pub fn name(&self) -> Option<StringToken>;
    pub fn browser_blocks(&self) -> impl Iterator<Item = BrowserBlock>;
}

impl OpenStmt {
    pub fn url(&self) -> Option<StringToken>;
}

impl ClickStmt {
    pub fn locator(&self) -> Option<Locator>;
}

impl IdLocator {
    pub fn value(&self) -> Option<StringToken>;
}
```

There must not be a second parser that produces these types.

---

# 13. Source pointers

Long-lived semantic objects should not hold complete Rowan `SyntaxNode`s indefinitely.

Use source pointers.

Conceptually:

```rust
pub struct SyntaxOrigin {
    pub file: FileId,
    pub range: TextRange,
}
```

or Rowan `SyntaxNodePtr` / `AstPtr` where appropriate.

HIR and TestPlan steps must preserve source origins.

Example:

```rust
pub struct HirClick {
    pub locator: HirLocator,
    pub origin: SyntaxOrigin,
}
```

The locator itself should preserve a more precise range:

```rust
pub struct HirLocator {
    pub kind: HirLocatorKind,
    pub origin: SyntaxOrigin,
}
```

This allows a runtime failure to underline:

```text
id("does-not-exist")
```

instead of the entire test.

---

# 14. `hir` crate

HIR describes semantic language constructs without browser implementation details.

Initial HIR:

```rust
pub struct HirFile {
    pub tests: Vec<HirTest>,
}

pub struct HirTest {
    pub id: TestId,
    pub name: String,
    pub body: Vec<HirStmt>,
    pub origin: SyntaxOrigin,
}

pub enum HirStmt {
    Browser(HirBrowserBlock),
}

pub struct HirBrowserBlock {
    pub operations: Vec<HirBrowserOp>,
    pub origin: SyntaxOrigin,
}

pub enum HirBrowserOp {
    Open(HirOpen),
    Click(HirClick),
}

pub struct HirOpen {
    pub url: String,
    pub origin: SyntaxOrigin,
}

pub struct HirClick {
    pub locator: HirLocator,
    pub origin: SyntaxOrigin,
}

pub enum HirLocatorKind {
    Id(String),
}
```

Use strongly typed IDs:

```rust
pub struct TestId(u32);
pub struct StepId(u32);
```

Do not use arbitrary strings as semantic identities.

---

# 15. Future HIR execution domains

Design HIR so these can eventually exist:

```text
server {
    ...
}

browser {
    ...
}
```

Operations will ultimately have execution capabilities such as:

```text
Pure
Server
Browser
Test
```

Future semantic analysis should reject:

```text
server {
    click id("foo")
}
```

before execution.

Do not implement the complete capability/effect system in the vertical slice.

Do not make architectural choices that prevent it.

---

# 16. `analysis` crate

This crate is the semantic/query layer.

It owns:

* workspace inputs;
* parse query;
* HIR lowering query;
* static diagnostics;
* eventually name resolution;
* eventually type inference;
* eventually package/module graphs.

Use Salsa or an equivalent query-driven architecture.

Initial conceptual queries:

```rust
fn parse(db: &dyn Db, file: FileId) -> Parse;

fn hir_file(db: &dyn Db, file: FileId) -> Arc<HirFile>;

fn diagnostics(db: &dyn Db, file: FileId) -> Arc<Vec<Diagnostic>>;

fn test_plan(db: &dyn Db, file: FileId) -> Arc<TestPlan>;
```

Inputs:

```text
file text
file path/URI
workspace config
```

The parser itself does not need to know Salsa exists.

---

# 17. Static diagnostic type

Do not expose LSP diagnostic types from the language core.

Define an internal diagnostic:

```rust
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

pub struct Diagnostic {
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub code: &'static str,
    pub message: String,
    pub source: DiagnosticSource,
}

pub enum DiagnosticSource {
    Syntax,
    Semantic,
    Runtime,
}
```

Example parser diagnostic:

```text
code: syntax.expected_string
message: expected string after `open`
```

---

# 18. Formatter

Formatting consumes the CST.

It must not consume a semantic AST that has lost comments/trivia.

Initial formatter can be intentionally simple.

For the vertical grammar, canonical output should look like:

```text
test "missing button" {
    browser {
        open "http://127.0.0.1:3000"
        click id("does-not-exist")
    }
}
```

Indent using four spaces initially.

Preserve line comments.

Expose:

```rust
pub fn format_file(parse: &Parse) -> String;
```

Later this can become an edit-producing formatter.

CLI and LSP both call the same implementation.

---

# 19. `plan` crate

The runtime should execute a test-plan IR rather than HIR directly.

This preserves a clean boundary between language semantics and execution.

Initial representation:

```rust
pub struct TestPlan {
    pub file: FileId,
    pub source_revision: SourceRevision,
    pub tests: Vec<PlannedTest>,
}

pub struct PlannedTest {
    pub id: TestId,
    pub name: String,
    pub steps: Vec<PlannedStep>,
    pub origin: SyntaxOrigin,
}

pub struct PlannedStep {
    pub id: StepId,
    pub operation: TestOperation,
    pub origin: SyntaxOrigin,
}

pub enum TestOperation {
    Browser(BrowserOperation),
}

pub enum BrowserOperation {
    Open {
        url: String,
    },

    Click {
        locator: Locator,
    },
}

pub enum Locator {
    Id(String),
}
```

Every executable step must have:

```text
StepId
source revision
source origin
```

The runtime must never need to inspect syntax nodes.

---

# 20. Future TestPlan model

Eventually extend:

```text
TestOperation
├── Browser
├── Http
├── Process
├── File
├── Fixture
├── Assertion
├── Sequence
├── Parallel
├── Race
├── Retry
└── Timeout
```

The IR should eventually support structured concurrency explicitly rather than compiling arbitrary asynchronous behavior into opaque callbacks.

Do not implement these yet.

---

# 21. Browser abstraction

The language/runtime must not expose CDP concepts.

The core abstraction should look conceptually like:

```rust
#[async_trait]
pub trait BrowserHost {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError>;
}

#[async_trait]
pub trait BrowserSession {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError>;
}

#[async_trait]
pub trait Page {
    async fn open(&mut self, url: &str) -> Result<(), BrowserError>;

    async fn click(
        &mut self,
        locator: &Locator,
    ) -> Result<(), BrowserError>;
}
```

The exact trait design may use generics instead of trait objects.

Keep the semantic abstraction.

The browser crate owns:

```text
Locator
BrowserError
BrowserHost
BrowserSession
Page
```

The CDP crate owns its implementation.

---

# 22. Browser error model

Never turn browser failures into opaque strings too early.

Use structured errors:

```rust
pub enum BrowserError {
    LocatorNotFound {
        locator: Locator,
    },

    LocatorAmbiguous {
        locator: Locator,
        matches: usize,
    },

    NavigationFailed {
        url: String,
        reason: String,
    },

    BrowserDisconnected,

    Protocol {
        method: String,
        message: String,
    },
}
```

Later extend with evidence:

```text
page URL
page title
candidate elements
DOM snapshot
screenshot
console messages
network events
timing
```

Presentation happens above the browser layer.

---

# 23. Chromium integration

Use **Chrome DevTools Protocol** directly.

Architecture:

```text
BrowserOperation
      |
      v
browser abstraction
      |
      v
CDP backend
      |
      v
WebSocket
      |
      v
Chrome / Chromium
```

Do not make Playwright a required runtime dependency.

The long-term implementation should have generated typed CDP bindings from the protocol schema.

For the first vertical slice, hand-writing the tiny required subset is acceptable and preferable to building a code generator before proving the architecture.

---

# 24. Initial CDP subset

The first slice only needs enough protocol support to:

1. launch Chrome;
2. connect;
3. create or attach to one page target;
4. navigate;
5. wait for a basic load signal;
6. evaluate JavaScript;
7. detect whether an element with an ID exists;
8. click it;
9. close Chrome cleanly.

Implement generic envelopes:

```rust
#[derive(Serialize)]
struct Command {
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}
```

Incoming messages:

```rust
#[derive(Deserialize)]
struct IncomingMessage {
    id: Option<u64>,
    method: Option<String>,
    result: Option<serde_json::Value>,
    error: Option<CdpError>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    params: Option<serde_json::Value>,
}
```

Implement request correlation through monotonically increasing IDs and oneshot channels.

Conceptually:

```text
send command #17
      |
      v
pending[17] = oneshot sender
      |
      v
WebSocket
      |
 response #17
      |
      v
pending.remove(17)
      |
      v
oneshot.send(response)
```

A background receive task routes events and responses.

---

# 25. Launching Chrome

Initial implementation should accept the browser executable through:

```text
WEBTEST_CHROME_PATH
```

and/or:

```text
--chrome-path
```

This keeps the first vertical slice independent of browser-download infrastructure.

Launch approximately:

```text
chrome
--headless=new
--remote-debugging-port=0
--user-data-dir=<temporary-directory>
--no-first-run
--no-default-browser-check
about:blank
```

Chrome writes the selected debugging port into:

```text
<user-data-dir>/DevToolsActivePort
```

Read that file with retries and a bounded timeout.

Then query Chrome's debugging endpoint or derive/connect to the WebSocket debugger URL.

Long term, add a `BrowserManager` responsible for downloading a pinned Chrome for Testing version.

Do not implement browser downloading as part of the first slice unless it is trivial after everything else works.

---

# 26. Initial browser click behavior

The final product should implement real actionability semantics.

The first slice intentionally does not.

For `Locator::Id(value)`, evaluate code equivalent to:

```javascript
(() => {
    const element = document.getElementById(ID);
    if (!element) {
        return { found: false };
    }

    element.click();

    return { found: true };
})()
```

Do not interpolate unescaped strings into JavaScript source.

Serialize the ID using JSON or use CDP function arguments.

Return:

```text
found == false
```

as:

```rust
BrowserError::LocatorNotFound {
    locator: Locator::Id(...),
}
```

This simple behavior validates the important architecture.

Later replace it with real browser actions using DOM, Runtime, Input, accessibility information, hit testing, stability checks, visibility checks, auto-waiting, and physical input dispatch.

---

# 27. Future actionability engine

Eventually:

```text
click locator
     |
     v
resolve locator
     |
     v
exactly one element?
     |
     v
attached?
     |
     v
visible?
     |
     v
stable?
     |
     v
enabled?
     |
     v
receives pointer input?
     |
     v
scroll into view
     |
     v
calculate click point
     |
     v
dispatch real browser input
     |
     v
observe resulting navigation/state
```

This belongs in:

```text
browser/actionability
```

not in the DSL semantics and not in the CDP transport.

---

# 28. Runtime

The runtime executes a `TestPlan`.

Conceptual API:

```rust
pub struct Runner<B> {
    browser: B,
    observations: ObservationSink,
}

impl<B: BrowserHost> Runner<B> {
    pub async fn run(
        &self,
        plan: &TestPlan,
    ) -> RunResult;
}
```

For the initial DSL:

```text
for each test
    start browser session/page
    execute steps sequentially
    record events
    stop test on failure
```

Do not introduce parallel execution yet.

---

# 29. Runtime event model

Every operation should generate structured events.

Initial event types:

```rust
pub enum ExecutionEvent {
    RunStarted {
        execution_id: ExecutionId,
    },

    TestStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        name: String,
    },

    StepStarted {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
    },

    StepPassed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
    },

    StepFailed {
        execution_id: ExecutionId,
        test_id: TestId,
        step_id: StepId,
        failure: RuntimeFailure,
    },

    TestFinished {
        execution_id: ExecutionId,
        test_id: TestId,
        passed: bool,
    },

    RunFinished {
        execution_id: ExecutionId,
    },
}
```

Use a unique:

```rust
pub struct ExecutionId(...)
```

UUIDs or sufficiently unique random IDs are fine.

---

# 30. Runtime observations

Events describe execution history.

Observations describe source-relevant facts that editor services may present.

Define:

```rust
pub struct RuntimeObservation {
    pub execution_id: ExecutionId,
    pub file: FileId,
    pub source_revision: SourceRevision,
    pub test_id: TestId,
    pub step_id: StepId,
    pub range: TextRange,
    pub kind: RuntimeObservationKind,
}
```

Initial kind:

```rust
pub enum RuntimeObservationKind {
    LocatorNotFound {
        locator: Locator,
        page_url: Option<String>,
    },
}
```

Later:

```text
LocatorAmbiguous
AssertionMismatch
NavigationFailure
HttpFailure
ConsoleError
NetworkFailure
ObservedValue
ExecutionTiming
RetryCount
```

---

# 31. Observation store

Implement:

```rust
pub struct ObservationStore {
    // keyed by file and/or source revision
}
```

Required operations:

```rust
clear_for_execution(...)
replace_for_file_revision(...)
record(...)
observations_for(file, revision)
```

At the beginning of a new run for a file:

```text
previous current observations become stale/cleared
```

After failure:

```text
new current observation is recorded
```

If a later run succeeds:

```text
the old runtime failure disappears
```

---

# 32. Runtime diagnostic mapping

Convert:

```rust
RuntimeObservationKind::LocatorNotFound
```

to an editor diagnostic:

```text
severity: Error
source: Runtime
code: runtime.locator_not_found

message:
No element with id "does-not-exist" was found during the last test run.
```

The source range should point to the locator expression:

```text
click id("does-not-exist")
      ^^^^^^^^^^^^^^^^^^^^
```

not the whole browser block.

---

# 33. Source revision safety

This is non-negotiable.

Suppose runtime observed:

```text
source revision = A
```

and the editor currently contains revision:

```text
B
```

Then:

```text
A != B
```

means the observation must not be published as a current runtime error.

For the first slice:

```text
runtime observations generated inside the LSP process
```

will naturally use the editor's current text.

For future cross-process CLI-to-LSP observations, BLAKE3 `SourceRevision` provides the consistency check.

---

# 34. Editor services

Define a protocol-independent API.

Conceptually:

```rust
pub struct EditorService {
    db: AnalysisDatabase,
    observations: Arc<ObservationStore>,
}
```

Expose:

```rust
impl EditorService {
    pub fn diagnostics(
        &self,
        file: FileId,
    ) -> Vec<Diagnostic>;

    pub fn format(
        &self,
        file: FileId,
    ) -> Result<String, EditorError>;

    pub async fn run_file(
        &self,
        file: FileId,
        browser: &dyn BrowserHost,
    ) -> RunResult;
}
```

Later:

```text
completion
hover
definition
references
rename
signature help
code actions
inlay hints
semantic tokens
document symbols
workspace symbols
folding ranges
selection ranges
```

None of these methods may return LSP types.

---

# 35. Diagnostic composition

`EditorService::diagnostics(file)` should combine:

```text
static syntax diagnostics
+
static semantic diagnostics
+
current runtime observations
```

Conceptually:

```rust
pub fn diagnostics(&self, file: FileId) -> Vec<Diagnostic> {
    let revision = self.db.source_revision(file);

    let mut result = self.db.static_diagnostics(file);

    result.extend(
        self.observations
            .observations_for(file, revision)
            .map(runtime_observation_to_diagnostic)
    );

    result
}
```

This is the core feature differentiating the project.

---

# 36. Tower-based LSP implementation

The native LSP implementation must use Tower.

Prefer:

```text
tower-lsp-server
```

The LSP crate is an adapter around `EditorService`.

Architecture:

```text
                        webtest lsp
                             |
                             v
                    tower-lsp-server
                             |
                  +----------+----------+
                  |                     |
                  v                     v
             request routing       LSP client
                  |
                  v
               Backend
                  |
                  v
           EditorService
          /      |       \
         v       v        v
      syntax  semantics observations
```

Do not put semantic logic in LSP handlers.

---

# 37. Initial LSP capabilities

Implement only what the vertical slice needs:

```text
initialize
initialized
shutdown
didOpen
didChange
didClose
textDocument/publishDiagnostics
textDocument/formatting
workspace/executeCommand
```

Advertise:

```text
textDocumentSync = incremental or full
documentFormattingProvider = true

executeCommandProvider:
    webtest.runFile
```

Using full text synchronization initially is acceptable if that dramatically simplifies the implementation.

The architecture should allow incremental updates later.

---

# 38. LSP backend shape

Conceptually:

```rust
pub struct Backend {
    client: tower_lsp_server::Client,
    editor: Arc<EditorService>,
    documents: Arc<DocumentStore>,
    browser: Arc<dyn BrowserHost + Send + Sync>,
}
```

LSP handler logic should look approximately like:

```text
didOpen
  |
  v
update workspace text
  |
  v
editor.diagnostics
  |
  v
convert to LSP
  |
  v
publishDiagnostics
```

`didChange` follows the same path.

---

# 39. LSP conversion module

Put protocol conversion in:

```text
crates/lsp/src/convert.rs
```

Functions should include:

```rust
fn text_range_to_lsp(
    text: &str,
    range: TextRange,
) -> lsp::Range;

fn diagnostic_to_lsp(
    text: &str,
    diagnostic: Diagnostic,
) -> lsp::Diagnostic;
```

Internally use byte offsets.

At the LSP boundary convert between:

```text
UTF-8 byte offsets
```

and LSP line/character coordinates.

Be careful with Unicode.

Do not assume:

```text
byte offset == UTF-16 character offset
```

If the selected LSP protocol version supports negotiated UTF-8 positions and both sides support it, that can simplify things later.

The initial implementation must still be correct for normal VS Code/LSP position semantics.

---

# 40. Running tests from LSP

For the vertical slice, the easiest architecture is deliberately:

```text
VS Code
   |
   | workspace/executeCommand
   | webtest.runFile
   v
webtest lsp process
   |
   v
EditorService::run_file
   |
   v
TestPlan
   |
   v
Runtime
   |
   v
Chromium
   |
   v
RuntimeObservation
   |
   v
ObservationStore
   |
   v
EditorService::diagnostics
   |
   v
publishDiagnostics
```

This proves runtime-to-editor integration without first building cross-process IPC.

After `run_file` finishes, republish diagnostics for that document immediately.

---

# 41. CLI test command

Also implement:

```text
webtest test file.webtest
```

Its flow:

```text
read source
   |
   v
workspace FileId
   |
   v
parse / static diagnostics
   |
   +---- errors? -> print and exit nonzero
   |
   v
TestPlan
   |
   v
Runtime
   |
   v
Chrome
   |
   v
terminal reporter
```

For the initial vertical slice, CLI runtime observations are process-local and do not need to reach a simultaneously running LSP server.

That is a deliberate phase-one limitation.

---

# 42. Future CLI-to-LSP runtime bridge

After the first slice works, add local IPC.

Desired behavior:

```text
terminal:
webtest test foo.webtest

             |
             | local observation protocol
             v

editor:
webtest lsp
             |
             v
ObservationStore
             |
             v
runtime diagnostic appears
```

This must still use the same executable.

No daemon executable should be introduced.

Possible transports:

```text
Unix domain socket on Unix
named pipe on Windows
```

The LSP process can advertise a workspace endpoint through a small runtime metadata file.

Possible metadata:

```json
{
  "pid": 12345,
  "workspace": "...",
  "endpoint": "...",
  "protocol_version": 1
}
```

Every transmitted observation includes:

```text
source path/URI
SourceRevision
TestId
StepId
TextRange
observation kind
```

The LSP process rejects stale revisions.

This is phase two, not part of the first required implementation.

---

# 43. VS Code extension

Create:

```text
editors/vscode
```

Use TypeScript.

Dependencies should include:

```text
vscode
vscode-languageclient
```

The extension contains no language intelligence.

Its responsibilities are:

1. register the language ID;
2. locate the `webtest` executable;
3. spawn:

```text
webtest lsp
```

4. create a `LanguageClient`;
5. register a command to execute the current test file;
6. stop the client on deactivation.

---

# 44. VS Code language configuration

In `package.json`, contribute a language:

```json
{
  "id": "webtest",
  "aliases": ["WebTest", "webtest"],
  "extensions": [".webtest"]
}
```

Add a configuration setting:

```text
webtest.serverPath
```

Default:

```text
webtest
```

This allows development use such as:

```text
/absolute/path/to/target/debug/webtest
```

---

# 45. VS Code server startup

Conceptually:

```typescript
const serverPath =
    vscode.workspace
        .getConfiguration("webtest")
        .get<string>("serverPath") ?? "webtest";

const serverOptions = {
    command: serverPath,
    args: ["lsp"],
};

const clientOptions = {
    documentSelector: [
        {
            scheme: "file",
            language: "webtest",
        },
    ],
};

client = new LanguageClient(
    "webtest",
    "WebTest Language Server",
    serverOptions,
    clientOptions,
);

await client.start();
```

Adapt to the current `vscode-languageclient` API.

Do not embed parser logic in the extension.

---

# 46. VS Code run command

Register:

```text
webtest.runFile
```

User-visible title:

```text
WebTest: Run Current File
```

The VS Code command should send:

```text
workspace/executeCommand
```

to the language server with:

```text
command = "webtest.runFile"
argument = current document URI
```

The LSP server runs the currently synchronized editor contents, not an independently re-read stale disk copy.

That means unsaved tests can be run and receive correct runtime diagnostics.

---

# 47. Desired VS Code experience

Given:

```text
test "missing button" {
    browser {
        open "http://127.0.0.1:3000"
        click id("does-not-exist")
    }
}
```

the user invokes:

```text
WebTest: Run Current File
```

Chromium executes the test.

If no element exists:

```text
click id("does-not-exist")
      ^^^^^^^^^^^^^^^^^^^^
```

VS Code displays an error diagnostic:

```text
No element with id "does-not-exist" was found during the last test run.
```

If the page is fixed and the user reruns the test successfully, the diagnostic disappears.

If the source changes after the run, stale runtime diagnostics disappear.

This is the primary acceptance criterion for the vertical slice.

---

# 48. Browser test fixture for development

Do not require a real application repository for automated tests.

Create a tiny integration-test HTTP server serving:

```html
<!doctype html>
<html>
<body>
    <button id="submit">Submit</button>
</body>
</html>
```

Serve it on:

```text
127.0.0.1:<random-port>
```

Generate DSL source using that port.

Passing:

```text
test "button exists" {
    browser {
        open "http://127.0.0.1:PORT"
        click id("submit")
    }
}
```

Failing:

```text
test "button missing" {
    browser {
        open "http://127.0.0.1:PORT"
        click id("missing")
    }
}
```

Avoid hard-coded ports in automated tests.

---

# 49. Vertical slice implementation plan

Implement the vertical slice in the following order.

## Phase 1 — Workspace and core types

Create the Cargo workspace.

Implement:

```text
FileId
SourceRevision
SyntaxOrigin
TestId
StepId
ExecutionId
```

Add workspace CI commands:

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Acceptance:

```text
workspace compiles
all crates have intentional dependency boundaries
```

---

## Phase 2 — Lexer and lossless CST

Implement lexer tokens for the minimal grammar.

Input:

```text
test "x" {
    // comment
    browser {
        open "http://example.test"
        click id("foo")
    }
}
```

Ensure the reconstructed CST text exactly equals input.

Test:

```rust
assert_eq!(parse.syntax().text().to_string(), source);
```

This property should be tested on every syntax fixture.

Acceptance:

```text
lossless round trip
comments retained
whitespace retained
invalid input produces Error nodes/diagnostics rather than panic
```

---

## Phase 3 — Typed AST

Implement typed wrappers for:

```text
TestDecl
BrowserBlock
OpenStmt
ClickStmt
IdLocator
```

Add tests proving accessors work from the CST.

No second tree builder.

---

## Phase 4 — HIR lowering

Lower typed syntax to:

```text
HirFile
HirTest
HirBrowserBlock
HirOpen
HirClick
HirLocator
```

Preserve source origins.

Tests must verify that the `HirLocator` range points at:

```text
id("foo")
```

or the precise desired locator source range.

---

## Phase 5 — Static diagnostics

Implement at least:

```text
syntax errors
missing test name
missing URL
missing locator string
```

Expose them through `analysis`.

Implement:

```text
webtest check file.webtest
```

Behavior:

```text
exit 0 when statically valid
exit nonzero on static errors
```

---

## Phase 6 — Formatter

Implement the simple CST formatter.

Add:

```text
webtest fmt file.webtest
```

Initially it may rewrite the file in place.

Also expose the same implementation through `EditorService`.

Verify comments survive.

---

## Phase 7 — TestPlan lowering

Lower valid HIR to:

```text
TestPlan
PlannedTest
PlannedStep
BrowserOperation::Open
BrowserOperation::Click
```

Assign stable-for-the-plan `StepId`s.

Preserve exact source ranges.

Test snapshot/debug output if useful.

---

## Phase 8 — CDP connection

Implement:

```text
ChromeProcess
CdpConnection
CdpSession
```

First create a test capable of:

```text
launch Chrome
connect
navigate to data:text/html,<h1>Hello</h1>
evaluate document.querySelector("h1").textContent
assert "Hello"
close Chrome
```

This test may be ignored automatically if Chrome cannot be located.

Use:

```text
WEBTEST_CHROME_PATH
```

for development/CI.

---

## Phase 9 — Browser abstraction

Implement CDP-backed:

```text
BrowserHost
BrowserSession
Page
```

Support:

```text
open
click Locator::Id
```

Test:

```text
click id("submit") -> success
click id("missing") -> BrowserError::LocatorNotFound
```

---

## Phase 10 — Runtime and events

Implement sequential test execution.

For each step emit:

```text
StepStarted
StepPassed
or
StepFailed
```

On `LocatorNotFound`, create a `RuntimeObservation`.

Include:

```text
FileId
SourceRevision
TestId
StepId
TextRange
locator
```

---

## Phase 11 — CLI test command

Implement:

```text
webtest test <file>
```

Example output:

```text
running 1 test

test missing button ... FAILED

failure:
  click id("does-not-exist")

  No element with id "does-not-exist" was found.

1 failed
```

Use structured runtime errors and format them only in the CLI reporting layer.

---

## Phase 12 — EditorService

Implement:

```text
diagnostics
format
run_file
```

`run_file` must write runtime observations into the `ObservationStore`.

After a successful rerun, previous failures for the file/revision are removed.

---

## Phase 13 — Tower LSP

Implement:

```text
webtest lsp
```

using `tower-lsp-server`.

Support:

```text
didOpen
didChange
didClose
formatting
executeCommand
publishDiagnostics
```

On edit:

```text
update file text
compute new SourceRevision
publish static diagnostics
runtime observations from old revision are omitted
```

On:

```text
webtest.runFile
```

execute current editor text and republish diagnostics.

---

## Phase 14 — VS Code extension

Implement:

```text
editors/vscode
```

Features:

```text
.webtest language registration
spawn webtest lsp
format document through LSP
display diagnostics
WebTest: Run Current File
```

Add a development launch configuration if useful.

Document:

```text
npm install
npm run compile
F5
```

for extension development.

---

## Phase 15 — End-to-end acceptance test

The final manual acceptance path is:

1. Start fixture application containing:

```html
<button id="submit">Submit</button>
```

2. Open:

```text
failing.webtest
```

in VS Code.

3. Source contains:

```text
click id("does-not-exist")
```

4. Run:

```text
WebTest: Run Current File
```

5. Headless Chrome starts.

6. Test navigates successfully.

7. Runtime attempts click.

8. Runtime finds zero matching elements.

9. Browser layer emits structured `LocatorNotFound`.

10. Runtime converts it to a source observation.

11. Observation is stored for the current source revision.

12. EditorService combines it with static diagnostics.

13. Tower LSP publishes the diagnostic.

14. VS Code underlines:

```text
id("does-not-exist")
```

15. Change source to:

```text
click id("submit")
```

16. Old runtime diagnostic immediately becomes stale and disappears.

17. Run again.

18. Test passes.

19. No runtime diagnostic remains.

If this works, the architecture has been validated.

---

# 50. Testing strategy

Every layer should be testable independently.

## Syntax

Golden/fixture tests:

```text
valid file
comments
missing brace
missing string
garbage token
half-typed locator
```

Invariant:

```text
CST text == input text
```

---

## HIR

Given syntax:

```text
click id("foo")
```

assert:

```text
HIR operation = Click
locator = Id("foo")
origin range maps to locator source
```

---

## Plan

Verify:

```text
same file revision
deterministic step ordering
source mapping retained
```

---

## Browser

Tests using real Chrome:

```text
navigate
find existing ID
missing ID
click existing button
```

---

## Runtime

Use a fake browser implementation for most runtime tests.

Example:

```rust
struct FakeBrowser {
    click_result: Result<(), BrowserError>,
}
```

This allows testing observation behavior without Chrome.

---

## Editor

Use fake runtime/browser to verify:

```text
runtime failure -> DiagnosticSource::Runtime
successful rerun -> diagnostic cleared
source revision change -> diagnostic hidden
```

---

## LSP

Do not require VS Code for most LSP tests.

Test backend/service requests where practical.

Verify:

```text
didOpen -> diagnostics
formatting -> formatted edit
executeCommand -> runtime diagnostic published
```

---

# 51. Error-handling rules

No normal malformed user program should panic.

Use typed errors.

Distinguish:

```text
syntax error
semantic error
runtime test failure
browser infrastructure failure
internal bug
```

A missing button is a test/runtime failure, not a CDP infrastructure error.

A dead Chrome WebSocket is infrastructure failure.

A malformed DSL string is a static syntax error.

---

# 52. Logging

Use `tracing`.

Critical rule for LSP:

```text
never write arbitrary logs to stdout
```

LSP owns stdout.

Logs go to stderr or a configured file.

Useful spans:

```text
parse
lower_hir
build_plan
run_test
run_step
browser_command
cdp_command
lsp_request
```

Do not log large DOMs or secrets by default.

---

# 53. Runtime/editor distinction

Never describe runtime observations internally as "LSP errors."

Correct model:

```text
runtime
    |
    v
RuntimeObservation
    |
    v
ObservationStore
    |
    v
EditorService
    |
    +---- LSP diagnostic
    |
    +---- Monaco marker
    |
    +---- terminal report
    |
    +---- future HTML trace
```

LSP is one presentation channel.

---

# 54. Future editor functionality

Once the vertical slice works, add editor functionality from shared semantics:

```text
hover
completion
go to definition
references
rename
semantic tokens
code actions
inlay hints
```

Runtime observations can later produce dynamic features such as:

```text
click id("submit")
         ^ runtime: matched 0 elements
```

or:

```text
click text("Save")
     ^ runtime: matched 4 elements
```

Potential code action:

```text
Use id("save-changes")
```

Do not build these before the vertical slice is complete.

---

# 55. Future locator model

Expand:

```rust
pub enum Locator {
    Id(String),

    Role {
        role: Role,
        name: Option<String>,
    },

    Label(String),
    Text(String),
    Placeholder(String),
    TestId(String),
    Css(String),
    XPath(String),
}
```

Prefer semantic locators in language design:

```text
click role("button", name: "Save")

fill label("Email") with "me@example.com"
```

rather than encouraging CSS as the primary interface.

---

# 56. Future static type system

Eventually add domain-specific types:

```text
String
Int
Bool
Url
Duration
StatusCode
Json
Locator
Element
Request
Response<T>
Cookie
Page
BrowserContext
```

Examples that should eventually fail statically:

```text
open 100
```

and:

```text
expect response.status == "200"
```

Do not implement the full type system in the first slice.

---

# 57. Future server-side model

The language should eventually express:

```text
test "password reset" {
    server {
        user = create_user()
    }

    browser {
        open "/forgot-password"
        fill label("Email") with user.email
        click role("button", name: "Send reset link")
    }

    server {
        mail = mailbox.latest(to: user.email)
        reset_url = mail.link("Reset password")
    }

    browser {
        open reset_url
    }
}
```

Cross-domain values must eventually be statically checked for transferability.

Transferable examples:

```text
String
Bool
Int
Url
JSON-like records
```

Non-transferable examples:

```text
database connections
file handles
browser Page handles
sockets
```

This should eventually resemble an execution-capability/effect system.

---

# 58. Future WASM architecture

Portable crates should compile for:

```text
wasm32-unknown-unknown
```

The browser integration eventually becomes:

```text
Monaco UI thread
      |
      | messages
      v
Web Worker
      |
      v
webtest.wasm
      |
      +-- parser
      +-- CST
      +-- HIR
      +-- semantics
      +-- formatter
      +-- editor services
```

The WASM API should expose stable DTOs rather than Rust internals.

Example conceptual JS-facing API:

```text
openDocument(uri, text)
updateDocument(uri, text)
diagnostics(uri)
format(uri)
completion(uri, offset)
hover(uri, offset)
compileTestPlan(uri)
```

The actual editor services remain Rust implementations.

Do not make Monaco speak LSP unless there is a compelling reason.

Monaco should call editor services directly through the WASM worker.

---

# 59. Future npm package

Eventually publish:

```text
@webtest/editor
```

containing:

```text
webtest.wasm
worker.js
index.js
index.d.ts
Monaco adapter
```

TypeScript must remain glue.

No alternate TypeScript parser.

---

# 60. Native shipping strategy

Release precompiled `webtest` executables for:

```text
macOS arm64
macOS x86_64 if desired
Linux x86_64
Linux arm64
Windows x86_64
```

Possible distribution:

```text
release archives
shell installer
Homebrew
winget
Scoop
optional npm wrapper
official container image
```

Users should not require the Rust toolchain.

---

# 61. Browser distribution strategy

After the vertical slice, implement managed Chrome for Testing.

Conceptual layout:

```text
~/.cache/webtest/
    browsers/
        chrome/
            <version>/
```

Commands:

```text
webtest browser install
webtest browser list
webtest browser path
webtest browser clean
```

Each `webtest` release should identify a tested/pinned browser version.

Default:

```text
use managed pinned Chrome
```

Override:

```text
WEBTEST_CHROME_PATH
```

or config.

Do not embed hundreds of megabytes of Chromium into the `webtest` executable itself.

The user experience can still feel like one product.

---

# 62. CI shipping

Eventually provide an official container containing:

```text
webtest
matching Chrome for Testing
required Linux browser libraries
```

Then CI can run:

```text
webtest test
```

without managing browser dependencies manually.

---

# 63. Long-term trace architecture

All execution should continue moving toward an event stream:

```text
runtime
   |
   v
ExecutionEvent stream
   |
   +---- terminal reporter
   |
   +---- ObservationStore
   |
   +---- trace artifact
   |
   +---- future HTML viewer
```

A future trace should contain:

```text
manifest
events
source revisions
screenshots
network information
console information
DOM snapshots
timings
```

Do not couple the runner directly to terminal formatting.

---

# 64. Architectural invariants

These rules should be written into project documentation and treated as design constraints.

```text
ONE grammar.

ONE lexer.

ONE parser.

ONE canonical lossless CST.

Typed AST nodes are views over CST nodes.

ONE syntax-to-HIR lowering path.

ONE semantic engine.

ONE formatter implementation.

ONE TestPlan lowering path.

ONE browser semantic abstraction.

ONE runtime event model.

ONE editor-services implementation.

LSP is an adapter.

Monaco is an adapter.

VS Code is an adapter.

CLI is a host/adapter.

CDP is a browser backend, not the language model.

Runtime failures are structured observations, not strings.

All runtime observations carry source revision identity.

Core crates do not depend on LSP protocol types.

Core crates do not depend on Monaco or VS Code.

Portable core crates should avoid native OS assumptions.

The native product ships as one executable.
```

---

# 65. Explicit dependency prohibitions

Do not allow:

```text
syntax -> analysis
syntax -> runtime
syntax -> lsp

hir -> lsp
hir -> browser-cdp

analysis -> lsp
analysis -> vscode
analysis -> cdp

editor -> lsp protocol types

browser -> DSL parser

runtime -> terminal output formatting
```

Instead:

```text
lsp -> editor
editor -> analysis
runtime -> plan
browser-cdp -> browser
app -> everything necessary to compose the product
```

---

# 66. Initial CLI UX

## Check

```text
$ webtest check test.webtest

ok
```

Failure:

```text
$ webtest check test.webtest

test.webtest:3:14: error[syntax.expected_string]
    expected string after `open`
```

---

## Format

```text
$ webtest fmt test.webtest
```

Rewrites canonical formatting.

Later add:

```text
--check
--stdout
```

---

## Test

```text
$ webtest test test.webtest

running 1 test
test "existing button" ... ok

1 passed
```

Failure:

```text
running 1 test
test "missing button" ... FAILED

test.webtest:4:15
    click id("does-not-exist")
          ^^^^^^^^^^^^^^^^^^^^

runtime error[runtime.locator_not_found]:
    No element with id "does-not-exist" was found.

0 passed; 1 failed
```

---

## LSP

```text
webtest lsp
```

No human-readable output to stdout.

---

# 67. Suggested root Cargo configuration

Conceptually:

```toml
[workspace]
resolver = "2"
members = [
    "crates/text",
    "crates/syntax",
    "crates/hir",
    "crates/analysis",
    "crates/format",
    "crates/plan",
    "crates/observation",
    "crates/browser",
    "crates/browser-cdp",
    "crates/runtime",
    "crates/editor",
    "crates/lsp",
    "crates/wasm",
    "crates/app",
]

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
rowan = "0.16"
salsa = "0.28"
tower-lsp-server = "0.23"

serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
futures = "0.3"
thiserror = "2"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
clap = { version = "4", features = ["derive"] }
blake3 = "1"
tempfile = "3"
```

Add the current compatible stable WebSocket/WASM dependencies when those crates are implemented.

Do not blindly preserve these exact versions if Cargo reveals incompatibilities; preserve the package choices and architecture.

---

# 68. Code quality expectations

Use:

```text
cargo fmt
cargo clippy
cargo test
```

Avoid:

```rust
unwrap()
expect()
```

in normal runtime paths unless the invariant is genuinely impossible to violate and explained.

Prefer typed domain errors.

Keep public APIs narrow.

Use `Arc` only where ownership/lifetime requires it.

Do not introduce global mutable state.

Use `tracing` rather than print debugging.

Use comments for architectural reasoning, not restating obvious code.

---

# 69. Performance expectations for the first slice

Do not prematurely optimize.

However:

* parser should be linear in source size;
* no subsystem should reparse the same source unnecessarily;
* LSP must retain workspace state between requests;
* editor requests must not recreate the entire compiler object;
* browser process may be reused across tests later;
* the first implementation may create one browser/session per run.

Incremental reparsing is not required for the first slice.

A full-file reparse on edit is acceptable initially because all consumers still use the same parser.

---

# 70. Security expectations

Tests intentionally execute against websites and may eventually execute processes.

Still:

* never evaluate user values by unsafely interpolating them into JavaScript;
* pass/serialize values safely;
* do not log secrets unnecessarily;
* isolate Chrome with a temporary profile;
* do not use the user's normal Chrome profile;
* do not enable remote debugging on a publicly accessible interface;
* bind/debug locally only;
* clean temporary profiles after runs.

Do not add `--no-sandbox` by default.

CI environments requiring it may explicitly opt in later.

---

# 71. Things explicitly not required in the first slice

Do not let these delay the vertical slice:

```text
full Playwright-equivalent actionability
automatic Chrome downloads
Firefox
WebKit
WebDriver BiDi
CSS locators
role locators
text locators
fill/type
assertions
HTTP DSL
fixtures
package manager
module system
type inference
parallel tests
retries
snapshots
network mocking
screenshots
trace viewer
CLI-to-LSP IPC
Monaco package
WASM runtime execution
database integration
code actions
completion
hover
rename
```

The architecture should accommodate them.

The implementation should not include them until the basic runtime diagnostic loop works.

---

# 72. Definition of success for the first milestone

The milestone is complete when this entire path works:

```text
                     VS CODE

                        |
                        | source edit
                        v
                 tower-based LSP
                        |
                        v
                  EditorService
                        |
                        v
                     parser
                        |
                        v
                 lossless CST
                        |
                        v
                       HIR
                        |
                        v
                    TestPlan
                        |
                        v
                     runtime
                        |
                        v
                  browser API
                        |
                        v
                    CDP backend
                        |
                        v
              headless Chromium
                        |
              missing DOM element
                        |
                        v
              structured failure
                        |
                        v
             RuntimeObservation
                        |
                        v
              ObservationStore
                        |
                        v
                 EditorService
                        |
                        v
                 Tower LSP
                        |
                        v
              publishDiagnostics
                        |
                        v
                     VS CODE

 click id("does-not-exist")
       ^^^^^^^^^^^^^^^^^^^^
 No element with id "does-not-exist"
 was found during the last test run.
```

No duplicate parser or semantic implementation is allowed anywhere in that path.

---

# 73. Implementation instructions for the coding agent

Implement this project incrementally rather than generating a giant untested codebase in one pass.

For every phase:

1. make the workspace compile;
2. add unit tests;
3. run the complete workspace test suite;
4. run Clippy;
5. only then move to the next phase.

Prefer the narrowest implementation that preserves the architecture.

When faced with a choice between:

```text
more functionality
```

and:

```text
clean dependency boundaries
```

choose clean dependency boundaries.

When faced with a choice between:

```text
duplicating logic in an adapter
```

and:

```text
adding a reusable internal abstraction
```

add the reusable internal abstraction.

Never create a separate parser for:

```text
formatter
LSP
runtime
VS Code
```

Never return LSP types from `editor`.

Never make HIR depend on CDP.

Never make browser actions depend on source syntax.

Always preserve source origins when lowering:

```text
CST
-> HIR
-> TestPlan
-> runtime StepId
-> observation
```

Before considering the vertical slice complete, demonstrate the missing-button runtime failure visually in VS Code and demonstrate that editing or fixing the source clears the stale diagnostic.

---

# 74. Recommended first implementation sequence for an autonomous agent

A coding agent should create commits or logical checkpoints corresponding approximately to:

```text
01 workspace skeleton
02 lexer + rowan CST
03 parser + recovery
04 typed AST
05 HIR lowering + source origins
06 analysis DB + static diagnostics
07 formatter + CLI check/fmt
08 TestPlan lowering
09 Chrome process + CDP WebSocket
10 CDP page navigation/evaluation
11 browser abstraction + id click
12 runtime event model
13 observation store
14 CLI test
15 EditorService
16 Tower LSP
17 VS Code extension
18 runtime observation -> LSP diagnostic
19 end-to-end fixture
20 cleanup/docs/tests
```

At checkpoint 20, stop and evaluate the architecture before expanding the language.

---

# 75. Final design summary

The project is not fundamentally:

```text
a browser automation library
```

and it is not fundamentally:

```text
an LSP server
```

It is a single **language platform for web-system testing**.

Its core model is:

```text
source
   |
   v
lossless syntax
   |
   v
semantic test program
   |
   v
typed execution plan
   |
   v
observable runtime
   |
   +--------------------+
   |                    |
   v                    v
server world        browser world
                         |
                         v
                     Chromium
   |                    |
   +---------+----------+
             |
             v
      runtime observations
             |
             v
       editor services
        /           \
       v             v
     Tower          WASM
      LSP           Monaco
```

The native product is:

```text
webtest
```

One executable.

The browser tooling build is:

```text
webtest.wasm
```

The parser, formatter, compiler frontend, semantic engine, editor intelligence, TestPlan compiler, and runtime source mappings all originate from the same Rust implementation.

The most important product feature to prove first is:

> A browser runtime fact can flow all the way back to the exact DSL source expression that caused it.

The first demonstration of that principle is intentionally simple:

```text
click id("does-not-exist")
```

Headless Chromium observes that no such element exists.

That observation flows:

```text
Chromium
-> CDP
-> BrowserError
-> RuntimeObservation
-> ObservationStore
-> EditorService
-> Tower LSP
-> VS Code
```

and the user's editor immediately shows:

```text
click id("does-not-exist")
      ^^^^^^^^^^^^^^^^^^^^
      No element with id "does-not-exist" was found during the last test run.
```

That end-to-end feedback loop should be implemented before expanding the DSL substantially. It validates the most important architectural bet and provides the foundation for everything that follows.
