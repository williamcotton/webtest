# Language-neutral application bridge examples

Every directory is a self-contained WebTest project. All nine copy the byte-identical
`created-user.webtest`, export the same semantic `create_user(email: String, admin?: Bool = false)`
schema, and make the bridge handler mutate the same in-process store used by `POST /login`.

| Directory | Runtime | Binding |
| --- | --- | --- |
| `node/` | TypeScript/Node.js | official `@webtest/node` core SDK |
| `ruby/` | Ruby | official `webtest` core SDK |
| `go/` | Go | generated example-local binding |
| `python/` | Python | generated example-local binding |
| `elixir/` | Elixir | generated example-local binding |
| `java/` | Java/JVM | generated example-local binding |
| `dotnet/` | C#/.NET | generated example-local binding |
| `rust/` | Rust | generated example-local binding |
| `php/` | PHP | generated example-local binding |

Example-local bindings implement only transport, registration, schema export/validation, and error
conversion. They are not supported SDK packages and do not parse the DSL or duplicate WebTest
semantics. Every app binds loopback, starts with isolated memory, and enables its bridge only with
`WEBTEST=1`.

Build WebTest, verify byte/schema equality and stopped-app checks, then smoke one installed runtime:

```sh
cargo build
python3 scripts/check-application-bridge-examples.py
python3 scripts/check-application-bridge-examples.py --example node --smoke
```

The smoke checker replaces the documented fixed port with a dynamically allocated loopback port in
a temporary project and proves the port can be rebound after WebTest teardown. Missing toolchains are
explicit local skips; release CI passes `--require-toolchain` and treats them as failures.

The `stdio/` directory is a separate provider-only project for the independently implemented no-SDK
reference executable. The existing `examples/simple-server/` remains the built-in `http.*` example.

