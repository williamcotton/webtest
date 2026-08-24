# rust application bridge example

Uses Rust 2024 and `serde_json` with an example-local generated protocol binding. The bridge and login
server share one in-process user store. Requires Rust stable and Chrome.

```sh
cargo fetch
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example rust
```

The final two commands verify deterministic schema regeneration.
