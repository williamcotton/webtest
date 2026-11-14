//! Static project declarations project the shared workspace/signature queries.
use crate::{
    callables,
    incremental::{self, Db},
};
use serde::Serialize;
use webtest_model::{
    Capability, FixtureLifetime, ModuleId, SymbolId, TestVariantId, Type, WorkspaceId,
};
use webtest_text::SourceLocation;
#[derive(Clone, Debug, Serialize)]
pub struct ProjectDeclaration {
    pub id: SymbolId,
    pub kind: String,
    pub name: String,
    pub module: ModuleId,
    pub location: SourceLocation,
    pub documentation: String,
    pub parameters: Vec<crate::DeclarationParameter>,
    pub result: Option<Type>,
    pub required_capabilities: Vec<Capability>,
    pub lifetime: Option<FixtureLifetime>,
    pub shareable: bool,
    pub default_variant: Option<TestVariantId>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ProjectSemanticDescription {
    pub kind: String,
    pub description_schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub workspace_revision: u64,
    pub declarations: Vec<ProjectDeclaration>,
    pub truncated: bool,
}
fn text(value: &str, max: usize) -> String {
    super::truncate_utf8(
        &value
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect::<String>(),
        max,
    )
}
pub(crate) fn describe(db: &dyn Db, revision: u64, limit: usize) -> ProjectSemanticDescription {
    let mut declarations = Vec::new();
    let mut truncated = false;
    let mut seen = std::collections::BTreeSet::new();
    let mut bytes = 0usize;
    let limit = limit.min(crate::navigation::MAX_QUERY_ITEMS);
    'files: for (_, input) in db.workspace_input().files(db) {
        db.unwind_if_revision_cancelled();
        if !seen.insert(input.file(db).get()) {
            continue;
        }
        let hir = incremental::lowered(db, *input);
        let mut candidates = Vec::new();
        for (id, exported, kind) in hir
            .functions
            .iter()
            .map(|function| (SymbolId(function.id.0), function.exported, "helper"))
            .chain(
                hir.guards
                    .iter()
                    .map(|guard| (SymbolId(guard.id.0), guard.exported, "guard")),
            )
            .chain(
                hir.patterns
                    .iter()
                    .map(|pattern| (SymbolId(pattern.id.0), pattern.exported, "pattern")),
            )
            .chain(
                hir.fixtures
                    .iter()
                    .map(|fixture| (SymbolId(fixture.id.0), fixture.exported, "fixture")),
            )
        {
            if !exported {
                continue;
            }
            let Some(signature) = callables::signature(db, *input, id) else {
                continue;
            };
            candidates.push(ProjectDeclaration {
                id,
                kind: kind.into(),
                name: text(&signature.name, 1024),
                module: hir.module_id,
                location: SourceLocation {
                    path: input.path(db).clone(),
                    origin: signature.origin,
                    revision: webtest_text::SourceRevision::of(input.text(db)),
                },
                documentation: text(&signature.documentation, 8192),
                parameters: signature.parameters.clone(),
                result: Some(signature.result.clone()),
                required_capabilities: if signature.capability == Capability::Test {
                    vec![Capability::Server, Capability::Browser]
                } else {
                    vec![signature.capability]
                },
                lifetime: signature.lifetime,
                shareable: signature.shareable,
                default_variant: None,
            });
        }
        if !hir.tests.is_empty() {
            let compiled = incremental::compiled(db, *input);
            for test in &hir.tests {
                let capabilities = compiled
                    .plan
                    .tests
                    .iter()
                    .find(|planned| planned.test_declaration_id == test.declaration_id)
                    .map_or_else(Vec::new, |test| test.required_host_capabilities.clone());
                candidates.push(ProjectDeclaration {
                    id: SymbolId(test.declaration_id.0),
                    kind: "test".into(),
                    name: text(&test.name, 1024),
                    module: hir.module_id,
                    location: SourceLocation {
                        path: input.path(db).clone(),
                        origin: test.origin,
                        revision: webtest_text::SourceRevision::of(input.text(db)),
                    },
                    documentation: String::new(),
                    parameters: Vec::new(),
                    result: None,
                    required_capabilities: capabilities,
                    lifetime: None,
                    shareable: false,
                    default_variant: test.cases.is_none().then_some(test.variant_id),
                });
            }
        }
        candidates.sort_by_key(|declaration| declaration.location.origin.range.start());
        for declaration in candidates {
            let length = serde_json::to_vec(&declaration).map_or(usize::MAX, |bytes| bytes.len());
            if declarations.len() >= limit || length > (1024 * 1024usize).saturating_sub(bytes) {
                truncated = true;
                break 'files;
            }
            bytes += length;
            declarations.push(declaration);
        }
    }
    ProjectSemanticDescription {
        kind: "project_declarations".into(),
        description_schema_version: super::DESCRIPTION_SCHEMA_VERSION,
        workspace_id: db.workspace_input().identity(db),
        workspace_revision: revision,
        declarations,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AnalysisDatabase, DescriptionLimits, DescriptionRequest, DescriptionResponse,
        WorkspaceConfiguration,
    };
    #[test]
    fn project_description_uses_resolved_public_signatures_and_variant_identities_with_bounds() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(WorkspaceConfiguration {
            root: "/workspace".into(),
            ..Default::default()
        });
        db.open_file("/workspace/lib.webtest", "/// Normalize a Unicode name.\nexport fn normalize(name: String) -> String = lower(name) fn private_value() -> Int = 1 export fixture seed() -> Int scope suite shareable { setup { provide 7 } }");
        let file = db.open_file("/workspace/test.webtest", "import { normalize, seed } from \"./lib\" test \"é😀\" { let n = normalize(\"É\") let seed = use seed() }");
        assert!(
            db.diagnostics(file).unwrap().is_empty(),
            "{:?}",
            db.diagnostics(file)
        );
        let DescriptionResponse::Project(project) = db.describe(
            DescriptionRequest::Query("project.declarations".into()),
            None,
            DescriptionLimits::default(),
        ) else {
            panic!("project description")
        };
        assert_eq!(project.declarations.len(), 3);
        assert!(
            !project
                .declarations
                .iter()
                .any(|declaration| declaration.name == "private_value")
        );
        let helper = project
            .declarations
            .iter()
            .find(|declaration| declaration.name == "normalize")
            .unwrap();
        assert_eq!(helper.parameters[0].ty, webtest_model::Type::String);
        assert!(helper.documentation.contains("Unicode"));
        let fixture = project
            .declarations
            .iter()
            .find(|declaration| declaration.name == "seed")
            .unwrap();
        assert_eq!(
            fixture.lifetime,
            Some(webtest_model::FixtureLifetime::Suite)
        );
        assert!(fixture.shareable);
        let test = project
            .declarations
            .iter()
            .find(|declaration| declaration.kind == "test")
            .unwrap();
        assert_eq!(
            test.default_variant,
            Some(db.test_plan(file).unwrap().tests[0].variant_id)
        );
        let bounded = db.project_description(1);
        assert_eq!(bounded.declarations.len(), 1);
        assert!(bounded.truncated);
    }
}
