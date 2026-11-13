//! One resolved declaration/signature contract for planning and editor queries.
use crate::{
    incremental::{self, Db, SourceInput},
    workspace::{ModuleDiagnostic, ModuleGraph},
};
use std::{collections::BTreeSet, sync::Arc};
use webtest_hir::{HirExpr, HirExprKind, HirNameRef, HirStmt};
use webtest_model::{Capability, FixtureLifetime, ModuleId, ParameterId, SymbolId, Type};
use webtest_text::SyntaxOrigin;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DeclarationParameter {
    pub id: ParameterId,
    pub name: String,
    pub ty: Type,
    pub origin: SyntaxOrigin,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationSignature {
    pub id: SymbolId,
    pub name: String,
    pub origin: SyntaxOrigin,
    pub parameters: Vec<DeclarationParameter>,
    pub result: Type,
    pub capability: Capability,
    pub expression_only: bool,
    pub lifetime: Option<FixtureLifetime>,
    pub shareable: bool,
    pub documentation: String,
    pub diagnostics: Vec<ModuleDiagnostic>,
}

#[derive(Clone, Copy)]
pub(crate) enum Callable<'a> {
    Function(&'a webtest_hir::HirFunction),
    Fixture(&'a webtest_hir::HirFixture),
}
pub(crate) fn find(module: &crate::Module, id: SymbolId) -> Option<Callable<'_>> {
    module
        .hir
        .functions
        .iter()
        .find(|function| function.id.0 == id.0)
        .map(Callable::Function)
        .or_else(|| {
            module
                .hir
                .fixtures
                .iter()
                .find(|fixture| fixture.id.0 == id.0)
                .map(Callable::Fixture)
        })
}
pub(crate) fn resolve(
    graph: &ModuleGraph,
    module: ModuleId,
    name: &str,
) -> Option<(ModuleId, SymbolId)> {
    let module = graph.modules.get(&module)?;
    resolve_in_module(module, name)
}
fn resolve_in_module(module: &crate::Module, name: &str) -> Option<(ModuleId, SymbolId)> {
    module
        .hir
        .functions
        .iter()
        .find(|function| function.name == name)
        .map(|function| (module.id, SymbolId(function.id.0)))
        .or_else(|| {
            module
                .hir
                .fixtures
                .iter()
                .find(|fixture| fixture.name == name)
                .map(|fixture| (module.id, SymbolId(fixture.id.0)))
        })
        .or_else(|| {
            module
                .imports
                .get(name)
                .map(|import| (import.target_module, import.target))
        })
}
fn basic(db: &dyn Db, file: SourceInput, id: SymbolId) -> Option<DeclarationSignature> {
    let hir = incremental::lowered(db, file);
    let mut diagnostics = Vec::new();
    let (
        name,
        origin,
        parameters,
        result,
        capability,
        expression_only,
        lifetime,
        shareable,
        documentation,
    ) = if let Some(function) = hir.functions.iter().find(|function| function.id.0 == id.0) {
        (
            &function.name,
            function.name_origin,
            &function.parameters,
            &function.result_type,
            function.capability.unwrap_or(Capability::Pure),
            function.expression.is_some(),
            None,
            false,
            &function.documentation,
        )
    } else if let Some(fixture) = hir.fixtures.iter().find(|fixture| fixture.id.0 == id.0) {
        if fixture.setup_count != 1 {
            diagnostics.push(ModuleDiagnostic {
                code: "semantic.fixture_setup_count",
                message: "a fixture requires exactly one setup phase".into(),
                origin: fixture.name_origin,
                related: Vec::new(),
                candidates: Vec::new(),
            });
        }
        if fixture.teardown_count > 1 {
            diagnostics.push(ModuleDiagnostic {
                code: "semantic.fixture_teardown_count",
                message: "a fixture has at most one teardown phase".into(),
                origin: fixture.name_origin,
                related: Vec::new(),
                candidates: Vec::new(),
            });
        }
        (
            &fixture.name,
            fixture.name_origin,
            &fixture.parameters,
            &fixture.result_type,
            Capability::Pure,
            false,
            Some(fixture.lifetime),
            fixture.shareable,
            &fixture.documentation,
        )
    } else {
        return None;
    };
    let mut names = BTreeSet::new();
    let parameters: Vec<_> = parameters
        .iter()
        .map(|parameter| {
            let (ty, errors) = crate::compiler::declared_type(&parameter.annotation);
            diagnostics.extend(errors.into_iter().map(|error| ModuleDiagnostic {
                code: error.code,
                message: error.message,
                origin: SyntaxOrigin::new(parameter.origin.file, error.range),
                related: Vec::new(),
                candidates: Vec::new(),
            }));
            if !names.insert(parameter.name.clone()) {
                diagnostics.push(ModuleDiagnostic {
                    code: "semantic.duplicate_parameter",
                    message: format!("duplicate parameter `{}`", parameter.name),
                    origin: parameter.name_origin,
                    related: Vec::new(),
                    candidates: Vec::new(),
                });
            }
            DeclarationParameter {
                id: parameter.symbol_id,
                name: parameter.name.clone(),
                ty,
                origin: parameter.name_origin,
            }
        })
        .collect();
    if lifetime.is_some() {
        for parameter in &parameters {
            if !parameter.ty.is_transferable() {
                diagnostics.push(ModuleDiagnostic {
                    code: "semantic.non_transferable_fixture_parameter",
                    message: "fixture parameters must be transferable values; acquire native dependencies inside setup".into(),
                    origin: parameter.origin,
                    related: Vec::new(),
                    candidates: Vec::new(),
                });
            }
        }
    }
    let (result, errors) = crate::compiler::declared_type(result);
    diagnostics.extend(errors.into_iter().map(|error| ModuleDiagnostic {
        code: error.code,
        message: error.message,
        origin: SyntaxOrigin::new(origin.file, error.range),
        related: Vec::new(),
        candidates: Vec::new(),
    }));
    if lifetime.is_some_and(|lifetime| lifetime != FixtureLifetime::Test)
        && (!shareable || !result.is_transferable())
    {
        diagnostics.push(ModuleDiagnostic {
            code: "semantic.fixture_not_shareable",
            message:
                "file, worker, and suite fixtures require `shareable` and a transferable result"
                    .into(),
            origin,
            related: Vec::new(),
            candidates: Vec::new(),
        });
    }
    Some(DeclarationSignature {
        id,
        name: name.clone(),
        origin,
        parameters,
        result,
        capability,
        expression_only,
        lifetime,
        shareable,
        documentation: documentation.clone(),
        diagnostics,
    })
}
fn cycle(
    db: &dyn Db,
    _key: salsa::Id,
    file: SourceInput,
    id: SymbolId,
) -> Option<Arc<DeclarationSignature>> {
    let mut signature = basic(db, file, id)?;
    signature.diagnostics.push(ModuleDiagnostic {
        code: if signature.lifetime.is_some() {
            "semantic.fixture_dependency_cycle"
        } else {
            "semantic.recursive_helper"
        },
        message: format!(
            "declaration `{}` participates in a recursive dependency",
            signature.name
        ),
        origin: signature.origin,
        related: Vec::new(),
        candidates: Vec::new(),
    });
    Some(Arc::new(signature))
}
fn capability(capabilities: &BTreeSet<Capability>) -> Capability {
    if capabilities.contains(&Capability::Test) {
        return Capability::Test;
    }
    match (
        capabilities.contains(&Capability::Server),
        capabilities.contains(&Capability::Browser),
    ) {
        (true, true) => Capability::Test,
        (true, false) => Capability::Server,
        (false, true) => Capability::Browser,
        _ => {
            if capabilities.contains(&Capability::Test) {
                Capability::Test
            } else {
                Capability::Pure
            }
        }
    }
}
#[salsa::tracked(no_eq, lru = 2048, cycle_result = cycle)]
pub(crate) fn signature(
    db: &dyn Db,
    file: SourceInput,
    id: SymbolId,
) -> Option<Arc<DeclarationSignature>> {
    let mut result = basic(db, file, id)?;
    let resolved = crate::workspace::resolved_module(db, file);
    let module = &resolved.module;
    let callable = find(module, id)?;
    let mut roots = Vec::new();
    let mut capabilities = BTreeSet::from([result.capability]);
    fn statements<'a>(
        body: &'a [HirStmt],
        roots: &mut Vec<&'a HirExpr>,
        capabilities: &mut BTreeSet<Capability>,
    ) {
        for statement in body {
            if matches!(statement, HirStmt::BrowserOperation(_)) {
                capabilities.insert(Capability::Browser);
            }
            // Each callback sees nested calls once; do not traverse child bodies again.
            webtest_hir::visit_statement_expressions(statement, &mut |expression| {
                roots.push(expression)
            });
            fn browser(body: &[HirStmt], capabilities: &mut BTreeSet<Capability>) {
                for statement in body {
                    if matches!(statement, HirStmt::BrowserOperation(_)) {
                        capabilities.insert(Capability::Browser);
                    }
                    browser(statement.children(), capabilities);
                }
            }
            browser(statement.children(), capabilities);
        }
    }
    match callable {
        Callable::Function(function) => {
            if let Some(expression) = &function.expression {
                webtest_hir::visit_expression(expression, &mut |expression| roots.push(expression));
            }
            statements(&function.body, &mut roots, &mut capabilities);
        }
        Callable::Fixture(fixture) => {
            statements(&fixture.setup, &mut roots, &mut capabilities);
            statements(&fixture.teardown, &mut roots, &mut capabilities);
        }
    }
    for expression in roots {
        db.unwind_if_revision_cancelled();
        let HirExprKind::Call { callee, .. } = &expression.kind else {
            continue;
        };
        match &callee.kind {
            HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                if let Some((target_module, target)) = resolve_in_module(module, name)
                    && let Some(target_file) = if target_module == module.id {
                        Some(file)
                    } else {
                        resolved.targets.iter().copied().find(|target| {
                            incremental::lowered(db, *target).module_id == target_module
                        })
                    }
                    && let Some(dependency) = signature(db, target_file, target)
                {
                    capabilities.insert(dependency.capability);
                    result
                        .diagnostics
                        .extend(dependency.diagnostics.iter().cloned());
                    if let (Some(owner), Some(dependency_lifetime)) =
                        (result.lifetime, dependency.lifetime)
                        && owner.rank() > dependency_lifetime.rank()
                    {
                        result.diagnostics.push(ModuleDiagnostic {
                            code: "semantic.fixture_lifetime_dependency",
                            message:
                                "a longer-lived fixture cannot depend on a shorter-lived fixture"
                                    .into(),
                            origin: expression.origin,
                            related: vec![dependency.origin],
                            candidates: Vec::new(),
                        });
                    }
                }
            }
            HirExprKind::Member {
                receiver, member, ..
            } => {
                if let HirExprKind::Name(HirNameRef::Unresolved(provider)) = &receiver.kind
                    && let Some(operation) = db
                        .schema_slot(provider)
                        .schema(db)
                        .as_ref()
                        .and_then(|schema| schema.operations.get(member))
                {
                    capabilities.insert(operation.capability);
                }
            }
            _ => {}
        }
    }
    let inferred = capability(&capabilities);
    if let Callable::Function(function) = callable
        && let Some(declared) = function.capability
        && inferred != Capability::Pure
        && inferred != declared
    {
        result.diagnostics.push(ModuleDiagnostic {
            code: "semantic.helper_capability_mismatch",
            message: format!("helper declares {declared} but requires {inferred}"),
            origin: function.name_origin,
            related: Vec::new(),
            candidates: Vec::new(),
        });
    }
    result.capability = inferred;
    result.diagnostics.dedup();
    Some(Arc::new(result))
}
#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn declaration_diagnostics(
    db: &dyn Db,
    file: SourceInput,
) -> Arc<Vec<ModuleDiagnostic>> {
    let hir = incremental::lowered(db, file);
    let mut diagnostics = Vec::new();
    for id in hir
        .functions
        .iter()
        .map(|function| SymbolId(function.id.0))
        .chain(hir.fixtures.iter().map(|fixture| SymbolId(fixture.id.0)))
    {
        if let Some(signature) = signature(db, file, id) {
            diagnostics.extend(signature.diagnostics.iter().cloned());
        }
    }
    Arc::new(diagnostics)
}
/// Installed pure builtins share signature facts across checking and editor/catalog projections.
pub(crate) struct BuiltinSignature {
    pub id: &'static str,
    pub name: &'static str,
    pub parameters: &'static [(&'static str, Type)],
    pub result: Type,
    pub documentation: &'static str,
}
pub(crate) fn builtins() -> &'static [BuiltinSignature] {
    &[BuiltinSignature {
        id: "builtin.lower",
        name: "lower",
        parameters: &[("value", Type::String)],
        result: Type::String,
        documentation: "Return the Unicode lowercase form of a String without host effects.",
    }]
}
pub(crate) fn builtin(name: &str) -> Option<&'static BuiltinSignature> {
    builtins().iter().find(|builtin| builtin.name == name)
}
pub(crate) fn builtin_id(builtin: &BuiltinSignature) -> SymbolId {
    SymbolId(webtest_text::semantic_key(
        &[0; 32], "builtin", builtin.id, 0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnalysisDatabase;
    #[test]
    fn helper_signatures_follow_schema_capabilities_and_recover_from_recursion() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("helpers.webtest", "export fn recurse(value: String) -> String = recurse(value)\nfn read() -> Response<Json> server { let result = http.get(\"https://example.test\") provide result }");
        let hir = database.hir_file(file).unwrap();
        let recursive = database
            .declaration_signature(file, SymbolId(hir.functions[0].id.0))
            .unwrap()
            .unwrap();
        assert!(
            recursive
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.recursive_helper")
        );
        let read = database
            .declaration_signature(file, SymbolId(hir.functions[1].id.0))
            .unwrap()
            .unwrap();
        assert_eq!(read.capability, Capability::Server);
        database.set_file_text(file, "export fn recurse(value: String) -> String = value");
        let hir = database.hir_file(file).unwrap();
        let signature = database
            .declaration_signature(file, SymbolId(hir.functions[0].id.0))
            .unwrap()
            .unwrap();
        assert!(
            signature.diagnostics.is_empty(),
            "{:?}",
            signature.diagnostics
        );
        assert_eq!(signature.capability, Capability::Pure);
    }
    #[test]
    fn fixture_signatures_reject_shorter_dependencies_and_native_shared_values() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("fixtures.webtest", "fixture short() -> Int { setup { provide 1 } }\nfixture long() -> Int scope suite shareable { setup { let value = use short() provide value } }\nfixture unsafe_value() -> Response<Json> scope file shareable { setup {} }");
        let hir = database.hir_file(file).unwrap();
        let long = database
            .declaration_signature(file, SymbolId(hir.fixtures[1].id.0))
            .unwrap()
            .unwrap();
        assert!(
            long.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.fixture_lifetime_dependency")
        );
        let invalid = database
            .declaration_signature(file, SymbolId(hir.fixtures[2].id.0))
            .unwrap()
            .unwrap();
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.fixture_not_shareable")
        );
    }
}
