//! Pure module resolution and workspace indexes over explicit tracked inputs.
use crate::incremental::{self, Db, SourceInput};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use webtest_hir::HirFile;
use webtest_model::{ModuleId, SymbolId};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WorkspaceConfiguration {
    /// Canonical host-supplied root; no query canonicalizes a host path.
    pub root: String,
    pub source_roots: Vec<String>,
    pub case_sensitive: bool,
    pub compiler_features: BTreeSet<String>,
    pub resolved_configuration: serde_json::Value,
    pub active_profile: Option<String>,
    pub toolchain_identity: String,
    pub invalid_inputs: bool,
}
impl Default for WorkspaceConfiguration {
    fn default() -> Self {
        Self {
            root: String::new(),
            source_roots: vec![String::new()],
            case_sensitive: true,
            compiler_features: BTreeSet::new(),
            resolved_configuration: serde_json::Value::Null,
            active_profile: None,
            toolchain_identity: format!("webtest/{}", env!("CARGO_PKG_VERSION")),
            invalid_inputs: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleGraph {
    pub root: ModuleId,
    pub input_fingerprint: SourceRevision,
    pub modules: BTreeMap<ModuleId, Arc<Module>>,
    file_modules: std::collections::HashMap<u32, ModuleId>,
    actionable: std::collections::HashMap<u32, SyntaxOrigin>,
    pub diagnostics: Vec<ModuleDiagnostic>,
    pub required_inputs: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Module {
    pub(crate) input: SourceInput,
    pub id: ModuleId,
    pub file: FileId,
    pub path: String,
    pub revision: SourceRevision,
    pub hir: Arc<HirFile>,
    pub imports: BTreeMap<String, ResolvedImport>,
    pub dependencies: Vec<(ModuleId, SyntaxOrigin)>,
}
// The Salsa handle is a query-local lookup optimization, never semantic identity.
impl PartialEq for Module {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.file == other.file
            && self.path == other.path
            && self.revision == other.revision
            && self.hir == other.hir
            && self.imports == other.imports
            && self.dependencies == other.dependencies
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedImport {
    pub target_module: ModuleId,
    pub target: SymbolId,
    pub name: String,
    pub alias: String,
    pub origin: SyntaxOrigin,
    pub declaration: SyntaxOrigin,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleDiagnostic {
    pub code: &'static str,
    pub message: String,
    pub origin: SyntaxOrigin,
    pub related: Vec<SyntaxOrigin>,
    /// Host adapters may supply only these candidates, then rerun resolution.
    pub candidates: Vec<String>,
}

pub(crate) fn normalize_path(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let absolute = path.starts_with('/');
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value),
        }
    }
    Some(format!(
        "{}{}",
        if absolute { "/" } else { "" },
        components.join("/")
    ))
}
fn within(path: &str, root: &str) -> bool {
    if root == "/" {
        return path.starts_with('/');
    }
    if root.is_empty() {
        !path.starts_with('/')
    } else {
        path == root
            || path
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }
}
pub(crate) fn candidates(
    config: &WorkspaceConfiguration,
    importing: &str,
    requested: &str,
) -> Result<Vec<String>, &'static str> {
    if requested.is_empty()
        || requested.contains('\0')
        || requested.contains(':')
        || requested.starts_with('/')
        || requested.starts_with('\\')
    {
        return Err("semantic.invalid_module_path");
    }
    let mut requested = requested.replace('\\', "/");
    if requested
        .rsplit('/')
        .next()
        .is_none_or(|name| !name.contains('.'))
    {
        requested.push_str(".webtest");
    }
    if !requested.ends_with(".webtest") {
        return Err("semantic.invalid_module_extension");
    }
    let roots: Vec<_> =
        config
            .source_roots
            .iter()
            .filter_map(|root| {
                normalize_path(
                    format!("{}/{}", config.root, root)
                        .trim_start_matches(if config.root.is_empty() { '/' } else { '\0' }),
                )
            })
            .collect();
    let bases: Vec<_> = if requested.starts_with("./") || requested.starts_with("../") {
        vec![
            importing
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent)
                .to_owned(),
        ]
    } else if let Some(relative) = requested.strip_prefix("project/") {
        requested = relative.to_owned();
        vec![config.root.clone()]
    } else {
        roots.clone()
    };
    let mut resolved = BTreeSet::new();
    for base in bases {
        let joined = if base.is_empty() {
            requested.clone()
        } else {
            format!("{base}/{requested}")
        };
        if let Some(path) = normalize_path(&joined)
            && roots.iter().any(|root| within(&path, root))
        {
            resolved.insert(path);
        }
    }
    if resolved.is_empty() {
        Err("semantic.module_outside_source_roots")
    } else {
        Ok(resolved.into_iter().collect())
    }
}

fn exports(hir: &HirFile, name: &str) -> Option<(SymbolId, bool, SyntaxOrigin)> {
    hir.functions
        .iter()
        .find(|function| function.name == name)
        .map(|function| {
            (
                SymbolId(function.id.0),
                function.exported,
                function.name_origin,
            )
        })
        .or_else(|| {
            hir.fixtures
                .iter()
                .find(|fixture| fixture.name == name)
                .map(|fixture| {
                    (
                        SymbolId(fixture.id.0),
                        fixture.exported,
                        fixture.name_origin,
                    )
                })
        })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedModule {
    pub module: Arc<Module>,
    pub targets: Vec<SourceInput>,
    diagnostics: Vec<ModuleDiagnostic>,
    required_inputs: Vec<String>,
}
#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn resolved_module(db: &dyn Db, input: SourceInput) -> Arc<ResolvedModule> {
    let config = db.workspace_input().configuration(db);
    let hir = incremental::lowered(db, input);
    let id = hir.module_id;
    let mut diagnostics = Vec::new();
    let mut required_inputs = BTreeSet::new();
    let mut targets = Vec::new();
    let mut dependencies = Vec::new();
    let path = normalize_path(input.path(db)).unwrap_or_else(|| input.path(db).clone());
    if !config.case_sensitive {
        let entries = db.case_slot(&path.to_lowercase()).entries(db);
        let identities: BTreeSet<_> = entries
            .iter()
            .map(|(_, file)| file.file(db).get())
            .collect();
        if identities.len() > 1 {
            diagnostics.push(ModuleDiagnostic {
                code: "semantic.module_case_collision",
                message: format!("module path `{path}` collides by case with another input"),
                origin: SyntaxOrigin::new(input.file(db), TextRange::empty(TextSize::from(0))),
                related: Vec::new(),
                candidates: entries.iter().map(|(path, _)| path.clone()).collect(),
            });
        }
    }
    let mut local_names = BTreeMap::new();
    for (name, origin) in hir
        .functions
        .iter()
        .map(|function| (&function.name, function.name_origin))
        .chain(
            hir.fixtures
                .iter()
                .map(|fixture| (&fixture.name, fixture.name_origin)),
        )
    {
        if let Some(previous) = local_names.insert(name.clone(), origin) {
            diagnostics.push(ModuleDiagnostic {
                code: "semantic.duplicate_declaration",
                message: format!("duplicate declaration `{name}`"),
                origin,
                related: vec![previous],
                candidates: Vec::new(),
            });
        }
    }
    let mut imports = BTreeMap::new();
    for import in &hir.imports {
        let paths = match candidates(config, &path, &import.path) {
            Ok(paths) => paths,
            Err(code) => {
                diagnostics.push(ModuleDiagnostic {
                    code,
                    message: format!(
                        "cannot resolve module path `{}` within configured source roots",
                        import.path
                    ),
                    origin: import.path_origin,
                    related: Vec::new(),
                    candidates: Vec::new(),
                });
                continue;
            }
        };
        let mut matches = BTreeMap::new();
        let mut case_collision = false;
        for path in &paths {
            if db.file_slot(path).target(db).is_none() {
                required_inputs.insert(path.clone());
            }
            if let Some(target) = db.file_slot(path).target(db) {
                matches.insert(target.file(db).get(), target);
            } else if !config.case_sensitive {
                let entries = db.case_slot(&path.to_lowercase()).entries(db);
                let mut identities = BTreeSet::new();
                for (_, target) in entries {
                    identities.insert(target.file(db).get());
                    matches.insert(target.file(db).get(), *target);
                }
                case_collision |= identities.len() > 1;
            }
        }
        if matches.len() != 1 {
            diagnostics.push(ModuleDiagnostic {
                code: if case_collision {
                    "semantic.module_case_collision"
                } else if matches.is_empty() {
                    "semantic.missing_module"
                } else {
                    "semantic.ambiguous_module"
                },
                message: format!(
                    "module `{}` resolved to {} inputs",
                    import.path,
                    matches.len()
                ),
                origin: import.path_origin,
                related: Vec::new(),
                candidates: paths,
            });
            continue;
        }
        let Some(target) = matches.values().next().copied() else {
            continue;
        };
        let target_hir = incremental::lowered(db, target);
        targets.push(target);
        dependencies.push((target_hir.module_id, import.path_origin));
        for binding in &import.bindings {
            if local_names.contains_key(&binding.alias) || imports.contains_key(&binding.alias) {
                diagnostics.push(ModuleDiagnostic {
                    code: "semantic.import_alias_collision",
                    message: format!("import alias `{}` is already declared", binding.alias),
                    origin: binding.alias_origin,
                    related: local_names
                        .get(&binding.alias)
                        .copied()
                        .into_iter()
                        .collect(),
                    candidates: Vec::new(),
                });
                continue;
            }
            match exports(&target_hir, &binding.name) {
                Some((symbol, true, declaration)) => {
                    imports.insert(
                        binding.alias.clone(),
                        ResolvedImport {
                            target_module: target_hir.module_id,
                            target: symbol,
                            name: binding.name.clone(),
                            alias: binding.alias.clone(),
                            origin: binding.alias_origin,
                            declaration,
                        },
                    );
                }
                Some((_, false, declaration)) => diagnostics.push(ModuleDiagnostic {
                    code: "semantic.non_exported_import",
                    message: format!("`{}` is not exported", binding.name),
                    origin: binding.name_origin,
                    related: vec![declaration],
                    candidates: Vec::new(),
                }),
                None => diagnostics.push(ModuleDiagnostic {
                    code: "semantic.missing_export",
                    message: format!("module does not declare `{}`", binding.name),
                    origin: binding.name_origin,
                    related: Vec::new(),
                    candidates: Vec::new(),
                }),
            }
        }
    }
    Arc::new(ResolvedModule {
        module: Arc::new(Module {
            input,
            id,
            file: input.file(db),
            path,
            revision: SourceRevision::of(input.text(db)),
            hir,
            imports,
            dependencies,
        }),
        targets,
        diagnostics,
        required_inputs: required_inputs.into_iter().collect(),
    })
}

#[salsa::tracked(no_eq, lru = 2048)]
pub(crate) fn module_graph(db: &dyn Db, root: SourceInput) -> Arc<ModuleGraph> {
    const MAX_MODULES: usize = 16_384;
    let root_hir = incremental::lowered(db, root);
    let root_id = root_hir.module_id;
    let mut modules = BTreeMap::new();
    let mut file_modules = std::collections::HashMap::new();
    let mut actionable = std::collections::HashMap::new();
    let mut diagnostics = Vec::new();
    let mut required_inputs = BTreeSet::new();
    let mut pending = VecDeque::from([(root, None)]);
    let mut visited = BTreeSet::new();
    while let Some((input, first)) = pending.pop_front() {
        db.unwind_if_revision_cancelled();
        let record = resolved_module(db, input);
        let id = record.module.id;
        if !visited.insert(id) {
            continue;
        }
        if visited.len() > MAX_MODULES {
            diagnostics.push(ModuleDiagnostic {
                code: "semantic.module_graph_limit",
                message: format!("module graph exceeds {MAX_MODULES} files"),
                origin: SyntaxOrigin::new(root.file(db), TextRange::empty(0.into())),
                related: Vec::new(),
                candidates: Vec::new(),
            });
            break;
        }
        file_modules.insert(input.file(db).get(), id);
        if let Some(first) = first {
            actionable.insert(input.file(db).get(), first);
        }
        if input.file(db) != root.file(db) {
            for error in incremental::parsed(db, input).errors() {
                diagnostics.push(ModuleDiagnostic {
                    code: error.code,
                    message: error.message.clone(),
                    origin: SyntaxOrigin::new(input.file(db), error.range),
                    related: Vec::new(),
                    candidates: Vec::new(),
                });
            }
        }
        diagnostics.extend(record.diagnostics.iter().cloned());
        required_inputs.extend(record.required_inputs.iter().cloned());
        for (target, (_, edge)) in record.targets.iter().zip(&record.module.dependencies) {
            pending.push_back((*target, Some(first.unwrap_or(*edge))));
        }
        modules.insert(id, record.module.clone());
    }
    let shortest = shortest_cycle(db, &modules);
    if let Some(mut cycle) = shortest
        && let Some(origin) = cycle.first().copied()
    {
        cycle.remove(0);
        diagnostics.push(ModuleDiagnostic {
            code: "semantic.import_cycle",
            message: "module imports form a cycle".into(),
            origin,
            related: cycle,
            candidates: Vec::new(),
        });
    }
    let configuration = serde_json::json!(db.workspace_input().configuration(db)).to_string();
    let input_fingerprint = SourceRevision::of_parts(
        std::iter::once(configuration.as_bytes()).chain(modules.values().flat_map(|module| {
            [
                module.path.as_bytes(),
                module.revision.as_bytes().as_slice(),
            ]
        })),
    );
    Arc::new(ModuleGraph {
        root: root_id,
        input_fingerprint,
        file_modules,
        actionable,
        required_inputs: required_inputs.into_iter().collect(),
        modules,
        diagnostics,
    })
}

impl ModuleGraph {
    pub fn source_location(&self, origin: SyntaxOrigin) -> Option<webtest_text::SourceLocation> {
        let module = self
            .modules
            .get(self.file_modules.get(&origin.file.get())?)?;
        Some(webtest_text::SourceLocation {
            path: module.path.clone(),
            origin,
            revision: module.revision,
        })
    }

    pub fn actionable_origin(&self, origin: SyntaxOrigin) -> SyntaxOrigin {
        self.actionable
            .get(&origin.file.get())
            .copied()
            .unwrap_or(origin)
    }
}

fn shortest_cycle(
    db: &dyn Db,
    modules: &BTreeMap<ModuleId, Arc<Module>>,
) -> Option<Vec<SyntaxOrigin>> {
    // A dense Kahn pass handles the overwhelmingly common DAG without building
    // SCC sets/reverse maps. Cyclic inputs retain exact shortest-cycle reporting.
    let indices = modules
        .keys()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect::<std::collections::HashMap<_, _>>();
    let mut forward = vec![Vec::new(); modules.len()];
    let mut incoming = vec![0usize; modules.len()];
    for (source, module) in modules {
        let targets = &module.dependencies;
        let Some(&source) = indices.get(source) else {
            continue;
        };
        for (target, _) in targets {
            if let Some(&target) = indices.get(target) {
                forward[source].push(target);
                incoming[target] += 1;
            }
        }
    }
    let mut ready = incoming
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<Vec<_>>();
    let mut count = 0;
    while let Some(source) = ready.pop() {
        db.unwind_if_revision_cancelled();
        count += 1;
        for &target in &forward[source] {
            incoming[target] -= 1;
            if incoming[target] == 0 {
                ready.push(target);
            }
        }
    }
    if count == modules.len() {
        return None;
    }
    let edges = modules
        .iter()
        .map(|(id, module)| (*id, module.dependencies.clone()))
        .collect::<BTreeMap<_, _>>();
    // Iterative Kosaraju avoids recursion and avoids quadratic cycle searches in
    // the common acyclic graph (including the thousand-file benchmark chain).
    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    for start in modules.keys() {
        let mut stack = vec![(*start, false)];
        while let Some((node, exiting)) = stack.pop() {
            db.unwind_if_revision_cancelled();
            if exiting {
                order.push(node);
                continue;
            }
            if !visited.insert(node) {
                continue;
            }
            stack.push((node, true));
            for (target, _) in edges.get(&node).into_iter().flatten().rev() {
                if !visited.contains(target) {
                    stack.push((*target, false));
                }
            }
        }
    }
    let mut reverse = BTreeMap::<ModuleId, Vec<ModuleId>>::new();
    for (source, targets) in &edges {
        for (target, _) in targets {
            reverse.entry(*target).or_default().push(*source);
        }
    }
    visited.clear();
    let mut best: Option<Vec<SyntaxOrigin>> = None;
    for start in order.into_iter().rev() {
        if visited.contains(&start) {
            continue;
        }
        let mut component = BTreeSet::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            component.insert(node);
            stack.extend(reverse.get(&node).into_iter().flatten().copied());
        }
        if component.len() == 1
            && !edges
                .get(&start)
                .into_iter()
                .flatten()
                .any(|(target, _)| *target == start)
        {
            continue;
        }
        for start in &component {
            let mut queue = VecDeque::from([(*start, 0usize)]);
            let mut parents = BTreeMap::<ModuleId, (ModuleId, SyntaxOrigin)>::new();
            let mut seen = BTreeSet::from([*start]);
            while let Some((node, depth)) = queue.pop_front() {
                db.unwind_if_revision_cancelled();
                if best.as_ref().is_some_and(|best| depth + 1 >= best.len()) {
                    continue;
                }
                for (target, origin) in edges.get(&node).into_iter().flatten() {
                    if target == start {
                        let mut cycle = vec![*origin];
                        let mut current = node;
                        while current != *start {
                            let Some((parent, edge)) = parents.get(&current) else {
                                break;
                            };
                            cycle.push(*edge);
                            current = *parent;
                        }
                        cycle.reverse();
                        best = Some(cycle);
                    } else if component.contains(target) && seen.insert(*target) {
                        parents.insert(*target, (node, *origin));
                        queue.push_back((*target, depth + 1));
                    }
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnalysisDatabase;
    fn database() -> AnalysisDatabase {
        let mut database = AnalysisDatabase::default();
        database.configure_workspace(WorkspaceConfiguration {
            root: "/project".into(),
            source_roots: vec!["src".into(), "tests".into()],
            ..Default::default()
        });
        database
    }
    #[test]
    fn relative_project_paths_aliases_and_extension_rule_resolve_one_canonical_module() {
        let mut database = database();
        let helper = database.open_file(
            "/project/src/é.webtest",
            "export fn pick(value: Int) -> Int = value",
        );
        database
            .alias_file("/project/src/link.webtest", helper)
            .unwrap();
        let root = database.open_file("/project/tests/main.webtest", "import { pick as first } from \"../src/é\"\nimport { pick as second } from \"project/src/link.webtest\"\ntest \"x\" {}");
        let graph = database.module_graph(root).unwrap();
        assert!(graph.diagnostics.is_empty(), "{:?}", graph.diagnostics);
        assert_eq!(graph.modules.len(), 2);
        let root = &graph.modules[&graph.root];
        assert_eq!(root.imports["first"].target, root.imports["second"].target);
        assert_eq!(root.imports["first"].declaration.file, helper);
    }
    #[test]
    fn missing_input_addition_deletion_and_alias_deletion_invalidate_only_affected_graphs() {
        let mut database = database();
        let root = database.open_file(
            "/project/tests/main.webtest",
            "import { pick } from \"../src/link\"\ntest \"x\" {}",
        );
        let unrelated =
            database.open_file("/project/tests/unrelated.webtest", "test \"unrelated\" {}");
        assert_eq!(
            database.module_graph(root).unwrap().diagnostics[0].code,
            "semantic.missing_module"
        );
        let unchanged = database.module_graph(unrelated).unwrap();
        let before = database.query_counts();
        let helper =
            database.open_file("/project/src/helper.webtest", "export fn pick() -> Int = 1");
        database
            .alias_file("/project/src/link.webtest", helper)
            .unwrap();
        assert!(database.module_graph(root).unwrap().diagnostics.is_empty());
        assert!(Arc::ptr_eq(
            &unchanged,
            &database.module_graph(unrelated).unwrap()
        ));
        assert_eq!(database.query_counts().parse, before.parse + 1);
        assert_eq!(database.query_counts().hir, before.hir + 1);
        database.close_file(helper);
        assert_eq!(
            database.module_graph(root).unwrap().diagnostics[0].code,
            "semantic.missing_module"
        );
        assert!(Arc::ptr_eq(
            &unchanged,
            &database.module_graph(unrelated).unwrap()
        ));
    }
    #[test]
    fn distinct_module_errors_and_shortest_cycles_keep_related_origins() {
        let mut database = database();
        database.open_file(
            "/project/src/helpers.webtest",
            "fn private_value() -> Int = 1\nexport fn pick() -> Int = 2",
        );
        let cases = [
            (
                "import { private_value } from \"../src/helpers\"",
                "semantic.non_exported_import",
            ),
            (
                "import { absent } from \"../src/helpers\"",
                "semantic.missing_export",
            ),
            (
                "import { pick as same } from \"../src/helpers\"\nfn same() -> Int = 0",
                "semantic.import_alias_collision",
            ),
            (
                "import { pick, pick } from \"../src/helpers\"",
                "semantic.import_alias_collision",
            ),
            (
                "import { pick } from \"../../outside\"",
                "semantic.module_outside_source_roots",
            ),
            (
                "import { pick } from \"https://remote.test/code.webtest\"",
                "semantic.invalid_module_path",
            ),
            (
                "import { pick } from \"../src/helpers.js\"",
                "semantic.invalid_module_extension",
            ),
        ];
        for (index, (source, code)) in cases.iter().enumerate() {
            let file = database.open_file(format!("/project/tests/case{index}.webtest"), *source);
            assert!(
                database
                    .module_graph(file)
                    .unwrap()
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == *code),
                "{source}"
            );
        }
        let a = database.open_file(
            "/project/src/a.webtest",
            "import { b } from \"./b\"\nexport fn a() -> Int = 1",
        );
        database.open_file(
            "/project/src/b.webtest",
            "import { a } from \"./a\"\nimport { c } from \"./c\"\nexport fn b() -> Int = 1",
        );
        database.open_file(
            "/project/src/c.webtest",
            "import { c } from \"./c\"\nexport fn c() -> Int = 1",
        );
        let graph = database.module_graph(a).unwrap();
        let cycle = graph
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "semantic.import_cycle")
            .unwrap();
        assert!(
            cycle.related.is_empty(),
            "the self-cycle is shorter than a->b->a"
        );
    }
    #[test]
    fn ambiguous_source_roots_and_case_collisions_are_different_from_missing_modules() {
        let mut database = database();
        database.open_file("/project/src/helper.webtest", "export fn pick() -> Int = 1");
        database.open_file(
            "/project/tests/helper.webtest",
            "export fn pick() -> Int = 2",
        );
        let root = database.open_file(
            "/project/tests/main.webtest",
            "import { pick } from \"helper\"",
        );
        assert_eq!(
            database.module_graph(root).unwrap().diagnostics[0].code,
            "semantic.ambiguous_module"
        );
        database.configure_workspace(WorkspaceConfiguration {
            root: "/project".into(),
            case_sensitive: false,
            ..Default::default()
        });
        database.open_file("/project/src/Helper.webtest", "export fn pick() -> Int = 3");
        database.set_file_text(root, "import { pick } from \"../src/HELPER\"");
        assert!(
            database
                .module_graph(root)
                .unwrap()
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.module_case_collision")
        );
    }
    #[test]
    fn semantic_declaration_and_default_variant_ids_ignore_input_and_declaration_order() {
        let main = "import { pick } from \"../src/helper\"\ntest \"stable\" {}";
        let mut first = database();
        let root_a = first.open_file("/project/tests/main.webtest", main);
        first.open_file(
            "/project/src/helper.webtest",
            "export fn pick(value: Int) -> Int = value",
        );
        let mut second = database();
        second.open_file(
            "/project/src/helper.webtest",
            "fn unrelated() -> Bool = true\nexport fn pick(value: Int) -> Int = value",
        );
        let root_b = second.open_file("/project/tests/main.webtest", main);
        let a = first.module_graph(root_a).unwrap();
        let b = second.module_graph(root_b).unwrap();
        assert_eq!(a.root, b.root);
        assert_eq!(
            a.modules[&a.root].imports["pick"].target,
            b.modules[&b.root].imports["pick"].target
        );
        let a = &a.modules[&a.root].hir.tests[0];
        let b = &b.modules[&b.root].hir.tests[0];
        assert_eq!(a.declaration_id, b.declaration_id);
        assert_eq!(a.variant_id, b.variant_id);
        assert_ne!(a.declaration_id.0, a.variant_id.0);
    }
}
