//! Scope traversal, bindings, expectations, and ordered step construction.

use std::collections::HashSet;

use super::{BindingState, Compiler};
use webtest_hir::{HirExpr, HirExprKind, HirNameRef, HirStmt};
use webtest_model::{BinaryOperator, Capability, StepId, Type};
use webtest_plan::{
    AssertionOperation, EvaluatePureOperation, PlanExpr, PlannedStep, ServerProviderCall,
    TestOperation, ValueMatcher,
};
use webtest_text::SyntaxOrigin;

struct ConcurrentInput<'a> {
    branches: &'a [HirStmt],
    origin: SyntaxOrigin,
    race: bool,
    binding: Option<&'a webtest_hir::HirResultBinding>,
}

impl Compiler<'_> {
    pub(super) fn compile_sequence(
        &mut self,
        test: webtest_model::PlanDeclarationId,
        origin: SyntaxOrigin,
        statements: &[HirStmt],
        domain: Capability,
        path: Vec<u32>,
    ) -> webtest_plan::PlanNode {
        let mut children = Vec::new();
        let mut provided = false;
        for (ordinal, statement) in statements.iter().enumerate() {
            let unreachable = provided;
            provided |= statement_provides(statement);
            let mut child_path = path.clone();
            child_path.push(ordinal as u32);
            let node = match statement {
                HirStmt::Parallel(block) => self.compile_concurrent(
                    test,
                    ConcurrentInput {
                        branches: &block.branches,
                        origin: block.origin,
                        race: false,
                        binding: None,
                    },
                    domain,
                    child_path,
                ),
                HirStmt::Race(block) => self.compile_concurrent(
                    test,
                    ConcurrentInput {
                        branches: &block.branches,
                        origin: block.origin,
                        race: true,
                        binding: block.binding.as_ref(),
                    },
                    domain,
                    child_path,
                ),
                HirStmt::Timeout(block) => {
                    self.type_fact(
                        block.duration_origin.range,
                        Type::Duration,
                        Capability::Pure,
                    );
                    let duration = block.duration.filter(|duration| {
                        !duration.is_zero() && *duration <= webtest_plan::MAX_CONTROL_TIMEOUT
                    });
                    if duration.is_none() {
                        self.error(
                            block.duration_origin.range,
                            "semantic.invalid_timeout",
                            "timeout must be positive and no greater than 24 hours".into(),
                        );
                    }
                    let bindings = self.bindings.clone();
                    let names = self.names.clone();
                    let mut body_path = child_path.clone();
                    body_path.push(0);
                    let body = self.compile_sequence(
                        test,
                        block.body_origin,
                        &block.statements,
                        domain,
                        body_path,
                    );
                    self.bindings = bindings;
                    self.names = names;
                    webtest_plan::PlanNode::timeout(
                        test,
                        block.origin,
                        self.revision,
                        child_path,
                        body,
                        duration.unwrap_or(std::time::Duration::from_millis(1)),
                    )
                }
                HirStmt::Server(block) => self.compile_sequence(
                    test,
                    block.origin,
                    &block.statements,
                    Capability::Server,
                    child_path,
                ),
                HirStmt::Browser(block) => {
                    let body = self.compile_sequence(
                        test,
                        block.origin,
                        &block.statements,
                        Capability::Browser,
                        child_path,
                    );
                    if self.concurrent_depth > 0 && !body.required_resources().is_empty() {
                        body.with_browser_resource(test)
                    } else {
                        body
                    }
                }
                _ => {
                    let mut steps = Vec::new();
                    self.compile_statement(statement, domain, &mut steps);
                    let Some(step) = steps.into_iter().next() else {
                        continue;
                    };
                    webtest_plan::PlanNode::operation(test, self.revision, child_path, step)
                }
            };
            if unreachable {
                self.error(
                    node.origin.range,
                    "semantic.unreachable_after_provide",
                    "provide must be the final statement of its race branch".into(),
                );
            }
            children.push(node);
        }
        webtest_plan::PlanNode::sequence(test, origin, self.revision, path, children)
    }

    fn compile_concurrent(
        &mut self,
        test: webtest_model::PlanDeclarationId,
        block: ConcurrentInput<'_>,
        domain: Capability,
        path: Vec<u32>,
    ) -> webtest_plan::PlanNode {
        if block.branches.is_empty() || block.branches.len() > webtest_plan::MAX_PARALLEL_BRANCHES {
            self.error(
                block.origin.range,
                if block.race {
                    "semantic.invalid_race"
                } else {
                    "semantic.invalid_parallel"
                },
                format!(
                    "{} requires 1 to {} direct child blocks",
                    if block.race { "race" } else { "parallel" },
                    webtest_plan::MAX_PARALLEL_BRANCHES
                ),
            );
        }
        let outer_provides = self.provide_types.take();
        let annotation = block
            .binding
            .and_then(|binding| binding.annotation.as_ref())
            .map(|ty| self.lower_type(ty));
        let mut result_types = Vec::new();
        let bindings = self.bindings.clone();
        let names = self.names.clone();
        let outer_captures = self.concurrent_captures.clone();
        self.concurrent_captures.extend(
            bindings
                .iter()
                .filter_map(|(id, binding)| (!binding.ty.is_transferable()).then_some(*id)),
        );
        self.concurrent_depth += 1;
        let mut children = Vec::new();
        for (ordinal, branch) in block.branches.iter().enumerate() {
            self.provide_types = block.race.then(Vec::new);
            self.bindings = bindings.clone();
            self.names = names.clone();
            let origin = match branch {
                HirStmt::Server(block) => block.origin,
                HirStmt::Browser(block) => block.origin,
                HirStmt::Timeout(block) => block.origin,
                HirStmt::Parallel(block) => block.origin,
                HirStmt::Race(block) => block.origin,
                _ => {
                    self.error(
                        block.origin.range,
                        if block.race {
                            "semantic.expected_race_block"
                        } else {
                            "semantic.expected_parallel_block"
                        },
                        "each concurrent branch must be a capability or control block".into(),
                    );
                    continue;
                }
            };
            let mut child_path = path.clone();
            child_path.push(ordinal as u32);
            let child = self.compile_sequence(
                test,
                origin,
                std::slice::from_ref(branch),
                domain,
                child_path,
            );
            if !child.required_resources().is_empty() && domain == Capability::Browser {
                self.error(origin.range, "semantic.concurrent_resource_conflict", "concurrent browser operations require a lexical browser block in a flow domain; the enclosing browser context is exclusive".into());
            }
            let provides = self.provide_types.take().unwrap_or_default();
            if block.binding.is_some() && provides.is_empty() {
                self.error(
                    origin.range,
                    "semantic.missing_race_result",
                    "every branch of a bound race must provide a result".into(),
                );
            }
            result_types.extend(provides);
            children.push(child);
        }
        self.provide_types = outer_provides;
        self.bindings = bindings;
        self.names = names;
        self.concurrent_captures = outer_captures;
        self.concurrent_depth -= 1;
        let uses = children
            .iter()
            .map(webtest_plan::PlanNode::required_resources)
            .collect::<Vec<_>>();
        if !webtest_plan::conflicting_resource_accesses(&uses).is_empty() {
            self.error(
                block.origin.range,
                "semantic.concurrent_resource_conflict",
                "concurrent branches cannot share an exclusive resource".into(),
            );
        }
        if !block.race {
            return webtest_plan::PlanNode::parallel(
                test,
                block.origin,
                self.revision,
                path,
                children,
            );
        }
        let mut node =
            webtest_plan::PlanNode::race(test, block.origin, self.revision, path, children);
        if let Some(binding) = block.binding {
            let mut ty = annotation.clone().unwrap_or(Type::Unknown);
            for (actual, origin) in &result_types {
                if ty == Type::Unknown {
                    ty = actual.clone();
                } else if !ty.accepts(actual) {
                    if let Some(joined) = annotation
                        .is_none()
                        .then(|| compatible_result_type(&ty, actual))
                        .flatten()
                    {
                        ty = joined;
                    } else {
                        self.type_mismatch(origin.range, &ty, actual);
                    }
                }
            }
            if !ty.is_transferable() {
                self.error(
                    binding.name_origin.range,
                    "semantic.non_transferable_race_result",
                    "a race binding must have a transferable result type".into(),
                );
            }
            if self.names.contains_key(&binding.name) {
                self.error(
                    binding.name_origin.range,
                    "semantic.duplicate_binding",
                    format!("binding `{}` is already declared", binding.name),
                );
            }
            self.names.insert(binding.name.clone(), binding.name_origin);
            self.bindings.insert(
                binding.id,
                BindingState {
                    name: binding.name.clone(),
                    ty: ty.clone(),
                    domain,
                    provider_operation: None,
                },
            );
            self.type_fact(binding.name_origin.range, ty.clone(), domain);
            if let webtest_plan::PlanNodeKind::Race { result, .. } = &mut node.kind {
                *result = Some(webtest_plan::RaceBinding {
                    id: binding.id,
                    name: binding.name.clone(),
                    ty,
                });
            }
        }
        node
    }

    pub(super) fn compile_statement(
        &mut self,
        statement: &HirStmt,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        match statement {
            HirStmt::Timeout(_) | HirStmt::Parallel(_) | HirStmt::Race(_) => {
                unreachable!("control nodes compile through the execution tree")
            }
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
            HirStmt::Provide(statement) => {
                let value = self.infer_expr(&statement.expression, domain, None);
                if !value.ty.is_transferable() {
                    self.error(
                        statement.expression.origin.range,
                        "semantic.non_transferable_race_result",
                        "race results must be transferable values".into(),
                    );
                }
                if let Some(provides) = &mut self.provide_types {
                    provides.push((value.ty.clone(), statement.expression.origin));
                } else {
                    self.error(
                        statement.origin.range,
                        "semantic.provide_outside_race",
                        "provide is only valid inside a race branch".into(),
                    );
                }
                self.push_step(
                    steps,
                    statement.expression.origin,
                    TestOperation::Provide(webtest_plan::ProvideOperation {
                        expression: value.expression,
                        result_type: value.ty,
                    }),
                );
            }
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
        HirStmt::Race(block) => {
            if let Some(binding) = &block.binding {
                names.insert(binding.name.clone());
            }
            for statement in &block.branches {
                collect_binding_names(statement, names);
            }
        }
        HirStmt::Parallel(block) => {
            for statement in &block.branches {
                collect_binding_names(statement, names);
            }
        }
        HirStmt::Timeout(block) => {
            for statement in &block.statements {
                collect_binding_names(statement, names);
            }
        }
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
        HirStmt::Provide(_)
        | HirStmt::Expression(_)
        | HirStmt::Expect(_)
        | HirStmt::BrowserOperation(_) => {}
    }
}

fn statement_provides(statement: &HirStmt) -> bool {
    match statement {
        HirStmt::Provide(_) => true,
        HirStmt::Server(block) => block.statements.iter().any(statement_provides),
        HirStmt::Browser(block) => block.statements.iter().any(statement_provides),
        HirStmt::Timeout(block) => block.statements.iter().any(statement_provides),
        _ => false,
    }
}

fn compatible_result_type(left: &Type, right: &Type) -> Option<Type> {
    if left.accepts(right) {
        return Some(left.clone());
    }
    if right.accepts(left) {
        return Some(right.clone());
    }
    match (left, right) {
        (Type::Null, ty) | (ty, Type::Null) => Some(Type::Option(Box::new(ty.clone()))),
        (Type::Option(left), Type::Option(right)) => {
            compatible_result_type(left, right).map(|ty| Type::Option(Box::new(ty)))
        }
        (Type::List(left), Type::List(right)) => {
            compatible_result_type(left, right).map(|ty| Type::List(Box::new(ty)))
        }
        (Type::Option(inner), other) | (other, Type::Option(inner)) => {
            compatible_result_type(inner, other).map(|ty| Type::Option(Box::new(ty)))
        }
        _ => None,
    }
}
