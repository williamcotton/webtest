# php application bridge example

Uses PHP 8 standard socket/JSON functions with an example-local generated protocol binding. The bridge
and login server share one in-process user store. Requires PHP 8+, Composer, and Chrome.

```sh
composer install
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example php
```

The final two commands verify deterministic schema regeneration.
