use super::*;
use webtest_hir::{HirExpr, HirExprKind, HirGuard, HirGuardScope, HirNameRef, HirStmt};
use webtest_model::{AlternativeId, Capability, EventSource, SymbolId, Type};
use webtest_plan::{
    PlanNode,
    semantics::{GuardClause, SelectAlternative, SemanticOperation},
};
impl Compiler<'_> {
    fn resolve_guard(&self, name: &str) -> Option<(Option<webtest_model::ModuleId>, HirGuard)> {
        if let (Some((_, graph)), Some(module)) = (self.workspace, self.module) {
            let module = graph.modules.get(&module)?;
            if let Some(guard) = module.hir.guards.iter().find(|g| g.name == name) {
                return Some((Some(module.id), guard.clone()));
            }
            let import = module.imports.get(name)?;
            let target = graph.modules.get(&import.target_module)?;
            return target
                .hir
                .guards
                .iter()
                .find(|g| g.id.0 == import.target.0)
                .cloned()
                .map(|g| (Some(target.id), g));
        }
        self.local_hir?
            .guards
            .iter()
            .find(|g| g.name == name)
            .cloned()
            .map(|g| (None, g))
    }
    pub(super) fn validate_guards(&mut self, hir: &webtest_hir::HirFile) {
        let bindings = self.bindings.clone();
        let names = self.names.clone();
        let required = self.required.clone();
        let test_required = self.test_required.clone();
        let step = self.next_step;
        let temporary = self.next_temporary;
        let budget = self.specialization_budget;
        for guard in &hir.guards {
            if guard.clauses.is_empty() || guard.clauses.len() > 64 {
                self.error(
                    guard.origin.range,
                    "semantic.invalid_guard_clauses",
                    "guard requires 1–64 event clauses".into(),
                );
            }
            let mut parameters = HashMap::new();
            for parameter in &guard.parameters {
                let ty = self.lower_type(&parameter.annotation);
                if ty != Type::BrowserActor && !ty.is_transferable() {
                    self.error(
                        parameter.origin.range,
                        "semantic.invalid_guard_parameter",
                        "guard parameters must be BrowserActor or transferable values".into(),
                    );
                }
                parameters.insert(
                    parameter.id,
                    BindingState {
                        name: parameter.name.clone(),
                        ty,
                        domain: Capability::Pure,
                        provider_operation: None,
                    },
                );
            }
            for clause in &guard.clauses {
                if !guard_body(&clause.statements) {
                    self.error(
                        clause.origin.range,
                        "semantic.guard_effect_not_allowed",
                        "guard bodies allow pure local bindings followed by fail or inconclusive"
                            .into(),
                    );
                    continue;
                }
                let Some((actor, source)) = guard_source(&clause.source) else {
                    self.error(
                        clause.source.origin.range,
                        "semantic.unknown_event_source",
                        "guard source must name an actor parameter and normalized event".into(),
                    );
                    continue;
                };
                if !parameters
                    .get(&actor)
                    .is_some_and(|p| p.ty == Type::BrowserActor)
                {
                    self.error(
                        clause.source.origin.range,
                        "semantic.expected_actor",
                        "guard event source requires a BrowserActor parameter".into(),
                    );
                }
                let Some(source) = EventSource::from_name(&source) else {
                    self.error(
                        clause.source.origin.range,
                        "semantic.unknown_event_source",
                        "unknown normalized browser event".into(),
                    );
                    continue;
                };
                let Some(event) = &clause.binding else {
                    self.error(
                        clause.source.origin.range,
                        "semantic.invalid_event_binding",
                        "event source requires one binding".into(),
                    );
                    continue;
                };
                self.bindings = parameters.clone();
                self.names = HashMap::new();
                self.bindings.insert(
                    event.id,
                    BindingState {
                        name: event.name.clone(),
                        ty: source.payload_type(),
                        domain: Capability::Pure,
                        provider_operation: None,
                    },
                );
                self.type_fact(
                    event.name_origin.range,
                    source.payload_type(),
                    Capability::Pure,
                );
                if let Some(filter) = &clause.filter {
                    let value = self.infer_expr(filter, Capability::Pure, Some(&Type::Bool));
                    self.expect_type(filter.origin.range, &Type::Bool, &value.ty);
                }
                let body = self.compile_sequence(
                    webtest_model::PlanDeclarationId(guard.id.0),
                    clause.origin,
                    &clause.statements,
                    Capability::Pure,
                    Vec::new(),
                );
                if !safe_guard_plan(&body) {
                    self.error(clause.origin.range,"semantic.guard_effect_not_allowed","guard bodies cannot invoke host operations, acquire resources, or execute effectful helpers".into());
                }
            }
        }
        self.bindings = bindings;
        self.names = names;
        self.required = required;
        self.test_required = test_required;
        self.next_step = step;
        self.next_temporary = temporary;
        self.specialization_budget = budget;
    }
    pub(super) fn compile_guards(
        &mut self,
        test: webtest_model::PlanDeclarationId,
        guarded: &HirGuardScope,
        domain: Capability,
        path: Vec<u32>,
    ) -> PlanNode {
        self.record_capability(Capability::Browser);
        let saved = self.bindings.clone();
        let names = self.names.clone();
        let body = self.compile_sequence(test, guarded.origin, &guarded.statements, domain, vec![]);
        self.bindings = saved.clone();
        self.names = names.clone();
        let mut children = vec![body];
        let mut clauses = Vec::new();
        let HirExprKind::List(activations) = &guarded.activations.kind else {
            self.error(
                guarded.activations.origin.range,
                "semantic.expected_guard_list",
                "with guards requires a static activation list".into(),
            );
            return PlanNode::sequence(test, guarded.origin, self.revision, path, children);
        };
        if activations.is_empty() || activations.len() > 64 {
            self.error(
                guarded.activations.origin.range,
                "semantic.invalid_guard_activations",
                "activate 1–64 guards".into(),
            );
        }
        for activation in activations {
            let HirExprKind::Call { callee, arguments } = &activation.kind else {
                self.error(
                    activation.origin.range,
                    "semantic.expected_guard_call",
                    "expected guard activation".into(),
                );
                continue;
            };
            let HirExprKind::Name(HirNameRef::Unresolved(name)) = &callee.kind else {
                self.error(
                    callee.origin.range,
                    "semantic.expected_guard_call",
                    "expected named guard".into(),
                );
                continue;
            };
            let Some((module, guard)) = self.resolve_guard(name) else {
                self.error(
                    callee.origin.range,
                    "semantic.unknown_guard",
                    format!("unknown guard `{name}`"),
                );
                continue;
            };
            let mut actors = HashMap::new();
            let mut values = HashMap::new();
            let mut types = HashMap::new();
            let mut supplied = HashSet::new();
            let mut positional = 0;
            for argument in arguments {
                let parameter = if let Some(name) = &argument.name {
                    guard.parameters.iter().find(|p| p.name == *name)
                } else {
                    let p = guard.parameters.get(positional);
                    positional += 1;
                    p
                };
                let Some(parameter) = parameter else {
                    self.error(
                        argument.origin.range,
                        "semantic.guard_arguments",
                        "unknown guard argument".into(),
                    );
                    continue;
                };
                if !supplied.insert(parameter.id) {
                    self.error(
                        argument.origin.range,
                        "semantic.guard_arguments",
                        "duplicate guard argument".into(),
                    );
                }
                let ty = self.lower_type(&parameter.annotation);
                if ty != Type::BrowserActor && !ty.is_transferable() {
                    self.error(
                        argument.origin.range,
                        "semantic.invalid_guard_parameter",
                        "guard parameters must be actors or transferable values".into(),
                    );
                }
                if ty == Type::BrowserActor {
                    if let Some(actor) = self.actor_reference(&argument.value) {
                        actors.insert(parameter.id, actor);
                    }
                } else {
                    let value = self.infer_expr(&argument.value, Capability::Pure, Some(&ty));
                    self.expect_type(argument.origin.range, &ty, &value.ty);
                    values.insert(parameter.id, value.expression);
                }
                types.insert(
                    parameter.id,
                    BindingState {
                        name: parameter.name.clone(),
                        ty,
                        domain: Capability::Pure,
                        provider_operation: None,
                    },
                );
            }
            if supplied.len() != guard.parameters.len() {
                self.error(
                    activation.origin.range,
                    "semantic.guard_arguments",
                    "missing guard arguments".into(),
                );
            }
            let old_module = self.module;
            let revision = self.revision;
            self.module = module;
            if let (Some((_, graph)), Some(module)) = (self.workspace, module)
                && let Some(module) = graph.modules.get(&module)
            {
                self.revision = module.revision;
            }
            self.active_calls
                .push((SymbolId(guard.id.0), activation.origin));
            for (ordinal, clause) in guard.clauses.iter().enumerate() {
                if clauses.len() >= 64 {
                    self.error(
                        activation.origin.range,
                        "semantic.guard_limit",
                        "a guard scope may activate at most 64 event clauses in total".into(),
                    );
                    break;
                }
                if self.specialization_budget == 0 {
                    self.error(
                        activation.origin.range,
                        "semantic.guard_limit",
                        "guard specialization budget exceeded".into(),
                    );
                    break;
                }
                self.specialization_budget -= 1;
                if !guard_body(&clause.statements) {
                    self.error(
                        clause.origin.range,
                        "semantic.guard_effect_not_allowed",
                        "guard bodies allow pure local values followed by fail or inconclusive"
                            .into(),
                    );
                    continue;
                }
                let Some((parameter, source)) = guard_source(&clause.source) else {
                    self.error(
                        clause.source.origin.range,
                        "semantic.unknown_event_source",
                        "guard source must name an actor parameter and normalized event".into(),
                    );
                    continue;
                };
                let Some(actor) = actors.get(&parameter).copied() else {
                    self.error(
                        clause.source.origin.range,
                        "semantic.expected_actor",
                        "guard source requires a BrowserActor parameter".into(),
                    );
                    continue;
                };
                let Some(source) = EventSource::from_name(&source) else {
                    self.error(
                        clause.source.origin.range,
                        "semantic.unknown_event_source",
                        "unknown normalized browser event".into(),
                    );
                    continue;
                };
                let Some(event) = &clause.binding else {
                    self.error(
                        clause.source.origin.range,
                        "semantic.invalid_event_binding",
                        "event source requires one binding".into(),
                    );
                    continue;
                };
                self.bindings = types.clone();
                self.names = HashMap::new();
                self.bindings.insert(
                    event.id,
                    BindingState {
                        name: event.name.clone(),
                        ty: source.payload_type(),
                        domain: Capability::Pure,
                        provider_operation: None,
                    },
                );
                let filter = clause.filter.as_ref().map(|filter| {
                    let value = self.infer_expr(filter, Capability::Pure, Some(&Type::Bool));
                    self.expect_type(filter.origin.range, &Type::Bool, &value.ty);
                    super::patterns::substitute(value.expression, &values)
                });
                let mut body = self.compile_sequence(
                    test,
                    clause.origin,
                    &clause.statements,
                    Capability::Pure,
                    vec![],
                );
                specialize(&mut body, &values);
                if !safe_guard_plan(&body) {
                    self.error(
                        clause.origin.range,
                        "semantic.guard_effect_not_allowed",
                        "guard bodies cannot invoke host operations or acquire resources".into(),
                    );
                }
                let child = children.len();
                children.push(body);
                clauses.push(GuardClause {
                    guard: guard.id,
                    actor,
                    alternative: SelectAlternative {
                        id: AlternativeId(webtest_hir::semantic_key(
                            &guard.id.0,
                            "guard clause",
                            &format!("{path:?}:{}", children.len()),
                            ordinal as u32,
                        )),
                        source,
                        binding: event.id,
                        name: event.name.clone(),
                        filter,
                    },
                    child,
                });
            }
            self.active_calls.pop();
            self.module = old_module;
            self.revision = revision;
            self.bindings = saved.clone();
            self.names = names.clone();
        }
        self.bindings = saved;
        self.names = names;
        let first = children.len();
        let failures = [first, first + 1, first + 2];
        for reason in [
            webtest_plan::semantics::EventFailureKind::Overflow,
            webtest_plan::semantics::EventFailureKind::CursorExpired,
            webtest_plan::semantics::EventFailureKind::SourceClosed,
        ] {
            let mut steps = Vec::new();
            self.push_step(
                &mut steps,
                guarded.origin,
                webtest_plan::TestOperation::Reactive(
                    webtest_plan::semantics::ReactiveOperation::Failure {
                        reason,
                        actor: clauses.first().map(|c| c.actor).unwrap_or_default(),
                    },
                ),
            );
            children.push(PlanNode::operation(
                test,
                self.revision,
                vec![],
                steps.remove(0),
            ));
        }
        let mut node = PlanNode::semantic(
            test,
            guarded.origin,
            self.revision,
            vec![],
            SemanticOperation::Guards { clauses, failures },
            children,
        );
        node.rebase(test, path);
        node
    }
}
fn guard_body(body: &[HirStmt]) -> bool {
    !body.is_empty()
        && body.iter().all(|s| {
            matches!(
                s,
                HirStmt::Let(_)
                    | HirStmt::Verdict(webtest_hir::HirVerdict {
                        kind: webtest_model::VerdictKind::Fail
                            | webtest_model::VerdictKind::Inconclusive,
                        ..
                    })
            )
        })
        && matches!(body.last(), Some(HirStmt::Verdict(_)))
}
fn guard_source(expression: &HirExpr) -> Option<(webtest_model::BindingId, String)> {
    fn path(expression: &HirExpr) -> Option<(webtest_model::BindingId, Vec<String>)> {
        match &expression.kind {
            HirExprKind::Name(HirNameRef::Binding { id, .. }) => Some((*id, vec![])),
            HirExprKind::Member {
                receiver, member, ..
            } => {
                let (id, mut path) = path(receiver)?;
                path.push(member.clone());
                Some((id, path))
            }
            _ => None,
        }
    }
    let HirExprKind::Call { callee, arguments } = &expression.kind else {
        return None;
    };
    if arguments.len() != 1 {
        return None;
    }
    let (actor, path) = path(callee)?;
    Some((actor, path.join(".")))
}
fn specialize(
    node: &mut PlanNode,
    arguments: &HashMap<webtest_model::BindingId, webtest_plan::PlanExpr>,
) {
    if let webtest_plan::PlanNodeKind::Operation { step } = &mut node.kind {
        for expression in step.operation.expressions_mut() {
            let old = std::mem::replace(
                expression,
                webtest_plan::PlanExpr::Literal(webtest_model::Value::Null),
            );
            *expression = super::patterns::substitute(old, arguments);
        }
    }
    match &mut node.kind {
        webtest_plan::PlanNodeKind::Sequence { children }
        | webtest_plan::PlanNodeKind::Semantic { children, .. } => {
            for child in children {
                specialize(child, arguments)
            }
        }
        _ => (),
    }
}

fn safe_guard_plan(node: &PlanNode) -> bool {
    use webtest_plan::{PlanNodeKind, TestOperation};
    match &node.kind {
        PlanNodeKind::Sequence { children }
        | PlanNodeKind::Conditional {
            branches: children, ..
        } => children.iter().all(safe_guard_plan),
        PlanNodeKind::Helper { body, .. } => safe_guard_plan(body),
        PlanNodeKind::Operation { step } => matches!(
            &step.operation,
            TestOperation::EvaluatePure(_)
                | TestOperation::Return(_)
                | TestOperation::Verdict { .. }
        ),
        _ => false,
    }
}
