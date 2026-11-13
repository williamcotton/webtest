//! Pure completion, signature, and documentation projections over cached analysis inputs.

use std::collections::HashSet;

use crate::facts::{
    Completion, CompletionKind, DocumentationFact, Signature, SignatureParameter, TypeFact,
};
use webtest_model::Type;
use webtest_provider::{OperationSchema, ProviderRegistry};
use webtest_syntax::{Parse, SyntaxKind, SyntaxNode};
use webtest_text::TextSize;

pub(crate) fn completions(
    parsed: &Parse,
    type_facts: &[TypeFact],
    providers: &ProviderRegistry,
    offset: TextSize,
) -> Vec<Completion> {
    let syntax = parsed.syntax();
    let offset_u32 = u32::from(offset);
    let containing = |node: &SyntaxNode| {
        let range = node.text_range();
        u32::from(range.start()) <= offset_u32 && u32::from(range.end()) >= offset_u32
    };
    if !syntax
        .descendants()
        .any(|node| node.kind() == SyntaxKind::MemberExpr && containing(&node))
        && let Some(call) = syntax
            .descendants()
            .filter(|node| node.kind() == SyntaxKind::CallExpr && containing(node))
            .min_by_key(|node| node.text_range().len())
        && let Some((provider, operation)) = provider_operation_tokens(&call)
        && let Some(operation) = providers
            .schema(&provider)
            .and_then(|schema| schema.operation(&operation))
    {
        let present = call
            .children()
            .filter(|node| node.kind() == SyntaxKind::CallArg)
            .filter_map(|argument| named_argument_token(&argument))
            .collect::<HashSet<_>>();
        return operation
            .parameters
            .iter()
            .filter(|parameter| !parameter.positional && !present.contains(&parameter.name))
            .map(|parameter| Completion {
                label: parameter.name.clone(),
                detail: format!(
                    "{}{}",
                    parameter.ty,
                    if parameter.required {
                        ""
                    } else {
                        " (optional)"
                    }
                ),
                documentation: parameter.documentation.clone(),
                kind: CompletionKind::Parameter,
                insert_text: format!("{}: ", parameter.name),
                ..Default::default()
            })
            .collect();
    }
    if let Some(member) = syntax
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::MemberExpr && containing(node))
        .min_by_key(|node| node.text_range().len())
    {
        use rowan::ast::AstNode;
        let provider = webtest_syntax::ast::MemberExpr::cast(member.clone()).and_then(|member| {
            match member.receiver()? {
                webtest_syntax::ast::Expr::Name(name) => Some(name.name()?.text().to_owned()),
                _ => None,
            }
        });
        if let Some(provider) = provider
            && let Some(schema) = providers.schema(&provider)
        {
            return schema
                .operations
                .values()
                .filter(|operation| {
                    crate::workspace_editor::context_domain(parsed, offset).is_none_or(|domain| {
                        operation.capability == webtest_model::Capability::Pure
                            || operation.capability == domain
                    })
                })
                .map(|operation| Completion {
                    label: operation.name.0.clone(),
                    detail: provider_signature(&provider, operation),
                    documentation: operation.documentation.clone(),
                    kind: CompletionKind::Function,
                    insert_text: format!("{}(", operation.name.0),
                    ..Default::default()
                })
                .collect();
        }
        if let Some(receiver) = member.children().next() {
            let receiver_fact = type_facts
                .iter()
                .find(|fact| fact.range == receiver.text_range())
                .or_else(|| {
                    type_facts
                        .iter()
                        .filter(|fact| receiver.text_range().contains_range(fact.range))
                        .max_by_key(|fact| fact.range.len())
                });
            let Some(fact) = receiver_fact else {
                return Vec::new();
            };
            return fact
                .ty
                .member_names()
                .into_iter()
                .filter_map(|name| {
                    let ty = fact.ty.member(&name)?;
                    let documentation = if let Type::Record(fields) = &fact.ty {
                        fields
                            .get(&name)
                            .map(|field| field.documentation.clone())
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    Some(Completion {
                        label: name.clone(),
                        detail: ty.to_string(),
                        documentation,
                        kind: CompletionKind::Property,
                        insert_text: name,
                        ..Default::default()
                    })
                })
                .collect();
        }
    }
    Vec::new()
}

pub(crate) fn signature_help(
    parsed: &Parse,
    providers: &ProviderRegistry,
    offset: TextSize,
) -> Option<Signature> {
    let syntax = parsed.syntax();
    let offset_u32 = u32::from(offset);
    let call = syntax
        .descendants()
        .filter(|node| {
            node.kind() == SyntaxKind::CallExpr
                && u32::from(node.text_range().start()) <= offset_u32
                && u32::from(node.text_range().end()) >= offset_u32
        })
        .min_by_key(|node| node.text_range().len())?;
    let (provider, operation_name) = provider_operation_tokens(&call)?;
    let operation = providers
        .schema(&provider)
        .and_then(|schema| schema.operation(&operation_name))?;
    let current = call
        .children()
        .filter(|node| node.kind() == SyntaxKind::CallArg)
        .enumerate()
        .filter(|(_, node)| node.text_range().start() <= offset)
        .last();
    let active_parameter = current
        .map_or(0, |(index, node)| {
            named_argument_token(&node)
                .and_then(|name| {
                    operation
                        .parameters
                        .iter()
                        .position(|parameter| parameter.name == name)
                })
                .unwrap_or(index)
        })
        .min(operation.parameters.len().saturating_sub(1));
    Some(Signature {
        symbol: None,
        capability: Some(operation.capability),
        lifetime: None,
        label: provider_signature(&provider, operation),
        documentation: operation.documentation.clone(),
        parameters: operation
            .parameters
            .iter()
            .map(|parameter| SignatureParameter {
                name: parameter.name.clone(),
                ty: Some(parameter.ty.clone()),
                optional: !parameter.required,
                default: if parameter.secret {
                    None
                } else {
                    parameter
                        .default
                        .as_ref()
                        .and_then(webtest_model::value_to_json)
                },
                label: format!("{}: {}", parameter.name, parameter.ty),
                documentation: parameter.documentation.clone(),
            })
            .collect(),
        active_parameter,
    })
}

pub(crate) fn documentation_at(
    parsed: &Parse,
    providers: &ProviderRegistry,
    offset: TextSize,
) -> Option<DocumentationFact> {
    let syntax = parsed.syntax();
    let offset_u32 = u32::from(offset);
    let mut calls = syntax
        .descendants()
        .filter(|node| {
            node.kind() == SyntaxKind::CallExpr
                && u32::from(node.text_range().start()) <= offset_u32
                && u32::from(node.text_range().end()) >= offset_u32
        })
        .collect::<Vec<_>>();
    calls.sort_by_key(|node| node.text_range().len());
    for call in calls {
        if let Some((provider, operation_name)) = provider_operation_tokens(&call)
            && let Some(operation) = providers
                .schema(&provider)
                .and_then(|schema| schema.operation(&operation_name))
        {
            return Some(DocumentationFact {
                range: call
                    .children()
                    .next()
                    .map_or(call.text_range(), |node| node.text_range()),
                contents: format!(
                    "{}\n\nReturns `{}`. Retry-safe: {}.",
                    operation.documentation, operation.result, operation.retry_safe
                ),
            });
        }
    }
    None
}

fn provider_operation_tokens(node: &SyntaxNode) -> Option<(String, String)> {
    use rowan::ast::AstNode;
    use webtest_syntax::ast;
    let call = ast::CallExpr::cast(node.clone())?;
    let ast::Expr::Member(member) = call.callee()? else {
        return None;
    };
    let ast::Expr::Name(receiver) = member.receiver()? else {
        return None;
    };
    Some((
        receiver.name()?.text().to_owned(),
        member.member()?.text().to_owned(),
    ))
}

fn named_argument_token(node: &SyntaxNode) -> Option<String> {
    use rowan::ast::AstNode;
    webtest_syntax::ast::CallArg::cast(node.clone())?.name()
}

fn provider_signature(provider: &str, operation: &OperationSchema) -> String {
    let parameters = operation
        .parameters
        .iter()
        .map(|parameter| {
            format!(
                "{}{}: {}",
                parameter.name,
                if parameter.required { "" } else { "?" },
                parameter.ty
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{provider}.{}({parameters}) -> {}",
        operation.name.0, operation.result
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnalysisDatabase;
    use webtest_model::{Capability, Value};
    use webtest_provider::{OperationName, ParameterSchema, ProviderName, ProviderSchema};

    fn offset(source: &str, needle: &str) -> TextSize {
        TextSize::from(u32::try_from(source.find(needle).expect("needle")).expect("offset"))
    }

    fn app_registry(operation: &str, retry_safe: bool) -> ProviderRegistry {
        let mut registry = ProviderRegistry::built_in_schemas();
        registry.register_schema(ProviderSchema {
            name: ProviderName("app".into()),
            operations: [(
                operation.into(),
                OperationSchema {
                    name: OperationName(operation.into()),
                    parameters: vec![
                        ParameterSchema {
                            name: "message".into(),
                            ty: Type::String,
                            required: true,
                            positional: false,
                            secret: false,
                            documentation: "Message to echo.".into(),
                            default: None,
                        },
                        ParameterSchema {
                            name: "uppercase".into(),
                            ty: Type::Bool,
                            required: false,
                            positional: false,
                            secret: false,
                            documentation: "Uppercase the result.".into(),
                            default: Some(Value::Bool(false)),
                        },
                    ],
                    result: Type::String,
                    capability: Capability::Server,
                    documentation: format!("Invoke {operation}."),
                    retry_safe,
                },
            )]
            .into(),
            schema_identity: None,
        });
        registry
    }

    #[test]
    fn provider_operation_and_missing_argument_completion_preserve_schema_order() {
        let provider_source = r#"test "x" { server { let response = http. } }"#;
        let parsed = webtest_syntax::parse(provider_source);
        let providers = ProviderRegistry::built_in_schemas();
        let operation_completions = completions(
            &parsed,
            &[],
            &providers,
            offset(provider_source, "http.") + TextSize::from(5),
        );
        assert!(!operation_completions.is_empty());
        assert!(
            operation_completions
                .iter()
                .all(|item| item.kind == CompletionKind::Function)
        );

        let argument_source =
            r#"test "x" { server { let response = http.post("/users", headers: {}) } }"#;
        let parsed = webtest_syntax::parse(argument_source);
        let call_offset = offset(argument_source, "headers") + TextSize::from(1);
        let argument_completions = completions(&parsed, &[], &providers, call_offset);
        assert!(argument_completions.iter().any(|item| item.label == "json"));
        assert!(
            !argument_completions
                .iter()
                .any(|item| item.label == "headers")
        );
    }

    #[test]
    fn record_member_completion_survives_a_half_typed_member() {
        let source = r#"test "x" { let value: { id: Int, name: String } = { id: 1, name: "A" } let selected = value. }"#;
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("completion.webtest", source);
        let completions = database
            .completions(
                file,
                TextSize::from(u32::try_from(source.len()).expect("length") - 2),
            )
            .expect("completion");
        assert_eq!(
            completions
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "name"]
        );
    }

    #[test]
    fn nested_signature_and_documentation_use_the_smallest_provider_call() {
        let source = r#"test "x" { server { let response = http.post("/", json: { value: http.get("/nested") }) } }"#;
        let parsed = webtest_syntax::parse(source);
        let providers = ProviderRegistry::built_in_schemas();
        let nested = offset(source, "\"/nested\"") + TextSize::from(2);
        let signature = signature_help(&parsed, &providers, nested).expect("signature");
        assert!(
            signature.label.starts_with("http.get("),
            "{}",
            signature.label
        );
        let documentation = documentation_at(&parsed, &providers, nested).expect("documentation");
        assert!(documentation.contents.contains("Returns"));
        assert!(documentation.contents.contains("Retry-safe"));
    }

    #[test]
    fn signature_active_parameter_clamps_at_argument_boundaries() {
        let source =
            r#"test "x" { server { let response = http.post("/", headers: {}, json: {}) } }"#;
        let parsed = webtest_syntax::parse(source);
        let providers = ProviderRegistry::built_in_schemas();
        let signature = signature_help(
            &parsed,
            &providers,
            offset(source, "json") + TextSize::from(1),
        )
        .expect("signature");
        assert_eq!(
            signature.active_parameter,
            signature
                .parameters
                .iter()
                .position(|parameter| parameter.name == "json")
                .unwrap()
        );
        assert!(signature.parameters.len() > signature.active_parameter);
    }

    #[test]
    fn provider_replacement_updates_all_editor_queries_without_a_source_edit() {
        let call_source = r#"test "x" { server { let value = app.echo(message: "hello") } }"#;
        let member_source = r#"test "x" { server { let value = app. } }"#;
        let mut database = AnalysisDatabase::with_provider_registry(app_registry("echo", true));
        let call_file = database.open_file("call.webtest", call_source);
        let member_file = database.open_file("member.webtest", member_source);
        let call_offset = offset(call_source, "message") + TextSize::from(1);
        let member_offset = offset(member_source, "app.") + TextSize::from(4);

        let signature = database
            .signature_help(call_file, call_offset)
            .expect("signature query")
            .expect("signature");
        assert!(signature.label.starts_with("app.echo("));
        let documentation = database
            .documentation_at(call_file, call_offset)
            .expect("documentation query")
            .expect("documentation");
        assert!(documentation.contents.contains("Retry-safe: true"));
        assert_eq!(
            database
                .completions(member_file, member_offset)
                .expect("completion")[0]
                .label,
            "echo"
        );

        database.set_provider_registry(app_registry("renamed", false));
        assert!(
            database
                .signature_help(call_file, call_offset)
                .expect("signature query")
                .is_none()
        );
        assert!(
            database
                .documentation_at(call_file, call_offset)
                .expect("documentation query")
                .is_none()
        );
        assert_eq!(
            database
                .completions(member_file, member_offset)
                .expect("completion")[0]
                .label,
            "renamed"
        );
    }

    #[test]
    fn unknown_provider_operation_and_receiver_do_not_produce_speculative_results() {
        let providers = ProviderRegistry::built_in_schemas();
        for source in [
            r#"test "x" { server { let value = unknown. } }"#,
            r#"test "x" { server { let value = http.unknown() } }"#,
            r#"test "x" { let value = 1 let selected = value. }"#,
        ] {
            let parsed = webtest_syntax::parse(source);
            let at = TextSize::from(u32::try_from(source.len()).expect("length") - 2);
            assert!(completions(&parsed, &[], &providers, at).is_empty());
        }
    }
}
