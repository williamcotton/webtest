//! Protocol-neutral semantic identities, operators, types, values, and capabilities.

use std::{collections::BTreeMap, fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TestId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StepId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BindingId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOperator {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOperator {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    And,
    Or,
    Contains,
    Matches,
}

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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub documentation: String,
    #[serde(default)]
    pub secret: bool,
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
            (Self::Option(expected), Self::Option(actual)) => expected.accepts(actual),
            (Self::Option(expected), actual) => expected.accepts(actual),
            (Self::List(expected), Self::List(actual))
            | (Self::Response(expected), Self::Response(actual)) => expected.accepts(actual),
            (Self::Record(expected), Self::Record(actual)) => {
                expected.iter().all(|(name, field)| {
                    let Some(actual) = actual.get(name) else {
                        return field.optional;
                    };
                    field.ty.accepts(&actual.ty) && (field.optional || !actual.optional)
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

    pub fn member_missing_is_null(&self, name: &str) -> bool {
        match self {
            Self::Record(fields) => fields.get(name).is_some_and(|field| field.optional),
            Self::Headers => true,
            _ => false,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn field(ty: Type, optional: bool) -> RecordField {
        RecordField {
            ty,
            optional,
            documentation: String::new(),
            secret: false,
        }
    }

    #[test]
    fn identities_and_operators_preserve_serde_and_order() {
        assert_eq!(serde_json::to_value(TestId(7)).unwrap(), 7);
        assert_eq!(serde_json::to_value(StepId(8)).unwrap(), 8);
        assert_eq!(serde_json::to_value(BindingId(9)).unwrap(), 9);
        assert!(StepId(1) < StepId(2));
        assert!(BindingId(3) < BindingId(4));
        for operator in [UnaryOperator::Not, UnaryOperator::Negate] {
            let encoded = serde_json::to_value(operator).unwrap();
            assert_eq!(
                serde_json::from_value::<UnaryOperator>(encoded).unwrap(),
                operator
            );
        }
        for operator in [
            BinaryOperator::Equal,
            BinaryOperator::NotEqual,
            BinaryOperator::Less,
            BinaryOperator::LessEqual,
            BinaryOperator::Greater,
            BinaryOperator::GreaterEqual,
            BinaryOperator::Add,
            BinaryOperator::Subtract,
            BinaryOperator::Multiply,
            BinaryOperator::Divide,
            BinaryOperator::And,
            BinaryOperator::Or,
            BinaryOperator::Contains,
            BinaryOperator::Matches,
        ] {
            let encoded = serde_json::to_value(operator).unwrap();
            assert_eq!(
                serde_json::from_value::<BinaryOperator>(encoded).unwrap(),
                operator
            );
        }
        for (capability, spelling) in [
            (Capability::Pure, "pure"),
            (Capability::Server, "server"),
            (Capability::Browser, "browser"),
            (Capability::Test, "test"),
        ] {
            assert_eq!(serde_json::to_value(capability).unwrap(), spelling);
        }
    }

    #[test]
    fn every_type_variant_has_stable_serde_and_display() {
        let cases = [
            (Type::Unknown, "unknown", "unknown"),
            (Type::Null, "null", "Null"),
            (Type::Bool, "bool", "Bool"),
            (Type::Int, "int", "Int"),
            (Type::Float, "float", "Float"),
            (Type::String, "string", "String"),
            (Type::Duration, "duration", "Duration"),
            (Type::Url, "url", "Url"),
            (Type::Json, "json", "Json"),
            (Type::List(Box::new(Type::String)), "list", "List<String>"),
            (Type::Option(Box::new(Type::Int)), "option", "Option<Int>"),
            (
                Type::Record(BTreeMap::from([("id".into(), field(Type::Int, false))])),
                "record",
                "{ id: Int }",
            ),
            (Type::StatusCode, "status_code", "StatusCode"),
            (Type::Headers, "headers", "Headers"),
            (Type::Bytes, "bytes", "Bytes"),
            (
                Type::Response(Box::new(Type::Json)),
                "response",
                "Response<Json>",
            ),
            (Type::ProcessResult, "process_result", "ProcessResult"),
            (Type::FilePath, "file_path", "FilePath"),
            (Type::TempDirectory, "temp_directory", "TempDirectory"),
            (Type::Locator, "locator", "Locator"),
            (Type::BrowserPage, "browser_page", "BrowserPage"),
        ];
        for (ty, kind, display) in cases {
            let encoded = serde_json::to_value(&ty).unwrap();
            assert_eq!(encoded["kind"], kind);
            assert_eq!(serde_json::from_value::<Type>(encoded).unwrap(), ty);
            assert_eq!(ty.to_string(), display);
        }
    }

    #[test]
    fn type_semantics_preserve_presence_members_and_transferability() {
        let optional = Type::Option(Box::new(Type::String));
        assert!(optional.accepts(&Type::Null));
        assert!(optional.accepts(&Type::String));
        assert!(optional.accepts(&Type::Option(Box::new(Type::String))));
        assert!(!optional.accepts(&Type::Int));
        assert!(!Type::String.accepts(&optional));
        assert!(Type::Float.accepts(&Type::Int));
        assert!(Type::StatusCode.accepts(&Type::Int));
        assert!(Type::Json.accepts(&Type::List(Box::new(Type::String))));
        let expected = Type::Record(BTreeMap::from([
            ("name".into(), field(Type::String, true)),
            ("id".into(), field(Type::Int, false)),
        ]));
        let actual = Type::Record(BTreeMap::from([("id".into(), field(Type::Int, false))]));
        assert!(expected.accepts(&actual));
        let record = |optional: bool, ty: Type| {
            Type::Record(BTreeMap::from([("name".into(), field(ty, optional))]))
        };
        let empty = Type::Record(BTreeMap::new());
        assert!(!record(false, Type::String).accepts(&record(true, Type::String)));
        assert!(record(true, Type::String).accepts(&record(false, Type::String)));
        assert!(record(true, Type::String).accepts(&empty));
        assert!(!record(false, Type::String).accepts(&empty));
        assert!(!record(true, Type::String).accepts(&record(false, Type::Int)));
        assert_eq!(
            expected.member("name"),
            Some(Type::Option(Box::new(Type::String)))
        );
        assert!(expected.member_missing_is_null("name"));
        assert!(
            !record(false, Type::Option(Box::new(Type::String))).member_missing_is_null("name")
        );
        assert!(expected.is_transferable());
        assert!(!Type::Response(Box::new(Type::Json)).is_transferable());
        assert_eq!(
            Type::Response(Box::new(Type::Json)).member("status"),
            Some(Type::StatusCode)
        );
        assert_eq!(Type::ProcessResult.member("stdout"), Some(Type::String));
        assert_eq!(
            Type::Headers.member("content-type"),
            Some(Type::Option(Box::new(Type::String)))
        );
    }

    #[test]
    fn every_value_variant_round_trips_and_members_are_structured() {
        let values = [
            (Value::Null, "null"),
            (Value::Bool(true), "bool"),
            (Value::Int(2), "int"),
            (Value::Float(2.5), "float"),
            (Value::String("value".into()), "string"),
            (Value::DurationMillis(50), "duration_millis"),
            (Value::List(vec![Value::Int(1)]), "list"),
            (
                Value::Record(BTreeMap::from([("field".into(), Value::Bool(true))])),
                "record",
            ),
            (
                Value::Headers(BTreeMap::from([(
                    "Content-Type".into(),
                    "text/plain".into(),
                )])),
                "headers",
            ),
            (Value::Bytes(vec![1, 2]), "bytes"),
            (
                Value::Response(ResponseValue {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: b"ok".to_vec(),
                    json: Some(Box::new(Value::Bool(true))),
                }),
                "response",
            ),
            (
                Value::ProcessResult(ProcessResultValue {
                    exit_code: 0,
                    stdout: "out".into(),
                    stderr: "err".into(),
                    stdout_bytes: b"out".to_vec(),
                    stderr_bytes: b"err".to_vec(),
                }),
                "process_result",
            ),
            (Value::FilePath(PathBuf::from("fixture.txt")), "file_path"),
            (Value::TempDirectory(PathBuf::from("tmp")), "temp_directory"),
        ];
        for (value, kind) in values {
            let encoded = serde_json::to_value(&value).unwrap();
            assert_eq!(encoded["kind"], kind);
            assert_eq!(serde_json::from_value::<Value>(encoded).unwrap(), value);
            assert!(!value.type_name().is_empty());
        }
        for (value, name) in [
            (Value::Null, "null"),
            (Value::Bool(false), "boolean"),
            (Value::Int(0), "integer"),
            (Value::Float(0.0), "number"),
            (Value::String(String::new()), "string"),
            (Value::DurationMillis(0), "duration"),
            (Value::List(Vec::new()), "array"),
            (Value::Record(BTreeMap::new()), "object"),
            (Value::Headers(BTreeMap::new()), "headers"),
            (Value::Bytes(Vec::new()), "bytes"),
            (
                Value::Response(ResponseValue {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: Vec::new(),
                    json: None,
                }),
                "response",
            ),
            (
                Value::ProcessResult(ProcessResultValue {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    stdout_bytes: Vec::new(),
                    stderr_bytes: Vec::new(),
                }),
                "process result",
            ),
            (Value::FilePath(PathBuf::new()), "file path"),
            (Value::TempDirectory(PathBuf::new()), "temporary directory"),
        ] {
            assert_eq!(value.type_name(), name);
        }
        let headers = Value::Headers(BTreeMap::from([(
            "Content-Type".into(),
            "text/plain".into(),
        )]));
        assert_eq!(
            headers.member("content-type"),
            Some(Value::String("text/plain".into()))
        );
        assert_eq!(headers.member("missing"), Some(Value::Null));
        let record = Value::Record(BTreeMap::from([("id".into(), Value::Int(7))]));
        assert_eq!(record.member("id"), Some(Value::Int(7)));
        let response = Value::Response(ResponseValue {
            status: 201,
            headers: BTreeMap::new(),
            body: b"created".to_vec(),
            json: Some(Box::new(record.clone())),
        });
        assert_eq!(response.member("status"), Some(Value::Int(201)));
        assert_eq!(response.member("json"), Some(record));
        let process = Value::ProcessResult(ProcessResultValue {
            exit_code: 4,
            stdout: "out".into(),
            stderr: "err".into(),
            stdout_bytes: b"out".to_vec(),
            stderr_bytes: b"err".to_vec(),
        });
        assert_eq!(process.member("exit_code"), Some(Value::Int(4)));
    }

    #[test]
    fn json_conversion_is_deterministic_and_rejects_host_values() {
        let source = serde_json::json!({"z": [1, true], "a": null});
        let value = value_from_json(source.clone());
        assert_eq!(value_to_json(&value), Some(source));
        assert!(
            matches!(value, Value::Record(fields) if fields.keys().collect::<Vec<_>>() == ["a", "z"])
        );
        for host_value in [
            Value::Headers(BTreeMap::new()),
            Value::Bytes(vec![1]),
            Value::Response(ResponseValue {
                status: 200,
                headers: BTreeMap::new(),
                body: Vec::new(),
                json: None,
            }),
            Value::ProcessResult(ProcessResultValue {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                stdout_bytes: Vec::new(),
                stderr_bytes: Vec::new(),
            }),
            Value::FilePath(PathBuf::from("file")),
            Value::TempDirectory(PathBuf::from("tmp")),
        ] {
            assert_eq!(value_to_json(&host_value), None);
        }
    }

    #[test]
    fn redaction_recurses_through_all_sensitive_value_shapes() {
        let value = Value::Record(BTreeMap::from([
            ("password".into(), Value::String("secret".into())),
            (
                "nested".into(),
                Value::List(vec![Value::String("has secret".into())]),
            ),
            (
                "headers".into(),
                Value::Headers(BTreeMap::from([
                    ("Authorization".into(), "secret".into()),
                    ("Visible".into(), "ok".into()),
                ])),
            ),
            (
                "response".into(),
                Value::Response(ResponseValue {
                    status: 200,
                    headers: BTreeMap::from([("Authorization".into(), "secret".into())]),
                    body: b"secret body".to_vec(),
                    json: Some(Box::new(Value::String("secret json".into()))),
                }),
            ),
            (
                "process".into(),
                Value::ProcessResult(ProcessResultValue {
                    exit_code: 0,
                    stdout: "secret out".into(),
                    stderr: "secret err".into(),
                    stdout_bytes: b"secret bytes".to_vec(),
                    stderr_bytes: b"untouched binary\xff".to_vec(),
                }),
            ),
            ("unchanged".into(), Value::Int(7)),
        ]));
        let redacted = value.redacted_with_secrets(
            &["password".into(), "authorization".into()],
            &["secret".into()],
        );
        let Value::Record(fields) = redacted else {
            panic!("record")
        };
        assert_eq!(fields["password"], Value::String("[redacted]".into()));
        assert_eq!(fields["unchanged"], Value::Int(7));
        assert!(format!("{:?}", fields["nested"]).contains("[redacted]"));
        assert!(format!("{:?}", fields["response"]).contains("[redacted]"));
        assert!(format!("{:?}", fields["process"]).contains("[redacted]"));
    }
}
