# Ruby application bridge example

Uses the official core Ruby SDK and WEBrick. Requires Ruby, WEBrick, and Chrome.

```sh
bundle install
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example ruby
```

The bridge mutates the same in-process `users` hash read by `POST /login`. Verify deterministic schema
regeneration with the repository matrix checker before committing.
