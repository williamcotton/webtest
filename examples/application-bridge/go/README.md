# Go application bridge example

Uses Go's standard library and an example-local generated binding. The bridge and HTTP handler share
one mutex-protected map. Requires Go 1.22+ and Chrome.

```sh
go mod download
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example go
```

The final two commands are the deterministic schema-regeneration check; update the canonical
manifest first when the registered operation changes.
