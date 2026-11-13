//! Semantic symbols/references and structural field constraints. No source scanning.
use crate::{
    callables::{self, Callable},
    facts::{Reference, SourceLocation, Symbol, SymbolKind},
    incremental::{self, Db, SourceInput},
    workspace::{Module, ModuleGraph},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use webtest_hir::{HirExpr, HirExprKind, HirNameRef, HirStmt, HirType, HirTypeKind};
use webtest_model::{BindingId, Capability, ModuleId, RecordField, SymbolId, Type};
use webtest_text::SyntaxOrigin;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scope {
    pub parent: Option<SymbolId>,
    pub names: BTreeMap<String, SymbolId>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SymbolIndex {
    pub symbols: BTreeMap<SymbolId, Symbol>,
    pub references: Vec<Reference>,
    pub scopes: BTreeMap<SymbolId, Scope>,
    pub field_constraints: Vec<(SymbolId, SymbolId)>,
}
#[derive(Clone, Debug, Default)]
struct Shape {
    fields: BTreeMap<String, Field>,
}
#[derive(Clone, Debug)]
struct Field {
    id: SymbolId,
    value: ValueInfo,
}
#[derive(Clone, Debug)]
struct ValueInfo {
    ty: Type,
    shape: Shape,
    external: bool,
}
impl ValueInfo {
    fn unknown() -> Self {
        Self {
            ty: Type::Unknown,
            shape: Shape::default(),
            external: false,
        }
    }
}

struct ReferenceRole {
    declaration: bool,
    quoted: bool,
    scope_sensitive: bool,
}
struct SymbolOrigin {
    name: Option<SyntaxOrigin>,
    full: Option<SyntaxOrigin>,
    container: Option<SymbolId>,
}
struct SymbolInfo {
    ty: Option<Type>,
    documentation: String,
    read_only: bool,
}
struct Indexer<'a> {
    db: &'a dyn Db,
    graph: &'a ModuleGraph,
    module: &'a Module,
    output: SymbolIndex,
    locals: HashMap<BindingId, SymbolId>,
    values: HashMap<BindingId, ValueInfo>,
    counters: HashMap<(SymbolId, String), u32>,
    globals: BTreeMap<String, (SymbolId, SymbolId)>,
    type_facts: Vec<crate::TypeFact>,
}
impl Indexer<'_> {
    fn location(&self, origin: SyntaxOrigin) -> Option<SourceLocation> {
        let module = self
            .graph
            .modules
            .values()
            .find(|module| module.file == origin.file)?;
        Some(SourceLocation {
            path: module.path.clone(),
            origin,
            revision: module.revision,
        })
    }
    fn key(&mut self, parent: SymbolId, kind: &str, name: &str) -> SymbolId {
        let ordinal = self
            .counters
            .entry((parent, format!("{kind}/{name}")))
            .or_default();
        let key = SymbolId(webtest_hir::semantic_key(&parent.0, kind, name, *ordinal));
        *ordinal += 1;
        key
    }
    fn scope(&mut self, parent: Option<SymbolId>, id: SymbolId) {
        self.output.scopes.entry(id).or_insert_with(|| Scope {
            parent,
            names: BTreeMap::new(),
        });
    }
    fn reference(
        &mut self,
        target: SymbolId,
        resolved: SymbolId,
        origin: SyntaxOrigin,
        scope: SymbolId,
        role: ReferenceRole,
    ) {
        let ReferenceRole {
            declaration,
            quoted,
            scope_sensitive,
        } = role;
        if origin.file != self.module.file {
            return;
        }
        if let Some(location) = self.location(origin) {
            self.output.references.push(Reference {
                symbol: target,
                resolved_symbol: resolved,
                location,
                declaration,
                scope,
                quoted,
                scope_sensitive,
            });
        }
    }
    fn add(
        &mut self,
        id: SymbolId,
        name: &str,
        kind: SymbolKind,
        origin: SymbolOrigin,
        info: SymbolInfo,
    ) {
        let SymbolOrigin {
            name: origin,
            full,
            container,
        } = origin;
        let SymbolInfo {
            ty,
            documentation,
            read_only,
        } = info;
        let location = origin.and_then(|origin| self.location(origin));
        self.output.symbols.entry(id).or_insert(Symbol {
            id,
            name: name.into(),
            kind,
            location,
            full_origin: full,
            container,
            ty,
            capability: Capability::Pure,
            lifetime: None,
            documentation,
            exported: false,
            read_only,
        });
    }
    fn callable(&mut self, module_id: ModuleId, id: SymbolId) {
        let Some(module) = self.graph.modules.get(&module_id) else {
            return;
        };
        let Some(callable) = callables::find(module, id) else {
            return;
        };
        let (name, origin, full, exported, parameters, result, documentation, lifetime) =
            match callable {
                Callable::Function(value) => (
                    &value.name,
                    value.name_origin,
                    value.origin,
                    value.exported,
                    &value.parameters,
                    &value.result_type,
                    &value.documentation,
                    None,
                ),
                Callable::Fixture(value) => (
                    &value.name,
                    value.name_origin,
                    value.origin,
                    value.exported,
                    &value.parameters,
                    &value.result_type,
                    &value.documentation,
                    Some(value.lifetime),
                ),
            };
        let ty = crate::compiler::declared_type(result).0;
        let label = format!(
            "{} {}({}) -> {}",
            if lifetime.is_some() { "fixture" } else { "fn" },
            name,
            parameters
                .iter()
                .map(|parameter| format!(
                    "{}: {}",
                    parameter.name,
                    crate::compiler::declared_type(&parameter.annotation).0
                ))
                .collect::<Vec<_>>()
                .join(", "),
            ty
        );
        self.add(
            id,
            name,
            if lifetime.is_some() {
                SymbolKind::Fixture
            } else {
                SymbolKind::Function
            },
            SymbolOrigin {
                name: Some(origin),
                full: Some(full),
                container: Some(SymbolId(module.id.0)),
            },
            SymbolInfo {
                ty: Some(ty),
                documentation: format!("{label}\n\n{documentation}"),
                read_only: false,
            },
        );
        if let Some(symbol) = self.output.symbols.get_mut(&id) {
            symbol.exported = exported;
            symbol.lifetime = lifetime;
        }
        if let Some(input) = self.db.file_slot(&module.path).target(self.db)
            && let Some(signature) = callables::signature(self.db, input, id)
            && let Some(symbol) = self.output.symbols.get_mut(&id)
        {
            symbol.capability = signature.capability;
        }
        self.scope(Some(SymbolId(module.id.0)), id);
        for parameter in parameters {
            let parameter_id = SymbolId(parameter.symbol_id.0);
            let ty = crate::compiler::declared_type(&parameter.annotation).0;
            self.add(
                parameter_id,
                &parameter.name,
                SymbolKind::Parameter,
                SymbolOrigin {
                    name: Some(parameter.name_origin),
                    full: Some(parameter.origin),
                    container: Some(id),
                },
                SymbolInfo {
                    documentation: format!("{}: {}", parameter.name, ty),
                    ty: Some(ty),
                    read_only: false,
                },
            );
            self.output
                .scopes
                .get_mut(&id)
                .map(|scope| scope.names.insert(parameter.name.clone(), parameter_id));
        }
    }
    fn unify(&mut self, left: &Shape, right: &Shape) {
        for (name, left) in &left.fields {
            if let Some(right) = right.fields.get(name) {
                if left.id != right.id {
                    self.output.field_constraints.push((left.id, right.id));
                }
                self.unify(&left.value.shape, &right.value.shape);
            }
        }
    }
    fn annotation(&mut self, annotation: &HirType, owner: SymbolId, read_only: bool) -> ValueInfo {
        let ty = crate::compiler::declared_type(annotation).0;
        let mut shape = Shape::default();
        if let HirTypeKind::Record(fields) = &annotation.kind {
            self.scope(None, owner);
            for field in fields {
                let id = SymbolId(webtest_hir::semantic_key(&owner.0, "field", &field.name, 0));
                let value = self.annotation(&field.ty, id, read_only);
                self.add(
                    id,
                    &field.name,
                    SymbolKind::RecordField,
                    SymbolOrigin {
                        name: Some(field.name_origin),
                        full: Some(field.origin),
                        container: Some(owner),
                    },
                    SymbolInfo {
                        ty: Some(value.ty.clone()),
                        documentation: String::new(),
                        read_only,
                    },
                );
                self.output
                    .scopes
                    .get_mut(&owner)
                    .map(|scope| scope.names.insert(field.name.clone(), id));
                self.reference(
                    id,
                    id,
                    field.name_origin,
                    owner,
                    ReferenceRole {
                        declaration: true,
                        quoted: false,
                        scope_sensitive: true,
                    },
                );
                shape.fields.insert(field.name.clone(), Field { id, value });
            }
        }
        ValueInfo {
            ty,
            shape,
            external: read_only,
        }
    }
    fn external(&mut self, ty: &Type, owner: SymbolId) -> ValueInfo {
        let mut shape = Shape::default();
        if let Type::Record(fields) = ty {
            self.scope(None, owner);
            for (name, field) in fields {
                let id = SymbolId(webtest_hir::semantic_key(&owner.0, "field", name, 0));
                let value = self.external(&field.ty, id);
                self.add(
                    id,
                    name,
                    SymbolKind::RecordField,
                    SymbolOrigin {
                        name: None,
                        full: None,
                        container: Some(owner),
                    },
                    SymbolInfo {
                        ty: Some(field.ty.clone()),
                        documentation: field.documentation.clone(),
                        read_only: true,
                    },
                );
                self.output
                    .scopes
                    .get_mut(&owner)
                    .map(|scope| scope.names.insert(name.clone(), id));
                shape.fields.insert(name.clone(), Field { id, value });
            }
        }
        ValueInfo {
            ty: ty.clone(),
            shape,
            external: true,
        }
    }
    fn expression(&mut self, expression: &HirExpr, scope: SymbolId, owner: SymbolId) -> ValueInfo {
        let mut value = self.expression_shape(expression, scope, owner);
        if let Some(fact) = self
            .type_facts
            .iter()
            .find(|fact| fact.range == expression.origin.range)
        {
            value.ty = fact.ty.clone();
        }
        value
    }
    fn expression_shape(
        &mut self,
        expression: &HirExpr,
        scope: SymbolId,
        owner: SymbolId,
    ) -> ValueInfo {
        self.db.unwind_if_revision_cancelled();
        match &expression.kind {
            HirExprKind::Name(HirNameRef::Binding { id, .. }) => {
                if let Some(symbol) = self.locals.get(id).copied() {
                    self.reference(
                        symbol,
                        symbol,
                        expression.origin,
                        scope,
                        ReferenceRole {
                            declaration: false,
                            quoted: false,
                            scope_sensitive: true,
                        },
                    );
                }
                self.values
                    .get(id)
                    .cloned()
                    .unwrap_or_else(ValueInfo::unknown)
            }
            HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                if let Some((symbol, resolved)) = self.globals.get(name).copied() {
                    self.reference(
                        symbol,
                        resolved,
                        expression.origin,
                        scope,
                        ReferenceRole {
                            declaration: false,
                            quoted: false,
                            scope_sensitive: true,
                        },
                    );
                }
                ValueInfo::unknown()
            }
            HirExprKind::Literal(_) => ValueInfo::unknown(),
            HirExprKind::Record(fields) => {
                self.scope(None, owner);
                let mut shape = Shape::default();
                let mut types = BTreeMap::new();
                for field in fields {
                    let id = SymbolId(webtest_hir::semantic_key(&owner.0, "field", &field.name, 0));
                    let value = self.expression(&field.value, scope, id);
                    self.add(
                        id,
                        &field.name,
                        SymbolKind::RecordField,
                        SymbolOrigin {
                            name: Some(field.name_origin),
                            full: Some(field.origin),
                            container: Some(owner),
                        },
                        SymbolInfo {
                            ty: Some(value.ty.clone()),
                            documentation: String::new(),
                            read_only: false,
                        },
                    );
                    self.output
                        .scopes
                        .get_mut(&owner)
                        .map(|scope| scope.names.insert(field.name.clone(), id));
                    self.reference(
                        id,
                        id,
                        field.name_origin,
                        owner,
                        ReferenceRole {
                            declaration: true,
                            quoted: field.quoted,
                            scope_sensitive: true,
                        },
                    );
                    types.insert(
                        field.name.clone(),
                        RecordField {
                            ty: value.ty.clone(),
                            optional: false,
                            documentation: String::new(),
                            secret: false,
                        },
                    );
                    shape.fields.insert(field.name.clone(), Field { id, value });
                }
                ValueInfo {
                    ty: Type::Record(types),
                    shape,
                    external: false,
                }
            }
            HirExprKind::List(values) => {
                let mut ty = Type::Unknown;
                for (index, value) in values.iter().enumerate() {
                    let id = SymbolId(webtest_hir::semantic_key(
                        &owner.0,
                        "item",
                        "",
                        index as u32,
                    ));
                    let value = self.expression(value, scope, id);
                    if index == 0 {
                        ty = value.ty;
                    }
                }
                ValueInfo {
                    ty: Type::List(Box::new(ty)),
                    shape: Shape::default(),
                    external: false,
                }
            }
            HirExprKind::Member {
                receiver,
                member,
                member_origin,
            } => {
                let value = self.expression(receiver, scope, owner);
                if let Some(field) = value.shape.fields.get(member).cloned() {
                    let field_scope = self
                        .output
                        .symbols
                        .get(&field.id)
                        .and_then(|symbol| symbol.container)
                        .unwrap_or(owner);
                    self.reference(
                        field.id,
                        field.id,
                        *member_origin,
                        field_scope,
                        ReferenceRole {
                            declaration: false,
                            quoted: false,
                            scope_sensitive: true,
                        },
                    );
                    return field.value;
                }
                if let Some(ty) = value.ty.member(member) {
                    return self.external(&ty, owner);
                }
                ValueInfo {
                    ty: Type::Unknown,
                    shape: Shape::default(),
                    external: value.external,
                }
            }
            HirExprKind::Call { callee, arguments } => {
                let _ = self.expression(callee, scope, owner);
                let target = match &callee.kind {
                    HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                        callables::resolve(self.graph, self.module.id, name)
                    }
                    _ => None,
                };
                if let Some((module, target)) = target {
                    self.callable(module, target);
                    let Some(target_module) = self.graph.modules.get(&module) else {
                        return ValueInfo::unknown();
                    };
                    let Some(callable) = callables::find(target_module, target) else {
                        return ValueInfo::unknown();
                    };
                    let (parameters, result) = match callable {
                        Callable::Function(value) => (&value.parameters, &value.result_type),
                        Callable::Fixture(value) => (&value.parameters, &value.result_type),
                    };
                    let argument_scope = self.key(scope, "arguments", "call");
                    self.scope(None, argument_scope);
                    let mut positional = 0;
                    for argument in arguments {
                        let parameter = if let Some(name) = &argument.name {
                            parameters.iter().find(|parameter| parameter.name == *name)
                        } else {
                            let parameter = parameters.get(positional);
                            positional += 1;
                            parameter
                        };
                        let value = self.expression(&argument.value, scope, owner);
                        if parameter.is_none()
                            && let Some(name) = &argument.name
                        {
                            let unknown = SymbolId(webtest_hir::semantic_key(
                                &argument_scope.0,
                                "unresolved",
                                name,
                                0,
                            ));
                            if let Some(scope) = self.output.scopes.get_mut(&argument_scope) {
                                scope.names.insert(name.clone(), unknown);
                            }
                        }
                        if let Some(parameter) = parameter {
                            let id = SymbolId(parameter.symbol_id.0);
                            let expected = self.annotation(&parameter.annotation, id, false);
                            self.unify(&expected.shape, &value.shape);
                            if let Some(origin) = argument.name_origin {
                                self.output
                                    .scopes
                                    .get_mut(&argument_scope)
                                    .map(|scope| scope.names.insert(parameter.name.clone(), id));
                                self.reference(
                                    id,
                                    id,
                                    origin,
                                    argument_scope,
                                    ReferenceRole {
                                        declaration: false,
                                        quoted: false,
                                        scope_sensitive: true,
                                    },
                                );
                            }
                        }
                    }
                    return self.annotation(
                        result,
                        SymbolId(webtest_hir::semantic_key(&target.0, "return", "", 0)),
                        false,
                    );
                }
                if let HirExprKind::Member {
                    receiver,
                    member,
                    member_origin,
                } = &callee.kind
                    && let HirExprKind::Name(HirNameRef::Unresolved(provider)) = &receiver.kind
                    && let Some(schema) = self.db.schema_slot(provider).schema(self.db).as_ref()
                    && let Some(operation) = schema.operations.get(member)
                {
                    let namespace = SymbolId(webtest_hir::semantic_key(
                        &self.db.workspace_input().identity(self.db).0,
                        "provider",
                        provider,
                        0,
                    ));
                    let function = SymbolId(webtest_hir::semantic_key(
                        &namespace.0,
                        "operation",
                        member,
                        0,
                    ));
                    self.add(
                        namespace,
                        provider,
                        SymbolKind::ProviderNamespace,
                        SymbolOrigin {
                            name: None,
                            full: None,
                            container: None,
                        },
                        SymbolInfo {
                            ty: None,
                            documentation: format!("provider {provider}; schema {}", schema.hash()),
                            read_only: true,
                        },
                    );
                    self.add(
                        function,
                        member,
                        SymbolKind::ProviderFunction,
                        SymbolOrigin {
                            name: None,
                            full: None,
                            container: Some(namespace),
                        },
                        SymbolInfo {
                            ty: Some(operation.result.clone()),
                            documentation: operation.documentation.clone(),
                            read_only: true,
                        },
                    );
                    if let Some(symbol) = self.output.symbols.get_mut(&function) {
                        symbol.capability = operation.capability;
                    }
                    self.reference(
                        namespace,
                        namespace,
                        receiver.origin,
                        scope,
                        ReferenceRole {
                            declaration: false,
                            quoted: false,
                            scope_sensitive: false,
                        },
                    );
                    self.reference(
                        function,
                        function,
                        *member_origin,
                        scope,
                        ReferenceRole {
                            declaration: false,
                            quoted: false,
                            scope_sensitive: false,
                        },
                    );
                    for argument in arguments {
                        self.expression(&argument.value, scope, owner);
                    }
                    return self.external(&operation.result, function);
                }
                for argument in arguments {
                    self.expression(&argument.value, scope, owner);
                }
                ValueInfo::unknown()
            }
            HirExprKind::FixtureUse(call) => self.expression(call, scope, owner),
            HirExprKind::Unary { operand, .. } => {
                self.expression(operand, scope, owner);
                ValueInfo::unknown()
            }
            HirExprKind::Binary { left, right, .. } => {
                self.expression(left, scope, owner);
                self.expression(right, scope, owner);
                ValueInfo::unknown()
            }
            HirExprKind::Missing => ValueInfo::unknown(),
        }
    }
    fn statements(
        &mut self,
        statements: &[HirStmt],
        scope: SymbolId,
        domain: Capability,
        returns: Option<&Shape>,
    ) {
        for statement in statements {
            match statement {
                HirStmt::Let(binding) => {
                    let id = self.key(scope, "binding", &binding.name);
                    let mut value = self.expression(&binding.value, scope, id);
                    if let Some(annotation) = &binding.annotation {
                        let annotated = self.annotation(annotation, id, value.external);
                        self.unify(&value.shape, &annotated.shape);
                        value.ty = annotated.ty;
                        if value.shape.fields.is_empty() {
                            value.shape = annotated.shape;
                        }
                    }
                    self.add(
                        id,
                        &binding.name,
                        SymbolKind::Binding,
                        SymbolOrigin {
                            name: Some(binding.name_origin),
                            full: Some(binding.origin),
                            container: Some(scope),
                        },
                        SymbolInfo {
                            ty: Some(value.ty.clone()),
                            documentation: String::new(),
                            read_only: false,
                        },
                    );
                    if let Some(symbol) = self.output.symbols.get_mut(&id) {
                        symbol.capability = domain;
                    }
                    self.output
                        .scopes
                        .get_mut(&scope)
                        .map(|scope| scope.names.insert(binding.name.clone(), id));
                    self.locals.insert(binding.id, id);
                    self.values.insert(binding.id, value);
                    self.reference(
                        id,
                        id,
                        binding.name_origin,
                        scope,
                        ReferenceRole {
                            declaration: true,
                            quoted: false,
                            scope_sensitive: true,
                        },
                    );
                }
                HirStmt::Expression(value) => {
                    self.expression(&value.expression, scope, scope);
                }
                HirStmt::Provide(value) => {
                    let value = self.expression(&value.expression, scope, scope);
                    if let Some(returns) = returns {
                        self.unify(&value.shape, returns);
                    }
                }
                HirStmt::Expect(value) => {
                    self.expression(&value.expression, scope, scope);
                }
                HirStmt::Server(value) => {
                    self.statements(&value.statements, scope, Capability::Server, returns)
                }
                HirStmt::Browser(value) => {
                    self.statements(&value.statements, scope, Capability::Browser, returns)
                }
                HirStmt::BrowserOperation(value) => match value {
                    webtest_hir::HirBrowserOp::Open(value) => {
                        self.expression(&value.url, scope, scope);
                    }
                    webtest_hir::HirBrowserOp::Fill(value)
                    | webtest_hir::HirBrowserOp::Type(value)
                    | webtest_hir::HirBrowserOp::Press(value)
                    | webtest_hir::HirBrowserOp::Select(value) => {
                        self.expression(&value.value, scope, scope);
                    }
                    _ => {}
                },
                _ => {
                    let child = self.key(
                        scope,
                        "scope",
                        match statement {
                            HirStmt::Timeout(_) => "timeout",
                            HirStmt::Retry(_) => "retry",
                            HirStmt::Parallel(_) => "parallel",
                            HirStmt::Race(_) => "race",
                            _ => "control",
                        },
                    );
                    self.scope(Some(scope), child);
                    if matches!(statement, HirStmt::Parallel(_) | HirStmt::Race(_)) {
                        for branch in statement.children() {
                            let branch_scope = self.key(child, "branch", "");
                            self.scope(Some(child), branch_scope);
                            self.statements(
                                std::slice::from_ref(branch),
                                branch_scope,
                                domain,
                                returns,
                            );
                        }
                    } else {
                        self.statements(statement.children(), child, domain, returns);
                    }
                    if let HirStmt::Race(race) = statement
                        && let Some(binding) = &race.binding
                    {
                        let id = self.key(scope, "binding", &binding.name);
                        self.add(
                            id,
                            &binding.name,
                            SymbolKind::Binding,
                            SymbolOrigin {
                                name: Some(binding.name_origin),
                                full: Some(race.origin),
                                container: Some(scope),
                            },
                            SymbolInfo {
                                ty: None,
                                documentation: String::new(),
                                read_only: false,
                            },
                        );
                        self.locals.insert(binding.id, id);
                        self.reference(
                            id,
                            id,
                            binding.name_origin,
                            scope,
                            ReferenceRole {
                                declaration: true,
                                quoted: false,
                                scope_sensitive: true,
                            },
                        );
                    }
                }
            }
        }
    }
}

#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn symbols(db: &dyn Db, file: SourceInput) -> Arc<SymbolIndex> {
    let graph = crate::workspace::module_graph(db, file);
    let Some(module) = graph.modules.get(&graph.root) else {
        return Arc::new(SymbolIndex::default());
    };
    let root = SymbolId(module.id.0);
    let mut index = Indexer {
        db,
        graph: &graph,
        module,
        output: SymbolIndex::default(),
        locals: HashMap::new(),
        values: HashMap::new(),
        counters: HashMap::new(),
        globals: BTreeMap::new(),
        type_facts: incremental::compiled(db, file).type_facts.clone(),
    };
    index.scope(None, root);
    index.add(
        root,
        &module.path,
        SymbolKind::Module,
        SymbolOrigin {
            name: Some(SyntaxOrigin::new(
                module.file,
                incremental::parsed(db, file).syntax().text_range(),
            )),
            full: None,
            container: None,
        },
        SymbolInfo {
            ty: None,
            documentation: String::new(),
            read_only: false,
        },
    );
    for function in &module.hir.functions {
        let id = SymbolId(function.id.0);
        index.callable(module.id, id);
        index.globals.insert(function.name.clone(), (id, id));
        index
            .output
            .scopes
            .get_mut(&root)
            .map(|scope| scope.names.insert(function.name.clone(), id));
        index.reference(
            id,
            id,
            function.name_origin,
            root,
            ReferenceRole {
                declaration: true,
                quoted: false,
                scope_sensitive: true,
            },
        );
    }
    for fixture in &module.hir.fixtures {
        let id = SymbolId(fixture.id.0);
        index.callable(module.id, id);
        index.globals.insert(fixture.name.clone(), (id, id));
        index
            .output
            .scopes
            .get_mut(&root)
            .map(|scope| scope.names.insert(fixture.name.clone(), id));
        index.reference(
            id,
            id,
            fixture.name_origin,
            root,
            ReferenceRole {
                declaration: true,
                quoted: false,
                scope_sensitive: true,
            },
        );
    }
    for import in &module.hir.imports {
        for binding in &import.bindings {
            let Some(resolved) = module.imports.get(&binding.alias) else {
                continue;
            };
            index.callable(resolved.target_module, resolved.target);
            let explicit = binding.name_origin != binding.alias_origin;
            index.reference(
                resolved.target,
                resolved.target,
                binding.name_origin,
                root,
                ReferenceRole {
                    declaration: false,
                    quoted: false,
                    scope_sensitive: !explicit,
                },
            );
            let local = if explicit {
                let id = index.key(root, "import", &binding.alias);
                let ty = index
                    .output
                    .symbols
                    .get(&resolved.target)
                    .and_then(|symbol| symbol.ty.clone());
                index.add(
                    id,
                    &binding.alias,
                    SymbolKind::Import,
                    SymbolOrigin {
                        name: Some(binding.alias_origin),
                        full: Some(import.origin),
                        container: Some(root),
                    },
                    SymbolInfo {
                        ty,
                        documentation: String::new(),
                        read_only: false,
                    },
                );
                index.reference(
                    id,
                    resolved.target,
                    binding.alias_origin,
                    root,
                    ReferenceRole {
                        declaration: true,
                        quoted: false,
                        scope_sensitive: true,
                    },
                );
                id
            } else {
                resolved.target
            };
            index
                .globals
                .insert(binding.alias.clone(), (local, resolved.target));
            index
                .output
                .scopes
                .get_mut(&root)
                .map(|scope| scope.names.insert(binding.alias.clone(), local));
        }
    }
    for builtin in callables::builtins() {
        if index.globals.contains_key(builtin.name) {
            continue;
        }
        let id = callables::builtin_id(builtin);
        index.add(
            id,
            builtin.name,
            SymbolKind::Function,
            SymbolOrigin {
                name: None,
                full: None,
                container: None,
            },
            SymbolInfo {
                ty: Some(builtin.result.clone()),
                documentation: format!(
                    "{}({}) -> {}\n\n{}",
                    builtin.name,
                    builtin
                        .parameters
                        .iter()
                        .map(|(name, ty)| format!("{name}: {ty}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    builtin.result,
                    builtin.documentation
                ),
                read_only: true,
            },
        );
        index.globals.insert(builtin.name.into(), (id, id));
    }
    for function in &module.hir.functions {
        let id = SymbolId(function.id.0);
        index.locals.clear();
        index.values.clear();
        for parameter in &function.parameters {
            let symbol = SymbolId(parameter.symbol_id.0);
            let value = index.annotation(&parameter.annotation, symbol, false);
            index.locals.insert(parameter.id, symbol);
            index.values.insert(parameter.id, value);
            index.reference(
                symbol,
                symbol,
                parameter.name_origin,
                id,
                ReferenceRole {
                    declaration: true,
                    quoted: false,
                    scope_sensitive: true,
                },
            );
        }
        let result = index.annotation(
            &function.result_type,
            SymbolId(webtest_hir::semantic_key(&id.0, "return", "", 0)),
            false,
        );
        if let Some(expression) = &function.expression {
            let value = index.expression(expression, id, id);
            index.unify(&value.shape, &result.shape);
        }
        index.statements(
            &function.body,
            id,
            function.capability.unwrap_or(Capability::Pure),
            Some(&result.shape),
        );
    }
    for fixture in &module.hir.fixtures {
        let id = SymbolId(fixture.id.0);
        index.locals.clear();
        index.values.clear();
        for parameter in &fixture.parameters {
            let symbol = SymbolId(parameter.symbol_id.0);
            let value = index.annotation(&parameter.annotation, symbol, false);
            index.locals.insert(parameter.id, symbol);
            index.values.insert(parameter.id, value);
            index.reference(
                symbol,
                symbol,
                parameter.name_origin,
                id,
                ReferenceRole {
                    declaration: true,
                    quoted: false,
                    scope_sensitive: true,
                },
            );
        }
        let result = index.annotation(
            &fixture.result_type,
            SymbolId(webtest_hir::semantic_key(&id.0, "return", "", 0)),
            false,
        );
        index.statements(&fixture.setup, id, Capability::Pure, Some(&result.shape));
        index.statements(&fixture.teardown, id, Capability::Pure, None);
    }
    for test in &module.hir.tests {
        let id = SymbolId(test.declaration_id.0);
        index.scope(Some(root), id);
        index.locals.clear();
        index.values.clear();
        index.add(
            id,
            &test.name,
            SymbolKind::Test,
            SymbolOrigin {
                name: Some(test.origin),
                full: Some(test.origin),
                container: Some(root),
            },
            SymbolInfo {
                ty: None,
                documentation: String::new(),
                read_only: true,
            },
        );
        index.statements(&test.body, id, Capability::Pure, None);
    }
    index.output.references.sort_by_key(|reference| {
        (
            u32::from(reference.location.origin.range.start()),
            u32::from(reference.location.origin.range.end()),
            reference.symbol,
        )
    });
    Arc::new(index.output)
}
