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
            if let Some((db, _)) = self.workspace {
                db.unwind_if_revision_cancelled();
            }
            let unreachable = provided;
            provided |= statement_provides(statement);
            let mut child_path = path.clone();
            child_path.push(ordinal as u32);
            let node = match statement {
                HirStmt::GuardScope(guard) => self.compile_guards(test, guard, domain, child_path),
                HirStmt::Let(binding) if matches!(&binding.value.kind,HirExprKind::Call{callee,..} if matches!(&callee.kind,HirExprKind::Name(HirNameRef::Unresolved(name)) if name=="checkpoint")) =>
                {
                    self.record_capability(Capability::Browser);
                    let HirExprKind::Call { arguments, .. } = &binding.value.kind else {
                        unreachable!()
                    };
                    let actor = if arguments.len() == 1 {
                        self.actor_reference(&arguments[0].value)
                    } else {
                        self.error(
                            binding.value.origin.range,
                            "semantic.checkpoint_arguments",
                            "checkpoint requires exactly one actor".into(),
                        );
                        None
                    }
                    .unwrap_or_default();
                    if self.names.contains_key(&binding.name) {
                        self.error(
                            binding.name_origin.range,
                            "semantic.duplicate_binding",
                            "binding already exists".into(),
                        );
                    }
                    self.cursors.insert(binding.id, (actor, self.retry_depth));
                    self.names.insert(binding.name.clone(), binding.name_origin);
                    self.bindings.insert(
                        binding.id,
                        BindingState {
                            name: binding.name.clone(),
                            ty: Type::EventCursor,
                            domain: Capability::Browser,
                            provider_operation: None,
                        },
                    );
                    self.type_fact(
                        binding.name_origin.range,
                        Type::EventCursor,
                        Capability::Browser,
                    );
                    let mut steps = Vec::new();
                    self.push_step(
                        &mut steps,
                        binding.value.origin,
                        TestOperation::Reactive(
                            webtest_plan::semantics::ReactiveOperation::Checkpoint {
                                actor,
                                binding: binding.id,
                                name: binding.name.clone(),
                            },
                        ),
                    );
                    let mut node = webtest_plan::PlanNode::operation(
                        test,
                        self.revision,
                        vec![],
                        steps.remove(0),
                    );
                    node.rebase(test, child_path);
                    node
                }
                HirStmt::Select(select) => self.compile_selection(test, select, domain, child_path),
                HirStmt::Actor(actor) => {
                    self.record_capability(Capability::Browser);
                    let saved = self.bindings.clone();
                    let names = self.names.clone();
                    let actors = self.actors.clone();
                    if self.names.contains_key(&actor.binding.name) {
                        self.error(
                            actor.binding.name_origin.range,
                            "semantic.duplicate_binding",
                            "actor name already declared".into(),
                        );
                    }
                    let id = webtest_model::ActorId(webtest_hir::semantic_key(
                        &test.0,
                        "actor",
                        &format!("{}:{:?}", actor.binding.name, child_path),
                        0,
                    ));
                    self.actors.insert(actor.binding.id, id);
                    self.names
                        .insert(actor.binding.name.clone(), actor.binding.name_origin);
                    self.bindings.insert(
                        actor.binding.id,
                        BindingState {
                            name: actor.binding.name.clone(),
                            ty: Type::BrowserActor,
                            domain: Capability::Browser,
                            provider_operation: None,
                        },
                    );
                    self.type_fact(
                        actor.binding.name_origin.range,
                        Type::BrowserActor,
                        Capability::Browser,
                    );
                    let body = self.compile_sequence(
                        test,
                        origin,
                        &statements[ordinal + 1..],
                        domain,
                        vec![],
                    );
                    let sources = body.event_sources(id);
                    self.bindings = saved;
                    self.names = names;
                    self.actors = actors;
                    let mut node = webtest_plan::PlanNode::semantic(
                        test,
                        actor.origin,
                        self.revision,
                        vec![],
                        webtest_plan::semantics::SemanticOperation::ActorScope {
                            actor: id,
                            name: actor.binding.name.clone(),
                            sources,
                        },
                        vec![body],
                    );
                    node.rebase(test, child_path);
                    children.push(node);
                    break;
                }
                HirStmt::ActorBlock(block) => {
                    let id = match &block.actor.kind {
                        HirExprKind::Name(HirNameRef::Binding { id: binding, .. }) => {
                            self.actors.get(binding).copied()
                        }
                        _ => None,
                    };
                    let Some(id) = id else {
                        self.error(
                            block.actor.origin.range,
                            "semantic.expected_actor",
                            "expected an actor declared in this lexical scope".into(),
                        );
                        continue;
                    };
                    self.type_fact(
                        block.actor.origin.range,
                        Type::BrowserActor,
                        Capability::Browser,
                    );
                    let saved = self.bindings.clone();
                    let names = self.names.clone();
                    let body = self.compile_sequence(
                        test,
                        block.origin,
                        &block.statements,
                        Capability::Browser,
                        vec![],
                    );
                    self.bindings = saved;
                    self.names = names;
                    let mut node = webtest_plan::PlanNode::semantic(
                        test,
                        block.origin,
                        self.revision,
                        vec![],
                        webtest_plan::semantics::SemanticOperation::ActorAccess {
                            actor: id,
                            name: match &block.actor.kind {
                                HirExprKind::Name(HirNameRef::Binding { id: binding, .. }) => self
                                    .bindings
                                    .get(binding)
                                    .map(|b| b.name.clone())
                                    .unwrap_or_default(),
                                _ => String::new(),
                            },
                        },
                        vec![body],
                    );
                    node.rebase(test, child_path);
                    node
                }

                HirStmt::Check(block) => {
                    let saved = self.bindings.clone();
                    let names = self.names.clone();
                    let body = self.compile_sequence(
                        test,
                        block.origin,
                        &block.statements,
                        domain,
                        Vec::new(),
                    );
                    self.bindings = saved;
                    self.names = names;
                    let webtest_plan::PlanNodeKind::Sequence { children } = body.kind else {
                        unreachable!()
                    };
                    let mut node = webtest_plan::PlanNode::semantic(
                        test,
                        block.origin,
                        self.revision,
                        Vec::new(),
                        webtest_plan::semantics::SemanticOperation::Check {
                            continuation:
                                webtest_plan::semantics::CheckContinuation::AssertionsOnly,
                        },
                        children,
                    );
                    node.rebase(test, child_path);
                    node
                }
                HirStmt::Assume(assume) => {
                    let condition = self.infer_expr(&assume.condition, domain, Some(&Type::Bool));
                    self.expect_type(assume.condition.origin.range, &Type::Bool, &condition.ty);
                    let mut steps = Vec::new();
                    self.compile_verdict(&assume.verdict, domain, &mut steps);
                    let children = steps
                        .into_iter()
                        .map(|step| {
                            webtest_plan::PlanNode::operation(test, self.revision, Vec::new(), step)
                        })
                        .collect();
                    let mut node = webtest_plan::PlanNode::semantic(
                        test,
                        assume.origin,
                        self.revision,
                        Vec::new(),
                        webtest_plan::semantics::SemanticOperation::Assume {
                            condition: condition.expression,
                        },
                        children,
                    );
                    node.rebase(test, child_path);
                    node
                }
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
                HirStmt::Retry(block) => self.compile_retry(test, block, domain, child_path),
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
                    self.resource_depth += 1;
                    let body = self.compile_sequence(
                        test,
                        block.body_origin,
                        &block.statements,
                        domain,
                        body_path,
                    );
                    self.resource_depth -= 1;
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
                    if (self.concurrent_depth > 0 || self.retry_depth > 0)
                        && !body.required_resources().is_empty()
                    {
                        body.with_browser_resource(test)
                    } else {
                        body
                    }
                }
                _ => {
                    if let Some(node) =
                        self.statement_helper(statement, domain, test, child_path.clone())
                    {
                        children.push(node);
                        continue;
                    }
                    let mut steps = Vec::new();
                    self.compile_statement(statement, domain, &mut steps);
                    if steps.is_empty() {
                        continue;
                    }
                    let mut operations = steps
                        .into_iter()
                        .map(|step| self.expand_pure_operation(test, self.revision, step))
                        .collect::<Vec<_>>();
                    let mut node = if operations.len() == 1 {
                        operations.remove(0)
                    } else {
                        webtest_plan::PlanNode::sequence(
                            test,
                            statement.origin(),
                            self.revision,
                            Vec::new(),
                            operations,
                        )
                    };
                    node.rebase(test, child_path);
                    node
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

    fn compile_retry(
        &mut self,
        test: webtest_model::PlanDeclarationId,
        block: &webtest_hir::HirRetry,
        domain: Capability,
        path: Vec<u32>,
    ) -> webtest_plan::PlanNode {
        self.type_fact(block.attempts_origin.range, Type::Int, Capability::Pure);
        let attempts = block
            .attempts
            .filter(|n| (1..=webtest_plan::MAX_RETRY_ATTEMPTS).contains(n));
        if attempts.is_none() {
            self.error(
                block.attempts_origin.range,
                "semantic.invalid_retry_attempts",
                format!(
                    "retry requires 1 to {} total attempts",
                    webtest_plan::MAX_RETRY_ATTEMPTS
                ),
            );
        }
        for (value, origin) in [
            (block.backoff, block.backoff_origin),
            (block.max_backoff, block.max_backoff_origin),
        ] {
            if let Some(origin) = origin {
                self.type_fact(origin.range, Type::Duration, Capability::Pure);
                if value.is_none_or(|value| value > webtest_plan::MAX_CONTROL_TIMEOUT) {
                    self.error(
                        origin.range,
                        "semantic.invalid_retry_backoff",
                        "retry backoff must be no greater than 24 hours".into(),
                    );
                }
            }
        }
        let initial = block.backoff.unwrap_or_default();
        let max = block.max_backoff.unwrap_or(initial);
        if max < initial {
            self.error(
                block.max_backoff_origin.unwrap_or(block.origin).range,
                "semantic.invalid_retry_backoff",
                "maximum backoff must be at least the initial backoff".into(),
            );
        }
        let bindings = self.bindings.clone();
        let names = self.names.clone();
        let captures = self.retry_captures.clone();
        self.retry_captures.extend(
            bindings
                .iter()
                .filter_map(|(id, binding)| (!binding.ty.is_transferable()).then_some(*id)),
        );
        self.retry_depth += 1;
        self.resource_depth += 1;
        let mut child_path = path.clone();
        child_path.push(0);
        let body = self.compile_sequence(
            test,
            block.body_origin,
            &block.statements,
            domain,
            child_path,
        );
        self.retry_depth -= 1;
        self.resource_depth -= 1;
        self.retry_captures = captures;
        self.bindings = bindings;
        self.names = names;
        for violation in body.retry_safety_violations() {
            self.error(violation.origin.range, "semantic.unsafe_retry", "operation or resource has no repeatability contract; provider calls require retry_safe and browser mutations cannot be retried".into());
        }
        webtest_plan::PlanNode::retry(
            test,
            block.origin,
            self.revision,
            path,
            body,
            webtest_plan::RetrySettings {
                attempts: attempts.unwrap_or(1),
                backoff: webtest_plan::RetryBackoff { initial, max },
                policy: webtest_plan::RetryPolicy::SafeFailures,
            },
        )
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
        let outer_return = std::mem::replace(&mut self.provide_is_return, false);
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
                HirStmt::Select(select) => select.origin,
                HirStmt::GuardScope(guard) => guard.origin,
                HirStmt::ActorBlock(block) => block.origin,
                HirStmt::Check(block) => block.origin,
                HirStmt::Server(block) => block.origin,
                HirStmt::Browser(block) => block.origin,
                HirStmt::Timeout(block) => block.origin,
                HirStmt::Retry(block) => block.origin,
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
            if child
                .required_resources()
                .iter()
                .any(|usage| usage.resource == webtest_plan::ResourceReference::BrowserContext)
                && domain == Capability::Browser
            {
                self.error(origin.range, "semantic.concurrent_resource_conflict", "concurrent browser operations require a lexical browser block in a flow domain; the enclosing browser context is exclusive".into());
            }
            let provides = self.provide_types.take().unwrap_or_default();
            if block.binding.is_some() && provides.is_empty() && !child.terminates_variant() {
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
        self.provide_is_return = outer_return;
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
            HirStmt::Verdict(verdict) => self.compile_verdict(verdict, domain, steps),
            HirStmt::GuardScope(_)
            | HirStmt::Select(_)
            | HirStmt::Actor(_)
            | HirStmt::ActorBlock(_)
            | HirStmt::Check(_)
            | HirStmt::Assume(_)
            | HirStmt::Timeout(_)
            | HirStmt::Retry(_)
            | HirStmt::Parallel(_)
            | HirStmt::Race(_) => {
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
                if self.provide_is_return
                    && !value.ty.is_transferable()
                    && self.resource_depth > self.return_boundary_depth
                {
                    self.error(
                        statement.expression.origin.range,
                        "semantic.non_transferable_scope_result",
                        "a native resource cannot be returned across a scope that tears it down"
                            .into(),
                    );
                }
                if !self.provide_is_return && !value.ty.is_transferable() {
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
                    if self.provide_is_return {
                        TestOperation::Return(webtest_plan::ProvideOperation {
                            expression: value.expression,
                            result_type: value.ty,
                        })
                    } else {
                        TestOperation::Provide(webtest_plan::ProvideOperation {
                            expression: value.expression,
                            result_type: value.ty,
                        })
                    },
                );
            }
            HirStmt::Expression(statement) => {
                if let Some(call) = self.fixture_acquire(&statement.expression, domain) {
                    self.push_step(
                        steps,
                        statement.expression.origin,
                        TestOperation::AcquireFixture(call),
                    );
                } else if let Some(call) = self.provider_call(&statement.expression, domain) {
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

        if let Some(mut call) = self.fixture_acquire(&binding.value, domain) {
            let mut result_type = call.result_type.clone();
            let annotation = binding
                .annotation
                .as_ref()
                .map(|annotation| self.lower_type(annotation));
            let decode = annotation.as_ref().is_some_and(|expected| {
                result_type == Type::Json && super::type_system::decodable_type(expected)
            });
            let result_id = if decode {
                let id = webtest_model::BindingId(self.next_temporary);
                self.next_temporary = self.next_temporary.saturating_sub(1);
                id
            } else {
                binding.id
            };
            if let Some(expected) = annotation {
                if !decode {
                    self.expect_type(binding.value.origin.range, &expected, &result_type);
                }
                result_type = expected;
            }
            call.result_binding = Some(result_id);
            call.result_name = (!decode).then(|| binding.name.clone());
            self.push_step(
                steps,
                binding.value.origin,
                TestOperation::AcquireFixture(call),
            );
            if decode {
                self.push_step(
                    steps,
                    binding.value.origin,
                    TestOperation::EvaluatePure(EvaluatePureOperation {
                        expression: PlanExpr::Decode {
                            value: Box::new(PlanExpr::Binding(result_id)),
                            target: result_type.clone(),
                            response_operation: None,
                        },
                        result_binding: Some(binding.id),
                        result_name: Some(binding.name.clone()),
                        result_type: result_type.clone(),
                    }),
                );
            }
            self.bindings.insert(
                binding.id,
                BindingState {
                    name: binding.name.clone(),
                    ty: result_type.clone(),
                    domain,
                    provider_operation: None,
                },
            );
            self.type_fact(binding.name_origin.range, result_type, Capability::Pure);
            return;
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
                let pattern = self.compile_matcher(right, domain, false, 0);
                (
                    ValueMatcher::Matches,
                    actual.expression,
                    Some(PlanExpr::Pattern(Box::new(pattern))),
                    actual.ty,
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

    fn compile_verdict(
        &mut self,
        verdict: &webtest_hir::HirVerdict,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        if self.in_fixture_teardown {
            self.error(
                verdict.origin.range,
                "semantic.verdict_in_teardown",
                "terminating verdicts are not allowed during fixture teardown".into(),
            );
        }
        let message = verdict.message.as_ref().map(|expr| {
            let value = self.infer_expr(expr, domain, Some(&Type::String));
            self.expect_type(expr.origin.range, &Type::String, &value.ty);
            value.expression
        });
        self.record_capability(Capability::Test);
        self.push_step(
            steps,
            verdict.origin,
            TestOperation::Verdict {
                verdict: verdict.kind,
                message,
            },
        );
    }
    pub(super) fn push_step(
        &mut self,
        steps: &mut Vec<PlannedStep>,
        origin: SyntaxOrigin,
        operation: TestOperation,
    ) {
        steps.push(PlannedStep {
            origin_chain: self.call_origin_chain(),
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
        HirStmt::Check(block) => {
            for statement in &block.statements {
                collect_binding_names(statement, names);
            }
        }
        HirStmt::GuardScope(guard) => {
            for stmt in &guard.statements {
                collect_binding_names(stmt, names);
            }
        }
        HirStmt::Select(select) => {
            if let Some(binding) = &select.binding {
                names.insert(binding.name.clone());
            }
            for arm in &select.arms {
                for stmt in &arm.statements {
                    collect_binding_names(stmt, names);
                }
            }
        }
        HirStmt::Actor(actor) => {
            names.insert(actor.binding.name.clone());
        }
        HirStmt::ActorBlock(block) => {
            for s in &block.statements {
                collect_binding_names(s, names);
            }
        }
        HirStmt::Verdict(_) | HirStmt::Assume(_) => (),
        HirStmt::Retry(block) => {
            for statement in &block.statements {
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
        HirStmt::Retry(block) => block.statements.iter().any(statement_provides),
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
