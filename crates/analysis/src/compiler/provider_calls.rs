//! Provider recognition, schema validation, arguments, and plan metadata.

use std::collections::{BTreeMap, BTreeSet};

use super::{CompiledProviderCall, Compiler};
use crate::diagnostic::{nearest_strings, text_hints};
use webtest_feedback::RepairHintKind;
use webtest_hir::{HirCallArgument, HirExpr, HirExprKind, HirNameRef};
use webtest_plan::PlanExpr;
use webtest_provider::{Capability, OperationSchema, Type};
use webtest_text::TextRange;

impl Compiler<'_> {
    pub(super) fn provider_call(
        &mut self,
        expression: &HirExpr,
        domain: Capability,
    ) -> Option<CompiledProviderCall> {
        let HirExprKind::Call { callee, arguments } = &expression.kind else {
            return None;
        };
        let HirExprKind::Member {
            receiver,
            member: operation,
            ..
        } = &callee.kind
        else {
            return None;
        };
        let HirExprKind::Name(HirNameRef::Unresolved(provider)) = &receiver.kind else {
            return None;
        };
        let Some(schema) = self.providers.schema(provider) else {
            if provider == "app" {
                self.error(
                    receiver.origin.range,
                    "semantic.reserved_provider",
                    "`app` is reserved for the application bridge".into(),
                );
            } else {
                let known = self
                    .providers
                    .schemas()
                    .map(|schema| schema.name.0.clone())
                    .collect::<Vec<_>>();
                let candidates = nearest_strings(&known, provider, 5);
                self.error_with_details(
                    receiver.origin.range,
                    "semantic.unknown_provider",
                    format!("unknown provider `{provider}`"),
                    serde_json::json!({"requested": provider, "known_providers": known}),
                    text_hints(
                        RepairHintKind::NameCandidate,
                        candidates,
                        receiver.origin.range,
                    ),
                    vec!["provider".into()],
                );
            }
            return Some(CompiledProviderCall {
                provider: provider.clone(),
                operation: operation.clone(),
                arguments: BTreeMap::new(),
                result_type: Type::Unknown,
                schema_hash: String::new(),
                redacted_arguments: Vec::new(),
                redacted_result_fields: Vec::new(),
                retry_safe: false,
            });
        };
        let Some(operation_schema) = schema.operation(operation) else {
            let known = schema.operations.keys().cloned().collect::<Vec<_>>();
            let candidates = nearest_strings(&known, operation, 5);
            self.error_with_details(
                callee.origin.range,
                "semantic.unknown_provider_operation",
                format!("provider `{provider}` has no operation `{operation}`"),
                serde_json::json!({
                    "provider": provider,
                    "requested": operation,
                    "known_operations": known,
                }),
                text_hints(
                    RepairHintKind::NameCandidate,
                    candidates,
                    callee.origin.range,
                ),
                vec![format!("provider.{provider}")],
            );
            return Some(CompiledProviderCall {
                provider: provider.clone(),
                operation: operation.clone(),
                arguments: BTreeMap::new(),
                result_type: Type::Unknown,
                schema_hash: schema.hash(),
                redacted_arguments: Vec::new(),
                redacted_result_fields: Vec::new(),
                retry_safe: false,
            });
        };
        if domain != operation_schema.capability {
            self.error(
                expression.origin.range,
                "semantic.capability_mismatch",
                format!(
                    "{}.{} requires {} capability but is used in {domain} context",
                    provider, operation, operation_schema.capability
                ),
            );
        }
        self.required.insert(operation_schema.capability);
        let values =
            self.provider_arguments(operation_schema, arguments, domain, expression.origin.range);
        Some(CompiledProviderCall {
            provider: provider.clone(),
            operation: operation.clone(),
            arguments: values,
            result_type: operation_schema.result.clone(),
            schema_hash: schema.hash(),
            redacted_arguments: operation_schema
                .parameters
                .iter()
                .filter(|parameter| parameter.secret)
                .map(|parameter| parameter.name.clone())
                .collect(),
            redacted_result_fields: secret_record_fields(&operation_schema.result),
            retry_safe: operation_schema.retry_safe,
        })
    }

    fn provider_arguments(
        &mut self,
        schema: &OperationSchema,
        arguments: &[HirCallArgument],
        domain: Capability,
        call_range: TextRange,
    ) -> BTreeMap<String, PlanExpr> {
        let mut values = BTreeMap::new();
        let positional: Vec<_> = schema
            .parameters
            .iter()
            .filter(|parameter| parameter.positional)
            .collect();
        let mut next_positional = 0;
        let mut body_argument = None;
        for argument in arguments {
            let parameter = if let Some(name) = &argument.name {
                schema
                    .parameters
                    .iter()
                    .find(|parameter| &parameter.name == name)
            } else {
                let parameter = positional.get(next_positional).copied();
                next_positional += 1;
                parameter
            };
            let Some(parameter) = parameter else {
                let requested = argument.name.clone();
                let known = schema
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>();
                let candidates = requested
                    .as_deref()
                    .map(|requested| nearest_strings(&known, requested, 5))
                    .unwrap_or_default();
                self.error_with_details(
                    argument.origin.range,
                    "semantic.unknown_argument",
                    argument.name.as_ref().map_or_else(
                        || "too many positional arguments".into(),
                        |name| format!("unknown argument `{name}`"),
                    ),
                    serde_json::json!({
                        "requested": requested,
                        "known_arguments": known,
                    }),
                    text_hints(
                        RepairHintKind::ArgumentCandidate,
                        candidates,
                        argument.origin.range,
                    ),
                    vec!["provider".into()],
                );
                continue;
            };
            if values.contains_key(&parameter.name) {
                self.error(
                    argument.origin.range,
                    "semantic.duplicate_argument",
                    format!("argument `{}` is provided more than once", parameter.name),
                );
                continue;
            }
            if matches!(parameter.name.as_str(), "json" | "text" | "bytes" | "form") {
                if let Some(previous) = &body_argument {
                    self.error(
                        argument.origin.range,
                        "semantic.conflicting_arguments",
                        format!(
                            "HTTP body arguments `{previous}` and `{}` cannot be combined",
                            parameter.name
                        ),
                    );
                } else {
                    body_argument = Some(parameter.name.clone());
                }
            }
            let value = self.infer_expr(&argument.value, domain, Some(&parameter.ty));
            self.expect_type(argument.value.origin.range, &parameter.ty, &value.ty);
            values.insert(parameter.name.clone(), value.expression);
        }
        for parameter in schema
            .parameters
            .iter()
            .filter(|parameter| parameter.required)
        {
            if !values.contains_key(&parameter.name) {
                self.error(
                    call_range,
                    "semantic.missing_argument",
                    format!("missing required argument `{}`", parameter.name),
                );
            }
        }
        values
    }
}

fn secret_record_fields(ty: &Type) -> Vec<String> {
    fn collect(ty: &Type, fields: &mut BTreeSet<String>) {
        match ty {
            Type::Record(record) => {
                for (name, field) in record {
                    if field.secret {
                        fields.insert(name.clone());
                    }
                    collect(&field.ty, fields);
                }
            }
            Type::List(item) | Type::Option(item) | Type::Response(item) => collect(item, fields),
            _ => {}
        }
    }
    let mut fields = BTreeSet::new();
    collect(ty, &mut fields);
    fields.into_iter().collect()
}
