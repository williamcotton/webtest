//! Shared types, values, schemas, and native server-provider contracts.

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

#[cfg(feature = "native")]
use std::{
    io::Read,
    path::{Component, Path},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Pure,
    Server,
    Browser,
    Test,
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Pure => "Pure",
            Self::Server => "Server",
            Self::Browser => "Browser",
            Self::Test => "Test",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecordField {
    pub ty: Type,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Type {
    Unknown,
    Null,
    Bool,
    Int,
    Float,
    String,
    Duration,
    Url,
    Json,
    List(Box<Type>),
    Option(Box<Type>),
    Record(BTreeMap<String, RecordField>),
    StatusCode,
    Headers,
    Bytes,
    Response(Box<Type>),
    ProcessResult,
    FilePath,
    TempDirectory,
    Locator,
    BrowserPage,
}

impl Type {
    pub fn is_transferable(&self) -> bool {
        match self {
            Self::Unknown => false,
            Self::Null
            | Self::Bool
            | Self::Int
            | Self::Float
            | Self::String
            | Self::Duration
            | Self::Url
            | Self::Json => true,
            Self::List(inner) | Self::Option(inner) => inner.is_transferable(),
            Self::Record(fields) => fields.values().all(|field| field.ty.is_transferable()),
            Self::Response(_)
            | Self::ProcessResult
            | Self::StatusCode
            | Self::Headers
            | Self::Bytes
            | Self::FilePath
            | Self::TempDirectory
            | Self::Locator
            | Self::BrowserPage => false,
        }
    }

    pub fn accepts(&self, actual: &Type) -> bool {
        if matches!(self, Self::Unknown) || matches!(actual, Self::Unknown) || self == actual {
            return true;
        }
        match (self, actual) {
            (Self::Float, Self::Int) => true,
            (Self::StatusCode, Self::Int) | (Self::Int, Self::StatusCode) => true,
            (
                Self::Json,
                Self::Null
                | Self::Bool
                | Self::Int
                | Self::Float
                | Self::String
                | Self::List(_)
                | Self::Record(_),
            ) => true,
            (Self::Option(_), Self::Null) => true,
            (Self::Option(expected), Self::Option(actual))
            | (Self::List(expected), Self::List(actual))
            | (Self::Response(expected), Self::Response(actual)) => expected.accepts(actual),
            (Self::Record(expected), Self::Record(actual)) => {
                expected.iter().all(|(name, field)| {
                    actual
                        .get(name)
                        .is_some_and(|actual| field.ty.accepts(&actual.ty))
                        || field.optional
                })
            }
            _ => false,
        }
    }

    pub fn member(&self, name: &str) -> Option<Type> {
        match self {
            Self::Record(fields) => fields.get(name).map(|field| {
                if field.optional {
                    Type::Option(Box::new(field.ty.clone()))
                } else {
                    field.ty.clone()
                }
            }),
            Self::Response(body) => match name {
                "status" => Some(Self::StatusCode),
                "headers" => Some(Self::Headers),
                "body" => Some(Self::Bytes),
                "text" => Some(Self::String),
                "json" => Some((**body).clone()),
                _ => None,
            },
            Self::ProcessResult => match name {
                "exit_code" => Some(Self::Int),
                "stdout" | "stderr" => Some(Self::String),
                "stdout_bytes" | "stderr_bytes" => Some(Self::Bytes),
                _ => None,
            },
            Self::Headers => Some(Self::Option(Box::new(Self::String))),
            _ => None,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => formatter.write_str("unknown"),
            Self::Null => formatter.write_str("Null"),
            Self::Bool => formatter.write_str("Bool"),
            Self::Int => formatter.write_str("Int"),
            Self::Float => formatter.write_str("Float"),
            Self::String => formatter.write_str("String"),
            Self::Duration => formatter.write_str("Duration"),
            Self::Url => formatter.write_str("Url"),
            Self::Json => formatter.write_str("Json"),
            Self::StatusCode => formatter.write_str("StatusCode"),
            Self::Headers => formatter.write_str("Headers"),
            Self::Bytes => formatter.write_str("Bytes"),
            Self::ProcessResult => formatter.write_str("ProcessResult"),
            Self::FilePath => formatter.write_str("FilePath"),
            Self::TempDirectory => formatter.write_str("TempDirectory"),
            Self::Locator => formatter.write_str("Locator"),
            Self::BrowserPage => formatter.write_str("BrowserPage"),
            Self::List(inner) => write!(formatter, "List<{inner}>"),
            Self::Option(inner) => write!(formatter, "Option<{inner}>"),
            Self::Response(inner) => write!(formatter, "Response<{inner}>"),
            Self::Record(fields) => {
                formatter.write_str("{ ")?;
                for (index, (name, field)) in fields.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(
                        formatter,
                        "{name}{}: {}",
                        if field.optional { "?" } else { "" },
                        field.ty
                    )?;
                }
                formatter.write_str(" }")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    DurationMillis(u64),
    List(Vec<Value>),
    Record(BTreeMap<String, Value>),
    Headers(BTreeMap<String, String>),
    Bytes(Vec<u8>),
    Response(ResponseValue),
    ProcessResult(ProcessResultValue),
    FilePath(PathBuf),
    TempDirectory(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseValue {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub json: Option<Box<Value>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessResultValue {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: Vec<u8>,
    pub stderr_bytes: Vec<u8>,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Int(_) => "integer",
            Self::Float(_) => "number",
            Self::String(_) => "string",
            Self::DurationMillis(_) => "duration",
            Self::List(_) => "array",
            Self::Record(_) => "object",
            Self::Headers(_) => "headers",
            Self::Bytes(_) => "bytes",
            Self::Response(_) => "response",
            Self::ProcessResult(_) => "process result",
            Self::FilePath(_) => "file path",
            Self::TempDirectory(_) => "temporary directory",
        }
    }

    pub fn member(&self, name: &str) -> Option<Value> {
        match self {
            Self::Record(fields) => fields.get(name).cloned(),
            Self::Response(response) => match name {
                "status" => Some(Self::Int(i64::from(response.status))),
                "headers" => Some(Self::Headers(response.headers.clone())),
                "body" => Some(Self::Bytes(response.body.clone())),
                "text" => String::from_utf8(response.body.clone())
                    .ok()
                    .map(Self::String),
                "json" => response.json.as_deref().cloned(),
                _ => None,
            },
            Self::ProcessResult(result) => match name {
                "exit_code" => Some(Self::Int(result.exit_code)),
                "stdout" => Some(Self::String(result.stdout.clone())),
                "stderr" => Some(Self::String(result.stderr.clone())),
                "stdout_bytes" => Some(Self::Bytes(result.stdout_bytes.clone())),
                "stderr_bytes" => Some(Self::Bytes(result.stderr_bytes.clone())),
                _ => None,
            },
            Self::Headers(headers) => Some(
                headers
                    .iter()
                    .find(|(header, _)| header.eq_ignore_ascii_case(name))
                    .map_or(Self::Null, |(_, value)| Self::String(value.clone())),
            ),
            _ => None,
        }
    }

    pub fn redacted(&self, fields: &[String]) -> Value {
        self.redacted_with_secrets(fields, &[])
    }

    pub fn redacted_with_secrets(&self, fields: &[String], secrets: &[String]) -> Value {
        match self {
            Self::Record(values) => Self::Record(
                values
                    .iter()
                    .map(|(name, value)| {
                        let value = if fields.iter().any(|field| field.eq_ignore_ascii_case(name)) {
                            Self::String("[redacted]".into())
                        } else {
                            value.redacted_with_secrets(fields, secrets)
                        };
                        (name.clone(), value)
                    })
                    .collect(),
            ),
            Self::List(values) => Self::List(
                values
                    .iter()
                    .map(|value| value.redacted_with_secrets(fields, secrets))
                    .collect(),
            ),
            Self::Headers(values) => Self::Headers(
                values
                    .iter()
                    .map(|(name, value)| {
                        let value = if fields.iter().any(|field| field.eq_ignore_ascii_case(name)) {
                            "[redacted]".into()
                        } else {
                            redact_text(value, secrets)
                        };
                        (name.clone(), value)
                    })
                    .collect(),
            ),
            Self::String(value) => Self::String(redact_text(value, secrets)),
            Self::ProcessResult(value) => Self::ProcessResult(ProcessResultValue {
                exit_code: value.exit_code,
                stdout: redact_text(&value.stdout, secrets),
                stderr: redact_text(&value.stderr, secrets),
                stdout_bytes: redact_bytes(&value.stdout_bytes, secrets),
                stderr_bytes: redact_bytes(&value.stderr_bytes, secrets),
            }),
            Self::Response(value) => Self::Response(ResponseValue {
                status: value.status,
                headers: value
                    .headers
                    .iter()
                    .map(|(name, value)| {
                        let value = if fields.iter().any(|field| field.eq_ignore_ascii_case(name)) {
                            "[redacted]".into()
                        } else {
                            redact_text(value, secrets)
                        };
                        (name.clone(), value)
                    })
                    .collect(),
                body: redact_bytes(&value.body, secrets),
                json: value
                    .json
                    .as_deref()
                    .map(|value| Box::new(value.redacted_with_secrets(fields, secrets))),
            }),
            value => value.clone(),
        }
    }
}

fn redact_text(value: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(value.to_owned(), |value, secret| {
            value.replace(secret, "[redacted]")
        })
}

fn redact_bytes(value: &[u8], secrets: &[String]) -> Vec<u8> {
    std::str::from_utf8(value)
        .map(|value| redact_text(value, secrets).into_bytes())
        .unwrap_or_else(|_| value.to_vec())
}

pub fn value_from_json(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(value) => Value::Bool(value),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(Value::Int)
            .or_else(|| value.as_f64().map(Value::Float))
            .unwrap_or(Value::Null),
        serde_json::Value::String(value) => Value::String(value),
        serde_json::Value::Array(values) => {
            Value::List(values.into_iter().map(value_from_json).collect())
        }
        serde_json::Value::Object(values) => Value::Record(
            values
                .into_iter()
                .map(|(name, value)| (name, value_from_json(value)))
                .collect(),
        ),
    }
}

pub fn value_to_json(value: &Value) -> Option<serde_json::Value> {
    match value {
        Value::Null => Some(serde_json::Value::Null),
        Value::Bool(value) => Some((*value).into()),
        Value::Int(value) => Some((*value).into()),
        Value::Float(value) => serde_json::Number::from_f64(*value).map(Into::into),
        Value::String(value) => Some(value.clone().into()),
        Value::DurationMillis(value) => Some((*value).into()),
        Value::List(values) => values
            .iter()
            .map(value_to_json)
            .collect::<Option<Vec<_>>>()
            .map(Into::into),
        Value::Record(values) => values
            .iter()
            .map(|(name, value)| Some((name.clone(), value_to_json(value)?)))
            .collect::<Option<serde_json::Map<_, _>>>()
            .map(Into::into),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderName(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationName(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterSchema {
    pub name: String,
    pub ty: Type,
    pub required: bool,
    pub positional: bool,
    pub secret: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSchema {
    pub name: OperationName,
    pub parameters: Vec<ParameterSchema>,
    pub result: Type,
    pub capability: Capability,
    pub documentation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSchema {
    pub name: ProviderName,
    pub operations: BTreeMap<String, OperationSchema>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSchemaProvenance {
    BuiltIn,
    ProjectSupplied,
}

impl ProviderSchema {
    pub fn operation(&self, name: &str) -> Option<&OperationSchema> {
        self.operations.get(name)
    }

    pub fn hash(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        blake3::hash(&bytes).to_hex().to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderCall {
    pub provider: ProviderName,
    pub operation: OperationName,
    pub arguments: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderResult {
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct CallContext {
    pub project_root: PathBuf,
    pub timeout: Duration,
    pub redacted_json_fields: Vec<String>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderError {
    #[error("provider `{provider}` is not registered")]
    NotRegistered { provider: String },
    #[error("unknown operation `{provider}.{operation}`")]
    UnknownOperation { provider: String, operation: String },
    #[error("invalid provider argument: {message}")]
    InvalidArgument { message: String },
    #[error("HTTP transport failed: {message}")]
    HttpTransport { message: String },
    #[error("response exceeded the configured {limit} byte limit")]
    ResponseTooLarge { limit: usize },
    #[error("process could not be spawned: {message}")]
    ProcessSpawn { message: String },
    #[error("process timed out after {timeout_ms}ms (cleanup succeeded: {cleanup_succeeded})")]
    ProcessTimeout {
        timeout_ms: u64,
        cleanup_succeeded: bool,
    },
    #[error("process output exceeded the configured {limit} byte limit")]
    ProcessOutputTooLarge { limit: usize },
    #[error("filesystem operation failed for `{path}`: {message}")]
    Filesystem { path: String, message: String },
    #[error("path `{path}` escapes the permitted project root")]
    PathEscape { path: String },
    #[error("provider operation is unavailable on this host")]
    Unavailable,
}

impl ProviderError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotRegistered { .. } => "provider_not_registered",
            Self::UnknownOperation { .. } => "provider_unknown_operation",
            Self::InvalidArgument { .. } => "provider_invalid_argument",
            Self::HttpTransport { .. } => "http_transport",
            Self::ResponseTooLarge { .. } => "response_too_large",
            Self::ProcessSpawn { .. } => "process_spawn",
            Self::ProcessTimeout { .. } => "process_timeout",
            Self::ProcessOutputTooLarge { .. } => "process_output_too_large",
            Self::Filesystem { .. } => "filesystem",
            Self::PathEscape { .. } => "path_escape",
            Self::Unavailable => "provider_unavailable",
        }
    }

    pub fn is_infrastructure(&self) -> bool {
        !matches!(self, Self::InvalidArgument { .. } | Self::PathEscape { .. })
    }

    pub fn redacted(&self, secrets: &[String]) -> Self {
        match self {
            Self::NotRegistered { provider } => Self::NotRegistered {
                provider: redact_text(provider, secrets),
            },
            Self::UnknownOperation {
                provider,
                operation,
            } => Self::UnknownOperation {
                provider: redact_text(provider, secrets),
                operation: redact_text(operation, secrets),
            },
            Self::InvalidArgument { message } => Self::InvalidArgument {
                message: redact_text(message, secrets),
            },
            Self::HttpTransport { message } => Self::HttpTransport {
                message: redact_text(message, secrets),
            },
            Self::ResponseTooLarge { limit } => Self::ResponseTooLarge { limit: *limit },
            Self::ProcessSpawn { message } => Self::ProcessSpawn {
                message: redact_text(message, secrets),
            },
            Self::ProcessTimeout {
                timeout_ms,
                cleanup_succeeded,
            } => Self::ProcessTimeout {
                timeout_ms: *timeout_ms,
                cleanup_succeeded: *cleanup_succeeded,
            },
            Self::ProcessOutputTooLarge { limit } => Self::ProcessOutputTooLarge { limit: *limit },
            Self::Filesystem { path, message } => Self::Filesystem {
                path: redact_text(path, secrets),
                message: redact_text(message, secrets),
            },
            Self::PathEscape { path } => Self::PathEscape {
                path: redact_text(path, secrets),
            },
            Self::Unavailable => Self::Unavailable,
        }
    }
}

#[async_trait]
pub trait ServerProvider: Send + Sync {
    fn schema(&self) -> ProviderSchema;
    async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError>;
}

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    schemas: BTreeMap<String, ProviderSchema>,
    schema_provenance: BTreeMap<String, ProviderSchemaProvenance>,
    providers: HashMap<String, Arc<dyn ServerProvider>>,
}

impl ProviderRegistry {
    pub fn built_in_schemas() -> Self {
        let mut registry = Self::default();
        for schema in [http_schema(), process_schema(), fs_schema()] {
            let name = schema.name.0.clone();
            registry.schemas.insert(name.clone(), schema);
            registry
                .schema_provenance
                .insert(name, ProviderSchemaProvenance::BuiltIn);
        }
        registry
    }

    #[cfg(feature = "native")]
    pub fn built_in(config: NativeProviderConfig) -> Self {
        let mut registry = Self::built_in_schemas();
        registry.register(Arc::new(HttpProvider::new(config.http)));
        registry.register(Arc::new(ProcessProvider::new(config.process)));
        registry.register(Arc::new(FsProvider::new(config.fs)));
        registry
    }

    pub fn register(&mut self, provider: Arc<dyn ServerProvider>) {
        let schema = provider.schema();
        let name = schema.name.0.clone();
        let provenance = self
            .schemas
            .get(&name)
            .filter(|existing| *existing == &schema)
            .and_then(|_| self.schema_provenance.get(&name).copied())
            .unwrap_or(ProviderSchemaProvenance::ProjectSupplied);
        self.providers.insert(name.clone(), provider);
        self.schemas.insert(name.clone(), schema);
        self.schema_provenance.insert(name, provenance);
    }

    /// Registers a statically visible provider schema without claiming that the
    /// current host can execute it. This is used by portable/project adapters
    /// that can analyze provider calls while keeping runtime availability
    /// explicit.
    pub fn register_schema(&mut self, schema: ProviderSchema) {
        let name = schema.name.0.clone();
        self.schemas.insert(name.clone(), schema);
        self.schema_provenance
            .insert(name, ProviderSchemaProvenance::ProjectSupplied);
    }

    pub fn schema(&self, provider: &str) -> Option<&ProviderSchema> {
        self.schemas.get(provider)
    }

    pub fn schemas(&self) -> impl Iterator<Item = &ProviderSchema> {
        self.schemas.values()
    }

    pub fn schema_provenance(&self, provider: &str) -> Option<ProviderSchemaProvenance> {
        self.schema_provenance.get(provider).copied()
    }

    pub async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        let provider =
            self.providers
                .get(&call.provider.0)
                .ok_or_else(|| ProviderError::NotRegistered {
                    provider: call.provider.0.clone(),
                })?;
        provider.call(call, context).await
    }
}

fn parameter(name: &str, ty: Type, required: bool, positional: bool) -> ParameterSchema {
    ParameterSchema {
        name: name.into(),
        ty,
        required,
        positional,
        secret: false,
    }
}

fn secret_parameter(name: &str, ty: Type, required: bool, positional: bool) -> ParameterSchema {
    ParameterSchema {
        name: name.into(),
        ty,
        required,
        positional,
        secret: true,
    }
}

fn open_record() -> Type {
    Type::Record(BTreeMap::new())
}

pub fn http_schema() -> ProviderSchema {
    let mut operations = BTreeMap::new();
    for method in ["get", "post", "put", "patch", "delete"] {
        operations.insert(
            method.into(),
            OperationSchema {
                name: OperationName(method.into()),
                parameters: vec![
                    parameter("url", Type::String, true, true),
                    parameter("query", open_record(), false, false),
                    parameter("headers", open_record(), false, false),
                    parameter("json", Type::Json, false, false),
                    parameter("text", Type::String, false, false),
                    parameter("bytes", Type::Bytes, false, false),
                    parameter("form", open_record(), false, false),
                    secret_parameter("cookie", Type::String, false, false),
                    parameter("timeout", Type::Duration, false, false),
                ],
                result: Type::Response(Box::new(Type::Json)),
                capability: Capability::Server,
                documentation: format!("Send an HTTP {} request.", method.to_uppercase()),
            },
        );
    }
    ProviderSchema {
        name: ProviderName("http".into()),
        operations,
    }
}

pub fn process_schema() -> ProviderSchema {
    let operation = OperationSchema {
        name: OperationName("run".into()),
        parameters: vec![
            parameter("executable", Type::String, true, true),
            parameter("args", Type::List(Box::new(Type::String)), false, false),
            secret_parameter("env", open_record(), false, false),
            parameter("cwd", Type::String, false, false),
            secret_parameter("stdin", Type::String, false, false),
            parameter("timeout", Type::Duration, false, false),
        ],
        result: Type::ProcessResult,
        capability: Capability::Server,
        documentation: "Run an executable directly without a shell.".into(),
    };
    ProviderSchema {
        name: ProviderName("process".into()),
        operations: [("run".into(), operation)].into_iter().collect(),
    }
}

pub fn fs_schema() -> ProviderSchema {
    let mut operations = BTreeMap::new();
    operations.insert(
        "read_text".into(),
        OperationSchema {
            name: OperationName("read_text".into()),
            parameters: vec![parameter("path", Type::String, true, true)],
            result: Type::String,
            capability: Capability::Server,
            documentation: "Read a project-relative UTF-8 file.".into(),
        },
    );
    operations.insert(
        "write_text".into(),
        OperationSchema {
            name: OperationName("write_text".into()),
            parameters: vec![
                parameter("path", Type::String, true, true),
                parameter("contents", Type::String, true, false),
            ],
            result: Type::FilePath,
            capability: Capability::Server,
            documentation: "Write a project-relative UTF-8 file.".into(),
        },
    );
    operations.insert(
        "copy_fixture".into(),
        OperationSchema {
            name: OperationName("copy_fixture".into()),
            parameters: vec![
                parameter("from", Type::String, true, true),
                parameter("to", Type::String, true, false),
            ],
            result: Type::FilePath,
            capability: Capability::Server,
            documentation: "Copy a fixture inside the project.".into(),
        },
    );
    operations.insert(
        "temp_dir".into(),
        OperationSchema {
            name: OperationName("temp_dir".into()),
            parameters: Vec::new(),
            result: Type::TempDirectory,
            capability: Capability::Server,
            documentation: "Create a managed temporary directory.".into(),
        },
    );
    ProviderSchema {
        name: ProviderName("fs".into()),
        operations,
    }
}

#[derive(Clone, Debug, Default)]
pub struct NativeProviderConfig {
    pub http: HttpProviderConfig,
    pub process: ProcessProviderConfig,
    pub fs: FsProviderConfig,
}

#[derive(Clone, Debug)]
pub struct HttpProviderConfig {
    pub base_url: Option<String>,
    pub follow_redirects: bool,
    pub max_response_bytes: usize,
}

impl Default for HttpProviderConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            follow_redirects: true,
            max_response_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProcessProviderConfig {
    pub allowed_working_roots: Vec<PathBuf>,
    pub max_output_bytes: usize,
}

impl Default for ProcessProviderConfig {
    fn default() -> Self {
        Self {
            allowed_working_roots: vec![PathBuf::from(".")],
            max_output_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FsProviderConfig {
    pub read_roots: Vec<PathBuf>,
    pub write_root: PathBuf,
}

impl Default for FsProviderConfig {
    fn default() -> Self {
        Self {
            read_roots: vec![PathBuf::from("fixtures")],
            write_root: PathBuf::from(".webtest/tmp"),
        }
    }
}

#[cfg(feature = "native")]
struct HttpProvider {
    config: HttpProviderConfig,
    agent: ureq::Agent,
}

#[cfg(feature = "native")]
impl HttpProvider {
    fn new(config: HttpProviderConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .redirects(if config.follow_redirects { 10 } else { 0 })
            .build();
        Self { config, agent }
    }
}

#[cfg(feature = "native")]
#[async_trait]
impl ServerProvider for HttpProvider {
    fn schema(&self) -> ProviderSchema {
        http_schema()
    }

    async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        let method = call.operation.0.to_uppercase();
        let raw_url = string_argument(&call.arguments, "url")?;
        let mut url = resolve_url(self.config.base_url.as_deref(), raw_url)?;
        if let Some(query) = record_argument(&call.arguments, "query")? {
            let mut parsed =
                url::Url::parse(&url).map_err(|error| ProviderError::InvalidArgument {
                    message: format!("invalid HTTP URL: {error}"),
                })?;
            {
                let mut pairs = parsed.query_pairs_mut();
                for (name, value) in query {
                    pairs.append_pair(name, scalar_string(value)?.as_ref());
                }
            }
            url = parsed.into();
        }
        let arguments = call.arguments.clone();
        let limit = self.config.max_response_bytes;
        let agent = self.agent.clone();
        let timeout = duration_argument(&arguments, "timeout").unwrap_or(context.timeout);
        tokio::task::spawn_blocking(move || {
            let mut request = agent.request(&method, &url).timeout(timeout);
            if let Some(headers) = record_argument(&arguments, "headers")? {
                for (name, value) in headers {
                    if let Value::String(value) = value {
                        request = request.set(name, value);
                    }
                }
            }
            if let Some(Value::String(cookie)) = arguments.get("cookie") {
                request = request.set("cookie", cookie);
            }
            let response = if let Some(json) = arguments.get("json") {
                let json = value_to_json(json).ok_or_else(|| ProviderError::InvalidArgument {
                    message: "HTTP json body is not serializable".into(),
                })?;
                let body = serde_json::to_string(&json).map_err(|error| {
                    ProviderError::InvalidArgument {
                        message: error.to_string(),
                    }
                })?;
                request
                    .set("content-type", "application/json")
                    .send_string(&body)
            } else if let Some(Value::String(text)) = arguments.get("text") {
                request.send_string(text)
            } else if let Some(Value::Bytes(bytes)) = arguments.get("bytes") {
                request.send_bytes(bytes)
            } else if let Some(form) = record_argument(&arguments, "form")? {
                let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                for (name, value) in form {
                    serializer.append_pair(name, scalar_string(value)?.as_ref());
                }
                request
                    .set("content-type", "application/x-www-form-urlencoded")
                    .send_string(&serializer.finish())
            } else {
                request.call()
            };
            let response = match response {
                Ok(response) => response,
                Err(ureq::Error::Status(_, response)) => response,
                Err(error) => {
                    return Err(ProviderError::HttpTransport {
                        message: error.to_string(),
                    });
                }
            };
            let status = response.status();
            let headers = response
                .headers_names()
                .into_iter()
                .filter_map(|name| response.header(&name).map(|value| (name, value.to_owned())))
                .collect();
            let mut reader = response.into_reader().take((limit + 1) as u64);
            let mut body = Vec::new();
            reader
                .read_to_end(&mut body)
                .map_err(|error| ProviderError::HttpTransport {
                    message: error.to_string(),
                })?;
            if body.len() > limit {
                return Err(ProviderError::ResponseTooLarge { limit });
            }
            let json = serde_json::from_slice(&body)
                .ok()
                .map(value_from_json)
                .map(Box::new);
            Ok(ProviderResult {
                value: Value::Response(ResponseValue {
                    status,
                    headers,
                    body,
                    json,
                }),
            })
        })
        .await
        .map_err(|error| ProviderError::HttpTransport {
            message: error.to_string(),
        })?
    }
}

#[cfg(feature = "native")]
struct ProcessProvider {
    config: ProcessProviderConfig,
}

#[cfg(feature = "native")]
impl ProcessProvider {
    fn new(config: ProcessProviderConfig) -> Self {
        Self { config }
    }
}

#[cfg(feature = "native")]
#[async_trait]
impl ServerProvider for ProcessProvider {
    fn schema(&self) -> ProviderSchema {
        process_schema()
    }

    async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        use tokio::io::AsyncWriteExt;
        let executable = string_argument(&call.arguments, "executable")?;
        let mut command = tokio::process::Command::new(executable);
        command.kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        command.current_dir(&context.project_root);
        if let Some(Value::List(args)) = call.arguments.get("args") {
            for argument in args {
                command.arg(value_string(argument)?);
            }
        }
        if let Some(env) = record_argument(&call.arguments, "env")? {
            for (name, value) in env {
                command.env(name, value_string(value)?);
            }
        }
        if let Some(Value::String(cwd)) = call.arguments.get("cwd") {
            let project_root = canonicalize_existing(&context.project_root)?;
            let cwd = canonicalize_existing(&safe_path(&project_root, cwd)?)?;
            let allowed = self.config.allowed_working_roots.iter().any(|allowed| {
                safe_path(&project_root, allowed.to_string_lossy().as_ref())
                    .and_then(|path| canonicalize_existing(&path))
                    .is_ok_and(|allowed| cwd.starts_with(allowed))
            });
            if !allowed {
                return Err(ProviderError::PathEscape {
                    path: cwd.display().to_string(),
                });
            }
            command.current_dir(cwd);
        }
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        if call.arguments.contains_key("stdin") {
            command.stdin(std::process::Stdio::piped());
        }
        let mut child = command
            .spawn()
            .map_err(|error| ProviderError::ProcessSpawn {
                message: error.to_string(),
            })?;
        if let Some(Value::String(stdin)) = call.arguments.get("stdin")
            && let Some(mut input) = child.stdin.take()
        {
            input.write_all(stdin.as_bytes()).await.map_err(|error| {
                ProviderError::ProcessSpawn {
                    message: error.to_string(),
                }
            })?;
        }
        let timeout = duration_argument(&call.arguments, "timeout").unwrap_or(context.timeout);
        let process_id = child.id();
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(output) => output.map_err(|error| ProviderError::ProcessSpawn {
                message: error.to_string(),
            })?,
            Err(_) => {
                return Err(ProviderError::ProcessTimeout {
                    timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                    cleanup_succeeded: cleanup_process_group(process_id).await,
                });
            }
        };
        if output.stdout.len() > self.config.max_output_bytes
            || output.stderr.len() > self.config.max_output_bytes
        {
            return Err(ProviderError::ProcessOutputTooLarge {
                limit: self.config.max_output_bytes,
            });
        }
        Ok(ProviderResult {
            value: Value::ProcessResult(ProcessResultValue {
                exit_code: output.status.code().map(i64::from).unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                stdout_bytes: output.stdout,
                stderr_bytes: output.stderr,
            }),
        })
    }
}

#[cfg(feature = "native")]
struct FsProvider {
    config: FsProviderConfig,
}

#[cfg(feature = "native")]
impl FsProvider {
    fn new(config: FsProviderConfig) -> Self {
        Self { config }
    }
}

#[cfg(feature = "native")]
#[async_trait]
impl ServerProvider for FsProvider {
    fn schema(&self) -> ProviderSchema {
        fs_schema()
    }

    async fn call(
        &self,
        call: ProviderCall,
        context: CallContext,
    ) -> Result<ProviderResult, ProviderError> {
        let root = canonicalize_existing(&context.project_root)?;
        match call.operation.0.as_str() {
            "read_text" => {
                let relative = string_argument(&call.arguments, "path")?;
                let path = canonicalize_existing(&safe_path(&root, relative)?)?;
                let allowed = self.config.read_roots.iter().any(|allowed| {
                    safe_path(&root, allowed.to_string_lossy().as_ref())
                        .and_then(|path| canonicalize_existing(&path))
                        .is_ok_and(|allowed| path.starts_with(allowed))
                });
                if !allowed {
                    return Err(ProviderError::PathEscape {
                        path: relative.into(),
                    });
                }
                let text = tokio::fs::read_to_string(&path).await.map_err(|error| {
                    ProviderError::Filesystem {
                        path: path.display().to_string(),
                        message: error.to_string(),
                    }
                })?;
                Ok(ProviderResult {
                    value: Value::String(text),
                })
            }
            "write_text" => {
                let relative = string_argument(&call.arguments, "path")?;
                let requested = safe_path(&root, relative)?;
                let configured_write_root =
                    safe_path(&root, self.config.write_root.to_string_lossy().as_ref())?;
                tokio::fs::create_dir_all(&configured_write_root)
                    .await
                    .map_err(|error| filesystem_error(&configured_write_root, error))?;
                let write_root = canonicalize_existing(&configured_write_root)?;
                if !canonical_target_is_within(&requested, &write_root)? {
                    return Err(ProviderError::PathEscape {
                        path: relative.into(),
                    });
                }
                let contents = string_argument(&call.arguments, "contents")?;
                if let Some(parent) = requested.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        ProviderError::Filesystem {
                            path: parent.display().to_string(),
                            message: error.to_string(),
                        }
                    })?;
                }
                tokio::fs::write(&requested, contents)
                    .await
                    .map_err(|error| ProviderError::Filesystem {
                        path: requested.display().to_string(),
                        message: error.to_string(),
                    })?;
                Ok(ProviderResult {
                    value: Value::FilePath(requested),
                })
            }
            "copy_fixture" => {
                let from = canonicalize_existing(&safe_path(
                    &root,
                    string_argument(&call.arguments, "from")?,
                )?)?;
                let to = safe_path(&root, string_argument(&call.arguments, "to")?)?;
                let readable = self.config.read_roots.iter().any(|allowed| {
                    safe_path(&root, allowed.to_string_lossy().as_ref())
                        .and_then(|path| canonicalize_existing(&path))
                        .is_ok_and(|allowed| from.starts_with(allowed))
                });
                let configured_write_root =
                    safe_path(&root, self.config.write_root.to_string_lossy().as_ref())?;
                tokio::fs::create_dir_all(&configured_write_root)
                    .await
                    .map_err(|error| filesystem_error(&configured_write_root, error))?;
                let write_root = canonicalize_existing(&configured_write_root)?;
                if !readable || !canonical_target_is_within(&to, &write_root)? {
                    return Err(ProviderError::PathEscape {
                        path: format!("{} -> {}", from.display(), to.display()),
                    });
                }
                if let Some(parent) = to.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        ProviderError::Filesystem {
                            path: parent.display().to_string(),
                            message: error.to_string(),
                        }
                    })?;
                }
                tokio::fs::copy(&from, &to)
                    .await
                    .map_err(|error| ProviderError::Filesystem {
                        path: from.display().to_string(),
                        message: error.to_string(),
                    })?;
                Ok(ProviderResult {
                    value: Value::FilePath(to),
                })
            }
            "temp_dir" => {
                let write_root =
                    safe_path(&root, self.config.write_root.to_string_lossy().as_ref())?;
                tokio::fs::create_dir_all(&write_root)
                    .await
                    .map_err(|error| ProviderError::Filesystem {
                        path: write_root.display().to_string(),
                        message: error.to_string(),
                    })?;
                let directory = tempfile::Builder::new()
                    .prefix("webtest-")
                    .tempdir_in(&write_root)
                    .map_err(|error| ProviderError::Filesystem {
                        path: write_root.display().to_string(),
                        message: error.to_string(),
                    })?
                    .keep();
                Ok(ProviderResult {
                    value: Value::TempDirectory(directory),
                })
            }
            operation => Err(ProviderError::UnknownOperation {
                provider: "fs".into(),
                operation: operation.into(),
            }),
        }
    }
}

#[cfg(feature = "native")]
fn string_argument<'a>(
    arguments: &'a BTreeMap<String, Value>,
    name: &str,
) -> Result<&'a str, ProviderError> {
    arguments
        .get(name)
        .and_then(|value| match value {
            Value::String(value) => Some(value.as_str()),
            _ => None,
        })
        .ok_or_else(|| ProviderError::InvalidArgument {
            message: format!("`{name}` must be a string"),
        })
}

#[cfg(feature = "native")]
fn record_argument<'a>(
    arguments: &'a BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<&'a BTreeMap<String, Value>>, ProviderError> {
    match arguments.get(name) {
        None => Ok(None),
        Some(Value::Record(value)) => Ok(Some(value)),
        Some(_) => Err(ProviderError::InvalidArgument {
            message: format!("`{name}` must be a record"),
        }),
    }
}

#[cfg(feature = "native")]
fn value_string(value: &Value) -> Result<&str, ProviderError> {
    if let Value::String(value) = value {
        Ok(value)
    } else {
        Err(ProviderError::InvalidArgument {
            message: "expected a string value".into(),
        })
    }
}

#[cfg(feature = "native")]
fn scalar_string(value: &Value) -> Result<std::borrow::Cow<'_, str>, ProviderError> {
    Ok(match value {
        Value::String(value) => std::borrow::Cow::Borrowed(value),
        Value::Bool(value) => std::borrow::Cow::Owned(value.to_string()),
        Value::Int(value) => std::borrow::Cow::Owned(value.to_string()),
        Value::Float(value) => std::borrow::Cow::Owned(value.to_string()),
        Value::Null => std::borrow::Cow::Borrowed(""),
        _ => {
            return Err(ProviderError::InvalidArgument {
                message: "query and form values must be scalar".into(),
            });
        }
    })
}

#[cfg(feature = "native")]
fn duration_argument(arguments: &BTreeMap<String, Value>, name: &str) -> Option<Duration> {
    match arguments.get(name) {
        Some(Value::DurationMillis(value)) => Some(Duration::from_millis(*value)),
        _ => None,
    }
}

#[cfg(feature = "native")]
fn resolve_url(base: Option<&str>, value: &str) -> Result<String, ProviderError> {
    if value.contains("://") {
        return Ok(value.into());
    }
    let base = base.ok_or_else(|| ProviderError::InvalidArgument {
        message: "relative HTTP URL requires server.base_url".into(),
    })?;
    Ok(format!(
        "{}/{}",
        base.trim_end_matches('/'),
        value.trim_start_matches('/')
    ))
}

#[cfg(feature = "native")]
fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, ProviderError> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(ProviderError::PathEscape {
            path: relative.into(),
        });
    }
    Ok(root.join(path))
}

#[cfg(feature = "native")]
fn canonicalize_existing(path: &Path) -> Result<PathBuf, ProviderError> {
    std::fs::canonicalize(path).map_err(|error| filesystem_error(path, error))
}

#[cfg(feature = "native")]
fn canonical_target_is_within(path: &Path, allowed_root: &Path) -> Result<bool, ProviderError> {
    let mut ancestor = path;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| ProviderError::PathEscape {
            path: path.display().to_string(),
        })?;
    }
    Ok(canonicalize_existing(ancestor)?.starts_with(allowed_root))
}

#[cfg(feature = "native")]
fn filesystem_error(path: &Path, error: std::io::Error) -> ProviderError {
    ProviderError::Filesystem {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

#[cfg(all(feature = "native", unix))]
async fn cleanup_process_group(process_id: Option<u32>) -> bool {
    let Some(process_id) = process_id.and_then(|id| i32::try_from(id).ok()) else {
        return false;
    };
    // The provider created this process group, so a negative PID targets only its descendants.
    let killed = unsafe { libc::kill(-process_id, libc::SIGKILL) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    if !killed {
        return false;
    }
    for _ in 0..50 {
        if unsafe { libc::kill(-process_id, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[cfg(all(feature = "native", not(unix)))]
async fn cleanup_process_group(_process_id: Option<u32>) -> bool {
    false
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    fn context(root: &Path) -> CallContext {
        CallContext {
            project_root: root.to_path_buf(),
            timeout: Duration::from_secs(2),
            redacted_json_fields: vec!["password".into(), "authorization".into()],
        }
    }

    fn call(provider: &str, operation: &str, arguments: BTreeMap<String, Value>) -> ProviderCall {
        ProviderCall {
            provider: ProviderName(provider.into()),
            operation: OperationName(operation.into()),
            arguments,
        }
    }

    #[test]
    fn schemas_are_stable_and_types_compose_transferability() {
        assert_eq!(
            http_schema().operation("post").unwrap().result,
            Type::Response(Box::new(Type::Json))
        );
        assert_eq!(http_schema().hash(), http_schema().hash());
        let fields = [(
            "page".into(),
            RecordField {
                ty: Type::BrowserPage,
                optional: false,
            },
        )]
        .into_iter()
        .collect();
        assert!(!Type::Record(fields).is_transferable());

        let mut registry = ProviderRegistry::built_in_schemas();
        assert_eq!(
            registry.schema_provenance("http"),
            Some(ProviderSchemaProvenance::BuiltIn)
        );
        let mut project_schema = fs_schema();
        project_schema.name = ProviderName("project_fs".into());
        registry.register_schema(project_schema);
        assert_eq!(
            registry.schema_provenance("project_fs"),
            Some(ProviderSchemaProvenance::ProjectSupplied)
        );
    }

    #[test]
    fn parent_paths_are_rejected() {
        assert!(matches!(
            safe_path(Path::new("/project"), "../secret"),
            Err(ProviderError::PathEscape { .. })
        ));
    }

    #[test]
    fn nested_fields_and_known_secret_values_are_redacted() {
        let value = Value::Record(
            [
                ("password".into(), Value::String("credential".into())),
                (
                    "message".into(),
                    Value::String("failed for credential".into()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let redacted = value.redacted_with_secrets(&["password".into()], &["credential".into()]);
        assert_eq!(
            redacted,
            Value::Record(
                [
                    ("password".into(), Value::String("[redacted]".into())),
                    (
                        "message".into(),
                        Value::String("failed for [redacted]".into()),
                    ),
                ]
                .into_iter()
                .collect(),
            )
        );
    }

    #[test]
    fn headers_are_case_insensitive_optional_members() {
        let headers = Value::Headers(
            [("Content-Type".into(), "application/json".into())]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            headers.member("content-type"),
            Some(Value::String("application/json".into()))
        );
        assert_eq!(headers.member("x-missing"), Some(Value::Null));
    }

    #[tokio::test]
    async fn http_provider_keeps_cookies_between_calls() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let fixture = std::thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("fixture request");
                let mut bytes = [0; 4096];
                let length = stream.read(&mut bytes).expect("read request");
                let request = String::from_utf8_lossy(&bytes[..length]).to_ascii_lowercase();
                if index == 1 {
                    assert!(request.contains("cookie: sid=session-token"), "{request}");
                }
                let cookie = if index == 0 {
                    "Set-Cookie: sid=session-token; Path=/\r\n"
                } else {
                    ""
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\n{cookie}Content-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                )
                .expect("write response");
            }
        });
        let provider = HttpProvider::new(HttpProviderConfig::default());
        let arguments: BTreeMap<String, Value> = [(
            "url".into(),
            Value::String(format!("http://{address}/session")),
        )]
        .into_iter()
        .collect();
        let root = tempfile::tempdir().expect("project root");
        provider
            .call(call("http", "get", arguments.clone()), context(root.path()))
            .await
            .expect("first request");
        provider
            .call(call("http", "get", arguments), context(root.path()))
            .await
            .expect("second request");
        fixture.join().expect("fixture thread");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_provider_rejects_symlink_escape_for_reads_and_writes() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().expect("project");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::create_dir_all(project.path().join("fixtures")).expect("fixtures");
        std::fs::create_dir_all(project.path().join(".webtest/tmp")).expect("write root");
        std::fs::write(outside.path().join("secret.txt"), "private").expect("outside fixture");
        symlink(outside.path(), project.path().join("fixtures/outside")).expect("read symlink");
        symlink(outside.path(), project.path().join(".webtest/tmp/outside"))
            .expect("write symlink");
        let provider = FsProvider::new(FsProviderConfig::default());

        let read = provider
            .call(
                call(
                    "fs",
                    "read_text",
                    [(
                        "path".into(),
                        Value::String("fixtures/outside/secret.txt".into()),
                    )]
                    .into_iter()
                    .collect(),
                ),
                context(project.path()),
            )
            .await;
        assert!(matches!(read, Err(ProviderError::PathEscape { .. })));

        let write = provider
            .call(
                call(
                    "fs",
                    "write_text",
                    [
                        (
                            "path".into(),
                            Value::String(".webtest/tmp/outside/leaked.txt".into()),
                        ),
                        ("contents".into(), Value::String("leak".into())),
                    ]
                    .into_iter()
                    .collect(),
                ),
                context(project.path()),
            )
            .await;
        assert!(matches!(write, Err(ProviderError::PathEscape { .. })));
        assert!(!outside.path().join("leaked.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_provider_runs_arguments_directly_and_enforces_timeout() {
        let project = tempfile::tempdir().expect("project");
        let provider = ProcessProvider::new(ProcessProviderConfig::default());
        let result = provider
            .call(
                call(
                    "process",
                    "run",
                    [
                        ("executable".into(), Value::String("/bin/echo".into())),
                        (
                            "args".into(),
                            Value::List(vec![Value::String("$HOME".into())]),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
                context(project.path()),
            )
            .await
            .expect("direct process call");
        let Value::ProcessResult(result) = result.value else {
            panic!("process result")
        };
        assert_eq!(result.stdout.trim(), "$HOME");

        let timed_out = provider
            .call(
                call(
                    "process",
                    "run",
                    [
                        ("executable".into(), Value::String("/bin/sleep".into())),
                        ("args".into(), Value::List(vec![Value::String("1".into())])),
                        ("timeout".into(), Value::DurationMillis(10)),
                    ]
                    .into_iter()
                    .collect(),
                ),
                context(project.path()),
            )
            .await;
        let Err(ProviderError::ProcessTimeout {
            cleanup_succeeded, ..
        }) = timed_out
        else {
            panic!("process timeout")
        };
        assert!(cleanup_succeeded);
    }
}
