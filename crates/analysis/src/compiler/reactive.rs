//! Actor-relative finite observation compiles through the normal typed expression
//! and scope path. No event bodies are callbacks or provider code.
use super::*;
use webtest_hir::{HirExpr, HirExprKind, HirNameRef, HirSelect};
use webtest_model::{ActorId, AlternativeId, EventSource};
use webtest_plan::{
    PlanNode,
    semantics::{SelectAlternative, SemanticOperation},
};
impl Compiler<'_> {
    pub(super) fn actor_reference(&mut self, expression: &HirExpr) -> Option<ActorId> {
        let id = match &expression.kind {
            HirExprKind::Name(HirNameRef::Binding { id, .. }) => self.actors.get(id).copied(),
            _ => None,
        };
        if id.is_none() {
            self.error(
                expression.origin.range,
                "semantic.expected_actor",
                "expected a live lexical BrowserActor".into(),
            );
        } else {
            self.type_fact(
                expression.origin.range,
                Type::BrowserActor,
                Capability::Browser,
            );
        }
        id
    }
    pub(super) fn compile_selection(
        &mut self,
        test: webtest_model::PlanDeclarationId,
        select: &HirSelect,
        domain: Capability,
        path: Vec<u32>,
    ) -> PlanNode {
        self.record_capability(Capability::Browser);
        let actor = self.actor_reference(&select.actor).unwrap_or_default();
        let cursor = select.cursor.as_ref().and_then(|expression| {
            if let HirExprKind::Name(HirNameRef::Binding { id, .. }) = &expression.kind
                && self
                    .cursors
                    .get(id)
                    .is_some_and(|(owner, depth)| *owner == actor && *depth == self.retry_depth)
            {
                self.type_fact(
                    expression.origin.range,
                    Type::EventCursor,
                    Capability::Browser,
                );
                return Some(*id);
            }
            self.error(
                expression.origin.range,
                "semantic.invalid_event_cursor",
                "cursor must belong to this actor and retry generation".into(),
            );
            None
        });
        let duration = select
            .duration
            .filter(|d| !d.is_zero() && *d <= webtest_plan::MAX_CONTROL_TIMEOUT)
            .unwrap_or_else(|| {
                self.error(
                    select.origin.range,
                    "semantic.invalid_select_duration",
                    "select requires a positive duration no greater than 24 hours".into(),
                );
                std::time::Duration::from_millis(1)
            });
        if select.arms.is_empty() || select.arms.len() > 64 || select.timeouts.len() > 1 {
            self.error(
                select.origin.range,
                "semantic.invalid_select_alternatives",
                "select requires 1–64 event alternatives and at most one timeout".into(),
            );
        }
        let bindings = self.bindings.clone();
        let names = self.names.clone();
        let outer = self.provide_types.take();
        let returning = std::mem::replace(&mut self.provide_is_return, false);
        let mut children = Vec::new();
        let mut alternatives = Vec::new();
        let mut results = Vec::new();
        for (ordinal, arm) in select.arms.iter().enumerate() {
            self.bindings = bindings.clone();
            self.names = names.clone();
            self.provide_types = Some(Vec::new());
            let source = event_source(&arm.source).and_then(|name| EventSource::from_name(&name));
            let Some(source) = source else {
                self.error(
                    arm.source.origin.range,
                    "semantic.unknown_event_source",
                    "expected a normalized event source(binding)".into(),
                );
                continue;
            };
            let Some(event) = &arm.binding else {
                self.error(
                    arm.source.origin.range,
                    "semantic.invalid_event_binding",
                    "event source requires exactly one named binding".into(),
                );
                continue;
            };
            self.bindings.insert(
                event.id,
                BindingState {
                    name: event.name.clone(),
                    ty: source.payload_type(),
                    domain: Capability::Pure,
                    provider_operation: None,
                },
            );
            self.names.insert(event.name.clone(), event.name_origin);
            self.type_fact(
                event.name_origin.range,
                source.payload_type(),
                Capability::Pure,
            );
            let filter = arm.filter.as_ref().map(|filter| {
                let value = self.infer_expr(filter, Capability::Pure, Some(&Type::Bool));
                self.expect_type(filter.origin.range, &Type::Bool, &value.ty);
                value.expression
            });
            let body = self.compile_sequence(test, arm.origin, &arm.statements, domain, Vec::new());
            let produced = self.provide_types.take().unwrap_or_default();
            if select.binding.is_some() && produced.is_empty() && !body.terminates_variant() {
                self.error(
                    arm.origin.range,
                    "semantic.missing_select_result",
                    "a successful bound select alternative must provide its result".into(),
                );
            }
            results.extend(produced);
            alternatives.push(SelectAlternative {
                id: AlternativeId(webtest_hir::semantic_key(
                    &test.0,
                    "select alternative",
                    &format!("{path:?}"),
                    ordinal as u32,
                )),
                source,
                binding: event.id,
                name: event.name.clone(),
                filter,
            });
            children.push(body);
        }
        let timeout = select.timeouts.first().map(|statements| {
            self.bindings = bindings.clone();
            self.names = names.clone();
            self.provide_types = Some(Vec::new());
            let body = self.compile_sequence(test, select.origin, statements, domain, vec![]);
            let produced = self.provide_types.take().unwrap_or_default();
            if select.binding.is_some() && produced.is_empty() && !body.terminates_variant() {
                self.error(
                    select.origin.range,
                    "semantic.missing_select_result",
                    "a successful timeout alternative must provide its result".into(),
                );
            }
            results.extend(produced);
            let index = children.len();
            children.push(body);
            index
        });
        self.bindings = bindings;
        self.names = names;
        self.provide_types = outer;
        self.provide_is_return = returning;
        let result = select.binding.as_ref().map(|binding| {
            let ty = results.first().map(|v| v.0.clone()).unwrap_or(Type::Null);
            for (actual, origin) in &results {
                if !ty.accepts(actual) || !actual.accepts(&ty) {
                    self.type_mismatch(origin.range, &ty, actual);
                }
            }
            if !ty.is_transferable() {
                self.error(
                    binding.name_origin.range,
                    "semantic.nontransferable_select_result",
                    "select results must be transferable".into(),
                );
            }
            if let Some(annotation) = &binding.annotation {
                let expected = self.lower_type(annotation);
                self.expect_type(binding.name_origin.range, &expected, &ty);
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
            self.type_fact(binding.name_origin.range, ty.clone(), Capability::Pure);
            webtest_plan::RaceBinding {
                id: binding.id,
                name: binding.name.clone(),
                ty,
            }
        });
        let start = children.len();
        let failures = [start, start + 1, start + 2, start + 3];
        for reason in [
            webtest_plan::semantics::EventFailureKind::SelectionTimeout,
            webtest_plan::semantics::EventFailureKind::Overflow,
            webtest_plan::semantics::EventFailureKind::CursorExpired,
            webtest_plan::semantics::EventFailureKind::SourceClosed,
        ] {
            let mut steps = Vec::new();
            self.push_step(
                &mut steps,
                select.origin,
                webtest_plan::TestOperation::Reactive(
                    webtest_plan::semantics::ReactiveOperation::Failure { reason, actor },
                ),
            );
            children.push(PlanNode::operation(
                test,
                self.revision,
                Vec::new(),
                steps.remove(0),
            ));
        }
        let mut node = PlanNode::semantic(
            test,
            select.origin,
            self.revision,
            vec![],
            SemanticOperation::Select {
                actor,
                cursor,
                duration,
                failures,
                alternatives,
                timeout,
                result,
            },
            children,
        );
        node.rebase(test, path);
        node
    }
}
pub(crate) fn event_source(expression: &HirExpr) -> Option<String> {
    fn path(expression: &HirExpr) -> Option<String> {
        match &expression.kind {
            HirExprKind::Name(HirNameRef::Unresolved(name)) => Some(name.clone()),
            HirExprKind::Name(HirNameRef::Binding { .. }) => Some(String::new()),
            HirExprKind::Member {
                receiver, member, ..
            } => Some(
                format!("{}.{}", path(receiver)?, member)
                    .trim_start_matches('.')
                    .to_owned(),
            ),
            _ => None,
        }
    }
    match &expression.kind {
        HirExprKind::Call { callee, arguments } if arguments.len() == 1 => path(callee),
        _ => None,
    }
}
