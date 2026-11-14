//! Pure specialization becomes ordinary helper scopes before leaving analysis.
//! Temporary evaluations retain left-to-right errors and lazy boolean operands.
use super::Compiler;
use std::collections::HashMap;
use webtest_model::{BinaryOperator as B, BindingId, PlanDeclarationId, RecordField, Type, Value};
use webtest_plan::{
    EvaluatePureOperation, HelperArgument, HelperCall, PlanExpr, PlanNode, PlannedStep,
    ProvideOperation, RaceBinding, TestOperation,
};
use webtest_text::{SourceRevision, SyntaxOrigin};

impl Compiler<'_> {
    pub(super) fn expand_pure_operation(
        &mut self,
        declaration: PlanDeclarationId,
        revision: SourceRevision,
        mut step: PlannedStep,
    ) -> PlanNode {
        if !step.operation.expressions().into_iter().any(contains_call) {
            return PlanNode::operation(declaration, revision, Vec::new(), step);
        }
        let mut nodes = Vec::new();
        let mut types = self
            .bindings
            .iter()
            .map(|(id, binding)| (*id, binding.ty.clone()))
            .collect();
        // All operands are materialized when one contains a call. An earlier
        // arithmetic/decode failure must precede a later helper safe point.
        for expression in step.operation.expressions_mut() {
            let expression_type = expression_type(expression, &types);
            let original = std::mem::replace(expression, PlanExpr::Literal(Value::Null));
            let lowered = self.expand_pure_expression(
                declaration,
                revision,
                step.origin,
                original,
                &mut nodes,
                &mut types,
                &step.origin_chain,
            );
            *expression = self.materialize(
                declaration,
                revision,
                step.origin,
                lowered,
                expression_type,
                &mut nodes,
                &mut types,
                &step.origin_chain,
            );
        }
        nodes.push(PlanNode::operation(
            declaration,
            revision,
            Vec::new(),
            step.clone(),
        ));
        PlanNode::sequence(declaration, step.origin, revision, Vec::new(), nodes)
    }
    fn temporary_binding(&mut self, ty: Type, types: &mut HashMap<BindingId, Type>) -> BindingId {
        let binding = BindingId(self.next_temporary);
        self.next_temporary -= 1;
        types.insert(binding, ty);
        binding
    }
    #[allow(clippy::too_many_arguments)]
    fn materialize(
        &mut self,
        declaration: PlanDeclarationId,
        revision: SourceRevision,
        origin: SyntaxOrigin,
        expression: PlanExpr,
        ty: Type,
        nodes: &mut Vec<PlanNode>,
        types: &mut HashMap<BindingId, Type>,
        chain: &[webtest_text::SourceLocation],
    ) -> PlanExpr {
        if matches!(expression, PlanExpr::Pattern(_) | PlanExpr::Type(_)) {
            return expression;
        }
        let binding = self.temporary_binding(ty.clone(), types);
        nodes.push(self.pure_operation(
            declaration,
            revision,
            origin,
            TestOperation::EvaluatePure(EvaluatePureOperation {
                expression,
                result_binding: Some(binding),
                result_name: None,
                result_type: ty,
            }),
            chain,
        ));
        PlanExpr::Binding(binding)
    }
    fn pure_operation(
        &mut self,
        declaration: PlanDeclarationId,
        revision: SourceRevision,
        origin: SyntaxOrigin,
        operation: TestOperation,
        chain: &[webtest_text::SourceLocation],
    ) -> PlanNode {
        let mut steps = Vec::new();
        self.push_step(&mut steps, origin, operation);
        let mut step = steps.remove(0);
        step.origin_chain = chain.to_vec();
        PlanNode::operation(declaration, revision, Vec::new(), step)
    }
    #[allow(clippy::too_many_arguments)]
    fn expand_pure_expression(
        &mut self,
        declaration_id: PlanDeclarationId,
        revision: SourceRevision,
        origin: SyntaxOrigin,
        mut expression: PlanExpr,
        nodes: &mut Vec<PlanNode>,
        types: &mut HashMap<BindingId, Type>,
        chain: &[webtest_text::SourceLocation],
    ) -> PlanExpr {
        if !contains_call(&expression) {
            return expression;
        }
        if let PlanExpr::PureCall {
            metadata,
            arguments,
            body,
        } = expression
        {
            let webtest_plan::PureCallMetadata {
                function,
                name,
                parameters,
                result_type,
                body_origin,
                declaration,
                declaration_revision,
                call,
                call_revision,
            } = *metadata;
            let mut compiled = Vec::new();
            for (parameter, expression) in arguments {
                let parameter_info = parameters.iter().find(|info| info.binding == parameter);
                let ty = parameter_info.map_or_else(
                    || expression_type(&expression, types),
                    |parameter| parameter.ty.clone(),
                );
                let expression = self.expand_pure_expression(
                    declaration_id,
                    revision,
                    call,
                    expression,
                    nodes,
                    types,
                    chain,
                );
                let expression = self.materialize(
                    declaration_id,
                    revision,
                    call,
                    expression,
                    ty,
                    nodes,
                    types,
                    chain,
                );
                compiled.push(HelperArgument {
                    parameter,
                    name: parameter_info
                        .map_or_else(String::new, |parameter| parameter.name.clone()),
                    value: expression,
                });
            }
            let result = self.temporary_binding(result_type.clone(), types);
            let mut locals = parameters
                .into_iter()
                .map(|parameter| (parameter.binding, parameter.ty))
                .collect::<HashMap<_, _>>();
            let mut body_nodes = Vec::new();
            let mut body_chain = chain.to_vec();
            if let Some(location) = self
                .workspace
                .and_then(|(_, graph)| graph.source_location(call))
            {
                body_chain.push(location);
            }
            let body = self.expand_pure_expression(
                declaration_id,
                declaration_revision,
                body_origin,
                *body,
                &mut body_nodes,
                &mut locals,
                &body_chain,
            );
            body_nodes.push(self.pure_operation(
                declaration_id,
                declaration_revision,
                body_origin,
                TestOperation::Return(ProvideOperation {
                    expression: body,
                    result_type: result_type.clone(),
                }),
                &body_chain,
            ));
            let body = PlanNode::sequence(
                declaration_id,
                declaration,
                declaration_revision,
                Vec::new(),
                body_nodes,
            );
            nodes.push(PlanNode::helper(
                declaration_id,
                call,
                call_revision,
                Vec::new(),
                HelperCall {
                    function,
                    name,
                    declaration,
                    declaration_revision,
                    arguments: compiled,
                    result: Some(RaceBinding {
                        id: result,
                        name: String::new(),
                        ty: result_type.clone(),
                    }),
                    result_type,
                },
                body,
            ));
            return PlanExpr::Binding(result);
        }
        if let PlanExpr::Binary {
            operator: operator @ (B::And | B::Or),
            left,
            right,
        } = expression
        {
            let left = self.expand_pure_expression(
                declaration_id,
                revision,
                origin,
                *left,
                nodes,
                types,
                chain,
            );
            let left = self.materialize(
                declaration_id,
                revision,
                origin,
                left,
                Type::Bool,
                nodes,
                types,
                chain,
            );
            let result = self.temporary_binding(Type::Bool, types);
            let mut right_nodes = Vec::new();
            let right = self.expand_pure_expression(
                declaration_id,
                revision,
                origin,
                *right,
                &mut right_nodes,
                types,
                chain,
            );
            right_nodes.push(self.pure_operation(
                declaration_id,
                revision,
                origin,
                TestOperation::EvaluatePure(EvaluatePureOperation {
                    expression: right,
                    result_binding: Some(result),
                    result_name: None,
                    result_type: Type::Bool,
                }),
                chain,
            ));
            let skipped = self.pure_operation(
                declaration_id,
                revision,
                origin,
                TestOperation::EvaluatePure(EvaluatePureOperation {
                    expression: PlanExpr::Literal(Value::Bool(operator == B::Or)),
                    result_binding: Some(result),
                    result_name: None,
                    result_type: Type::Bool,
                }),
                chain,
            );
            let right =
                PlanNode::sequence(declaration_id, origin, revision, Vec::new(), right_nodes);
            let branches = if operator == B::And {
                vec![right, skipped]
            } else {
                vec![skipped, right]
            };
            nodes.push(PlanNode::conditional(
                declaration_id,
                origin,
                revision,
                Vec::new(),
                left,
                branches,
            ));
            return PlanExpr::Binding(result);
        }
        for child in expression.children_mut() {
            let ty = expression_type(child, types);
            let original = std::mem::replace(child, PlanExpr::Literal(Value::Null));
            let lowered = self.expand_pure_expression(
                declaration_id,
                revision,
                origin,
                original,
                nodes,
                types,
                chain,
            );
            *child = self.materialize(
                declaration_id,
                revision,
                origin,
                lowered,
                ty,
                nodes,
                types,
                chain,
            );
        }
        expression
    }
}
fn contains_call(expression: &PlanExpr) -> bool {
    matches!(expression, PlanExpr::PureCall { .. })
        || expression.children().into_iter().any(contains_call)
}
fn expression_type(expression: &PlanExpr, types: &HashMap<BindingId, Type>) -> Type {
    match expression {
        PlanExpr::PureCall { metadata, .. } => metadata.result_type.clone(),
        PlanExpr::Binding(id) => types.get(id).cloned().unwrap_or(Type::Unknown),
        PlanExpr::Literal(value) => literal_type(value),
        PlanExpr::Pattern(_) => Type::Unknown,
        PlanExpr::Lower { .. } => Type::String,
        PlanExpr::Decode { target, .. } | PlanExpr::Type(target) => target.clone(),
        PlanExpr::List(values) => Type::List(Box::new(
            values
                .first()
                .map_or(Type::Unknown, |value| expression_type(value, types)),
        )),
        PlanExpr::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        RecordField {
                            ty: expression_type(value, types),
                            optional: false,
                            documentation: String::new(),
                            secret: false,
                        },
                    )
                })
                .collect(),
        ),
        PlanExpr::Member {
            receiver, member, ..
        } => expression_type(receiver, types)
            .member(member)
            .unwrap_or(Type::Unknown),
        PlanExpr::Unary {
            operator: webtest_model::UnaryOperator::Not,
            ..
        } => Type::Bool,
        PlanExpr::Unary { operand, .. } => expression_type(operand, types),
        PlanExpr::Binary {
            operator,
            left,
            right,
        } => match operator {
            B::Divide => Type::Float,
            B::And
            | B::Or
            | B::Equal
            | B::NotEqual
            | B::Less
            | B::LessEqual
            | B::Greater
            | B::GreaterEqual => Type::Bool,
            _ => {
                let left = expression_type(left, types);
                let right = expression_type(right, types);
                if left == Type::Float || right == Type::Float {
                    Type::Float
                } else {
                    left
                }
            }
        },
    }
}
fn literal_type(value: &Value) -> Type {
    match value {
        Value::Null => Type::Null,
        Value::Bool(_) => Type::Bool,
        Value::Int(_) => Type::Int,
        Value::Float(_) => Type::Float,
        Value::String(_) => Type::String,
        Value::DurationMillis(_) => Type::Duration,
        Value::List(values) => {
            Type::List(Box::new(values.first().map_or(Type::Unknown, literal_type)))
        }
        Value::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        RecordField {
                            ty: literal_type(value),
                            optional: false,
                            documentation: String::new(),
                            secret: false,
                        },
                    )
                })
                .collect(),
        ),
        Value::Headers(_) => Type::Headers,
        Value::Bytes(_) => Type::Bytes,
        Value::Response(_) => Type::Response(Box::new(Type::Json)),
        Value::ProcessResult(_) => Type::ProcessResult,
        Value::FilePath(_) => Type::FilePath,
        Value::TempDirectory(_) => Type::TempDirectory,
    }
}
