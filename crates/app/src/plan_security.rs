//! Literal-secret emission policy follows plan environments, including isolated
//! helper parameters. This is provenance policy, never language type inference.
use crate::error::AppError;
use std::collections::HashMap;
use webtest_model::BindingId;
use webtest_plan::{PlanEnvelope, PlanExpr, PlanNode, PlanNodeKind, TestOperation};
use webtest_project::Project;

#[derive(Clone, Copy, Default)]
struct Provenance {
    literal: bool,
    json: bool,
    header: bool,
}
impl Provenance {
    fn merge(self, other: Self) -> Self {
        Self {
            literal: self.literal || other.literal,
            json: self.json || other.json,
            header: self.header || other.header,
        }
    }
}
struct Policy<'a> {
    fixtures: &'a [webtest_plan::FixtureDefinition],
    json: &'a [String],
    headers: &'a [String],
}
impl Policy<'_> {
    fn expression(
        &self,
        expression: &PlanExpr,
        bindings: &HashMap<BindingId, Provenance>,
    ) -> Provenance {
        match expression {
            PlanExpr::Binding(id) => bindings.get(id).copied().unwrap_or_default(),
            PlanExpr::Literal(value) => self.literal(value),
            PlanExpr::Type(_) => Provenance::default(),
            PlanExpr::Record(fields) => {
                fields
                    .iter()
                    .fold(Provenance::default(), |result, (name, value)| {
                        let mut value = self.expression(value, bindings);
                        value.json |= value.literal
                            && self
                                .json
                                .iter()
                                .any(|field| field.eq_ignore_ascii_case(name));
                        value.header |= value.literal
                            && self
                                .headers
                                .iter()
                                .any(|field| field.eq_ignore_ascii_case(name));
                        result.merge(value)
                    })
            }
            PlanExpr::PureCall {
                arguments, body, ..
            } => {
                let locals = arguments
                    .iter()
                    .map(|(id, value)| (*id, self.expression(value, bindings)))
                    .collect();
                self.expression(body, &locals)
            }
            _ => expression
                .children()
                .into_iter()
                .fold(Provenance::default(), |result, value| {
                    result.merge(self.expression(value, bindings))
                }),
        }
    }
    fn literal(&self, value: &webtest_model::Value) -> Provenance {
        let mut result = Provenance {
            literal: true,
            ..Default::default()
        };
        match value {
            webtest_model::Value::Record(fields) => {
                for (name, value) in fields {
                    result = result.merge(self.literal(value));
                    result.json |= self
                        .json
                        .iter()
                        .any(|field| field.eq_ignore_ascii_case(name));
                    result.header |= self
                        .headers
                        .iter()
                        .any(|field| field.eq_ignore_ascii_case(name));
                }
            }
            webtest_model::Value::List(values) => {
                for value in values {
                    result = result.merge(self.literal(value));
                }
            }
            _ => {}
        }
        result
    }
    fn node(
        &self,
        node: &PlanNode,
        bindings: &mut HashMap<BindingId, Provenance>,
    ) -> Result<Provenance, AppError> {
        match &node.kind {
            PlanNodeKind::Operation { step } => match &step.operation {
                TestOperation::AcquireFixture(call) => {
                    let definition = self
                        .fixtures
                        .iter()
                        .find(|definition| definition.id == call.fixture)
                        .ok_or_else(|| {
                            AppError::internal("fixture definition is absent during plan emission")
                        })?;
                    let mut locals = call
                        .arguments
                        .iter()
                        .map(|argument| {
                            (
                                argument.parameter,
                                self.expression(&argument.value, bindings),
                            )
                        })
                        .collect();
                    let result = self.node(&definition.setup, &mut locals)?;
                    self.node(&definition.teardown, &mut locals)?;
                    if let Some(binding) = call.result_binding {
                        bindings.insert(binding, result);
                    }
                    Ok(Provenance::default())
                }
                TestOperation::EvaluatePure(value) => {
                    let summary = self.expression(&value.expression, bindings);
                    if let Some(id) = value.result_binding {
                        bindings.insert(id, summary);
                    }
                    Ok(Provenance::default())
                }
                TestOperation::Return(value) | TestOperation::Provide(value) => {
                    Ok(self.expression(&value.expression, bindings))
                }
                TestOperation::ServerProviderCall(call) => {
                    for (name, argument) in &call.arguments {
                        let value = self.expression(argument, bindings);
                        if (call.redacted_arguments.contains(name) && value.literal)
                            || (call.provider == "http"
                                && ((name == "json" && value.json)
                                    || (name == "headers" && value.header)))
                        {
                            return Err(secret_plan_error(&call.provider, &call.operation, name));
                        }
                    }
                    if let Some(id) = call.result_binding {
                        bindings.insert(id, Provenance::default());
                    }
                    Ok(Provenance::default())
                }
                TestOperation::Reactive(_)
                | TestOperation::Verdict { .. }
                | TestOperation::Browser(_)
                | TestOperation::Assertion(_) => Ok(Provenance::default()),
            },
            PlanNodeKind::Helper { call, body } => {
                let mut locals = call
                    .arguments
                    .iter()
                    .map(|argument| {
                        (
                            argument.parameter,
                            self.expression(&argument.value, bindings),
                        )
                    })
                    .collect();
                let result = self.node(body, &mut locals)?;
                if let Some(binding) = &call.result {
                    bindings.insert(binding.id, result);
                }
                Ok(Provenance::default())
            }
            PlanNodeKind::Conditional { branches, .. } => {
                let original = bindings.clone();
                for branch in branches {
                    let mut locals = original.clone();
                    self.node(branch, &mut locals)?;
                    for (binding, value) in locals {
                        let prior = bindings.entry(binding).or_default();
                        *prior = prior.merge(value);
                    }
                }
                Ok(Provenance::default())
            }
            PlanNodeKind::Semantic { children, .. } | PlanNodeKind::Sequence { children } => {
                let mut result = Provenance::default();
                for child in children {
                    result = result.merge(self.node(child, bindings)?);
                }
                Ok(result)
            }
            PlanNodeKind::Parallel { children, .. } | PlanNodeKind::Race { children, .. } => {
                let mut result = Provenance::default();
                for child in children {
                    result = result.merge(self.node(child, &mut bindings.clone())?);
                }
                if let PlanNodeKind::Race {
                    result: Some(binding),
                    ..
                } = &node.kind
                {
                    bindings.insert(binding.id, result);
                }
                Ok(Provenance::default())
            }
            PlanNodeKind::Retry { child, .. } | PlanNodeKind::Timeout { child, .. } => {
                self.node(child, &mut bindings.clone())
            }
            PlanNodeKind::ResourceScope { body, .. } => self.node(body, bindings),
        }
    }
}
pub(crate) fn reject_literal_secrets(
    envelope: &PlanEnvelope,
    project: &Project,
) -> Result<(), AppError> {
    let policy = Policy {
        fixtures: &envelope.fixtures,
        json: &project.config.redaction.json_fields,
        headers: &project.config.redaction.headers,
    };
    for test in &envelope.tests {
        policy.node(&test.body, &mut HashMap::new())?;
    }
    for fixture in &envelope.fixtures {
        let mut locals = HashMap::new();
        policy.node(&fixture.setup, &mut locals)?;
        policy.node(&fixture.teardown, &mut locals)?;
    }
    Ok(())
}
fn secret_plan_error(provider: &str, operation: &str, argument: &str) -> AppError {
    AppError::usage(format!(
        "cannot emit a plan containing a literal secret in `{provider}.{operation}` argument `{argument}`; use a late-bound secret source"
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    use webtest_model::{FunctionId, Type, Value};
    use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange};
    #[test]
    fn pure_call_provenance_uses_an_isolated_parameter_environment() {
        let policy = Policy {
            fixtures: &[],
            json: &["password".into()],
            headers: &["authorization".into()],
        };
        let origin = SyntaxOrigin::new(FileId::new(0), TextRange::default());
        let expression = PlanExpr::PureCall {
            metadata: Box::new(webtest_plan::PureCallMetadata {
                function: FunctionId::default(),
                name: "pure".into(),
                parameters: Vec::new(),
                result_type: webtest_model::Type::Json,
                body_origin: origin,
                declaration: origin,
                declaration_revision: SourceRevision::of(""),
                call: origin,
                call_revision: SourceRevision::of(""),
            }),
            arguments: vec![(BindingId(0), PlanExpr::Binding(BindingId(0)))],
            body: Box::new(PlanExpr::Record(
                [("password".into(), PlanExpr::Binding(BindingId(0)))].into(),
            )),
        };
        let value = policy.expression(
            &expression,
            &HashMap::from([(
                BindingId(0),
                Provenance {
                    literal: true,
                    ..Default::default()
                },
            )]),
        );
        assert!(value.literal && value.json);
        let value = policy.expression(&expression, &HashMap::new());
        assert!(!value.literal && !value.json);
        let value = policy.expression(
            &PlanExpr::Decode {
                value: Box::new(PlanExpr::Literal(Value::String("secret".into()))),
                target: Type::String,
                response_operation: None,
            },
            &HashMap::new(),
        );
        assert!(value.literal);
    }
}
