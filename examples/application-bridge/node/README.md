# Node application bridge example

Uses TypeScript on Node 22.6+ with the official `@webtest/node` SDK and native type stripping.
Requires Node 22.6+ and Chrome. The checked-in manifest makes
`webtest describe --project .` and `webtest check .` work while the app is stopped.

```sh
npm install
npm run check:types
cargo run -p webtest -- check .
cargo run -p webtest -- test .
python3 ../../../scripts/check-application-bridge-examples.py --example node
```

Export the deterministic manifest with `AppBridge.exportSchema`, then normalize and verify its
canonical hash with `cargo run -p webtest-app-bridge --example schema-hash -- --write
.webtest/app-schema.json`. Commit only deterministic output.
