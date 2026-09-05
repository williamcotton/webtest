use std::{collections::BTreeMap, fmt, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use webtest_model::{Capability, RecordField, Type, Value, value_from_json, value_to_json};
use webtest_provider::{
    OperationName, OperationSchema, ParameterSchema, ProviderName, ProviderSchema,
};

pub const MANIFEST_VERSION: u32 = 1;
const MAX_DOCUMENTATION_BYTES: usize = 1_024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypeSchema {
    Null,
    Boolean,
    Integer,
    Float,
    String,
    Array {
        items: Box<TypeSchema>,
    },
    Optional {
        item: Box<TypeSchema>,
    },
    Object {
        fields: BTreeMap<String, FieldSchema>,
    },
    Alias {
        name: String,
        base: Box<TypeSchema>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        validation: BTreeMap<String, serde_json::Value>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        display: BTreeMap<String, serde_json::Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldSchema {
    #[serde(flatten)]
    pub ty: TypeSchema,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub documentation: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionSchema {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub documentation: String,
    #[serde(default)]
    pub retry_safe: bool,
    pub params: TypeSchema,
    pub returns: TypeSchema,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppManifest {
    pub manifest_version: u32,
    pub protocol: u32,
    pub provider: String,
    pub sdk: String,
    pub sdk_version: String,
    pub schema_hash: String,
    pub functions: BTreeMap<String, FunctionSchema>,
}

#[derive(Clone, Copy, Debug)]
pub struct SchemaLimits {
    pub max_depth: usize,
    pub max_string_bytes: usize,
    pub max_collection_items: usize,
}

impl Default for SchemaLimits {
    fn default() -> Self {
        Self {
            max_depth: 32,
            max_string_bytes: 1_048_576,
            max_collection_items: 10_000,
        }
    }
}

#[derive(Debug, Error)]
pub enum AppSchemaError {
    #[error("could not read application schema `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("application schema is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid application schema: {0}")]
    Invalid(String),
    #[error(
        "application schema hash mismatch: manifest has {declared}, canonical schema is {computed}"
    )]
    HashMismatch { declared: String, computed: String },
}

impl AppManifest {
    pub fn read(path: &Path) -> Result<Self, AppSchemaError> {
        let source = read_manifest_source(path)?;
        Self::from_json(&source)
    }

    pub fn from_json(source: &str) -> Result<Self, AppSchemaError> {
        let manifest: Self = serde_json::from_str(source)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Reads an author-edited offline manifest and derives its schema identity
    /// from the canonical `functions` value.
    ///
    /// Protocol messages and generated manifests still use [`Self::read`] and
    /// [`Self::validate`] when the declared hash itself is part of the contract.
    /// Authoring tools use this path so changing a function does not require a
    /// second, manual hash edit before analysis can observe the new schema.
    pub fn read_normalized(path: &Path) -> Result<Self, AppSchemaError> {
        let source = read_manifest_source(path)?;
        Self::from_json_normalized(&source)
    }

    pub fn from_json_normalized(source: &str) -> Result<Self, AppSchemaError> {
        let manifest: Self = serde_json::from_str(source)?;
        manifest.validate_structure()?;
        manifest.with_computed_hash()
    }

    pub fn validate(&self) -> Result<(), AppSchemaError> {
        self.validate_structure()?;
        let computed = self.computed_hash()?;
        if self.schema_hash != computed {
            return Err(AppSchemaError::HashMismatch {
                declared: self.schema_hash.clone(),
                computed,
            });
        }
        Ok(())
    }

    fn validate_structure(&self) -> Result<(), AppSchemaError> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(AppSchemaError::Invalid(format!(
                "manifest_version must be {MANIFEST_VERSION}, got {}",
                self.manifest_version
            )));
        }
        if self.protocol != 1 {
            return Err(AppSchemaError::Invalid(format!(
                "protocol must be {}, got {}",
                1, self.protocol
            )));
        }
        if self.provider != "app" {
            return Err(AppSchemaError::Invalid(
                "provider must be exactly `app`".into(),
            ));
        }
        if self.sdk.trim().is_empty()
            || self.sdk_version.trim().is_empty()
            || self.sdk.len() > 128
            || self.sdk_version.len() > 64
        {
            return Err(AppSchemaError::Invalid(
                "sdk and sdk_version must be non-empty and bounded".into(),
            ));
        }
        if self.functions.len() > SchemaLimits::default().max_collection_items {
            return Err(AppSchemaError::Invalid(
                "application schema has too many functions".into(),
            ));
        }
        for (name, function) in &self.functions {
            validate_name(name, "function")?;
            validate_documentation(&function.documentation, &format!("function `{name}`"))?;
            let TypeSchema::Object { fields } = &function.params else {
                return Err(AppSchemaError::Invalid(format!(
                    "function `{name}` params must be an object"
                )));
            };
            for (field_name, field) in fields {
                validate_name(field_name, "parameter")?;
                validate_documentation(
                    &field.documentation,
                    &format!("parameter `{name}.{field_name}`"),
                )?;
                if field.default.is_some() && !field.optional {
                    return Err(AppSchemaError::Invalid(format!(
                        "parameter `{name}.{field_name}` has a default but is not optional"
                    )));
                }
                if let Some(default) = &field.default {
                    field.ty.validate_json(
                        default,
                        &format!("$.functions.{name}.params.{field_name}.default"),
                        SchemaLimits::default(),
                    )?;
                }
                field
                    .ty
                    .reject_defaults(&format!("$.functions.{name}.params.fields.{field_name}"))?;
            }
            function.params.validate_shape(0)?;
            function.returns.validate_shape(0)?;
            function
                .returns
                .reject_defaults(&format!("$.functions.{name}.returns"))?;
        }
        Ok(())
    }

    pub fn computed_hash(&self) -> Result<String, AppSchemaError> {
        canonical_schema_hash(&self.functions)
    }

    pub fn provider_schema(&self) -> ProviderSchema {
        let operations = self
            .functions
            .iter()
            .map(|(name, function)| {
                let fields = match &function.params {
                    TypeSchema::Object { fields } => fields,
                    _ => unreachable!("validated manifest params are objects"),
                };
                let parameters = fields
                    .iter()
                    .map(|(parameter_name, field)| ParameterSchema {
                        name: parameter_name.clone(),
                        ty: field.ty.provider_type(),
                        required: !field.optional,
                        positional: false,
                        secret: field.secret,
                        documentation: field.documentation.clone(),
                        default: field
                            .default
                            .clone()
                            .map(|value| field.ty.value_from_json(value)),
                    })
                    .collect();
                (
                    name.clone(),
                    OperationSchema {
                        name: OperationName(name.clone()),
                        parameters,
                        result: function.returns.provider_type(),
                        capability: Capability::Server,
                        documentation: function.documentation.clone(),
                        retry_safe: function.retry_safe,
                    },
                )
            })
            .collect();
        ProviderSchema {
            name: ProviderName("app".into()),
            operations,
            schema_identity: Some(self.schema_hash.clone()),
        }
    }

    pub fn semantic_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.functions).unwrap_or_else(|_| serde_json::json!({}))
    }

    pub fn with_computed_hash(mut self) -> Result<Self, AppSchemaError> {
        self.schema_hash = self.computed_hash()?;
        Ok(self)
    }
}

fn read_manifest_source(path: &Path) -> Result<String, AppSchemaError> {
    std::fs::read_to_string(path).map_err(|source| AppSchemaError::Read {
        path: path.display().to_string(),
        source,
    })
}

impl TypeSchema {
    pub fn provider_type(&self) -> Type {
        match self {
            Self::Null => Type::Null,
            Self::Boolean => Type::Bool,
            Self::Integer => Type::Int,
            Self::Float => Type::Float,
            Self::String => Type::String,
            Self::Array { items } => Type::List(Box::new(items.provider_type())),
            Self::Optional { item } => Type::Option(Box::new(item.provider_type())),
            Self::Object { fields } => Type::Record(
                fields
                    .iter()
                    .map(|(name, field)| {
                        (
                            name.clone(),
                            RecordField {
                                ty: field.ty.provider_type(),
                                optional: field.optional,
                                documentation: field.documentation.clone(),
                                secret: field.secret,
                            },
                        )
                    })
                    .collect(),
            ),
            Self::Alias { base, .. } => base.provider_type(),
        }
    }

    pub fn validate_value(
        &self,
        value: &Value,
        path: &str,
        limits: SchemaLimits,
    ) -> Result<(), AppSchemaError> {
        let json = value_to_json(value).ok_or_else(|| {
            AppSchemaError::Invalid(format!("{path} contains a non-transferable value"))
        })?;
        self.validate_json(&json, path, limits)
    }

    pub fn value_from_json(&self, value: serde_json::Value) -> Value {
        match (self, value) {
            (Self::Null, _) => Value::Null,
            (Self::Boolean, serde_json::Value::Bool(value)) => Value::Bool(value),
            (Self::Integer, serde_json::Value::Number(value)) => {
                value.as_i64().map_or(Value::Null, Value::Int)
            }
            (Self::Float, serde_json::Value::Number(value)) => {
                value.as_f64().map_or(Value::Null, Value::Float)
            }
            (Self::String, serde_json::Value::String(value)) => Value::String(value),
            (Self::Array { items }, serde_json::Value::Array(values)) => Value::List(
                values
                    .into_iter()
                    .map(|value| items.value_from_json(value))
                    .collect(),
            ),
            (Self::Optional { .. }, serde_json::Value::Null) => Value::Null,
            (Self::Optional { item }, value) => item.value_from_json(value),
            (Self::Object { fields }, serde_json::Value::Object(values)) => Value::Record(
                values
                    .into_iter()
                    .filter_map(|(name, value)| {
                        fields
                            .get(&name)
                            .map(|field| (name, field.ty.value_from_json(value)))
                    })
                    .collect(),
            ),
            (Self::Alias { base, .. }, value) => base.value_from_json(value),
            (_, value) => value_from_json(value),
        }
    }

    pub fn validate_json(
        &self,
        value: &serde_json::Value,
        path: &str,
        limits: SchemaLimits,
    ) -> Result<(), AppSchemaError> {
        self.validate_json_inner(value, path, limits, 0)
    }

    fn validate_json_inner(
        &self,
        value: &serde_json::Value,
        path: &str,
        limits: SchemaLimits,
        depth: usize,
    ) -> Result<(), AppSchemaError> {
        if depth > limits.max_depth {
            return Err(AppSchemaError::Invalid(format!(
                "{path} exceeds the maximum value depth {}",
                limits.max_depth
            )));
        }
        let mismatch = |expected: &str| {
            AppSchemaError::Invalid(format!(
                "{path} expected {expected}, got {}",
                json_kind(value)
            ))
        };
        match self {
            Self::Null if value.is_null() => Ok(()),
            Self::Boolean if value.is_boolean() => Ok(()),
            Self::Integer if value.as_i64().is_some() => Ok(()),
            Self::Float if value.as_f64().is_some() => Ok(()),
            Self::String => {
                let text = value.as_str().ok_or_else(|| mismatch("string"))?;
                if text.len() > limits.max_string_bytes {
                    Err(AppSchemaError::Invalid(format!(
                        "{path} exceeds the {} byte string limit",
                        limits.max_string_bytes
                    )))
                } else {
                    Ok(())
                }
            }
            Self::Array { items } => {
                let values = value.as_array().ok_or_else(|| mismatch("array"))?;
                if values.len() > limits.max_collection_items {
                    return Err(AppSchemaError::Invalid(format!(
                        "{path} exceeds the {} item collection limit",
                        limits.max_collection_items
                    )));
                }
                for (index, value) in values.iter().enumerate() {
                    items.validate_json_inner(
                        value,
                        &format!("{path}[{index}]"),
                        limits,
                        depth + 1,
                    )?;
                }
                Ok(())
            }
            Self::Optional { item } if value.is_null() => Ok(()),
            Self::Optional { item } => item.validate_json_inner(value, path, limits, depth + 1),
            Self::Object { fields } => {
                let values = value.as_object().ok_or_else(|| mismatch("object"))?;
                if values.len() > limits.max_collection_items {
                    return Err(AppSchemaError::Invalid(format!(
                        "{path} exceeds the {} field collection limit",
                        limits.max_collection_items
                    )));
                }
                for key in values.keys() {
                    if !fields.contains_key(key) {
                        return Err(AppSchemaError::Invalid(format!(
                            "{path}.{key} is not declared by the schema"
                        )));
                    }
                }
                for (name, field) in fields {
                    match values.get(name) {
                        Some(value) => field.ty.validate_json_inner(
                            value,
                            &format!("{path}.{name}"),
                            limits,
                            depth + 1,
                        )?,
                        None if field.optional => {}
                        None => {
                            return Err(AppSchemaError::Invalid(format!(
                                "{path}.{name} is required"
                            )));
                        }
                    }
                }
                Ok(())
            }
            Self::Alias { base, .. } => base.validate_json_inner(value, path, limits, depth + 1),
            Self::Null => Err(mismatch("null")),
            Self::Boolean => Err(mismatch("boolean")),
            Self::Integer => Err(mismatch("integer")),
            Self::Float => Err(mismatch("number")),
        }
    }

    fn validate_shape(&self, depth: usize) -> Result<(), AppSchemaError> {
        if depth > SchemaLimits::default().max_depth {
            return Err(AppSchemaError::Invalid(
                "schema exceeds the maximum type depth".into(),
            ));
        }
        match self {
            Self::Array { items } | Self::Optional { item: items } => {
                items.validate_shape(depth + 1)
            }
            Self::Object { fields } => {
                if fields.len() > SchemaLimits::default().max_collection_items {
                    return Err(AppSchemaError::Invalid("schema has too many fields".into()));
                }
                for (name, field) in fields {
                    validate_name(name, "field")?;
                    validate_documentation(&field.documentation, &format!("field `{name}`"))?;
                    field.ty.validate_shape(depth + 1)?;
                }
                Ok(())
            }
            Self::Alias {
                name,
                base,
                validation,
                display,
            } => {
                validate_name(name, "alias")?;
                validate_metadata_map(validation, "alias.validation", depth + 1)?;
                validate_metadata_map(display, "alias.display", depth + 1)?;
                base.validate_shape(depth + 1)
            }
            _ => Ok(()),
        }
    }

    fn reject_defaults(&self, path: &str) -> Result<(), AppSchemaError> {
        match self {
            Self::Array { items } | Self::Optional { item: items } => items.reject_defaults(path),
            Self::Object { fields } => {
                for (name, field) in fields {
                    if field.default.is_some() {
                        return Err(AppSchemaError::Invalid(format!(
                            "{path}.fields.{name}.default is only valid on a function parameter"
                        )));
                    }
                    field.ty.reject_defaults(&format!("{path}.fields.{name}"))?;
                }
                Ok(())
            }
            Self::Alias { base, .. } => base.reject_defaults(path),
            _ => Ok(()),
        }
    }
}

pub fn canonical_schema_hash(
    functions: &BTreeMap<String, FunctionSchema>,
) -> Result<String, AppSchemaError> {
    let semantic = serde_json::to_value(functions)?;
    let mut bytes = Vec::new();
    write_canonical_json(&semantic, &mut bytes)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn write_canonical_json(
    value: &serde_json::Value,
    output: &mut Vec<u8>,
) -> Result<(), AppSchemaError> {
    match value {
        serde_json::Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        _ => serde_json::to_writer(output, value)?,
    }
    Ok(())
}

fn validate_name(name: &str, kind: &str) -> Result<(), AppSchemaError> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if valid && name.len() <= 128 {
        Ok(())
    } else {
        Err(AppSchemaError::Invalid(format!(
            "{kind} name `{name}` is not a bounded identifier"
        )))
    }
}

fn validate_documentation(value: &str, subject: &str) -> Result<(), AppSchemaError> {
    if value.len() > MAX_DOCUMENTATION_BYTES {
        Err(AppSchemaError::Invalid(format!(
            "{subject} documentation exceeds {MAX_DOCUMENTATION_BYTES} bytes"
        )))
    } else if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        Err(AppSchemaError::Invalid(format!(
            "{subject} documentation contains a control character"
        )))
    } else {
        Ok(())
    }
}

fn validate_metadata_map(
    values: &BTreeMap<String, serde_json::Value>,
    path: &str,
    depth: usize,
) -> Result<(), AppSchemaError> {
    let limits = SchemaLimits::default();
    if values.len() > limits.max_collection_items {
        return Err(AppSchemaError::Invalid(format!(
            "{path} exceeds the {} field limit",
            limits.max_collection_items
        )));
    }
    for (name, value) in values {
        if name.len() > limits.max_string_bytes {
            return Err(AppSchemaError::Invalid(format!(
                "{path} contains an oversized key"
            )));
        }
        validate_metadata_json(value, &format!("{path}.{name}"), limits, depth)?;
    }
    Ok(())
}

fn validate_metadata_json(
    value: &serde_json::Value,
    path: &str,
    limits: SchemaLimits,
    depth: usize,
) -> Result<(), AppSchemaError> {
    if depth > limits.max_depth {
        return Err(AppSchemaError::Invalid(format!(
            "{path} exceeds the maximum metadata depth {}",
            limits.max_depth
        )));
    }
    match value {
        serde_json::Value::String(value) if value.len() > limits.max_string_bytes => Err(
            AppSchemaError::Invalid(format!("{path} exceeds the metadata string limit")),
        ),
        serde_json::Value::Array(values) => {
            if values.len() > limits.max_collection_items {
                return Err(AppSchemaError::Invalid(format!(
                    "{path} exceeds the metadata item limit"
                )));
            }
            for (index, value) in values.iter().enumerate() {
                validate_metadata_json(value, &format!("{path}[{index}]"), limits, depth + 1)?;
            }
            Ok(())
        }
        serde_json::Value::Object(values) => {
            if values.len() > limits.max_collection_items {
                return Err(AppSchemaError::Invalid(format!(
                    "{path} exceeds the metadata field limit"
                )));
            }
            for (name, value) in values {
                if name.len() > limits.max_string_bytes {
                    return Err(AppSchemaError::Invalid(format!(
                        "{path} contains an oversized key"
                    )));
                }
                validate_metadata_json(value, &format!("{path}.{name}"), limits, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

impl fmt::Display for TypeSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.provider_type().fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AppManifest {
        let fields = [(
            "email".into(),
            FieldSchema {
                ty: TypeSchema::String,
                documentation: "Unique sign-in email.".into(),
                optional: false,
                secret: false,
                default: None,
            },
        )]
        .into_iter()
        .collect();
        AppManifest {
            manifest_version: 1,
            protocol: 1,
            provider: "app".into(),
            sdk: "test".into(),
            sdk_version: "1.0.0".into(),
            schema_hash: String::new(),
            functions: [(
                "create_user".into(),
                FunctionSchema {
                    documentation: "Create a user.".into(),
                    retry_safe: false,
                    params: TypeSchema::Object { fields },
                    returns: TypeSchema::Integer,
                },
            )]
            .into_iter()
            .collect(),
        }
        .with_computed_hash()
        .expect("hash")
    }

    #[test]
    fn canonical_hash_is_stable_and_prefixed() {
        let manifest = manifest();
        assert!(manifest.schema_hash.starts_with("blake3:"));
        assert_eq!(
            manifest.computed_hash().expect("hash"),
            manifest.schema_hash
        );
        manifest.validate().expect("valid manifest");
    }

    #[test]
    fn normalized_manifest_replaces_a_stale_declared_hash() {
        let mut edited = manifest();
        let old_hash = edited.schema_hash.clone();
        edited.functions.insert(
            "new_function".into(),
            FunctionSchema {
                documentation: "Newly edited function.".into(),
                retry_safe: true,
                params: TypeSchema::Object {
                    fields: BTreeMap::new(),
                },
                returns: TypeSchema::String,
            },
        );
        let source = serde_json::to_string(&edited).expect("manifest JSON");

        let normalized = AppManifest::from_json_normalized(&source).expect("normalized manifest");

        assert_ne!(normalized.schema_hash, old_hash);
        assert_eq!(
            normalized.schema_hash,
            normalized.computed_hash().expect("canonical hash")
        );
        normalized
            .validate()
            .expect("normalized manifest is strict");
        assert!(matches!(
            AppManifest::from_json(&source),
            Err(AppSchemaError::HashMismatch { .. })
        ));
    }

    #[test]
    fn converts_to_shared_provider_schema() {
        let schema = manifest().provider_schema();
        let operation = schema.operation("create_user").expect("operation");
        assert_eq!(
            operation.parameters[0].documentation,
            "Unique sign-in email."
        );
        assert_eq!(operation.result, Type::Int);
    }

    #[test]
    fn offline_manifest_drives_analysis_plan_repairs_and_completion() {
        let manifest = manifest();
        let mut providers = webtest_provider::ProviderRegistry::built_in_schemas();
        providers.register_schema(manifest.provider_schema());
        let mut database = webtest_analysis::AnalysisDatabase::with_provider_registry(providers);
        let source = r#"test "x" { server { let value = app.create_user(emial: "a") } }"#;
        let file = database.open_file("test.webtest", source);
        let diagnostics = database.diagnostics(file).expect("diagnostics");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "semantic.reserved_provider")
        );
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "semantic.unknown_argument"
                    && diagnostic.repair_hints.iter().any(|hint| {
                        matches!(
                            &hint.replacement,
                            webtest_analysis::RepairReplacement::Text(value) if value == "email"
                        )
                    })
            }),
            "{diagnostics:#?}"
        );
        let plan = database.test_plan(file).expect("plan");
        let webtest_plan::TestOperation::ServerProviderCall(call) =
            &plan.tests[0].steps[0].operation
        else {
            panic!("provider call")
        };
        assert_eq!(call.schema_hash, manifest.schema_hash);
        assert!(!call.retry_safe);

        let partial = r#"test "x" { server { let value = app. } }"#;
        let file = database.open_file("partial.webtest", partial);
        let dot = partial.find("app.").expect("app") + 4;
        let completions = database
            .completions(file, webtest_text::TextSize::from(dot as u32))
            .expect("completions");
        assert!(
            completions
                .iter()
                .any(|completion| completion.label == "create_user")
        );
    }
}
