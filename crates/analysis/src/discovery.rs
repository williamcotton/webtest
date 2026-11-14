//! Static test discovery over the same tracked compilation used for execution.
use crate::{
    DiagnosticSeverity,
    incremental::{self, Db, SourceInput},
};
use std::{collections::BTreeSet, sync::Arc};
use webtest_model::{Capability, ModuleId, TestDeclarationId, TestVariantId, WorkspaceId};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin};
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaticStatus {
    Ready,
    Errors,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredVariant {
    pub id: TestVariantId,
    pub key: String,
    pub label: String,
    pub origin: SyntaxOrigin,
    pub runnable: bool,
    pub debuggable: bool,
    pub tags: Vec<String>,
    pub required_capabilities: Vec<Capability>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredTest {
    pub id: TestDeclarationId,
    pub label: String,
    pub origin: SyntaxOrigin,
    pub static_status: StaticStatus,
    pub variants: Vec<DiscoveredVariant>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredModule {
    pub id: ModuleId,
    pub file: FileId,
    pub path: String,
    pub source_revision: SourceRevision,
    pub tests: Vec<DiscoveredTest>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TestDiscovery {
    pub kind: String,
    pub discovery_schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub workspace_revision: u64,
    pub modules: Vec<DiscoveredModule>,
    pub truncated: bool,
}
#[salsa::tracked(no_eq, lru = 2048)]
fn module_tests(db: &dyn Db, file: SourceInput) -> Arc<DiscoveredModule> {
    let hir = incremental::lowered(db, file);
    let compiled = incremental::compiled(db, file);
    let valid = !compiled
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error);
    Arc::new(DiscoveredModule {
        id: hir.module_id,
        file: file.file(db),
        path: file.path(db).clone(),
        source_revision: SourceRevision::of(file.text(db)),
        tests: hir
            .tests
            .iter()
            .map(|test| {
                let _required_capabilities = compiled
                    .plan
                    .tests
                    .iter()
                    .find(|planned| planned.test_declaration_id == test.declaration_id)
                    .map(|planned| planned.required_host_capabilities.clone())
                    .unwrap_or_default();
                DiscoveredTest {
                    id: test.declaration_id,
                    label: test.name.clone(),
                    origin: test.origin,
                    static_status: if valid {
                        StaticStatus::Ready
                    } else {
                        StaticStatus::Errors
                    },
                    variants: compiled
                        .plan
                        .tests
                        .iter()
                        .filter(|planned| planned.test_declaration_id == test.declaration_id)
                        .map(|planned| DiscoveredVariant {
                            id: planned.variant_id,
                            key: planned
                                .case_label
                                .clone()
                                .unwrap_or_else(|| "default".into()),
                            label: planned
                                .case_label
                                .clone()
                                .unwrap_or_else(|| "default".into()),
                            origin: test
                                .cases
                                .as_ref()
                                .and_then(|cases| {
                                    cases.iter().find(|case| {
                                        Some(&case.label) == planned.case_label.as_ref()
                                    })
                                })
                                .map_or(test.origin, |case| case.label_origin),
                            runnable: valid,
                            debuggable: valid,
                            tags: Vec::new(),
                            required_capabilities: planned.required_host_capabilities.clone(),
                        })
                        .collect(),
                }
            })
            .collect(),
    })
}
pub(crate) fn discover(db: &dyn Db, revision: u64, limit: usize) -> TestDiscovery {
    let mut modules = Vec::new();
    let mut seen = BTreeSet::new();
    let mut count = 0;
    let limit = limit.min(crate::navigation::MAX_QUERY_ITEMS);
    let mut truncated = false;
    for (_, file) in db.workspace_input().files(db) {
        db.unwind_if_revision_cancelled();
        if !seen.insert(file.file(db).get()) {
            continue;
        }
        let module = module_tests(db, *file);
        if module.tests.is_empty() {
            continue;
        }
        let mut module = (*module).clone();
        let remaining = limit.saturating_sub(count);
        if module.tests.len() > remaining {
            truncated = true;
            module.tests.truncate(remaining);
        }
        count += module.tests.len();
        if !module.tests.is_empty() {
            modules.push(module);
        }
        if truncated {
            break;
        }
    }
    TestDiscovery {
        kind: "test_discovery".into(),
        discovery_schema_version: 1,
        workspace_id: db.workspace_input().identity(db),
        workspace_revision: revision,
        modules,
        truncated,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalysisDatabase, WorkspaceConfiguration};
    #[test]
    fn discovery_uses_authored_and_variant_ids_and_keeps_errors_static() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(WorkspaceConfiguration {
            root: "/project".into(),
            ..Default::default()
        });
        let source = "test \"same label\" {} test \"same label\" {}";
        let file = db.open_file("/project/tests.webtest", source);
        let tree = db.discover_tests(10);
        assert!(!tree.truncated);
        let tests = &tree.modules[0].tests;
        assert_eq!(tests.len(), 2);
        assert_ne!(tests[0].id, tests[1].id);
        assert_ne!(tests[0].variants[0].id, tests[1].variants[0].id);
        let plan = db.test_plan(file).unwrap();
        assert_eq!(plan.tests[0].test_declaration_id, tests[0].id);
        assert_eq!(plan.tests[0].variant_id, tests[0].variants[0].id);
        let original = tests[0].clone();
        db.set_file_text(
            file,
            format!("fn unrelated(value: Int) -> Int = value\n{source}"),
        );
        let changed = db.discover_tests(10);
        assert_eq!(original.id, changed.modules[0].tests[0].id);
        assert_eq!(
            original.variants[0].id,
            changed.modules[0].tests[0].variants[0].id
        );
        assert!(db.discover_tests(1).truncated);
        db.set_file_text(file, "test \"broken\" { expect missing == 1 }");
        let broken = db.discover_tests(10);
        assert_eq!(
            broken.modules[0].tests[0].static_status,
            StaticStatus::Errors
        );
        assert!(!broken.modules[0].tests[0].variants[0].runnable);
    }
}
