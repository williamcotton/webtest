//! Fixture phases use the ordinary compiler and one environment across setup/teardown.
use super::{BindingState, Compiler};
use webtest_hir::HirFile;
use webtest_model::{Capability, PlanDeclarationId, SymbolId};
use webtest_plan::{FixtureDefinition, FixtureParameter};
use webtest_text::SourceRevision;

impl Compiler<'_> {
    pub(super) fn fixture_acquire(
        &mut self,
        expression: &webtest_hir::HirExpr,
        domain: Capability,
    ) -> Option<webtest_plan::FixtureAcquire> {
        use webtest_hir::{HirExprKind, HirNameRef};
        let HirExprKind::FixtureUse(inner) = &expression.kind else {
            return None;
        };
        if self.in_fixture_teardown {
            self.error(
                expression.origin.range,
                "semantic.fixture_acquisition_in_teardown",
                "acquire fixture dependencies during setup and retain their bindings for teardown"
                    .into(),
            );
            return None;
        }
        let HirExprKind::Call { callee, arguments } = &inner.kind else {
            return None;
        };
        let HirExprKind::Name(HirNameRef::Unresolved(name)) = &callee.kind else {
            return None;
        };
        let (db, graph) = self.workspace?;
        let (module, id) = crate::callables::resolve(graph, self.module?, name)?;
        let crate::callables::Callable::Fixture(fixture) =
            crate::callables::find(&graph.modules[&module], id)?
        else {
            return None;
        };
        if self.active_calls.iter().any(|(active, _)| *active == id) {
            self.error(
                expression.origin.range,
                "semantic.fixture_dependency_cycle",
                "fixture dependencies must be acyclic".into(),
            );
            return None;
        }
        let source = db.file_slot(&graph.modules[&module].path).target(db)?;
        let compiled = self
            .referenced_fixtures
            .get(&fixture.id)
            .cloned()
            .map(|definition| {
                std::sync::Arc::new(super::FixtureCompileResult {
                    definition: Some(definition),
                    dependencies: Vec::new(),
                    diagnostics: Vec::new(),
                })
            })
            .unwrap_or_else(|| {
                if source.file(db) != self.file
                    && !graph
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.code == "semantic.import_cycle")
                {
                    crate::incremental::fixture_definition(db, source, fixture.id)
                } else {
                    crate::incremental::fixture_template(db, source, fixture.id)
                }
            });
        let Some(definition) = &compiled.definition else {
            self.error(
                expression.origin.range,
                "semantic.fixture_dependency_cycle",
                "fixture dependencies must be acyclic".into(),
            );
            return None;
        };
        for diagnostic in &compiled.diagnostics {
            let before = self.diagnostics.len();
            self.error(
                expression.origin.range,
                diagnostic.code,
                diagnostic.message.clone(),
            );
            let related = graph.source_location(webtest_text::SyntaxOrigin::new(
                definition.origin.file,
                diagnostic.range,
            ));
            if let Some(diagnostic) = self.diagnostics.get_mut(before) {
                diagnostic.related_locations.extend(related);
            }
        }
        let capabilities = definition.required_capabilities();
        if (domain == Capability::Browser && capabilities.contains(&Capability::Server))
            || (domain == Capability::Server && capabilities.contains(&Capability::Browser))
        {
            self.error(
                expression.origin.range,
                "semantic.capability_mismatch",
                format!("fixture `{name}` requires capabilities unavailable in {domain} context"),
            );
        }
        if self.concurrent_depth > 0 && !definition.result_type.is_transferable() {
            self.error(
                expression.origin.range,
                "semantic.non_transferable_fixture_capture",
                "a native fixture result cannot be acquired by concurrent branches".into(),
            );
        }
        for capability in &capabilities {
            self.record_capability(*capability);
        }
        let mut supplied = std::collections::BTreeSet::new();
        let mut positional = 0;
        let mut values = Vec::new();
        for argument in arguments {
            let parameter = if let Some(name) = &argument.name {
                definition
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name == *name)
            } else {
                let parameter = definition.parameters.get(positional);
                positional += 1;
                parameter
            };
            let Some(parameter) = parameter else {
                self.error(
                    argument.origin.range,
                    "semantic.unknown_argument",
                    "fixture argument does not match a declared parameter".into(),
                );
                continue;
            };
            if !supplied.insert(parameter.binding) {
                self.error(
                    argument.origin.range,
                    "semantic.duplicate_argument",
                    format!("parameter `{}` is supplied more than once", parameter.name),
                );
            }
            let value = self.infer_expr(&argument.value, Capability::Pure, Some(&parameter.ty));
            self.expect_type(argument.value.origin.range, &parameter.ty, &value.ty);
            values.push(webtest_plan::HelperArgument {
                parameter: parameter.binding,
                name: parameter.name.clone(),
                value: value.expression,
            });
        }
        for parameter in &definition.parameters {
            if !supplied.contains(&parameter.binding) {
                self.error(
                    expression.origin.range,
                    "semantic.missing_argument",
                    format!("missing required argument `{}`", parameter.name),
                );
            }
        }
        for dependency in &compiled.dependencies {
            self.referenced_fixtures
                .insert(dependency.id, dependency.clone());
        }
        self.referenced_fixtures
            .insert(definition.id, definition.clone());
        self.type_fact(
            expression.origin.range,
            definition.result_type.clone(),
            Capability::Pure,
        );
        Some(webtest_plan::FixtureAcquire {
            fixture: definition.id,
            definition_revision: definition.source_revision,
            input_fingerprint: definition.input_fingerprint,
            lifetime: definition.lifetime,
            arguments: values,
            result_binding: None,
            result_name: None,
            result_type: definition.result_type.clone(),
            required_capabilities: capabilities,
            retry_safe: definition.lifetime != webtest_model::FixtureLifetime::Test
                || (definition.setup.retry_safety_violations().is_empty()
                    && definition.teardown.retry_safety_violations().is_empty()),
        })
    }

    pub(super) fn compile_fixture_declarations(&mut self, hir: &HirFile) -> Vec<FixtureDefinition> {
        let mut definitions = Vec::new();
        for fixture in &hir.fixtures {
            self.validating_declaration = true;
            self.active_calls
                .push((SymbolId(fixture.id.0), fixture.name_origin));
            self.bindings.clear();
            self.names.clear();
            self.declared_names.clear();
            self.next_step = 0;
            self.next_temporary = u32::MAX;
            let mut parameters = Vec::new();
            for parameter in &fixture.parameters {
                let ty = self.lower_type(&parameter.annotation);
                parameters.push(FixtureParameter {
                    binding: parameter.id,
                    name: parameter.name.clone(),
                    ty: ty.clone(),
                });
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
            for statement in fixture.setup.iter().chain(&fixture.teardown) {
                super::statements::collect_binding_names(statement, &mut self.declared_names);
            }
            let result_type = self.lower_type(&fixture.result_type);
            let declaration = PlanDeclarationId(fixture.id.0);
            self.provide_is_return = true;
            self.provide_types = Some(Vec::new());
            let setup = self.compile_sequence(
                declaration,
                fixture.setup_origin,
                &fixture.setup,
                Capability::Pure,
                vec![0],
            );
            let returned = self.provide_types.take().unwrap_or_default();
            if returned.is_empty() {
                self.error(
                    fixture.setup_origin.range,
                    "semantic.missing_fixture_result",
                    "fixture setup must provide its declared result".into(),
                );
            }
            for (actual, origin) in returned {
                self.expect_type(origin.range, &result_type, &actual);
            }
            self.provide_is_return = false;
            self.in_fixture_teardown = true;
            let teardown = self.compile_sequence(
                declaration,
                fixture.teardown_origin,
                &fixture.teardown,
                Capability::Pure,
                vec![1],
            );
            self.in_fixture_teardown = false;
            let schemas = self
                .providers
                .names()
                .into_iter()
                .filter_map(|name| {
                    self.providers
                        .schema(&name)
                        .map(|schema| (name, schema.hash()))
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let fingerprint = SourceRevision::of(
                &serde_json::json!({"fixture":fixture.id,"inputs":self.workspace.map(|(_, graph)| graph.input_fingerprint),"schemas":schemas,"configuration":self.workspace.map(|(db, _)| db.workspace_input().configuration(db))})
                    .to_string(),
            );
            definitions.push(FixtureDefinition {
                id: fixture.id,
                name: fixture.name.clone(),
                origin: fixture.origin,
                source_revision: self.revision,
                input_fingerprint: fingerprint,
                lifetime: fixture.lifetime,
                shareable: fixture.shareable,
                parameters,
                result_type,
                setup,
                teardown,
            });
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
        definitions
    }
}

#[cfg(test)]
mod tests {
    use crate::AnalysisDatabase;
    use webtest_plan::{AssertionOperation, PlanExpr, TestOperation};

    #[test]
    fn imported_fixture_dependencies_lower_to_explicit_acquisitions_and_owned_phase_plans() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(crate::WorkspaceConfiguration {
            root: "/workspace".into(),
            ..Default::default()
        });
        db.open_file("/workspace/fixtures.webtest", "export fixture seed(value: Int) -> Int scope suite shareable { setup { provide value } teardown { expect value > 0 } } export fixture record() -> { value: Int } scope file shareable { setup { let seed = use seed(7) provide { value: seed } } }");
        let source = "import { record } from \"./fixtures\" test \"fixture\" { let data = use record() expect data.value == 7 }";
        let file = db.open_file("/workspace/root.webtest", source);
        let diagnostics = db.diagnostics(file).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let plan = db.test_plan(file).unwrap();
        plan.validate_tree().unwrap();
        assert_eq!(plan.fixtures.len(), 2);
        let TestOperation::AcquireFixture(call) = &plan.tests[0].steps()[0].operation else {
            panic!("explicit acquisition");
        };
        let definition = plan
            .fixtures
            .iter()
            .find(|definition| definition.id == call.fixture)
            .unwrap();
        assert_eq!(definition.name, "record");
        assert!(matches!(
            definition.setup.steps()[0].operation,
            TestOperation::AcquireFixture(_)
        ));
        assert_eq!(
            &source[usize::from(plan.tests[0].steps()[0].origin.range.start())
                ..usize::from(plan.tests[0].steps()[0].origin.range.end())],
            "use record()"
        );
    }

    #[test]
    fn fixture_phases_preserve_parameter_setup_binding_and_precise_source_identity() {
        let source = "export fixture user(value: String) -> String scope file shareable { setup { let normalized = lower(value) provide normalized } teardown { expect normalized == lower(value) } } test \"unrelated\" {}";
        let mut db = AnalysisDatabase::default();
        let file = db.open_file("fixtures.webtest", source);
        assert!(db.diagnostics(file).unwrap().is_empty());
        let definitions = db.fixture_definitions(file).unwrap();
        assert_eq!(definitions.len(), 1);
        let definition = &definitions[0];
        definition.validate().unwrap();
        assert_eq!(
            &source[usize::from(definition.setup.origin.range.start())
                ..usize::from(definition.setup.origin.range.end())],
            "setup { let normalized = lower(value) provide normalized }"
        );
        assert_eq!(
            &source[usize::from(definition.teardown.origin.range.start())
                ..usize::from(definition.teardown.origin.range.end())],
            "teardown { expect normalized == lower(value) }"
        );
        let TestOperation::EvaluatePure(binding) = &definition.setup.steps()[0].operation else {
            panic!("setup binding");
        };
        let TestOperation::Assertion(AssertionOperation::Value {
            actual: PlanExpr::Binding(actual),
            ..
        }) = &definition.teardown.steps()[0].operation
        else {
            panic!("teardown assertion");
        };
        assert_eq!(Some(*actual), binding.result_binding);
        let plan = db.test_plan(file).unwrap();
        assert_eq!(plan.tests.len(), 1);
        assert!(
            plan.tests[0].steps().is_empty(),
            "fixture phases are separate from test bodies"
        );
    }
    #[test]
    fn teardown_cannot_acquire_new_dependencies_directly_or_through_helpers() {
        for teardown in ["let value = use seed()", "let value = acquire()"] {
            let mut db = AnalysisDatabase::default();
            let file = db.open_file("fixture.webtest", format!(
                "fixture seed() -> Int {{ setup {{ provide 7 }} }} fn acquire() -> Int {{ let value = use seed() provide value }} fixture outer() -> Int {{ setup {{ provide 1 }} teardown {{ {teardown} }} }}"
            ));
            let diagnostics = db.diagnostics(file).unwrap();
            assert!(diagnostics.iter().any(|diagnostic| diagnostic.code == "semantic.fixture_acquisition_in_teardown"), "{diagnostics:?}");
        }
    }

    #[test]
    fn fixture_result_checks_apply_before_any_acquisition() {
        for (body, expected) in [
            ("setup { let value = 1 }", "semantic.missing_fixture_result"),
            ("setup { provide 7 }", "semantic.type_mismatch"),
            (
                r#"setup { provide "ok" } teardown { provide "bad" }"#,
                "semantic.provide_outside_race",
            ),
        ] {
            let mut db = AnalysisDatabase::default();
            let file = db.open_file(
                "fixture.webtest",
                format!("fixture fixture_value() -> String scope test {{ {body} }}"),
            );
            let diagnostics = db.diagnostics(file).unwrap();
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == expected),
                "{diagnostics:?}"
            );
        }
    }
}
