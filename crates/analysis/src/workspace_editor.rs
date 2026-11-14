//! Recovered AST context plus resolved declarations; no adapter-specific language model.
use crate::{
    callables,
    facts::{DocumentationFact, Signature, SignatureParameter},
    incremental::{Db, SourceInput},
    semantic_index,
};
use rowan::ast::AstNode;
use webtest_syntax::{Parse, SyntaxNode, ast};
use webtest_text::TextSize;
fn contains(node: &SyntaxNode, offset: TextSize) -> bool {
    node.text_range().start() <= offset && offset <= node.text_range().end()
}
pub(crate) fn context_domain(
    parsed: &Parse,
    offset: TextSize,
) -> Option<webtest_model::Capability> {
    use webtest_syntax::SyntaxKind as K;
    parsed
        .syntax()
        .descendants()
        .filter(|node| contains(node, offset))
        .filter_map(|node| {
            let domain = match node.kind() {
                K::GuardDecl => Some(webtest_model::Capability::Pure),
                K::ActorBlock | K::BrowserBlock => Some(webtest_model::Capability::Browser),
                K::ServerBlock => Some(webtest_model::Capability::Server),
                K::FunctionDecl | K::FixtureSetup | K::FixtureTeardown => node
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .find_map(|token| match token.kind() {
                        K::ServerKw => Some(webtest_model::Capability::Server),
                        K::BrowserKw => Some(webtest_model::Capability::Browser),
                        _ => None,
                    }),
                _ => None,
            }?;
            Some((node.text_range().len(), domain))
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, domain)| domain)
}
fn capability_allowed(
    domain: Option<webtest_model::Capability>,
    capability: webtest_model::Capability,
) -> bool {
    domain
        .is_none_or(|domain| capability == webtest_model::Capability::Pure || capability == domain)
}
fn call(parsed: &Parse, offset: TextSize) -> Option<ast::CallExpr> {
    parsed
        .syntax()
        .descendants()
        .filter_map(ast::CallExpr::cast)
        .filter(|call| contains(call.syntax(), offset))
        .min_by_key(|call| call.syntax().text_range().len())
}
pub(crate) fn signature(
    db: &dyn Db,
    file: SourceInput,
    parsed: &Parse,
    offset: TextSize,
) -> Option<Signature> {
    if let Some((source, _range)) = event_at(parsed, offset) {
        return Some(Signature {
            symbol: None,
            capability: Some(webtest_model::Capability::Browser),
            lifetime: None,
            label: format!("{}(event: {})", source.name(), source.payload_type()),
            documentation: format!("Normalized actor event. See event.{}", source.name()),
            parameters: vec![SignatureParameter {
                name: "event".into(),
                ty: Some(source.payload_type()),
                optional: false,
                default: None,
                label: format!("event: {}", source.payload_type()),
                documentation: String::new(),
            }],
            active_parameter: 0,
        });
    }
    let call = call(parsed, offset)?;
    let ast::Expr::Name(callee) = call.callee()? else {
        return None;
    };
    let name = callee.name()?.text().to_owned();
    let graph = crate::workspace::module_graph(db, file);
    let resolved = callables::resolve(&graph, graph.root, &name);
    if resolved.is_none()
        && let Some(builtin) = callables::builtin(&name)
    {
        let arguments = call.arguments().collect::<Vec<_>>();
        let active = arguments
            .iter()
            .enumerate()
            .filter(|(_, argument)| argument.syntax().text_range().start() <= offset)
            .map(|(index, _)| index)
            .next_back()
            .unwrap_or(0)
            .min(builtin.parameters.len().saturating_sub(1));
        return Some(Signature {
            symbol: Some(callables::builtin_id(builtin)),
            capability: Some(webtest_model::Capability::Pure),
            lifetime: None,
            label: format!(
                "{}({}) -> {} (Pure)",
                builtin.name,
                builtin
                    .parameters
                    .iter()
                    .map(|(name, ty)| format!("{name}: {ty}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                builtin.result
            ),
            documentation: builtin.documentation.into(),
            parameters: builtin
                .parameters
                .iter()
                .map(|(name, ty)| SignatureParameter {
                    name: (*name).into(),
                    ty: Some(ty.clone()),
                    optional: false,
                    default: None,
                    label: format!("{name}: {ty}"),
                    documentation: String::new(),
                })
                .collect(),
            active_parameter: active,
        });
    }
    let (module, id) = resolved?;
    let input = db.file_slot(&graph.modules.get(&module)?.path).target(db)?;
    let declaration = callables::signature(db, input, id)?;
    let arguments = call.arguments().collect::<Vec<_>>();
    let current = arguments
        .iter()
        .enumerate()
        .rfind(|(_, argument)| argument.syntax().text_range().start() <= offset);
    let active = current
        .and_then(|(index, argument)| {
            argument
                .name()
                .and_then(|name| {
                    declaration
                        .parameters
                        .iter()
                        .position(|parameter| parameter.name == name)
                })
                .or(Some(index))
        })
        .unwrap_or(0)
        .min(declaration.parameters.len().saturating_sub(1));
    let lifetime = declaration.lifetime.map_or_else(String::new, |lifetime| {
        format!("; scope {lifetime:?}").to_lowercase()
    });
    Some(Signature {
        symbol: Some(declaration.id),
        capability: Some(declaration.capability),
        lifetime: declaration.lifetime,
        label: format!(
            "{}({}) -> {} ({}{})",
            name,
            declaration
                .parameters
                .iter()
                .map(|parameter| format!("{}: {}", parameter.name, parameter.ty))
                .collect::<Vec<_>>()
                .join(", "),
            declaration.result,
            declaration.capability,
            lifetime
        ),
        documentation: declaration.documentation.clone(),
        parameters: declaration
            .parameters
            .iter()
            .map(|parameter| SignatureParameter {
                name: parameter.name.clone(),
                ty: Some(parameter.ty.clone()),
                optional: false,
                default: None,
                label: format!("{}: {}", parameter.name, parameter.ty),
                documentation: String::new(),
            })
            .collect(),
        active_parameter: active,
    })
}
pub(crate) fn documentation(
    db: &dyn Db,
    file: SourceInput,
    offset: TextSize,
) -> Option<DocumentationFact> {
    let parsed = crate::incremental::parsed(db, file);
    if let Some((source, range)) = event_at(&parsed, offset) {
        return Some(DocumentationFact {
            range,
            contents: format!(
                "{}(event: {})\nBrowser capability; bounded actor journal. See event.{}.",
                source.name(),
                source.payload_type(),
                source.name()
            ),
        });
    }
    let index = semantic_index::symbols(db, file);
    let reference = index
        .references
        .iter()
        .filter(|reference| reference.location.origin.range.contains(offset))
        .min_by_key(|reference| reference.location.origin.range.len())?;
    let symbol = index.symbols.get(&reference.resolved_symbol)?;
    if matches!(
        symbol.kind,
        crate::SymbolKind::ProviderNamespace | crate::SymbolKind::ProviderFunction
    ) || symbol.documentation.is_empty()
    {
        return None;
    }
    let lifetime = symbol.lifetime.map_or_else(String::new, |lifetime| {
        format!("\nFixture scope: {lifetime:?}.").to_lowercase()
    });
    Some(DocumentationFact {
        range: reference.location.origin.range,
        contents: format!(
            "{}\nCapability: {}.{lifetime}",
            symbol.documentation, symbol.capability
        ),
    })
}

use crate::{Completion, CompletionKind, SymbolKind};
use std::collections::{BTreeMap, BTreeSet};
use webtest_hir::HirStmt;
use webtest_text::SyntaxOrigin;

fn collect_visible(
    statements: &[HirStmt],
    offset: TextSize,
    visible: &mut BTreeMap<String, SyntaxOrigin>,
) {
    for statement in statements {
        let range = statement.origin().range;
        if range.start() > offset {
            break;
        }
        let inside = range.start() <= offset && offset <= range.end();
        match statement {
            HirStmt::Actor(actor) if range.end() <= offset => {
                visible.insert(actor.binding.name.clone(), actor.binding.name_origin);
            }
            HirStmt::Select(select) => {
                if inside {
                    for arm in &select.arms {
                        if arm.origin.range.start() <= offset && offset <= arm.origin.range.end() {
                            if let Some(binding) = &arm.binding {
                                visible.insert(binding.name.clone(), binding.name_origin);
                            }
                            collect_visible(&arm.statements, offset, visible);
                        }
                    }
                } else if let Some(binding) = &select.binding {
                    visible.insert(binding.name.clone(), binding.name_origin);
                }
            }
            HirStmt::Let(binding) if range.end() <= offset => {
                visible.insert(binding.name.clone(), binding.name_origin);
            }
            HirStmt::Race(race) if !inside => {
                if let Some(binding) = &race.binding {
                    visible.insert(binding.name.clone(), binding.name_origin);
                }
            }
            HirStmt::Server(block) => collect_visible(&block.statements, offset, visible),
            HirStmt::Browser(block) => collect_visible(&block.statements, offset, visible),
            _ if inside => match statement {
                HirStmt::Parallel(_) | HirStmt::Race(_) => {
                    for branch in statement.children().iter().filter(|branch| {
                        branch.origin().range.start() <= offset
                            && offset <= branch.origin().range.end()
                    }) {
                        collect_visible(std::slice::from_ref(branch), offset, visible);
                    }
                }
                _ => collect_visible(statement.children(), offset, visible),
            },
            _ => {}
        }
        if inside {
            break;
        }
    }
}
fn symbol_completion(symbol: &crate::Symbol, label: &str) -> Completion {
    Completion {
        label: label.into(),
        detail: format!(
            "{} ({})",
            symbol
                .ty
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            symbol.capability
        ),
        documentation: symbol.documentation.clone(),
        kind: match symbol.kind {
            SymbolKind::Pattern | SymbolKind::Guard | SymbolKind::Function => {
                CompletionKind::Function
            }
            SymbolKind::Fixture => CompletionKind::Fixture,
            SymbolKind::Parameter => CompletionKind::Parameter,
            SymbolKind::ProviderNamespace => CompletionKind::Namespace,
            _ => CompletionKind::Variable,
        },
        insert_text: label.into(),
        symbol: Some(symbol.id),
        ..Default::default()
    }
}
pub(crate) fn completions(
    db: &dyn Db,
    file: SourceInput,
    parsed: &Parse,
    offset: TextSize,
) -> Vec<Completion> {
    use webtest_syntax::SyntaxKind as K;
    let syntax = parsed.syntax();
    if let Some(items) = event_completions(parsed, offset) {
        return items;
    }
    let graph = crate::workspace::module_graph(db, file);
    let Some(module) = graph.modules.get(&graph.root) else {
        return Vec::new();
    };
    let index = semantic_index::symbols(db, file);
    // A member receiver has a precise compiler type; never replace a failed
    // member lookup with unrelated workspace names.
    if syntax
        .descendants()
        .any(|node| node.kind() == K::MemberExpr && contains(&node, offset))
    {
        return Vec::new();
    }
    if let Some(fixture) = syntax
        .descendants()
        .filter_map(ast::FixtureDecl::cast)
        .find(|fixture| contains(fixture.syntax(), offset))
    {
        let tokens = fixture
            .syntax()
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .collect::<Vec<_>>();
        if tokens
            .iter()
            .any(|token| token.kind() == K::ScopeKw && token.text_range().end() <= offset)
            && tokens
                .iter()
                .find(|token| token.kind() == K::LBrace)
                .is_none_or(|token| offset <= token.text_range().start())
        {
            return ["test", "file", "worker", "suite"]
                .into_iter()
                .map(|name| Completion {
                    label: name.into(),
                    kind: CompletionKind::Keyword,
                    insert_text: name.into(),
                    detail: "fixture lifetime".into(),
                    ..Default::default()
                })
                .collect();
        }
    }
    let token = syntax
        .token_at_offset(offset)
        .left_biased()
        .or_else(|| syntax.token_at_offset(offset).right_biased());
    let in_type = syntax.descendants().any(|node| {
        matches!(node.kind(), K::NamedType | K::GenericType) && contains(&node, offset)
    }) || token.as_ref().is_some_and(|token| {
        let mut token = token.clone();
        while token.kind().is_trivia() {
            let Some(previous) = token.prev_token() else {
                return false;
            };
            token = previous;
        }
        matches!(token.kind(), K::Colon | K::Arrow)
            && token.parent().is_some_and(|parent| {
                matches!(
                    parent.kind(),
                    K::LetStmt | K::Parameter | K::TypeField | K::FunctionDecl | K::FixtureDecl
                )
            })
    });
    if in_type {
        return crate::compiler::type_completions();
    }
    if let Some(import) = syntax
        .descendants()
        .filter_map(ast::ImportDecl::cast)
        .find(|import| contains(import.syntax(), offset))
    {
        if let Some(path) = import.path() {
            if path.syntax().text_range().start() <= offset
                && offset <= path.syntax().text_range().end()
            {
                return import_paths(db, file, path.syntax().text_range());
            }
            let origin = SyntaxOrigin::new(file.file(db), path.syntax().text_range());
            if let Some((target, _)) = module.dependencies.iter().find(|(_, edge)| *edge == origin)
                && let Some(target) = graph.modules.get(target)
            {
                let present = import
                    .bindings()
                    .filter_map(|binding| binding.name().map(|name| name.text().to_owned()))
                    .collect::<BTreeSet<_>>();
                return target
                    .hir
                    .functions
                    .iter()
                    .filter(|function| function.exported && !present.contains(&function.name))
                    .map(|function| {
                        (
                            function.name.clone(),
                            webtest_model::SymbolId(function.id.0),
                            CompletionKind::Function,
                        )
                    })
                    .chain(
                        target
                            .hir
                            .fixtures
                            .iter()
                            .filter(|fixture| fixture.exported && !present.contains(&fixture.name))
                            .map(|fixture| {
                                (
                                    fixture.name.clone(),
                                    webtest_model::SymbolId(fixture.id.0),
                                    CompletionKind::Fixture,
                                )
                            }),
                    )
                    .map(|(name, id, kind)| Completion {
                        label: name.clone(),
                        insert_text: name,
                        kind,
                        symbol: Some(id),
                        ..Default::default()
                    })
                    .collect();
            }
        }
        return Vec::new();
    }
    let domain = context_domain(parsed, offset);
    let mut candidates = BTreeMap::<String, Completion>::new();
    let mut visible = BTreeMap::new();
    for test in module
        .hir
        .tests
        .iter()
        .filter(|test| test.origin.range.contains(offset))
    {
        if let Some(binding) = &test.case_binding {
            visible.insert(binding.name.clone(), binding.name_origin);
        }
        collect_visible(&test.body, offset, &mut visible);
    }
    for function in module
        .hir
        .functions
        .iter()
        .filter(|function| function.origin.range.contains(offset))
    {
        for parameter in &function.parameters {
            visible.insert(parameter.name.clone(), parameter.name_origin);
        }
        collect_visible(&function.body, offset, &mut visible);
    }
    for fixture in module
        .hir
        .fixtures
        .iter()
        .filter(|fixture| fixture.origin.range.contains(offset))
    {
        for parameter in &fixture.parameters {
            visible.insert(parameter.name.clone(), parameter.name_origin);
        }
        collect_visible(&fixture.setup, offset, &mut visible);
        collect_visible(&fixture.teardown, offset, &mut visible);
    }
    for pattern in &module.hir.patterns {
        if pattern.origin.range.contains(offset) {
            for parameter in &pattern.parameters {
                visible.insert(parameter.name.clone(), parameter.name_origin);
            }
        }
    }
    for guard in &module.hir.guards {
        if guard.origin.range.contains(offset) {
            for parameter in &guard.parameters {
                visible.insert(parameter.name.clone(), parameter.name_origin);
            }
            for clause in &guard.clauses {
                if clause.origin.range.contains(offset) {
                    if let Some(binding) = &clause.binding {
                        visible.insert(binding.name.clone(), binding.name_origin);
                    }
                    collect_visible(&clause.statements, offset, &mut visible);
                }
            }
        }
    }
    for (name, origin) in visible {
        if let Some(symbol) = index.symbols.values().find(|symbol| {
            symbol
                .location
                .as_ref()
                .is_some_and(|location| location.origin == origin)
        }) && (domain != Some(webtest_model::Capability::Browser)
            || symbol.capability != webtest_model::Capability::Server
            || symbol.ty.as_ref().is_none_or(|ty| ty.is_transferable()))
        {
            candidates.insert(name.clone(), symbol_completion(symbol, &name));
        }
    }
    for (name, target) in module
        .hir
        .functions
        .iter()
        .map(|function| (&function.name, webtest_model::SymbolId(function.id.0)))
        .chain(
            module
                .hir
                .patterns
                .iter()
                .map(|pattern| (&pattern.name, webtest_model::SymbolId(pattern.id.0))),
        )
        .chain(
            module
                .hir
                .guards
                .iter()
                .map(|guard| (&guard.name, webtest_model::SymbolId(guard.id.0))),
        )
        .chain(
            module
                .hir
                .fixtures
                .iter()
                .map(|fixture| (&fixture.name, webtest_model::SymbolId(fixture.id.0))),
        )
        .chain(
            module
                .imports
                .iter()
                .map(|(name, import)| (name, import.target)),
        )
    {
        if let Some(symbol) = index
            .symbols
            .get(&target)
            .filter(|symbol| capability_allowed(domain, symbol.capability))
        {
            candidates
                .entry(name.clone())
                .or_insert_with(|| symbol_completion(symbol, name));
        }
    }
    if let Some(call) = call(parsed, offset)
        && let Some(ast::Expr::Name(callee)) = call.callee()
        && let Some(name) = callee.name()
        && let Some((target, id)) = callables::resolve(&graph, module.id, name.text())
        && let Some(input) = db.file_slot(&graph.modules[&target].path).target(db)
        && let Some(signature) = callables::signature(db, input, id)
    {
        let present = call
            .arguments()
            .filter_map(|argument| argument.name())
            .collect::<BTreeSet<_>>();
        for parameter in &signature.parameters {
            if !present.contains(&parameter.name) {
                candidates.insert(
                    format!("{}:", parameter.name),
                    Completion {
                        label: parameter.name.clone(),
                        detail: parameter.ty.to_string(),
                        kind: CompletionKind::Parameter,
                        insert_text: format!("{}: ", parameter.name),
                        symbol: Some(webtest_model::SymbolId(parameter.id.0)),
                        ..Default::default()
                    },
                );
            }
        }
    }
    for builtin in callables::builtins() {
        if let Some(symbol) = index.symbols.get(&callables::builtin_id(builtin)) {
            candidates
                .entry(builtin.name.into())
                .or_insert_with(|| symbol_completion(symbol, builtin.name));
        }
    }
    for name in crate::incremental::provider_names(db) {
        if !db
            .schema_slot(&name)
            .schema(db)
            .as_ref()
            .is_some_and(|schema| {
                schema
                    .operations
                    .values()
                    .any(|operation| capability_allowed(domain, operation.capability))
            })
        {
            continue;
        }
        candidates
            .entry(name.clone())
            .or_insert_with(|| Completion {
                label: name.clone(),
                insert_text: format!("{name}."),
                detail: "provider namespace".into(),
                kind: CompletionKind::Namespace,
                symbol: Some(webtest_model::SymbolId(webtest_hir::semantic_key(
                    &db.workspace_input().identity(db).0,
                    "provider",
                    &name,
                    0,
                ))),
                ..Default::default()
            });
    }
    let top_level = !syntax.descendants().any(|node| {
        matches!(node.kind(), K::TestDecl | K::FunctionDecl | K::FixtureDecl)
            && contains(&node, offset)
    });
    let statement_start = token.as_ref().is_none_or(|token| {
        token.parent_ancestors().all(|node| {
            !matches!(
                node.kind(),
                K::LetStmt | K::CallArg | K::RecordField | K::BinaryExpr | K::UnaryExpr
            )
        })
    });
    if top_level {
        for keyword in [
            "test", "import", "export", "fn", "fixture", "pattern", "guard",
        ] {
            candidates.insert(
                keyword.into(),
                Completion {
                    label: keyword.into(),
                    insert_text: format!("{keyword} "),
                    kind: CompletionKind::Keyword,
                    ..Default::default()
                },
            );
        }
    } else if statement_start
        && !module.hir.functions.iter().any(|function| {
            function
                .expression
                .as_ref()
                .is_some_and(|expression| expression.origin.range.contains(offset))
        })
    {
        let browser = domain == Some(webtest_model::Capability::Browser);
        let server = domain == Some(webtest_model::Capability::Server);
        let mut keywords = vec![
            "let",
            "timeout",
            "parallel",
            "race",
            "retry",
            "expect",
            "check",
            "pass",
            "fail",
            "skip",
            "inconclusive",
            "assume",
            "actor",
            "with guards",
            "select",
            "checkpoint",
        ];
        if !browser && !server {
            keywords.extend(["server", "browser"]);
        }
        if browser {
            keywords.extend([
                "open", "click", "fill", "type", "press", "check", "uncheck", "select", "hover",
                "wait",
            ]);
        }
        for keyword in keywords {
            candidates
                .entry(keyword.into())
                .or_insert_with(|| Completion {
                    label: keyword.into(),
                    insert_text: format!("{keyword} "),
                    kind: CompletionKind::Keyword,
                    ..Default::default()
                });
        }
    }
    let locator_context = token.as_ref().is_some_and(|token| {
        token.parent_ancestors().any(|node| {
            matches!(
                node.kind(),
                K::ClickStmt
                    | K::FillStmt
                    | K::TypeStmt
                    | K::PressStmt
                    | K::CheckStmt
                    | K::UncheckStmt
                    | K::SelectStmt
                    | K::HoverStmt
                    | K::WaitLocatorStmt
                    | K::ExpectLocatorStmt
            )
        })
    });
    if locator_context {
        let state_context = token.as_ref().is_some_and(|token| {
            token.parent_ancestors().any(|node| {
                matches!(node.kind(), K::WaitLocatorStmt | K::ExpectLocatorStmt)
                    && node
                        .children_with_tokens()
                        .filter_map(|element| element.into_token())
                        .any(|token| token.kind() == K::Dot && token.text_range().end() <= offset)
            })
        });
        let words = webtest_syntax::author_facing_language().reserved_words;
        if state_context {
            return words
                .into_iter()
                .filter(|word| {
                    webtest_syntax::keyword_kind(word).is_some_and(|kind| kind.is_locator_state())
                })
                .map(|word| Completion {
                    label: word.clone(),
                    insert_text: word,
                    kind: CompletionKind::Keyword,
                    ..Default::default()
                })
                .collect();
        }
        for word in words.into_iter().filter(|word| {
            webtest_syntax::keyword_kind(word).is_some_and(|kind| kind.is_locator_start())
        }) {
            candidates.insert(
                word.clone(),
                Completion {
                    label: word.clone(),
                    insert_text: format!("{word}("),
                    kind: CompletionKind::Function,
                    ..Default::default()
                },
            );
        }
    }
    if !top_level {
        for completion in auto_imports(db, file, parsed, offset) {
            candidates
                .entry(completion.label.clone())
                .or_insert(completion);
        }
    }
    candidates.into_values().take(1000).collect()
}
pub(crate) fn relative_import(importing: &str, target: &str) -> Option<String> {
    let importing = crate::workspace::normalize_path(importing)?;
    let target = crate::workspace::normalize_path(target)?;
    let parent = importing.rsplit_once('/').map_or("", |(parent, _)| parent);
    let from = parent
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let to = target
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(to[common..].iter().copied());
    let path = parts.join("/");
    Some(if path.starts_with("../") {
        path
    } else {
        format!("./{path}")
    })
}
fn allowed_import(db: &dyn Db, file: SourceInput, target: SourceInput) -> Option<String> {
    let path = relative_import(file.path(db), target.path(db))?;
    let config = db.workspace_input().configuration(db);
    let valid = crate::workspace::candidates(config, file.path(db), &path)
        .ok()?
        .into_iter()
        .any(|candidate| {
            db.file_slot(&candidate)
                .target(db)
                .is_some_and(|input| input.file(db) == target.file(db))
        });
    valid.then_some(path)
}
fn import_paths(db: &dyn Db, file: SourceInput, range: webtest_text::TextRange) -> Vec<Completion> {
    let mut seen = BTreeSet::new();
    db.workspace_input()
        .files(db)
        .iter()
        .filter_map(|(_, target)| {
            db.unwind_if_revision_cancelled();
            if target.file(db) == file.file(db) || !seen.insert(target.file(db).get()) {
                return None;
            }
            let path = allowed_import(db, file, *target)?;
            Some(Completion {
                label: path.clone(),
                insert_text: serde_json::to_string(&path).ok()?,
                kind: CompletionKind::Module,
                detail: "workspace module".into(),
                replacement_range: Some(webtest_feedback::ByteRange {
                    start: range.start().into(),
                    end: range.end().into(),
                }),
                ..Default::default()
            })
        })
        .take(1000)
        .collect()
}
pub(crate) fn auto_imports(
    db: &dyn Db,
    file: SourceInput,
    parsed: &Parse,
    offset: TextSize,
) -> Vec<Completion> {
    let token = parsed.syntax().token_at_offset(offset).left_biased();
    let prefix = token
        .as_ref()
        .filter(|token| webtest_syntax::is_name_kind(token.kind()))
        .and_then(|token| {
            token
                .text()
                .get(..usize::from(offset - token.text_range().start()))
        })
        .unwrap_or("");
    if prefix.is_empty() {
        return Vec::new();
    }
    let mut seen = BTreeSet::new();
    let mut choices =
        BTreeMap::<String, Vec<(SourceInput, webtest_model::SymbolId, CompletionKind)>>::new();
    for (_, target) in db.workspace_input().files(db) {
        db.unwind_if_revision_cancelled();
        if target.file(db) == file.file(db) || !seen.insert(target.file(db).get()) {
            continue;
        }
        let hir = crate::incremental::lowered(db, *target);
        for (name, exported, id, kind) in hir
            .functions
            .iter()
            .map(|function| {
                (
                    &function.name,
                    function.exported,
                    webtest_model::SymbolId(function.id.0),
                    CompletionKind::Function,
                )
            })
            .chain(hir.patterns.iter().map(|pattern| {
                (
                    &pattern.name,
                    pattern.exported,
                    webtest_model::SymbolId(pattern.id.0),
                    CompletionKind::Function,
                )
            }))
            .chain(hir.guards.iter().map(|guard| {
                (
                    &guard.name,
                    guard.exported,
                    webtest_model::SymbolId(guard.id.0),
                    CompletionKind::Function,
                )
            }))
            .chain(hir.fixtures.iter().map(|fixture| {
                (
                    &fixture.name,
                    fixture.exported,
                    webtest_model::SymbolId(fixture.id.0),
                    CompletionKind::Fixture,
                )
            }))
        {
            if exported && name.starts_with(prefix) {
                choices
                    .entry(name.clone())
                    .or_default()
                    .push((*target, id, kind));
            }
        }
    }
    choices
        .into_iter()
        .filter_map(|(name, mut declarations)| {
            if declarations.len() != 1 {
                return None;
            }
            let (target, id, kind) = declarations.pop()?;
            let path = allowed_import(db, file, target)?;
            let signature = callables::signature(db, target, id)?;
            if !capability_allowed(context_domain(parsed, offset), signature.capability) {
                return None;
            }
            Some(Completion {
                label: name.clone(),
                insert_text: name.clone(),
                kind,
                symbol: Some(id),
                detail: format!("import from {path}"),
                documentation: signature.documentation.clone(),
                additional_edits: vec![crate::TextEdit {
                    origin: SyntaxOrigin::new(
                        file.file(db),
                        webtest_text::TextRange::empty(0.into()),
                    ),
                    replacement: format!(
                        "import {{ {name} }} from {}\n",
                        serde_json::to_string(&path).ok()?
                    ),
                }],
                ..Default::default()
            })
        })
        .take(200)
        .collect()
}
pub(crate) fn decorate(parsed: &Parse, offset: TextSize, items: &mut Vec<Completion>) {
    let syntax = parsed.syntax();
    let token = syntax
        .token_at_offset(offset)
        .left_biased()
        .or_else(|| syntax.token_at_offset(offset).right_biased());
    let token = token.filter(|token| {
        (webtest_syntax::is_name_kind(token.kind()) || token.kind().is_keyword())
            && token.text_range().start() <= offset
            && offset <= token.text_range().end()
    });
    let prefix = token
        .as_ref()
        .and_then(|token| {
            token
                .text()
                .get(..usize::from(offset - token.text_range().start()))
        })
        .unwrap_or("");
    let range = token.as_ref().map_or_else(
        || webtest_text::TextRange::empty(offset),
        |token| token.text_range(),
    );
    items.retain(|item| prefix.is_empty() || item.label.starts_with(prefix));
    for (order, item) in items.iter_mut().enumerate() {
        if item.replacement_range.is_none() {
            item.replacement_range = Some(webtest_feedback::ByteRange {
                start: range.start().into(),
                end: range.end().into(),
            });
        }
        item.sort_text = format!("{order:05}");
        item.filter_text = item.label.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalysisDatabase, WorkspaceConfiguration};
    fn position(source: &str, needle: &str, skip: usize) -> TextSize {
        (source.find(needle).unwrap() as u32 + skip as u32).into()
    }
    #[test]
    fn imported_helper_signature_hover_and_returned_members_use_resolved_types() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(WorkspaceConfiguration {
            root: "/project".into(),
            ..Default::default()
        });
        db.open_file("/project/support.webtest","/// Create a typed user.\nexport fn user(email: String, age: Int) -> { email: String, age: Int } = { email: email, age: age }");
        let source = "import { user as make } from \"./support\"\ntest \"x\" { let result = make(age: 4, email: \"é\") expect result.email == \"é\" }";
        let file = db.open_file("/project/tests.webtest", source);
        let help = db
            .signature_help(file, position(source, "email: \"é\"", 2))
            .unwrap()
            .unwrap();
        assert_eq!(help.active_parameter, 0);
        assert!(help.label.starts_with("make(email: String, age: Int)"));
        assert!(help.documentation.contains("typed user"));
        assert!(help.symbol.is_some());
        let hover = db
            .documentation_at(file, position(source, "make(age", 1))
            .unwrap()
            .unwrap();
        assert!(hover.contents.contains("typed user"));
        assert!(hover.contents.contains("Pure"));
        db.set_file_text(file,"import { user as make } from \"./support\"\ntest \"x\" { let result = make(age: 4, email: \"é\") result. }");
        let source = db.source(file).unwrap();
        let completions = db
            .completions(file, position(&source, "result.", 7))
            .unwrap();
        assert_eq!(
            completions
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["age", "email"]
        );
        assert!(
            completions
                .iter()
                .all(|item| item.replacement_range.is_some())
        );
    }
    #[test]
    fn recovered_local_completion_does_not_escape_control_or_sibling_scopes() {
        let mut db = AnalysisDatabase::default();
        let source = "test \"x\" { let outer = 1 timeout 1s { let inner = 2 } parallel { server { let sibling = 1 } server { ou } } }";
        let file = db.open_file("scope.webtest", source);
        let items = db.completions(file, position(source, "{ ou", 4)).unwrap();
        assert!(items.iter().any(|item| item.label == "outer"));
        assert!(
            !items
                .iter()
                .any(|item| item.label == "inner" || item.label == "sibling")
        );
        let source = "fn user(email: String, age: Int) -> Int = age\ntest \"x\" { let value = user(email: \"é\", a) }";
        db.set_file_text(file, source);
        let items = db.completions(file, position(source, ", a)", 3)).unwrap();
        let age = items.iter().find(|item| item.label == "age").unwrap();
        assert_eq!(age.insert_text, "age: ");
        assert!(age.symbol.is_some());
        assert_eq!(age.filter_text, "age");
        assert!(
            db.completions(file, TextSize::from(source.len() as u32 + 1))
                .is_err()
        );
    }
    #[test]
    fn fixture_lifetimes_imports_types_and_browser_states_follow_recovered_cst() {
        let mut db = AnalysisDatabase::default();
        for (source, needle, skip, expected) in [
            (
                "fixture data() -> Int scope fi { setup { provide 1 } }",
                "scope fi",
                8,
                "file",
            ),
            ("test \"x\" { let value: Str = \"x\" }", "Str", 3, "String"),
            (
                "test \"x\" { browser { expect role(\"button\").vis } }",
                ".vis",
                4,
                "visible",
            ),
        ] {
            let file = db.open_file("recovered.webtest", source);
            let items = db
                .completions(file, position(source, needle, skip))
                .unwrap();
            assert!(
                items.iter().any(|item| item.label == expected),
                "{source}: {items:?}"
            );
        }
    }
}

#[cfg(test)]
mod refinement_tests {

    use crate::{AnalysisDatabase, WorkspaceConfiguration};
    #[test]
    fn module_paths_and_unique_auto_imports_work_on_recovered_source() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(WorkspaceConfiguration {
            root: "/project".into(),
            ..Default::default()
        });
        db.open_file(
            "/project/support.webtest",
            "export fn user_name(value: String) -> String = lower(value)",
        );
        let source = "import { user_name } from \"./sup\"";
        let file = db.open_file("/project/test.webtest", source);
        let completions = db
            .completions(file, (source.len() as u32 - 1).into())
            .unwrap();
        let path = completions
            .iter()
            .find(|item| item.label == "./support.webtest")
            .unwrap();
        assert_eq!(path.insert_text, "\"./support.webtest\"");
        let source = "test \"x\" { let value = user_na }";
        db.set_file_text(file, source);
        let offset = source.find("user_na").unwrap() + "user_na".len();
        let completions = db.completions(file, (offset as u32).into()).unwrap();
        let imported = completions
            .iter()
            .find(|item| item.label == "user_name")
            .unwrap();
        assert_eq!(imported.additional_edits.len(), 1);
        assert!(
            imported.additional_edits[0]
                .replacement
                .contains("./support.webtest")
        );
        db.open_file(
            "/project/ambiguous.webtest",
            "export fn user_name(value: String) -> String = value",
        );
        assert!(
            !db.completions(file, (offset as u32).into())
                .unwrap()
                .iter()
                .any(|item| item.label == "user_name")
        );
    }
    #[test]
    fn browser_completion_hides_server_provider_operations_and_helpers() {
        let mut db = AnalysisDatabase::default();
        let source = "fn server_value() -> Int server { provide 7 } test \"x\" { browser { let value = ser } }";
        let file = db.open_file("domain.webtest", source);
        let offset = (source.rfind("ser").unwrap() as u32 + 3).into();
        assert!(
            !db.completions(file, offset)
                .unwrap()
                .iter()
                .any(|item| item.label == "server_value")
        );
        let source = "test \"x\" { browser { http. } }";
        db.set_file_text(file, source);
        assert!(
            db.completions(file, (source.find("http.").unwrap() as u32 + 5).into())
                .unwrap()
                .is_empty()
        );
    }
}

fn event_path(expression: ast::Expr) -> Option<Vec<String>> {
    match expression {
        ast::Expr::Name(name) => Some(vec![name.name()?.text().into()]),
        ast::Expr::Member(member) => {
            let mut path = event_path(member.receiver()?)?;
            if let Some(name) = member.member() {
                path.push(name.text().into());
            }
            Some(path)
        }
        _ => None,
    }
}
fn event_name(
    arm: &ast::EventArm,
) -> Option<(webtest_model::EventSource, webtest_text::TextRange)> {
    let source = arm.source()?;
    let ast::Expr::Call(call) = source else {
        return None;
    };
    let callee = call.callee()?;
    let range = callee.syntax().text_range();
    let mut parts = event_path(callee)?;
    if arm
        .syntax()
        .ancestors()
        .any(|n| n.kind() == webtest_syntax::SyntaxKind::GuardDecl)
        && !parts.is_empty()
    {
        parts.remove(0);
    }
    Some((
        webtest_model::EventSource::from_name(&parts.join("."))?,
        range,
    ))
}
fn event_at(
    parsed: &Parse,
    offset: TextSize,
) -> Option<(webtest_model::EventSource, webtest_text::TextRange)> {
    parsed
        .syntax()
        .descendants()
        .filter_map(ast::EventArm::cast)
        .filter(|arm| arm.source().is_some_and(|s| contains(s.syntax(), offset)))
        .find_map(|arm| event_name(&arm))
}
fn event_completions(parsed: &Parse, offset: TextSize) -> Option<Vec<Completion>> {
    let arm = parsed
        .syntax()
        .descendants()
        .filter_map(ast::EventArm::cast)
        .find(|arm| {
            contains(arm.syntax(), offset)
                && arm.source().is_none_or(|s| contains(s.syntax(), offset))
        })?;
    let mut prefix = Vec::new();
    if let Some(source) = arm.source() {
        let callee = if let ast::Expr::Call(call) = source {
            let callee = call.callee()?;
            if !contains(callee.syntax(), offset) {
                return None;
            }
            callee
        } else {
            source
        };
        if let ast::Expr::Member(member) = callee {
            prefix = event_path(member.receiver()?).unwrap_or_default();
        }
    }
    if arm
        .syntax()
        .ancestors()
        .any(|n| n.kind() == webtest_syntax::SyntaxKind::GuardDecl)
        && !prefix.is_empty()
    {
        prefix.remove(0);
    }
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("{}.", prefix.join("."))
    };
    Some(
        webtest_model::EventSource::ALL
            .into_iter()
            .filter_map(|source| {
                let insert = source.name().strip_prefix(&prefix)?;
                Some(Completion {
                    label: source.name().into(),
                    insert_text: insert.into(),
                    kind: CompletionKind::Function,
                    detail: format!("event: {}", source.payload_type()),
                    ..Default::default()
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod reactive_tests {
    use crate::{AnalysisDatabase, WorkspaceConfiguration};
    use webtest_text::TextSize;
    fn at(text: &str, needle: &str) -> TextSize {
        (text.find(needle).unwrap() as u32).into()
    }
    #[test]
    fn imported_guards_patterns_and_actor_helpers_keep_shared_navigation_and_event_services() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(WorkspaceConfiguration {
            root: "/h".into(),
            ..Default::default()
        });
        let support = "export pattern Named(name: String) = subset {name: name}\nexport guard clean(actor: BrowserActor) {when actor.console.error(e) {fail e.message}}\nexport fn visit(actor: BrowserActor, target: String) -> Null browser {actor {open target}}";
        let library = db.open_file("/h/support.webtest", support);
        let source = "import {Named, clean, visit} from \"./support\"\ntest \"H\" cases [case \"a\" {name: \"Ada\"}] as input {actor alice browser with guards [clean(alice)] {visit(alice, \"http://example.test\") expect input matches Named(input.name)} select alice within 1s {when network.response(e) if e.status == 200 {pass} timeout {skip \"offline\"}}}";
        let file = db.open_file("/h/test.webtest", source);
        assert!(
            db.diagnostics(file).unwrap().is_empty(),
            "{:?}",
            db.diagnostics(file).unwrap()
        );
        db.test_plan(file).unwrap().validate_tree().unwrap();
        let definition = db
            .definition(file, at(source, "clean(alice)"))
            .unwrap()
            .unwrap();
        assert_eq!(definition.target.origin.file, library);
        let renamed = db
            .rename(library, at(support, "clean(actor"), "quiet")
            .unwrap();
        assert_eq!(renamed.documents.len(), 2);
        let renamed = db
            .rename(library, at(support, "actor: BrowserActor"), "participant")
            .unwrap();
        assert!(renamed.documents[0].edits.len() >= 2);
        let signature = db
            .signature_help(file, at(source, "response(e") + TextSize::from(10))
            .unwrap()
            .unwrap();
        assert!(signature.label.contains("network.response"));
        assert!(signature.label.contains("status"));
        let hover = db
            .documentation_at(file, at(source, "network.response") + TextSize::from(9))
            .unwrap()
            .unwrap();
        assert!(hover.contents.contains("event.network.response"));
        db.set_file_text(
            file,
            "test \"completion\" {actor alice browser select alice within 1s {when network.}}",
        );
        let edited = db.source(file).unwrap();
        let completion = db
            .completions(file, at(&edited, "network.") + TextSize::from(8))
            .unwrap();
        assert!(
            completion.iter().any(|c| c.label == "network.response"),
            "{completion:?}"
        );
    }
}
