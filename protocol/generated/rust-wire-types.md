# Rust wire DTO location

The generated Rust DTO implementation is `crates/app-bridge/src/wire.rs` (`BridgeMessage` and
`BridgeCapabilities`). Keeping it in the compiled crate makes drift a workspace build failure.
`protocol/schema.json` remains the wire-shape authority.
