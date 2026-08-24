# Python application bridge example

Uses Python's standard HTTP/socket libraries and an example-local generated protocol binding. The
bridge and login handler share the same `USERS` dictionary. Requires Python 3.9+ and Chrome; there
are no third-party packages to install.

```sh
python3 -m py_compile server.py
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example python
```

The final two commands verify deterministic schema regeneration.
