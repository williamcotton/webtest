# java application bridge example

Uses Java 17 source-file mode and JDK `HttpServer` with an example-local generated protocol binding.
The bridge and login server share one in-process user store. Requires JDK 17+ and Chrome.

```sh
javac Main.java
cargo run -p webtest -- check .
cargo run -p webtest -- test .
cargo run -q -p webtest-app-bridge --example schema-hash -- --write .webtest/app-schema.json
python3 ../../../scripts/check-application-bridge-examples.py --example java
```

The final two commands verify deterministic schema regeneration.
