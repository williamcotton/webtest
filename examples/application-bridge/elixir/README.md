# Elixir application bridge example

Uses OTP Agent/TCP primitives with an example-local generated binding and no dependencies. The bridge
and HTTP task share one Agent store. Requires Elixir 1.14+, a compatible Erlang/OTP release, and
Chrome.

```sh
mix deps.get
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example elixir
```

The final two commands verify deterministic schema regeneration.
