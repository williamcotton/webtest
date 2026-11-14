//! Pattern resolution and specialization over the same typed HIR used by expressions.
use super::{BindingState, Compiler};
use std::collections::{BTreeMap, HashMap, HashSet};
use webtest_hir::{HirExpr, HirExprKind, HirNameRef, HirPattern};
use webtest_model::{BindingId, Capability, SymbolId, Type, Value};
use webtest_plan::{PlanExpr, pattern::Matcher};
impl Compiler<'_> {
    pub(super) fn resolve_pattern(
        &self,
        name: &str,
    ) -> Option<(Option<webtest_model::ModuleId>, HirPattern)> {
        if let (Some((_, graph)), Some(module)) = (self.workspace, self.module) {
            let module = graph.modules.get(&module)?;
            if let Some(pattern) = module.hir.patterns.iter().find(|p| p.name == name) {
                return Some((Some(module.id), pattern.clone()));
            }
            let import = module.imports.get(name)?;
            let target = graph.modules.get(&import.target_module)?;
            return target
                .hir
                .patterns
                .iter()
                .find(|p| p.id.0 == import.target.0)
                .cloned()
                .map(|p| (Some(target.id), p));
        }
        self.local_hir?
            .patterns
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .map(|p| (None, p))
    }
    pub(super) fn validate_patterns(&mut self, hir: &webtest_hir::HirFile) {
        for pattern in &hir.patterns {
            let old = std::mem::take(&mut self.bindings);
            for p in &pattern.parameters {
                let ty = self.lower_type(&p.annotation);
                if !ty.is_transferable() {
                    self.error(
                        p.name_origin.range,
                        "semantic.nontransferable_pattern_argument",
                        "pattern parameters must be transferable values".into(),
                    );
                }
                self.bindings.insert(
                    p.id,
                    BindingState {
                        name: p.name.clone(),
                        ty,
                        domain: Capability::Pure,
                        provider_operation: None,
                    },
                );
            }
            self.active_calls
                .push((SymbolId(pattern.id.0), pattern.origin));
            self.compile_matcher(&pattern.matcher, Capability::Pure, false, 0);
            self.active_calls.pop();
            self.bindings = old;
        }
    }
    pub(super) fn compile_matcher(
        &mut self,
        expr: &HirExpr,
        domain: Capability,
        field: bool,
        depth: usize,
    ) -> Matcher {
        if let Some((db, _)) = self.workspace {
            db.unwind_if_revision_cancelled();
        }
        if depth > 64 || self.specialization_budget == 0 {
            self.error(
                expr.origin.range,
                "semantic.pattern_limit",
                "pattern depth/specialization budget exceeded".into(),
            );
            return Matcher::Any;
        }
        self.specialization_budget -= 1;
        match &expr.kind {
            HirExprKind::PatternRecord { exact, record } => {
                self.matcher_record(record, domain, *exact, depth + 1)
            }
            HirExprKind::Record(_) => self.matcher_record(expr, domain, false, depth + 1),
            HirExprKind::List(items) => {
                // Preserve the pre-H one-element type-list shape syntax.
                if items.len() == 1 && legacy_type(&items[0]) {
                    return Matcher::Each {
                        item: Box::new(self.compile_matcher(&items[0], domain, false, depth + 1)),
                    };
                }
                Matcher::List {
                    items: items
                        .iter()
                        .map(|item| self.compile_matcher(item, domain, false, depth + 1))
                        .collect(),
                }
            }
            HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                if name == "any" {
                    return Matcher::Any;
                }
                if name == "absent" {
                    if !field {
                        self.error(
                            expr.origin.range,
                            "semantic.invalid_pattern_position",
                            "absent is only valid as a record field matcher".into(),
                        );
                    }
                    return Matcher::Absent;
                }
                if let Some(ty) = super::type_system::named_types()
                    .iter()
                    .find_map(|(n, ty)| (*n == name).then(|| ty.clone()))
                {
                    if !ty.is_transferable() {
                        self.error(
                            expr.origin.range,
                            "semantic.nontransferable_pattern_argument",
                            "type matchers require transferable types".into(),
                        );
                    }
                    self.type_fact(expr.origin.range, ty.clone(), Capability::Pure);
                    return Matcher::Type { ty };
                }
                if let Some((module, pattern)) = self.resolve_pattern(name) {
                    return self.pattern_reference(expr, module, &pattern, &[], domain, depth + 1);
                }
                self.literal_matcher(expr, domain)
            }
            HirExprKind::Call { callee, arguments } => {
                let HirExprKind::Name(HirNameRef::Unresolved(name)) = &callee.kind else {
                    return self.literal_matcher(expr, domain);
                };
                if let Some((module, pattern)) = self.resolve_pattern(name) {
                    return self.pattern_reference(
                        expr,
                        module,
                        &pattern,
                        arguments,
                        domain,
                        depth + 1,
                    );
                }
                let unary = matches!(
                    name.as_str(),
                    "each" | "contains" | "optional" | "not" | "regex"
                );
                let n = arguments.len();
                if (unary && n != 1)
                    || (name == "range" && n != 2)
                    || (matches!(name.as_str(), "one_of" | "all_of") && (n == 0 || n > 64))
                {
                    self.error(
                        expr.origin.range,
                        "semantic.pattern_arity",
                        "invalid pattern argument count".into(),
                    );
                    return Matcher::Any;
                }
                if arguments.iter().any(|arg| arg.name.is_some()) {
                    self.error(
                        expr.origin.range,
                        "semantic.named_pattern_combinator_argument",
                        "pattern combinators use positional arguments".into(),
                    );
                }
                match name.as_str() {
                    "each" | "contains" | "optional" | "not" => {
                        if name == "optional" && !field {
                            self.error(
                                expr.origin.range,
                                "semantic.invalid_pattern_position",
                                "optional is only valid as a record field matcher".into(),
                            );
                        }
                        let item = Box::new(self.compile_matcher(
                            &arguments[0].value,
                            domain,
                            false,
                            depth + 1,
                        ));
                        match name.as_str() {
                            "each" => Matcher::Each { item },
                            "contains" => Matcher::Contains { item },
                            "optional" => Matcher::Optional { item },
                            _ => Matcher::Not { item },
                        }
                    }
                    "one_of" | "all_of" => {
                        let items = arguments
                            .iter()
                            .map(|arg| self.compile_matcher(&arg.value, domain, false, depth + 1))
                            .collect();
                        if name == "one_of" {
                            Matcher::OneOf { items }
                        } else {
                            Matcher::AllOf { items }
                        }
                    }
                    "regex" => {
                        let value =
                            self.infer_expr(&arguments[0].value, domain, Some(&Type::String));
                        let expr = substitute(value.expression, &self.pattern_arguments);
                        match webtest_plan::evaluation::evaluate(&expr, &HashMap::new()) {
                            Ok(Value::String(source)) => {
                                if let Err(error) = webtest_plan::pattern::validate_regex(&source) {
                                    self.error(
                                        arguments[0].origin.range,
                                        "semantic.invalid_regex",
                                        error,
                                    );
                                }
                                Matcher::Regex { source }
                            }
                            _ => {
                                self.error(
                                    arguments[0].origin.range,
                                    "semantic.nonconstant_regex",
                                    "regex source must be a statically known String".into(),
                                );
                                Matcher::Any
                            }
                        }
                    }
                    "range" => {
                        let lower = self.infer_expr(&arguments[0].value, domain, None);
                        let upper = self.infer_expr(&arguments[1].value, domain, Some(&lower.ty));
                        if !matches!(
                            lower.ty,
                            Type::Int | Type::Float | Type::StatusCode | Type::Duration
                        ) || (!lower.ty.accepts(&upper.ty) && !upper.ty.accepts(&lower.ty))
                        {
                            self.error(
                                expr.origin.range,
                                "semantic.invalid_pattern_range",
                                "range bounds require compatible numeric or Duration values".into(),
                            );
                        }
                        Matcher::Range {
                            lower: substitute(lower.expression, &self.pattern_arguments),
                            upper: substitute(upper.expression, &self.pattern_arguments),
                        }
                    }
                    _ => self.literal_matcher(expr, domain),
                }
            }
            _ => self.literal_matcher(expr, domain),
        }
    }
    fn literal_matcher(&mut self, expr: &HirExpr, domain: Capability) -> Matcher {
        let value = self.infer_expr(expr, domain, None);
        if !value.ty.is_transferable() {
            self.error(
                expr.origin.range,
                "semantic.nontransferable_pattern_argument",
                "pattern values must be transferable".into(),
            );
        }
        Matcher::Literal {
            value: substitute(value.expression, &self.pattern_arguments),
        }
    }
    fn matcher_record(
        &mut self,
        expr: &HirExpr,
        domain: Capability,
        exact: bool,
        depth: usize,
    ) -> Matcher {
        let HirExprKind::Record(fields) = &expr.kind else {
            return Matcher::Any;
        };
        let mut compiled = BTreeMap::new();
        for field in fields {
            if compiled.contains_key(&field.name) {
                self.error(
                    field.name_origin.range,
                    "semantic.duplicate_pattern_field",
                    "pattern fields must be unique".into(),
                );
            }
            compiled.insert(
                field.name.clone(),
                self.compile_matcher(&field.value, domain, true, depth + 1),
            );
        }
        Matcher::Record {
            exact,
            fields: compiled,
        }
    }
    fn pattern_reference(
        &mut self,
        expr: &HirExpr,
        module: Option<webtest_model::ModuleId>,
        pattern: &HirPattern,
        args: &[webtest_hir::HirCallArgument],
        domain: Capability,
        depth: usize,
    ) -> Matcher {
        if self.active_calls.iter().any(|(id, _)| id.0 == pattern.id.0) {
            self.error(
                expr.origin.range,
                "semantic.recursive_pattern",
                "patterns must be acyclic".into(),
            );
            return Matcher::Any;
        }
        let mut bound = HashMap::new();
        let mut supplied = HashSet::new();
        let mut positional = 0;
        for arg in args {
            let parameter = if let Some(name) = &arg.name {
                pattern.parameters.iter().find(|p| p.name == *name)
            } else {
                let p = pattern.parameters.get(positional);
                positional += 1;
                p
            };
            let Some(parameter) = parameter else {
                self.error(
                    arg.origin.range,
                    "semantic.pattern_argument",
                    "unknown pattern argument".into(),
                );
                continue;
            };
            if !supplied.insert(parameter.id) {
                self.error(
                    arg.origin.range,
                    "semantic.pattern_argument",
                    "duplicate pattern argument".into(),
                );
            }
            let ty = self.lower_type(&parameter.annotation);
            let value = self.infer_expr(&arg.value, domain, Some(&ty));
            self.expect_type(arg.origin.range, &ty, &value.ty);
            if !value.ty.is_transferable() {
                self.error(
                    arg.origin.range,
                    "semantic.nontransferable_pattern_argument",
                    "pattern arguments must be transferable".into(),
                );
            }
            bound.insert(
                parameter.id,
                substitute(value.expression, &self.pattern_arguments),
            );
        }
        for parameter in &pattern.parameters {
            if !supplied.contains(&parameter.id) {
                self.error(
                    expr.origin.range,
                    "semantic.pattern_argument",
                    format!("missing pattern argument `{}`", parameter.name),
                );
            }
        }
        let bindings = std::mem::take(&mut self.bindings);
        let old_arguments = std::mem::replace(&mut self.pattern_arguments, bound);
        let old_module = self.module;
        let revision = self.revision;
        self.module = module;
        if let (Some((_, graph)), Some(module)) = (self.workspace, module)
            && let Some(module) = graph.modules.get(&module)
        {
            self.revision = module.revision;
        }
        for parameter in &pattern.parameters {
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
        }
        self.active_calls
            .push((SymbolId(pattern.id.0), expr.origin));
        let body = self.compile_matcher(&pattern.matcher, Capability::Pure, false, depth + 1);
        self.active_calls.pop();
        self.bindings = bindings;
        self.pattern_arguments = old_arguments;
        self.module = old_module;
        self.revision = revision;
        Matcher::Reference {
            id: pattern.id,
            name: pattern.name.clone(),
            body: Box::new(body),
        }
    }
}
pub(super) fn substitute(mut expr: PlanExpr, arguments: &HashMap<BindingId, PlanExpr>) -> PlanExpr {
    if let PlanExpr::Binding(id) = &expr
        && let Some(value) = arguments.get(id)
    {
        return value.clone();
    }
    for child in expr.children_mut() {
        let old = std::mem::replace(child, PlanExpr::Literal(Value::Null));
        *child = substitute(old, arguments);
    }
    expr
}
fn legacy_type(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Name(HirNameRef::Unresolved(name)) => super::type_system::named_types()
            .iter()
            .any(|(n, _)| *n == name),
        HirExprKind::Record(fields) => fields.iter().all(|f| legacy_type(&f.value)),
        HirExprKind::List(items) => items.len() == 1 && legacy_type(&items[0]),
        _ => false,
    }
}
