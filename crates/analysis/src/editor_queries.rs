//! Pure completion, signature, and documentation projections over cached analysis inputs.

use std::collections::HashSet;

use crate::facts::{
    Completion, CompletionKind, DocumentationFact, Signature, SignatureParameter, TypeFact,
};
use webtest_model::Type;
use webtest_provider::{OperationSchema, ProviderRegistry};
use webtest_syntax::{Parse, SyntaxKind, SyntaxNode, SyntaxToken};
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
    if let Some(call) = syntax
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
            })
            .collect();
    }
    if let Some(member) = syntax
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::MemberExpr && containing(node))
        .min_by_key(|node| node.text_range().len())
    {
        let tokens = meaningful_tokens(&member);
        if let Some(provider) = tokens.first().map(|token| token.text().to_string())
            && tokens.iter().any(|token| token.kind() == SyntaxKind::Dot)
            && let Some(schema) = providers.schema(&provider)
        {
            return schema
                .operations
                .values()
                .map(|operation| Completion {
                    label: operation.name.0.clone(),
                    detail: provider_signature(&provider, operation),
                    documentation: operation.documentation.clone(),
                    kind: CompletionKind::Function,
                    insert_text: format!("{}(", operation.name.0),
                })
                .collect();
        }
        if let Some(receiver) = member.children().next() {
            let receiver_fact = type_facts
                .iter()
                .filter(|fact| {
                    fact.range.contains(receiver.text_range().start())
                        || fact.range == receiver.text_range()
                })
                .min_by_key(|fact| fact.range.len())
                .or_else(|| {
                    // A half-typed `value.` is deliberately absent from HIR because the member
                    // token is missing. Recover the receiver's declaration fact from the
                    // lossless CST so completion remains useful while editing.
                    let receiver_name = meaningful_tokens(&receiver)
                        .first()
                        .map(|token| token.text().to_string())?;
                    syntax
                        .descendants()
                        .filter(|node| {
                            node.kind() == SyntaxKind::LetStmt
                                && node.text_range().end() <= member.text_range().start()
                        })
                        .filter_map(|declaration| {
                            let tokens = meaningful_tokens(&declaration);
                            let name = tokens.get(1)?;
                            (name.text() == receiver_name).then_some(name.text_range())
                        })
                        .last()
                        .and_then(|range| type_facts.iter().find(|fact| fact.range == range))
                });
            let Some(fact) = receiver_fact else {
                return Vec::new();
            };
            let Type::Record(fields) = &fact.ty else {
                return Vec::new();
            };
            return fields
                .iter()
                .map(|(name, field)| Completion {
                    label: name.clone(),
                    detail: field.ty.to_string(),
                    documentation: field.documentation.clone(),
                    kind: CompletionKind::Property,
                    insert_text: name.clone(),
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
    let active_parameter = call
        .children()
        .filter(|node| node.kind() == SyntaxKind::CallArg)
        .take_while(|node| u32::from(node.text_range().start()) <= offset_u32)
        .count()
        .saturating_sub(1)
        .min(operation.parameters.len().saturating_sub(1));
    Some(Signature {
        label: provider_signature(&provider, operation),
        documentation: operation.documentation.clone(),
        parameters: operation
            .parameters
            .iter()
            .map(|parameter| SignatureParameter {
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
    for call in syntax.descendants().filter(|node| {
        node.kind() == SyntaxKind::CallExpr
            && u32::from(node.text_range().start()) <= offset_u32
            && u32::from(node.text_range().end()) >= offset_u32
    }) {
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

fn meaningful_tokens(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| !token.kind().is_trivia())
        .collect()
}

fn provider_operation_tokens(node: &SyntaxNode) -> Option<(String, String)> {
    let tokens = meaningful_tokens(node);
    let dot = tokens
        .iter()
        .position(|token| token.kind() == SyntaxKind::Dot)?;
    Some((
        tokens.get(dot.checked_sub(1)?)?.text().into(),
        tokens.get(dot + 1)?.text().into(),
    ))
}

fn named_argument_token(node: &SyntaxNode) -> Option<String> {
    let tokens = meaningful_tokens(node);
    tokens
        .iter()
        .any(|token| token.kind() == SyntaxKind::Colon)
        .then(|| tokens.first().map(|token| token.text().into()))?
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
        assert_eq!(signature.active_parameter, 2);
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
