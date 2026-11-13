//! Resolved helper specialization through the same expression/type compiler.
use super::{BindingState, Compiler, TypedExpr};
use crate::callables::{self, Callable};
use std::collections::{HashMap, HashSet};
use webtest_hir::{HirExpr, HirExprKind, HirNameRef};
use webtest_model::{Capability, SymbolId, Type};
use webtest_plan::PlanExpr;

impl Compiler<'_> {
    pub(super) fn validate_expression_helpers(&mut self, hir: &webtest_hir::HirFile) {
        for function in &hir.functions {
            self.validating_declaration = true;
            self.active_calls
                .push((SymbolId(function.id.0), function.name_origin));
            self.bindings.clear();
            self.names.clear();
            self.declared_names.clear();
            for parameter in &function.parameters {
                let ty = self.lower_type(&parameter.annotation);
                self.bindings.insert(
                    parameter.id,
                    BindingState {
                        name: parameter.name.clone(),
                        ty,
                        domain: Capability::Pure,
                        provider_operation: None,
                    },
                );
                self.names
                    .insert(parameter.name.clone(), parameter.name_origin);
                self.declared_names.insert(parameter.name.clone());
            }
            let expected = self.lower_type(&function.result_type);
            if let Some(expression) = &function.expression {
                let body = self.infer_expr(expression, Capability::Pure, Some(&expected));
                self.expect_type(expression.origin.range, &expected, &body.ty);
            } else {
                for statement in &function.body {
                    super::statements::collect_binding_names(statement, &mut self.declared_names);
                }
                self.provide_is_return = true;
                self.provide_types = Some(Vec::new());
                let _body = self.compile_sequence(
                    webtest_model::PlanDeclarationId(function.id.0),
                    function.origin,
                    &function.body,
                    function.capability.unwrap_or(Capability::Pure),
                    Vec::new(),
                );
                let returned = self.provide_types.take().unwrap_or_default();
                if returned.is_empty() && expected != Type::Null {
                    self.error(
                        function.name_origin.range,
                        "semantic.missing_helper_result",
                        "helper must provide its declared result".into(),
                    );
                }
                for (actual, origin) in returned {
                    self.expect_type(origin.range, &expected, &actual);
                }
                self.provide_is_return = false;
            }
            self.active_calls.clear();
            self.validating_declaration = false;
        }
        self.bindings.clear();
        self.names.clear();
        self.declared_names.clear();
        self.next_step = 0;
        self.next_temporary = u32::MAX;
        self.required.clear();
        self.test_required.clear();
        self.specialization_budget = 100_000;
    }
    pub(super) fn pure_helper(
        &mut self,
        expression: &HirExpr,
        domain: Capability,
    ) -> Option<TypedExpr> {
        let HirExprKind::Call { callee, arguments } = &expression.kind else {
            return None;
        };
        let HirExprKind::Name(HirNameRef::Unresolved(name)) = &callee.kind else {
            return None;
        };
        let shadowed = if let (Some((_, graph)), Some(module)) = (self.workspace, self.module) {
            callables::resolve(graph, module, name).is_some()
        } else {
            self.local_hir.is_some_and(|hir| {
                hir.functions.iter().any(|function| function.name == *name)
                    || hir.fixtures.iter().any(|fixture| fixture.name == *name)
            })
        };
        if let Some(builtin) = callables::builtin(name).filter(|_| !shadowed) {
            if arguments.len() != 1
                || arguments
                    .first()
                    .is_some_and(|arg| arg.name.as_deref().is_some_and(|name| name != "value"))
            {
                self.error(
                    expression.origin.range,
                    "semantic.invalid_arguments",
                    "lower requires one String argument `value`".into(),
                );
            }
            let value = arguments
                .first()
                .map(|arg| self.infer_expr(&arg.value, domain, Some(&builtin.parameters[0].1)))
                .unwrap_or_else(super::type_system::unknown_expr);
            self.expect_type(expression.origin.range, &builtin.parameters[0].1, &value.ty);
            return Some(TypedExpr {
                expression: PlanExpr::Lower {
                    value: Box::new(value.expression),
                },
                ty: builtin.result.clone(),
                capability: Capability::Pure,
            });
        }
        let (module, function) = if let (Some((_, graph)), Some(module)) =
            (self.workspace, self.module)
        {
            let (target, symbol) = callables::resolve(graph, module, name)?;
            let Callable::Function(function) = callables::find(&graph.modules[&target], symbol)?
            else {
                return None;
            };
            (Some(target), function.clone())
        } else {
            (
                None,
                self.local_hir?
                    .functions
                    .iter()
                    .find(|function| function.name == *name)?
                    .clone(),
            )
        };
        let id = SymbolId(function.id.0);
        if self.active_calls.iter().any(|(active, _)| *active == id)
            || self.active_calls.len() >= 128
            || self.specialization_budget == 0
        {
            self.error(
                expression.origin.range,
                "semantic.recursive_helper",
                "helper calls must be acyclic and fit the bounded specialization budget".into(),
            );
            return Some(super::type_system::unknown_expr());
        }
        self.specialization_budget -= 1;
        let Some(body) = function.expression.as_ref() else {
            self.error(
                expression.origin.range,
                "semantic.statement_helper_expression",
                "statement helpers must be called as a direct binding value or statement".into(),
            );
            return Some(super::type_system::unknown_expr());
        };
        if let (Some((db, graph)), Some(module)) = (self.workspace, module) {
            let source = db.file_slot(&graph.modules[&module].path).target(db)?;
            let signature = callables::signature(db, source, id)?;
            if signature.capability != Capability::Pure {
                self.error(
                    expression.origin.range,
                    "semantic.effectful_expression",
                    "expression helpers must be pure".into(),
                );
            }
        }
        let mut compiled = Vec::new();
        let mut supplied = HashSet::new();
        let mut positional = 0;
        for argument in arguments {
            let parameter = if let Some(name) = &argument.name {
                function.parameters.iter().find(|p| p.name == *name)
            } else {
                let parameter = function.parameters.get(positional);
                positional += 1;
                parameter
            };
            let Some(parameter) = parameter else {
                self.error(
                    argument.origin.range,
                    "semantic.unknown_argument",
                    "unknown helper argument".into(),
                );
                self.infer_expr(&argument.value, domain, None);
                continue;
            };
            if !supplied.insert(parameter.id) {
                self.error(
                    argument.origin.range,
                    "semantic.duplicate_argument",
                    format!("argument `{}` is supplied more than once", parameter.name),
                );
            }
            let expected = self.lower_type(&parameter.annotation);
            let value = self.infer_expr(&argument.value, domain, Some(&expected));
            self.expect_type(argument.value.origin.range, &expected, &value.ty);
            compiled.push((parameter.id, value.expression));
        }
        for parameter in &function.parameters {
            if !supplied.contains(&parameter.id) {
                self.error(
                    expression.origin.range,
                    "semantic.missing_argument",
                    format!("missing required argument `{}`", parameter.name),
                );
            }
        }
        let bindings = std::mem::take(&mut self.bindings);
        let names = std::mem::take(&mut self.names);
        let declared = std::mem::take(&mut self.declared_names);
        let old_module = self.module;
        let old_revision = self.revision;
        let captures = std::mem::take(&mut self.concurrent_captures);
        let retry_captures = std::mem::take(&mut self.retry_captures);
        self.module = module;
        if let Some((_, graph)) = self.workspace
            && let Some(module) = module
        {
            self.revision = graph.modules[&module].revision;
        }
        self.active_calls.push((id, expression.origin));
        self.names = HashMap::new();
        for parameter in &function.parameters {
            let ty = self.lower_type(&parameter.annotation);
            self.bindings.insert(
                parameter.id,
                BindingState {
                    name: parameter.name.clone(),
                    ty,
                    domain: Capability::Pure,
                    provider_operation: None,
                },
            );
            self.names
                .insert(parameter.name.clone(), parameter.name_origin);
            self.declared_names.insert(parameter.name.clone());
        }
        let expected = self.lower_type(&function.result_type);
        let body = self.infer_expr(body, Capability::Pure, Some(&expected));
        self.expect_type(function.result_type.origin.range, &expected, &body.ty);
        self.active_calls.pop();
        self.module = old_module;
        self.revision = old_revision;
        self.concurrent_captures = captures;
        self.retry_captures = retry_captures;
        self.bindings = bindings;
        self.names = names;
        self.declared_names = declared;
        self.type_fact(expression.origin.range, expected.clone(), Capability::Pure);
        Some(TypedExpr {
            expression: PlanExpr::PureCall {
                metadata: Box::new(webtest_plan::PureCallMetadata {
                    function: function.id,
                    name: function.name.clone(),
                    parameters: function
                        .parameters
                        .iter()
                        .map(|parameter| webtest_plan::HelperParameter {
                            binding: parameter.id,
                            name: parameter.name.clone(),
                            ty: self.lower_type(&parameter.annotation),
                        })
                        .collect(),
                    result_type: expected.clone(),
                    body_origin: function
                        .expression
                        .as_ref()
                        .map_or(function.origin, |body| body.origin),
                    declaration: function.origin,
                    declaration_revision: self
                        .workspace
                        .and_then(|(_, graph)| module.and_then(|module| graph.modules.get(&module)))
                        .map_or(self.revision, |module| module.revision),
                    call: expression.origin,
                    call_revision: self.revision,
                }),
                arguments: compiled,
                body: Box::new(body.expression),
            },
            ty: expected,
            capability: Capability::Pure,
        })
    }
}

impl Compiler<'_> {
    pub(super) fn statement_helper(
        &mut self,
        statement: &webtest_hir::HirStmt,
        domain: Capability,
        test: webtest_model::PlanDeclarationId,
        path: Vec<u32>,
    ) -> Option<webtest_plan::PlanNode> {
        use webtest_hir::HirStmt;
        use webtest_plan::{
            EvaluatePureOperation, HelperArgument, HelperCall, PlanNode, RaceBinding, TestOperation,
        };
        let (expression, binding) = match statement {
            HirStmt::Let(binding) => (&binding.value, Some(binding)),
            HirStmt::Expression(value) => (&value.expression, None),
            _ => return None,
        };
        let HirExprKind::Call { callee, arguments } = &expression.kind else {
            return None;
        };
        let HirExprKind::Name(HirNameRef::Unresolved(name)) = &callee.kind else {
            return None;
        };
        let (module, function, revision) = if let (Some((_, graph)), Some(module)) =
            (self.workspace, self.module)
        {
            let (target, symbol) = callables::resolve(graph, module, name)?;
            let Callable::Function(function) = callables::find(&graph.modules[&target], symbol)?
            else {
                return None;
            };
            (
                Some(target),
                function.clone(),
                graph.modules[&target].revision,
            )
        } else {
            (
                None,
                self.local_hir?
                    .functions
                    .iter()
                    .find(|function| function.name == *name)?
                    .clone(),
                self.revision,
            )
        };
        if function.expression.is_some() {
            return None;
        }
        let id = SymbolId(function.id.0);
        if self.active_calls.iter().any(|(active, _)| *active == id)
            || self.active_calls.len() >= 64
            || self.specialization_budget == 0
        {
            self.error(
                expression.origin.range,
                "semantic.recursive_helper",
                "helper calls must be acyclic and fit the bounded specialization budget".into(),
            );
            return Some(PlanNode::sequence(
                test,
                expression.origin,
                self.revision,
                path,
                Vec::new(),
            ));
        }
        self.specialization_budget -= 1;
        if let (Some((db, graph)), Some(module)) = (self.workspace, module) {
            let source = db.file_slot(&graph.modules[&module].path).target(db)?;
            if let Some(signature) = callables::signature(db, source, id)
                && domain != Capability::Pure
                && signature.capability != Capability::Pure
                && domain != signature.capability
            {
                self.error(
                    expression.origin.range,
                    "semantic.capability_mismatch",
                    format!(
                        "helper `{name}` requires {} capability but is used in {domain} context",
                        signature.capability
                    ),
                );
            }
        }
        let mut children = Vec::new();
        let mut call_arguments = Vec::new();
        let mut supplied = HashSet::new();
        let mut positional = 0;
        for argument in arguments {
            let parameter = if let Some(name) = &argument.name {
                function.parameters.iter().find(|p| p.name == *name)
            } else {
                let parameter = function.parameters.get(positional);
                positional += 1;
                parameter
            };
            let Some(parameter) = parameter else {
                self.error(
                    argument.origin.range,
                    "semantic.unknown_argument",
                    "unknown helper argument".into(),
                );
                self.infer_expr(&argument.value, domain, None);
                continue;
            };
            if !supplied.insert(parameter.id) {
                self.error(
                    argument.origin.range,
                    "semantic.duplicate_argument",
                    format!("argument `{}` is supplied more than once", parameter.name),
                );
            }
            let expected = self.lower_type(&parameter.annotation);
            let value = self.infer_expr(&argument.value, domain, Some(&expected));
            self.expect_type(argument.value.origin.range, &expected, &value.ty);
            let temporary = webtest_model::BindingId(self.next_temporary);
            self.next_temporary -= 1;
            let mut steps = Vec::new();
            self.push_step(
                &mut steps,
                argument.value.origin,
                TestOperation::EvaluatePure(EvaluatePureOperation {
                    expression: value.expression,
                    result_binding: Some(temporary),
                    result_name: None,
                    result_type: expected,
                }),
            );
            let mut arg_path = path.clone();
            arg_path.push(children.len() as u32);
            if let Some(step) = steps.pop() {
                children.push(PlanNode::operation(test, self.revision, arg_path, step));
            }
            call_arguments.push(HelperArgument {
                parameter: parameter.id,
                name: parameter.name.clone(),
                value: PlanExpr::Binding(temporary),
            });
        }
        for parameter in &function.parameters {
            if !supplied.contains(&parameter.id) {
                self.error(
                    expression.origin.range,
                    "semantic.missing_argument",
                    format!("missing required argument `{}`", parameter.name),
                );
            }
        }
        let result_type = self.lower_type(&function.result_type);
        let outer_bindings = std::mem::take(&mut self.bindings);
        let outer_names = std::mem::take(&mut self.names);
        let outer_declared = std::mem::take(&mut self.declared_names);
        let captures = std::mem::take(&mut self.concurrent_captures);
        let retry_captures = std::mem::take(&mut self.retry_captures);
        let provides = self.provide_types.replace(Vec::new());
        let is_return = std::mem::replace(&mut self.provide_is_return, true);
        let return_boundary =
            std::mem::replace(&mut self.return_boundary_depth, self.resource_depth);
        let old_module = self.module;
        let old_revision = self.revision;
        self.module = module;
        self.revision = revision;
        self.active_calls.push((id, expression.origin));
        for parameter in &function.parameters {
            let ty = self.lower_type(&parameter.annotation);
            self.bindings.insert(
                parameter.id,
                BindingState {
                    name: parameter.name.clone(),
                    ty,
                    domain: Capability::Pure,
                    provider_operation: None,
                },
            );
            self.names
                .insert(parameter.name.clone(), parameter.name_origin);
            self.declared_names.insert(parameter.name.clone());
        }
        for statement in &function.body {
            super::statements::collect_binding_names(statement, &mut self.declared_names);
        }
        let mut call_path = path.clone();
        call_path.push(children.len() as u32);
        let mut body_path = call_path.clone();
        body_path.push(0);
        let body = self.compile_sequence(
            test,
            function.origin,
            &function.body,
            function.capability.unwrap_or(Capability::Pure),
            body_path,
        );
        let returned = self.provide_types.take().unwrap_or_default();
        if returned.is_empty() && result_type != Type::Null {
            self.error(
                function.name_origin.range,
                "semantic.missing_helper_result",
                "helper must provide its declared result".into(),
            );
        }
        for (actual, origin) in returned {
            self.expect_type(origin.range, &result_type, &actual);
        }
        self.active_calls.pop();
        self.module = old_module;
        self.revision = old_revision;
        self.bindings = outer_bindings;
        self.names = outer_names;
        self.declared_names = outer_declared;
        self.concurrent_captures = captures;
        self.retry_captures = retry_captures;
        self.provide_types = provides;
        self.provide_is_return = is_return;
        self.return_boundary_depth = return_boundary;
        let mut decode_binding = None;
        let result = if let Some(binding) = binding {
            if self.names.contains_key(&binding.name) {
                self.error(
                    binding.name_origin.range,
                    "semantic.duplicate_binding",
                    format!("binding `{}` is already declared", binding.name),
                );
            }
            let ty = binding
                .annotation
                .as_ref()
                .map(|ty| self.lower_type(ty))
                .unwrap_or_else(|| result_type.clone());
            if result_type == Type::Json && super::type_system::decodable_type(&ty) {
                decode_binding = Some((binding.id, binding.name.clone(), ty.clone()));
            } else {
                self.expect_type(binding.value.origin.range, &ty, &result_type);
            }
            self.bindings.insert(
                binding.id,
                BindingState {
                    name: binding.name.clone(),
                    ty: ty.clone(),
                    domain,
                    provider_operation: None,
                },
            );
            self.names.insert(binding.name.clone(), binding.name_origin);
            self.type_fact(binding.name_origin.range, ty, domain);
            Some(RaceBinding {
                id: binding.id,
                name: binding.name.clone(),
                ty: result_type.clone(),
            })
        } else {
            None
        };
        children.push(PlanNode::helper(
            test,
            expression.origin,
            self.revision,
            call_path,
            HelperCall {
                function: function.id,
                name: function.name,
                declaration: function.origin,
                declaration_revision: revision,
                arguments: call_arguments,
                result,
                result_type,
            },
            body,
        ));
        if let Some((id, name, ty)) = decode_binding {
            let mut steps = Vec::new();
            self.push_step(
                &mut steps,
                expression.origin,
                TestOperation::EvaluatePure(EvaluatePureOperation {
                    expression: PlanExpr::Decode {
                        value: Box::new(PlanExpr::Binding(id)),
                        target: ty.clone(),
                        response_operation: None,
                    },
                    result_binding: Some(id),
                    result_name: Some(name),
                    result_type: ty,
                }),
            );
            let mut child_path = path.clone();
            child_path.push(children.len() as u32);
            if let Some(step) = steps.pop() {
                children.push(PlanNode::operation(test, self.revision, child_path, step));
            }
        }
        Some(PlanNode::sequence(
            test,
            statement.origin(),
            self.revision,
            path,
            children,
        ))
    }
}
