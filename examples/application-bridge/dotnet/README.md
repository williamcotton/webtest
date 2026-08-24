# dotnet application bridge example

Uses .NET 8 and `System.Text.Json` with an example-local generated protocol binding. The bridge and
login server share one in-process user store. Requires the .NET 8 SDK and Chrome.

```sh
dotnet restore
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example dotnet
```

The final two commands verify deterministic schema regeneration.
