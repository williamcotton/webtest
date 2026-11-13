//! Module boundaries and reusable declarations lowered only through typed CST views.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct HirImport {
    pub path: String,
    pub bindings: Vec<HirImportBinding>,
    pub origin: SyntaxOrigin,
    pub path_origin: SyntaxOrigin,
}
#[derive(Clone, Debug, PartialEq)]
pub struct HirImportBinding {
    pub name: String,
    pub alias: String,
    pub name_origin: SyntaxOrigin,
    pub alias_origin: SyntaxOrigin,
}
#[derive(Clone, Debug, PartialEq)]
pub struct HirParameter {
    pub symbol_id: ParameterId,
    pub id: BindingId,
    pub name: String,
    pub annotation: HirType,
    pub origin: SyntaxOrigin,
    pub name_origin: SyntaxOrigin,
}
#[derive(Clone, Debug, PartialEq)]
pub struct HirFunction {
    pub capability: Option<webtest_model::Capability>,
    pub id: FunctionId,
    pub name: String,
    pub exported: bool,
    pub documentation: String,
    pub parameters: Vec<HirParameter>,
    pub result_type: HirType,
    pub expression: Option<HirExpr>,
    pub body: Vec<HirStmt>,
    pub origin: SyntaxOrigin,
    pub name_origin: SyntaxOrigin,
}
#[derive(Clone, Debug, PartialEq)]
pub struct HirFixture {
    pub id: FixtureId,
    pub name: String,
    pub exported: bool,
    pub documentation: String,
    pub parameters: Vec<HirParameter>,
    pub result_type: HirType,
    pub lifetime: FixtureLifetime,
    pub shareable: bool,
    pub setup_origin: SyntaxOrigin,
    pub teardown_origin: SyntaxOrigin,
    pub setup: Vec<HirStmt>,
    pub teardown: Vec<HirStmt>,
    pub setup_count: usize,
    pub teardown_count: usize,
    pub origin: SyntaxOrigin,
    pub name_origin: SyntaxOrigin,
}

fn parameters(
    context: &mut LowerContext,
    parameters: impl Iterator<Item = ast::Parameter>,
) -> Vec<HirParameter> {
    parameters
        .filter_map(|parameter| {
            let name = parameter.name()?;
            let id = BindingId(context.next_binding);
            context.next_binding += 1;
            context.bindings.insert(name.text().into(), id);
            Some(HirParameter {
                symbol_id: ParameterId::default(),
                id,
                name: name.text().into(),
                annotation: parameter
                    .annotation()
                    .and_then(|ty| lower_type(context.file, ty))
                    .unwrap_or(HirType {
                        kind: crate::HirTypeKind::Missing,
                        origin: SyntaxOrigin::new(
                            context.file,
                            webtest_text::TextRange::empty(name.text_range().end()),
                        ),
                    }),
                origin: origin(context.file, parameter.syntax()),
                name_origin: SyntaxOrigin::new(context.file, name.text_range()),
            })
        })
        .collect()
}

pub(super) fn lower_declarations(
    file: FileId,
    root: ast::Root,
    next_binding: &mut u32,
    lowered: &mut HirFile,
) {
    lowered.imports = root
        .imports()
        .filter_map(|import| {
            let path = import.path()?;
            Some(HirImport {
                path: path.value()?,
                path_origin: SyntaxOrigin::new(file, path.syntax().text_range()),
                origin: origin(file, import.syntax()),
                bindings: import
                    .bindings()
                    .filter_map(|binding| {
                        let name = binding.name()?;
                        let alias = binding.alias().unwrap_or_else(|| name.clone());
                        Some(HirImportBinding {
                            name: name.text().into(),
                            alias: alias.text().into(),
                            name_origin: SyntaxOrigin::new(file, name.text_range()),
                            alias_origin: SyntaxOrigin::new(file, alias.text_range()),
                        })
                    })
                    .collect(),
            })
        })
        .collect();
    let mut occurrences = HashMap::<String, u32>::new();
    for function in root.functions() {
        let Some(name) = function.name() else {
            continue;
        };
        let result_type = function
            .result_type()
            .and_then(|ty| lower_type(file, ty))
            .unwrap_or(HirType {
                kind: crate::HirTypeKind::Missing,
                origin: SyntaxOrigin::new(
                    file,
                    webtest_text::TextRange::empty(name.text_range().end()),
                ),
            });
        let mut context = LowerContext {
            file,
            bindings: HashMap::new(),
            next_binding: *next_binding,
        };
        let ordinal = occurrences
            .entry(
                function
                    .name()
                    .map(|name| name.text().to_owned())
                    .unwrap_or_default(),
            )
            .or_default();
        let function_id = FunctionId(semantic_key(
            &lowered.module_id.0,
            "function",
            name.text(),
            *ordinal,
        ));
        *ordinal += 1;
        let mut parameters = parameters(&mut context, function.parameters());
        let mut parameter_occurrences = HashMap::<String, u32>::new();
        for parameter in &mut parameters {
            let ordinal = parameter_occurrences
                .entry(parameter.name.clone())
                .or_default();
            parameter.symbol_id = ParameterId(semantic_key(
                &function_id.0,
                "parameter",
                &parameter.name,
                *ordinal,
            ));
            *ordinal += 1;
        }
        let expression = function
            .expression()
            .and_then(|expression| lower_expr(&context, expression));
        let capability = function.capability().map(|capability| match capability {
            ast::DeclaredCapability::Server => webtest_model::Capability::Server,
            ast::DeclaredCapability::Browser => webtest_model::Capability::Browser,
        });
        let body = if capability.is_some() {
            function
                .domain_statements()
                .filter_map(|statement| lower_domain_statement(&mut context, statement))
                .collect()
        } else {
            function
                .body()
                .into_iter()
                .flat_map(|body| body.statements().collect::<Vec<_>>())
                .filter_map(|statement| lower_flow_statement(&mut context, statement))
                .collect()
        };
        *next_binding = context.next_binding;
        lowered.functions.push(HirFunction {
            capability,
            id: function_id,
            name: name.text().into(),
            exported: function.exported(),
            documentation: function.documentation(),
            parameters,
            result_type,
            expression,
            body,
            origin: origin(file, function.syntax()),
            name_origin: SyntaxOrigin::new(file, name.text_range()),
        });
    }
    let mut occurrences = HashMap::<String, u32>::new();
    for fixture in root.fixtures() {
        let Some(name) = fixture.name() else {
            continue;
        };
        let result_type = fixture
            .result_type()
            .and_then(|ty| lower_type(file, ty))
            .unwrap_or(HirType {
                kind: crate::HirTypeKind::Missing,
                origin: SyntaxOrigin::new(
                    file,
                    webtest_text::TextRange::empty(name.text_range().end()),
                ),
            });
        let mut context = LowerContext {
            file,
            bindings: HashMap::new(),
            next_binding: *next_binding,
        };
        let ordinal = occurrences
            .entry(
                fixture
                    .name()
                    .map(|name| name.text().to_owned())
                    .unwrap_or_default(),
            )
            .or_default();
        let fixture_id = FixtureId(semantic_key(
            &lowered.module_id.0,
            "fixture",
            name.text(),
            *ordinal,
        ));
        *ordinal += 1;
        let mut parameters = parameters(&mut context, fixture.parameters());
        let mut parameter_occurrences = HashMap::<String, u32>::new();
        for parameter in &mut parameters {
            let ordinal = parameter_occurrences
                .entry(parameter.name.clone())
                .or_default();
            parameter.symbol_id = ParameterId(semantic_key(
                &fixture_id.0,
                "parameter",
                &parameter.name,
                *ordinal,
            ));
            *ordinal += 1;
        }
        let mut setup = Vec::new();
        for phase in fixture.setups() {
            lower_phase(
                &mut context,
                phase.body(),
                phase.server_body(),
                phase.browser_body(),
                &mut setup,
            );
        }
        let mut teardown = Vec::new();
        for phase in fixture.teardowns() {
            lower_phase(
                &mut context,
                phase.body(),
                phase.server_body(),
                phase.browser_body(),
                &mut teardown,
            );
        }
        *next_binding = context.next_binding;
        let lifetime = match fixture.lifetime() {
            ast::FixtureLifetime::Test => FixtureLifetime::Test,
            ast::FixtureLifetime::File => FixtureLifetime::File,
            ast::FixtureLifetime::Worker => FixtureLifetime::Worker,
            ast::FixtureLifetime::Suite => FixtureLifetime::Suite,
        };
        lowered.fixtures.push(HirFixture {
            id: fixture_id,
            name: name.text().into(),
            exported: fixture.exported(),
            documentation: fixture.documentation(),
            parameters,
            result_type,
            lifetime,
            shareable: fixture.shareable(),
            setup_origin: fixture
                .setups()
                .next()
                .map(|phase| origin(file, phase.syntax()))
                .unwrap_or_else(|| origin(file, fixture.syntax())),
            teardown_origin: fixture
                .teardowns()
                .next()
                .map(|phase| origin(file, phase.syntax()))
                .unwrap_or_else(|| {
                    SyntaxOrigin::new(
                        file,
                        webtest_text::TextRange::empty(fixture.syntax().text_range().end()),
                    )
                }),
            setup,
            teardown,
            setup_count: fixture.setups().count(),
            teardown_count: fixture.teardowns().count(),
            origin: origin(file, fixture.syntax()),
            name_origin: SyntaxOrigin::new(file, name.text_range()),
        });
    }
}

fn lower_phase(
    context: &mut LowerContext,
    flow: Option<ast::Block>,
    server: Option<ast::ServerBlock>,
    browser: Option<ast::BrowserBlock>,
    output: &mut Vec<HirStmt>,
) {
    if let Some(body) = flow {
        output.extend(
            body.statements()
                .filter_map(|statement| lower_flow_statement(context, statement)),
        );
    }
    if let Some(body) = server {
        let statements = body
            .statements()
            .filter_map(|statement| lower_domain_statement(context, statement))
            .collect();
        output.push(HirStmt::Server(HirServerBlock {
            statements,
            origin: origin(context.file, body.syntax()),
        }));
    }
    if let Some(body) = browser {
        let statements = body
            .statements()
            .filter_map(|statement| lower_domain_statement(context, statement))
            .collect();
        output.push(HirStmt::Browser(HirBrowserBlock {
            statements,
            origin: origin(context.file, body.syntax()),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declaration_types_bindings_and_origins_come_from_typed_ast() {
        let source = r#"import { original as alias } from "./other.webtest"
export fn email(name: String) -> String = name + "@example.com"
export fixture user(email: String) -> { email: String } scope worker shareable {
setup server { let value = { email: email } provide value }
teardown server { expect value.email != "" }
}
test "consumer" { let result = use user(email: "é") }
"#;
        let hir = lower(FileId::new(3), &webtest_syntax::parse(source));
        assert_eq!(hir.imports[0].bindings[0].alias, "alias");
        assert_eq!(
            hir.functions[0].parameters[0].annotation.kind,
            HirTypeKind::Named("String".into())
        );
        let fixture = &hir.fixtures[0];
        assert_eq!(fixture.lifetime, FixtureLifetime::Worker);
        assert!(fixture.shareable);
        assert_eq!(fixture.setup_count, 1);
        assert_eq!(fixture.teardown_count, 1);
        let HirStmt::Let(binding) = &hir.tests[0].body[0] else {
            panic!("let")
        };
        assert!(matches!(binding.value.kind, HirExprKind::FixtureUse(_)));
        for origin in [
            hir.functions[0].name_origin,
            fixture.name_origin,
            hir.imports[0].bindings[0].alias_origin,
        ] {
            assert_eq!(origin.file, FileId::new(3));
            assert!(
                !source[usize::from(origin.range.start())..usize::from(origin.range.end())]
                    .is_empty()
            );
        }
    }
}
