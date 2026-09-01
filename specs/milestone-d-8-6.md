# Milestone D.8.6 — Order-Independent Runner Provider Configuration

## 0. Status and dependencies

**Status: implemented (research baseline verified at `580b5708a20707449b8487f393fc46b11bd6c628`; implementation completed 2026-09-01).**

**Implementation status (2026-09-01):** `Runner` records either built-ins derived from the final
options or an explicit registry. `with_options` changes only options, explicit registries take
precedence in every builder order, repeated setters use their last value, and the selected registry
is resolved once per run. Runtime lifecycle coverage exercises both option/registry permutations,
event-sink independence, and final HTTP/process/filesystem configuration. Editor execution uses the
reversed builder order in production and verifies that its composed custom registry still executes,
while app and DAP retain their existing shared registry composition paths.

This is a narrow public-API correctness milestone. It can be implemented after the outcome work in
[`milestone-d-8-1.md`](./milestone-d-8-1.md) through
[`milestone-d-8-5.md`](./milestone-d-8-5.md), but it has no semantic dependency on browser
allocation or plan format changes.

## 1. Outcome

`Runner` builder methods are order independent:

```rust
Runner::new(store)
    .with_provider_registry(custom.clone())
    .with_options(options.clone());

Runner::new(store)
    .with_options(options)
    .with_provider_registry(custom);
```

Both runners use the explicit custom registry. `with_options` mutates only runner options; it never
silently replaces another field.

When no explicit registry is supplied, built-ins are derived from the final
`RunnerOptions.provider_config` at execution/construction finalization. The convenience of
`Runner::new(...).with_options(options)` therefore remains correct.

## 2. Research baseline

`Runner::new` currently constructs built-ins from `NativeProviderConfig::default()`.
`with_provider_registry` assigns the caller's registry. `with_options` then performs two unrelated
mutations:

```text
self.providers = ProviderRegistry::built_in(options.provider_config.clone())
self.options = options
```

As a result:

```text
with_options(...).with_provider_registry(custom)  -> custom survives
with_provider_registry(custom).with_options(...)  -> custom is discarded
```

The app, editor, and DAP currently happen to use the safe order. Runtime lifecycle test
`with_options_rebuilds_builtins_before_a_later_registry_override` explicitly characterizes the
unsafe order by expecting `ProviderError::NotRegistered` for the discarded custom provider.

This behavior violates the ordinary builder expectation that setters for independent fields
commute. It is especially dangerous because the chain compiles, builds a valid runner, and fails
only when a custom operation is called.

## 3. Required design

Represent provider selection explicitly, for example:

```rust
enum ProviderSelection {
    BuiltInsFromOptions,
    Explicit(ProviderRegistry),
}

pub struct Runner {
    options: RunnerOptions,
    providers: ProviderSelection,
    // observations and event sink
}
```

Semantics:

- `Runner::new` selects `BuiltInsFromOptions`;
- `with_options` replaces only `options`;
- `with_provider_registry` selects `Explicit` and stores the registry;
- at run start, `BuiltInsFromOptions` creates/uses a registry from the final
  `options.provider_config`;
- `Explicit` uses the caller registry exactly, regardless of builder order;
- options never attempt to merge, rewrite, or patch an explicit registry.

The actual implementation may eagerly cache the built-in registry if it can prove a later options
change updates only that derived cache. A small selection enum is preferred because it makes
precedence visible and prevents future builder setters from recreating order dependence.

If registry construction can fail in the future, expose that relationship with a fallible
constructor/build step rather than hiding failure or replacement in `with_options`.

## 4. Public API contract

Keep current root-level names unless a focused API cleanup is separately approved:

- `Runner::new`;
- `Runner::with_options`;
- `Runner::with_provider_registry`;
- `Runner::with_event_sink`.

Document explicit-registry precedence on both relevant builder methods. The event sink and
observation store remain independent and order insensitive as today.

`RunnerOptions.provider_config` stays because `Runner::new + with_options` and editor/DAP default
composition use it. D.8.6 does not move application-provider lifecycle into runtime or teach
runtime how to compose an app bridge. App/editor/DAP may continue passing a fully composed explicit
registry.

## 5. Implementation scope

### `runtime`

- replace the implicit registry reset with explicit provider selection;
- update public API/characterization tests;
- ensure the selected registry is stable for one run and cannot change mid-execution;
- avoid constructing built-ins repeatedly per test.

### `app`, `editor`, and `dap`

- retain existing composition but add at least one test using the reversed builder order;
- remove comments/tests that imply call ordering is required;
- do not duplicate provider precedence logic in adapters.

No provider, plan, analysis, or project API change is required.

## 6. Required tests

1. both permutations of `with_options` and `with_provider_registry` call the same custom provider;
2. repeated `with_options` calls preserve an explicit registry and last options win;
3. repeated `with_provider_registry` calls use the last explicit registry;
4. `with_options` without an explicit registry configures built-in HTTP/process/filesystem
   providers from the final `NativeProviderConfig`;
5. `with_event_sink` commutes with both provider/options setters;
6. app/editor/DAP construction still uses its composed project registry;
7. the old regression test is inverted and no test describes replacement as desired behavior.

## 7. Out of scope

- mutable provider reconfiguration during a run;
- merging two explicit registries;
- changing provider schema invalidation in analysis/editor;
- application-process start/shutdown ownership;
- removing `provider_config` from `RunnerOptions`;
- a generic dependency-injection container or typestate builder.

## 8. Verification

```sh
cargo test -p webtest-runtime
cargo test -p webtest-editor -p webtest-dap -p webtest
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## 9. Acceptance criteria

1. An explicit provider registry survives `with_options` in every call order.
2. Built-ins still reflect the final options when no explicit registry exists.
3. Provider selection is documented, tested, and represented explicitly rather than as setter side
   effects.
4. App/editor/DAP retain one shared registry composition path and no adapter-specific workaround.
