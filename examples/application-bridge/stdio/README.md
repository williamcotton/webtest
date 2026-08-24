# No-SDK stdio bridge

This project proves that a standalone executable can implement protocol 1 without an SDK. The
application is stopped for static analysis:

```sh
cargo run -p webtest -- check examples/application-bridge/stdio
```

The provider-only test needs no browser and starts the reference executable over persistent stdio:

```sh
cargo run -p webtest -- test examples/application-bridge/stdio
```

