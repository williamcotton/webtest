//! Language-neutral application bridge protocol, schemas, transports, and provider.

#[cfg(feature = "native")]
mod protocol;
#[cfg(feature = "native")]
mod provider;
mod schema;
mod wire;

#[cfg(feature = "native")]
pub use protocol::{DEFAULT_MAX_MESSAGE_BYTES, ProtocolError, read_frame, write_frame};
#[cfg(feature = "native")]
pub use provider::{
    AppAdapter, AppHttpConfig, AppProcessConfig, AppProvider, AppProviderConfig, AppTransport,
    ApplicationLifecycle, HealthCheck, HttpOperation,
};
pub use schema::{
    AppManifest, AppSchemaError, FieldSchema, FunctionSchema, SchemaLimits, TypeSchema,
    canonical_schema_hash,
};
pub use wire::{BridgeCapabilities, BridgeMessage, PROTOCOL_VERSION};
