//! Bounded navigation and versioned rename over the shared semantic index.
use crate::{
    facts::*,
    incremental::{Db, SourceInput},
    semantic_index::{self, SymbolIndex},
};
use rowan::ast::AstNode;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use webtest_model::SymbolId;
use webtest_text::TextSize;

pub const MAX_QUERY_ITEMS: usize = 10_000;
#[derive(Debug, thiserror::Error)]
pub enum RenameError {
    #[error(transparent)]
    Analysis(#[from] crate::AnalysisError),
    #[error("no renameable symbol at this position")]
    NoSymbol,
    #[error("the new name is not a legal identifier")]
    InvalidName,
    #[error("the symbol is read-only or belongs to an external data contract")]
    ReadOnly,
    #[error("renaming would collide with or shadow `{0}`")]
    Collision(String),
    #[error("rename exceeds the workspace edit limit; no partial edit was produced")]
    Limit,
}
fn at(index: &SymbolIndex, offset: TextSize) -> Option<&Reference> {
    index
        .references
        .iter()
        .filter(|r| r.location.origin.range.contains(offset))
        .min_by_key(|r| r.location.origin.range.len())
        .or_else(|| {
            index
                .references
                .iter()
                .filter(|r| r.location.origin.range.end() == offset)
                .min_by_key(|r| r.location.origin.range.len())
        })
}
fn inputs(db: &dyn Db) -> Vec<SourceInput> {
    let mut files = db
        .workspace_input()
        .files(db)
        .iter()
        .map(|(_, input)| *input)
        .collect::<Vec<_>>();
    files.sort_by_key(|input| input.file(db).get());
    files.dedup();
    files
}
fn indexes(db: &dyn Db) -> Vec<Arc<SymbolIndex>> {
    inputs(db)
        .into_iter()
        .map(|file| {
            db.unwind_if_revision_cancelled();
            semantic_index::symbols(db, file)
        })
        .collect()
}
pub(crate) fn document_symbols(db: &dyn Db, file: SourceInput) -> Vec<Symbol> {
    let index = semantic_index::symbols(db, file);
    let mut symbols = index
        .symbols
        .values()
        .filter(|symbol| {
            symbol
                .location
                .as_ref()
                .is_some_and(|location| location.origin.file == file.file(db))
        })
        .cloned()
        .collect::<Vec<_>>();
    symbols.sort_by_key(|symbol| {
        symbol
            .location
            .as_ref()
            .map(|location| location.origin.range.start())
    });
    symbols
}
/// Hints are projected from compiler-backed symbols; annotations remain authoritative.
pub(crate) fn inlay_hints(db: &dyn Db, file: SourceInput) -> QueryResult<InlayHint> {
    use rowan::ast::AstNode;
    let parsed = crate::incremental::parsed(db, file);
    let index = semantic_index::symbols(db, file);
    let mut items = Vec::new();
    for binding in parsed
        .syntax()
        .descendants()
        .filter_map(webtest_syntax::ast::LetStmt::cast)
    {
        db.unwind_if_revision_cancelled();
        if binding.annotation().is_some() {
            continue;
        }
        let Some(name) = binding.name() else {
            continue;
        };
        let ty = index
            .symbols
            .values()
            .find(|symbol| {
                symbol.kind == SymbolKind::Binding
                    && symbol
                        .location
                        .as_ref()
                        .is_some_and(|location| location.origin.range == name.text_range())
            })
            .and_then(|symbol| symbol.ty.as_ref());
        let Some(ty) = ty.filter(|ty| **ty != webtest_model::Type::Unknown) else {
            continue;
        };
        if items.len() == MAX_QUERY_ITEMS {
            return QueryResult {
                items,
                truncated: true,
            };
        }
        items.push(InlayHint {
            file: file.file(db),
            offset: name.text_range().end().into(),
            label: format!(": {ty}"),
        });
    }
    QueryResult {
        items,
        truncated: false,
    }
}

/// Compiler diagnostics and canonical formatting produce revision-bound edits.
pub(crate) fn code_actions(
    db: &dyn Db,
    file: SourceInput,
    requested: webtest_text::TextRange,
) -> QueryResult<CodeAction> {
    let compiled = crate::incremental::compiled(db, file);
    let source = file.text(db);
    let mut items = Vec::new();
    let parsed = crate::incremental::parsed(db, file);
    let edit = |edits| WorkspaceEdit {
        documents: vec![DocumentEdit {
            path: file.path(db).clone(),
            file: file.file(db),
            revision: webtest_text::SourceRevision::of(source),
            version: file.version(db),
            edits,
        }],
    };
    for diagnostic in &compiled.diagnostics {
        db.unwind_if_revision_cancelled();
        if diagnostic
            .range
            .intersect(requested)
            .is_none_or(|range| range.is_empty())
            && !diagnostic.range.contains(requested.start())
        {
            continue;
        }
        if diagnostic.related_locations.is_empty()
            && matches!(
                diagnostic.code,
                "semantic.unknown_function" | "semantic.unknown_name"
            )
            && let Some(token) = parsed
                .syntax()
                .token_at_offset(diagnostic.range.start())
                .right_biased()
                .filter(|token| webtest_syntax::is_name_kind(token.kind()))
        {
            let name = token.text();
            for completion in
                crate::workspace_editor::auto_imports(db, file, &parsed, token.text_range().end())
                    .into_iter()
                    .filter(|completion| completion.label == name)
            {
                items.push(CodeAction {
                    title: format!("Import `{name}`"),
                    diagnostic_code: Some(diagnostic.code.into()),
                    edit: edit(completion.additional_edits),
                });
            }
            let qualified = crate::incremental::provider_names(db)
                .into_iter()
                .filter(|provider| {
                    db.schema_slot(provider)
                        .schema(db)
                        .as_ref()
                        .is_some_and(|schema| schema.operation(name).is_some())
                })
                .collect::<Vec<_>>();
            if qualified.len() == 1 {
                items.push(CodeAction {
                    title: format!("Qualify as `{}.{name}`", qualified[0]),
                    diagnostic_code: Some(diagnostic.code.into()),
                    edit: edit(vec![TextEdit {
                        origin: webtest_text::SyntaxOrigin::new(file.file(db), token.text_range()),
                        replacement: format!("{}.{name}", qualified[0]),
                    }]),
                });
            }
        }
        if diagnostic.code == "semantic.missing_argument"
            && diagnostic.related_locations.is_empty()
            && let Some(details) = &diagnostic.semantic_details
            && let (Some(name), Some(ty)) = (
                details.get("parameter").and_then(|value| value.as_str()),
                details.get("type").and_then(|value| {
                    serde_json::from_value::<webtest_model::Type>(value.clone()).ok()
                }),
            )
            && let Some(value) = placeholder(&ty, 0)
            && let Some(call) = parsed
                .syntax()
                .descendants()
                .filter_map(webtest_syntax::ast::CallExpr::cast)
                .filter(|call| call.syntax().text_range().contains_range(diagnostic.range))
                .min_by_key(|call| call.syntax().text_range().len())
            && let Some(close) = call
                .syntax()
                .children_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| token.kind() == webtest_syntax::SyntaxKind::RParen)
        {
            let separator = if call.arguments().next().is_some() {
                ", "
            } else {
                ""
            };
            items.push(CodeAction {
                title: format!("Add required argument `{name}`"),
                diagnostic_code: Some(diagnostic.code.into()),
                edit: edit(vec![TextEdit {
                    origin: webtest_text::SyntaxOrigin::new(
                        file.file(db),
                        webtest_text::TextRange::empty(close.text_range().start()),
                    ),
                    replacement: format!("{separator}{name}: {value}"),
                }]),
            });
        }
        for hint in &diagnostic.repair_hints {
            let Some(range) = hint.source_range else {
                continue;
            };
            let replacement = match &hint.replacement {
                crate::RepairReplacement::Text(value)
                | crate::RepairReplacement::Locator { source: value } => value,
            };
            if range.start > range.end
                || !source.is_char_boundary(range.start as usize)
                || !source.is_char_boundary(range.end as usize)
            {
                continue;
            }
            if items.len() == MAX_QUERY_ITEMS {
                return QueryResult {
                    items,
                    truncated: true,
                };
            }
            items.push(CodeAction {
                title: format!("Replace with `{replacement}`"),
                diagnostic_code: Some(diagnostic.code.into()),
                edit: WorkspaceEdit {
                    documents: vec![DocumentEdit {
                        path: file.path(db).clone(),
                        file: file.file(db),
                        revision: webtest_text::SourceRevision::of(source),
                        version: file.version(db),
                        edits: vec![TextEdit {
                            origin: webtest_text::SyntaxOrigin::new(
                                file.file(db),
                                webtest_text::TextRange::new(
                                    TextSize::from(range.start),
                                    TextSize::from(range.end),
                                ),
                            ),
                            replacement: replacement.clone(),
                        }],
                    }],
                },
            });
        }
    }
    let formatted = webtest_format::format_file(&parsed);
    if formatted != source.as_ref() {
        items.push(CodeAction {
            title: "Format document".into(),
            diagnostic_code: None,
            edit: edit(vec![TextEdit {
                origin: webtest_text::SyntaxOrigin::new(
                    file.file(db),
                    webtest_text::TextRange::new(0.into(), (source.len() as u32).into()),
                ),
                replacement: formatted,
            }]),
        });
    }
    let truncated = items.len() > MAX_QUERY_ITEMS;
    items.truncate(MAX_QUERY_ITEMS);
    QueryResult { items, truncated }
}
fn placeholder(ty: &webtest_model::Type, depth: usize) -> Option<String> {
    use webtest_model::Type as T;
    if depth > 16 {
        return None;
    }
    let value = match ty {
        T::String | T::Url => "\"\"".into(),
        T::Int | T::StatusCode => "0".into(),
        T::Float => "0.0".into(),
        T::Bool => "false".into(),
        T::Null | T::Json | T::Option(_) => "null".into(),
        T::Duration => "1s".into(),
        T::List(_) => "[]".into(),
        T::Record(fields) => {
            let mut values = Vec::new();
            for (name, field) in fields.iter().filter(|(_, field)| !field.optional) {
                if !webtest_syntax::is_valid_binding_identifier(name) {
                    return None;
                }
                values.push(format!("{name}: {}", placeholder(&field.ty, depth + 1)?));
            }
            format!("{{ {} }}", values.join(", "))
        }
        _ => return None,
    };
    (value.len() <= 4096).then_some(value)
}

pub(crate) fn workspace_symbols(db: &dyn Db, query: &str, limit: usize) -> QueryResult<Symbol> {
    let limit = limit.min(MAX_QUERY_ITEMS);
    let query = query.to_lowercase();
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for file in inputs(db) {
        db.unwind_if_revision_cancelled();
        for symbol in document_symbols(db, file) {
            if symbol.kind == SymbolKind::Module
                || !symbol.name.to_lowercase().contains(&query)
                || !seen.insert(symbol.id)
            {
                continue;
            }
            if items.len() == limit {
                return QueryResult {
                    items,
                    truncated: true,
                };
            }
            items.push(symbol);
        }
    }
    QueryResult {
        items,
        truncated: false,
    }
}
pub(crate) fn definition(db: &dyn Db, file: SourceInput, offset: TextSize) -> Option<LocationLink> {
    let index = semantic_index::symbols(db, file);
    let reference = at(&index, offset)?;
    let target = index.symbols.get(&reference.resolved_symbol)?;
    Some(LocationLink {
        origin: reference.location.origin,
        target: target.location.clone()?,
        target_range: target
            .full_origin
            .or_else(|| target.location.as_ref().map(|l| l.origin))?,
    })
}
fn connected(indexes: &[Arc<SymbolIndex>], symbol: SymbolId) -> BTreeSet<SymbolId> {
    let mut group = BTreeSet::from([symbol]);
    loop {
        let mut changed = false;
        for index in indexes {
            for (left, right) in &index.field_constraints {
                if group.contains(left) || group.contains(right) {
                    changed |= group.insert(*left);
                    changed |= group.insert(*right);
                }
            }
        }
        if !changed {
            return group;
        }
    }
}
pub(crate) fn references(
    db: &dyn Db,
    file: SourceInput,
    offset: TextSize,
    include_declaration: bool,
    limit: usize,
) -> QueryResult<Reference> {
    let index = semantic_index::symbols(db, file);
    let Some(reference) = at(&index, offset) else {
        return QueryResult {
            items: Vec::new(),
            truncated: false,
        };
    };
    let indexes = indexes(db);
    let group = connected(&indexes, reference.resolved_symbol);
    let mut items = Vec::new();
    for index in indexes {
        db.unwind_if_revision_cancelled();
        for reference in &index.references {
            if group.contains(&reference.resolved_symbol)
                && (include_declaration || !reference.declaration)
            {
                if items.len() == limit.min(MAX_QUERY_ITEMS) {
                    return QueryResult {
                        items,
                        truncated: true,
                    };
                }
                items.push(reference.clone());
            }
        }
    }
    QueryResult {
        items,
        truncated: false,
    }
}
pub(crate) fn rename(
    db: &dyn Db,
    file: SourceInput,
    offset: TextSize,
    name: &str,
) -> Result<WorkspaceEdit, RenameError> {
    if !webtest_syntax::is_valid_identifier(name) {
        return Err(RenameError::InvalidName);
    }
    let index = semantic_index::symbols(db, file);
    let reference = at(&index, offset).ok_or(RenameError::NoSymbol)?;
    let symbol = index
        .symbols
        .get(&reference.symbol)
        .ok_or(RenameError::NoSymbol)?;
    if symbol.kind == SymbolKind::Binding && !webtest_syntax::is_valid_binding_identifier(name) {
        return Err(RenameError::InvalidName);
    }
    if symbol.read_only {
        return Err(RenameError::ReadOnly);
    }
    let indexes = indexes(db);
    let group = connected(&indexes, symbol.id);
    for index in &indexes {
        for id in &group {
            if index.symbols.get(id).is_some_and(|symbol| symbol.read_only) {
                return Err(RenameError::ReadOnly);
            }
        }
        for reference in index
            .references
            .iter()
            .filter(|r| group.contains(&r.symbol) && r.scope_sensitive)
        {
            let mut scope = Some(reference.scope);
            while let Some(id) = scope {
                let Some(current) = index.scopes.get(&id) else {
                    break;
                };
                if current
                    .names
                    .get(name)
                    .is_some_and(|id| !group.contains(id))
                {
                    return Err(RenameError::Collision(name.into()));
                }
                scope = current.parent;
            }
        }
        // A declaration moved into an existing reference's scope must not capture it.
        for scope in index.scopes.values() {
            if scope.names.values().any(|id| group.contains(id))
                && scope.names.get(name).is_some_and(|id| !group.contains(id))
            {
                return Err(RenameError::Collision(name.into()));
            }
        }
    }
    let mut documents = BTreeMap::<String, DocumentEdit>::new();
    let mut count = 0;
    for index in indexes {
        db.unwind_if_revision_cancelled();
        for reference in &index.references {
            if !group.contains(&reference.symbol) {
                continue;
            }
            count += 1;
            if count > MAX_QUERY_ITEMS {
                return Err(RenameError::Limit);
            }
            let location = &reference.location;
            let document = documents
                .entry(location.path.clone())
                .or_insert_with(|| DocumentEdit {
                    path: location.path.clone(),
                    file: location.origin.file,
                    revision: location.revision,
                    version: None,
                    edits: Vec::new(),
                });
            document.edits.push(TextEdit {
                origin: location.origin,
                replacement: if reference.quoted {
                    serde_json::to_string(name).map_err(|_| RenameError::InvalidName)?
                } else {
                    name.into()
                },
            });
        }
    }
    for document in documents.values_mut() {
        document
            .edits
            .sort_by_key(|edit| std::cmp::Reverse(edit.origin.range.start()));
        document.edits.dedup();
    }
    Ok(WorkspaceEdit {
        documents: documents.into_values().collect(),
    })
}

pub(crate) fn folding(db: &dyn Db, file: SourceInput) -> Vec<FoldingRange> {
    use webtest_syntax::SyntaxKind as K;
    crate::incremental::parsed(db, file)
        .syntax()
        .descendants()
        .filter(|node| {
            matches!(
                node.kind(),
                K::TestDecl
                    | K::FunctionDecl
                    | K::FixtureDecl
                    | K::FixtureSetup
                    | K::FixtureTeardown
                    | K::ServerBlock
                    | K::BrowserBlock
                    | K::TimeoutStmt
                    | K::ParallelStmt
                    | K::RaceStmt
                    | K::RetryStmt
                    | K::RecordExpr
                    | K::ListExpr
            )
        })
        .filter_map(|node| {
            let text = node.text().to_string();
            text.contains('\n').then(|| FoldingRange {
                range: byte_range(node.text_range()),
            })
        })
        .collect()
}
pub(crate) fn selection(
    db: &dyn Db,
    file: SourceInput,
    offset: TextSize,
) -> Option<SelectionRange> {
    let syntax = crate::incremental::parsed(db, file).syntax();
    let token = syntax
        .token_at_offset(offset)
        .right_biased()
        .or_else(|| syntax.token_at_offset(offset).left_biased())?;
    let mut ranges = vec![token.text_range()];
    ranges.extend(token.parent_ancestors().map(|node| node.text_range()));
    ranges.dedup();
    let mut result = None;
    for range in ranges.into_iter().rev() {
        result = Some(SelectionRange {
            range: byte_range(range),
            parent: result.map(Box::new),
        });
    }
    result
}

fn byte_range(range: webtest_text::TextRange) -> webtest_feedback::ByteRange {
    webtest_feedback::ByteRange {
        start: range.start().into(),
        end: range.end().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalysisDatabase, WorkspaceConfiguration};
    use webtest_text::DocumentVersion;
    fn offset(source: &str, needle: &str) -> TextSize {
        TextSize::from(source.find(needle).unwrap() as u32)
    }
    #[test]
    fn imported_alias_navigation_and_versioned_rename_have_distinct_semantics() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(WorkspaceConfiguration {
            root: "/project".into(),
            ..Default::default()
        });
        let library = "export fn identité(value: String) -> String = value";
        let caller = "import { identité as alias } from \"./library\"\ntest \"x\" { let result = alias(value: \"é\") }";
        let lib = db.open_file("/project/library.webtest", library);
        let file = db.open_file("/project/test.webtest", caller);
        db.set_document_version(file, DocumentVersion(42)).unwrap();
        let call = offset(caller, "alias(value");
        let definition = db.definition(file, call).unwrap().unwrap();
        assert_eq!(definition.target.origin.file, lib);
        assert_eq!(
            &library[usize::from(definition.target.origin.range.start())
                ..usize::from(definition.target.origin.range.end())],
            "identité"
        );
        let edit = db.rename(file, call, "renamed").unwrap();
        assert_eq!(edit.documents.len(), 1);
        assert_eq!(edit.documents[0].version, Some(DocumentVersion(42)));
        assert_eq!(edit.documents[0].edits.len(), 2);
        let edit = db
            .rename(lib, offset(library, "identité"), "identity")
            .unwrap();
        assert_eq!(edit.documents.len(), 2);
        assert_eq!(
            edit.documents.iter().map(|d| d.edits.len()).sum::<usize>(),
            2
        );
        assert_eq!(
            db.references(lib, offset(library, "identité"), true, 100)
                .unwrap()
                .items
                .len(),
            4
        );
        assert!(matches!(
            db.rename(file, call, "test"),
            Err(RenameError::InvalidName)
        ));
        assert!(db.workspace_symbols("ident", 0).truncated);
    }
    #[test]
    fn local_rename_rejects_shadowing_and_does_not_touch_unrelated_records() {
        let mut db = AnalysisDatabase::default();
        let source = "test \"x\" { let value = { email: \"a\" } let other = { email: \"b\" } let taken = 1 expect value.email == other.email }";
        let file = db.open_file("test.webtest", source);
        assert!(matches!(
            db.rename(file, offset(source, "value ="), "taken"),
            Err(RenameError::Collision(_))
        ));
        let edit = db
            .rename(
                file,
                offset(source, "value.email") + TextSize::from(6),
                "address",
            )
            .unwrap();
        assert_eq!(edit.documents[0].edits.len(), 2);
        for edit in &edit.documents[0].edits {
            assert_eq!(
                &source
                    [usize::from(edit.origin.range.start())..usize::from(edit.origin.range.end())],
                "email"
            );
        }
    }
    #[test]
    fn selection_nests_lossless_unicode_ranges_and_folding_uses_cst_nodes() {
        let db = &mut AnalysisDatabase::default();
        let source =
            "test \"é😀\" {\n  let üser = { email: \"é\" }\n  expect üser.email == \"é\"\n}";
        let file = db.open_file("unicode.webtest", source);
        assert!(db.parse(file).unwrap().errors().is_empty());
        let mut selection = db
            .selection_range(file, offset(source, "üser"))
            .unwrap()
            .unwrap();
        assert_eq!(
            &source[selection.range.start as usize..selection.range.end as usize],
            "üser"
        );
        while let Some(parent) = selection.parent {
            assert!(
                parent.range.start <= selection.range.start
                    && parent.range.end >= selection.range.end
            );
            selection = *parent;
        }
        assert_eq!(selection.range.end as usize, source.len());
        assert!(!db.folding_ranges(file).unwrap().is_empty());
    }
    #[test]
    fn inferred_hints_and_repairs_use_compiler_types_and_versioned_edits() {
        let mut db = AnalysisDatabase::default();
        let source = "test \"hints\" { let üser = { email: \"a\" } let fixed: Int = 1 expect üser.emali == \"a\" }";
        let file = db.open_file("hints.webtest", source);
        db.set_document_version(file, webtest_text::DocumentVersion(7))
            .unwrap();
        let hints = db.inlay_hints(file).unwrap();
        assert_eq!(hints.items.len(), 1);
        assert!(hints.items[0].label.contains("email: String"));
        assert_eq!(
            hints.items[0].offset as usize,
            source.find("üser").unwrap() + "üser".len()
        );
        let actions = db
            .code_actions(
                file,
                webtest_text::TextRange::new(0.into(), (source.len() as u32).into()),
            )
            .unwrap();
        let action = actions
            .items
            .iter()
            .find(|action| action.title.contains("email"))
            .unwrap();
        let document = &action.edit.documents[0];
        assert_eq!(document.version, Some(webtest_text::DocumentVersion(7)));
        assert_eq!(document.revision, webtest_text::SourceRevision::of(source));
        let edit = &document.edits[0];
        assert_eq!(
            &source[usize::from(edit.origin.range.start())..usize::from(edit.origin.range.end())],
            "emali"
        );
        assert_eq!(edit.replacement, "email");
    }

    #[test]
    fn native_helper_results_cannot_escape_an_inner_cleanup_scope() {
        for (body, rejected) in [
            ("let directory = fs.temp_dir() provide directory", false),
            (
                "timeout 1s { let directory = fs.temp_dir() provide directory }",
                true,
            ),
        ] {
            let mut db = AnalysisDatabase::default();
            let source = format!(
                "fn directory() -> TempDirectory server {{ {body} }} test \"x\" {{ server {{ let result = directory() }} }}"
            );
            let file = db.open_file("resources.webtest", source);
            let diagnostics = db.diagnostics(file).unwrap();
            assert_eq!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "semantic.non_transferable_scope_result"),
                rejected,
                "{diagnostics:?}"
            );
            if !rejected {
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
            }
        }
    }
}

#[cfg(test)]
mod action_tests {
    use crate::{AnalysisDatabase, WorkspaceConfiguration};
    use webtest_text::{DocumentVersion, TextRange, TextSize};
    fn apply(source: &str, edits: &[crate::TextEdit]) -> String {
        let mut source = source.to_owned();
        let mut edits = edits.to_vec();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.origin.range.start()));
        for edit in edits {
            source.replace_range(
                usize::from(edit.origin.range.start())..usize::from(edit.origin.range.end()),
                &edit.replacement,
            );
        }
        source
    }
    #[test]
    fn import_missing_argument_qualification_and_format_actions_are_versioned_and_idempotent() {
        let mut db = AnalysisDatabase::default();
        db.configure_workspace(WorkspaceConfiguration {
            root: "/project".into(),
            ..Default::default()
        });
        db.open_file(
            "/project/support.webtest",
            "export fn user_name(value: String) -> String = lower(value)",
        );
        let file = db.open_file(
            "/project/test.webtest",
            "test \"x\" { let value = user_name(\"É\") }",
        );
        for (source, title) in [
            (
                "test \"x\" { let value = user_name(\"É\") }",
                "Import `user_name`",
            ),
            (
                "test \"x\" { server { fs.write_text(path: \"a\") } }",
                "Add required argument `contents`",
            ),
            (
                "test \"x\" { server { read_text(path: \"a\") } }",
                "Qualify as `fs.read_text`",
            ),
            ("test \"x\"{let value=7}", "Format document"),
        ] {
            db.set_file_text(file, source);
            db.set_document_version(file, DocumentVersion(9)).unwrap();
            let actions = db
                .code_actions(
                    file,
                    TextRange::new(0.into(), TextSize::from(source.len() as u32)),
                )
                .unwrap();
            let action = actions
                .items
                .iter()
                .find(|action| action.title == title)
                .unwrap_or_else(|| panic!("missing {title}: {:?}", actions.items));
            assert_eq!(action.edit.documents[0].version, Some(DocumentVersion(9)));
            let changed = apply(source, &action.edit.documents[0].edits);
            db.set_file_text(file, changed.clone());
            assert!(
                !db.code_actions(
                    file,
                    TextRange::new(0.into(), (changed.len() as u32).into())
                )
                .unwrap()
                .items
                .iter()
                .any(|action| action.title == title)
            );
            assert!(
                db.diagnostics(file).unwrap().is_empty(),
                "{changed}: {:?}",
                db.diagnostics(file)
            );
        }
    }
}
