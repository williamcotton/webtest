//! Scope traversal, bindings, expectations, and ordered step construction.

use std::collections::HashSet;

use super::{BindingState, Compiler};
use webtest_hir::{BinaryOperator, HirExpr, HirExprKind, HirNameRef, HirStmt, StepId};
use webtest_plan::{
    AssertionOperation, EvaluatePureOperation, PlanExpr, PlannedStep, ServerProviderCall,
    TestOperation, ValueMatcher,
};
use webtest_provider::{Capability, Type};
use webtest_text::SyntaxOrigin;

impl Compiler<'_> {
    pub(super) fn compile_statement(
        &mut self,
        statement: &HirStmt,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        match statement {
            HirStmt::Server(block) => {
                for statement in &block.statements {
                    self.compile_statement(statement, Capability::Server, steps);
                }
            }
            HirStmt::Browser(block) => {
                for statement in &block.statements {
                    self.compile_statement(statement, Capability::Browser, steps);
                }
            }
            HirStmt::Let(binding) => self.compile_let(binding, domain, steps),
            HirStmt::Expression(statement) => {
                if let Some(call) = self.provider_call(&statement.expression, domain) {
                    self.push_step(
                        steps,
                        statement.expression.origin,
                        TestOperation::ServerProviderCall(ServerProviderCall {
                            provider: call.provider,
                            operation: call.operation,
                            arguments: call.arguments,
                            result_binding: None,
                            result_name: None,
                            result_type: call.result_type,
                            schema_hash: call.schema_hash,
                            timeout: None,
                            redacted_arguments: call.redacted_arguments,
                            redacted_result_fields: call.redacted_result_fields,
                            retry_safe: call.retry_safe,
                        }),
                    );
                } else {
                    let value = self.infer_expr(&statement.expression, domain, None);
                    self.push_step(
                        steps,
                        statement.expression.origin,
                        TestOperation::EvaluatePure(EvaluatePureOperation {
                            expression: value.expression,
                            result_binding: None,
                            result_name: None,
                            result_type: value.ty,
                        }),
                    );
                }
            }
            HirStmt::Expect(expectation) => {
                self.compile_expectation(&expectation.expression, domain, steps)
            }
            HirStmt::BrowserOperation(operation) => {
                self.compile_browser_operation(operation, domain, steps)
            }
        }
    }

    fn compile_let(
        &mut self,
        binding: &webtest_hir::HirLet,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        if let Some(previous) = self.names.get(&binding.name) {
            self.error(
                binding.name_origin.range,
                "semantic.duplicate_binding",
                format!(
                    "binding `{}` is already declared at byte {}",
                    binding.name,
                    u32::from(previous.range.start())
                ),
            );
        } else {
            self.names.insert(binding.name.clone(), binding.name_origin);
        }

        if let Some(call) = self.provider_call(&binding.value, domain) {
            let mut result_type = call.result_type.clone();
            if let Some(annotation) = &binding.annotation {
                let expected = self.lower_type(annotation);
                if !expected.accepts(&result_type) {
                    self.type_mismatch(binding.value.origin.range, &expected, &result_type);
                }
                result_type = expected;
            }
            self.bindings.insert(
                binding.id,
                BindingState {
                    name: binding.name.clone(),
                    ty: result_type.clone(),
                    domain,
                    provider_operation: Some(format!("{}.{}", call.provider, call.operation)),
                },
            );
            self.type_fact(binding.name_origin.range, result_type.clone(), domain);
            self.push_step(
                steps,
                binding.value.origin,
                TestOperation::ServerProviderCall(ServerProviderCall {
                    provider: call.provider,
                    operation: call.operation,
                    arguments: call.arguments,
                    result_binding: Some(binding.id),
                    result_name: Some(binding.name.clone()),
                    result_type,
                    schema_hash: call.schema_hash,
                    timeout: None,
                    redacted_arguments: call.redacted_arguments,
                    redacted_result_fields: call.redacted_result_fields,
                    retry_safe: call.retry_safe,
                }),
            );
            return;
        }

        let annotation = binding
            .annotation
            .as_ref()
            .map(|annotation| self.lower_type(annotation));
        let mut value = self.infer_expr(&binding.value, domain, annotation.as_ref());
        if let Some(expected) = annotation {
            if value.ty == Type::Json && super::type_system::decodable_type(&expected) {
                let response_operation = self.response_operation(&binding.value);
                value.expression = PlanExpr::Decode {
                    value: Box::new(value.expression),
                    target: expected.clone(),
                    response_operation,
                };
                value.ty = expected;
            } else if !expected.accepts(&value.ty) {
                self.type_mismatch(binding.value.origin.range, &expected, &value.ty);
                value.ty = expected;
            } else {
                value.ty = expected;
            }
        }
        self.bindings.insert(
            binding.id,
            BindingState {
                name: binding.name.clone(),
                ty: value.ty.clone(),
                domain,
                provider_operation: None,
            },
        );
        self.type_fact(
            binding.name_origin.range,
            value.ty.clone(),
            value.capability,
        );
        self.push_step(
            steps,
            binding.value.origin,
            TestOperation::EvaluatePure(EvaluatePureOperation {
                expression: value.expression,
                result_binding: Some(binding.id),
                result_name: Some(binding.name.clone()),
                result_type: value.ty,
            }),
        );
    }

    fn compile_expectation(
        &mut self,
        expression: &HirExpr,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        let (matcher, actual, expected, value_type) = if let HirExprKind::Binary {
            operator,
            left,
            right,
        } = &expression.kind
        {
            if *operator == BinaryOperator::Matches {
                let actual = self.infer_expr(left, domain, None);
                let pattern = self.pattern_type(right);
                if actual.ty != Type::Json && !matches!(actual.ty, Type::Record(_)) {
                    self.error(
                        left.origin.range,
                        "semantic.invalid_matcher",
                        format!("`matches` requires Json or a record, got {}", actual.ty),
                    );
                }
                (
                    ValueMatcher::Matches,
                    actual.expression,
                    Some(PlanExpr::Type(pattern.clone())),
                    pattern,
                )
            } else {
                let left = self.infer_expr(left, domain, None);
                let right = self.infer_expr(right, domain, Some(&left.ty));
                self.validate_binary(*operator, &left.ty, &right.ty, expression.origin.range);
                (
                    super::type_system::matcher_for(*operator).unwrap_or(ValueMatcher::Truthy),
                    left.expression,
                    Some(right.expression),
                    left.ty,
                )
            }
        } else {
            let value = self.infer_expr(expression, domain, Some(&Type::Bool));
            if value.ty != Type::Bool && value.ty != Type::Unknown {
                self.type_mismatch(expression.origin.range, &Type::Bool, &value.ty);
            }
            (ValueMatcher::Truthy, value.expression, None, Type::Bool)
        };
        self.record_capability(Capability::Test);
        self.push_step(
            steps,
            expression.origin,
            TestOperation::Assertion(AssertionOperation::Value {
                matcher,
                actual,
                expected,
                value_type,
            }),
        );
    }

    fn response_operation(&self, expression: &HirExpr) -> Option<String> {
        let HirExprKind::Member {
            receiver, member, ..
        } = &expression.kind
        else {
            return None;
        };
        if member != "json" {
            return None;
        }
        let HirExprKind::Name(HirNameRef::Binding { id, .. }) = receiver.kind else {
            return None;
        };
        self.bindings
            .get(&id)
            .and_then(|binding| binding.provider_operation.clone())
    }

    pub(super) fn push_step(
        &mut self,
        steps: &mut Vec<PlannedStep>,
        origin: SyntaxOrigin,
        operation: TestOperation,
    ) {
        steps.push(PlannedStep {
            id: StepId(self.next_step),
            operation,
            origin,
        });
        self.next_step += 1;
    }
}

pub(super) fn collect_binding_names(statement: &HirStmt, names: &mut HashSet<String>) {
    match statement {
        HirStmt::Server(block) => {
            for statement in &block.statements {
                collect_binding_names(statement, names);
            }
        }
        HirStmt::Browser(block) => {
            for statement in &block.statements {
                collect_binding_names(statement, names);
            }
        }
        HirStmt::Let(binding) => {
            names.insert(binding.name.clone());
        }
        HirStmt::Expression(_) | HirStmt::Expect(_) | HirStmt::BrowserOperation(_) => {}
    }
}
