//! Incremental workspace, static semantics, and deterministic plan construction.

mod description;

pub use description::{
    Availability, CategoryDescription, ConstraintDescription, ConstructDescription,
    DescriptionDiagnostic, DescriptionIndex, DescriptionLimits, DescriptionProject,
    DescriptionRequest, DescriptionResponse, GuidanceDescription, LanguageDescription,
    ParameterDescription, Provenance, SearchDescription, SearchResult, SourceExample,
    SyntaxElement, SyntaxForm, describe,
};

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use thiserror::Error;
pub use webtest_feedback::{RepairHint, RepairHintKind, RepairReplacement};
use webtest_hir::{
    BinaryOperator, BindingId, HirBrowserOp, HirExpr, HirExprKind, HirFile, HirLiteral, HirNameRef,
    HirStmt, HirType, HirTypeKind, StepId, UnaryOperator,
};
use webtest_plan::{
    AssertionOperation, BrowserOperation, EvaluatePureOperation, PlanExpr, PlannedStep,
    PlannedTest, ServerProviderCall, TestOperation, TestPlan, ValueMatcher, locator_from_hir,
    locator_state_from_hir,
};
use webtest_provider::{Capability, OperationSchema, ProviderRegistry, RecordField, Type, Value};
use webtest_syntax::Parse;
use webtest_text::{FileId, SourceRevision, SyntaxOrigin, TextRange, TextSize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSource {
    Syntax,
    Semantic,
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub code: &'static str,
    pub message: String,
    pub source: DiagnosticSource,
    pub semantic_details: Option<serde_json::Value>,
    pub repair_hints: Vec<RepairHint>,
    pub reference_queries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeFact {
    pub range: TextRange,
    pub ty: Type,
    pub capability: Capability,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    Function,
    Parameter,
    Property,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Completion {
    pub label: String,
    pub detail: String,
    pub documentation: String,
    pub kind: CompletionKind,
    pub insert_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SignatureParameter {
    pub label: String,
    pub documentation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Signature {
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<SignatureParameter>,
    pub active_parameter: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentationFact {
    pub range: TextRange,
    pub contents: String,
}

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("unknown source file {0:?}")]
    UnknownFile(FileId),
}

#[derive(Clone, Debug)]
struct SourceFile {
    path: String,
    text: Arc<str>,
    revision: SourceRevision,
}

#[derive(Clone, Debug)]
struct CachedQueries {
    revision: SourceRevision,
    parse: Parse,
    hir: Arc<HirFile>,
    diagnostics: Arc<Vec<Diagnostic>>,
    type_facts: Arc<Vec<TypeFact>>,
    plan: Arc<TestPlan>,
}

pub struct AnalysisDatabase {
    files: HashMap<FileId, SourceFile>,
    paths: HashMap<String, FileId>,
    cache: HashMap<FileId, CachedQueries>,
    next_file: u32,
    providers: ProviderRegistry,
}

impl Default for AnalysisDatabase {
    fn default() -> Self {
        Self {
            files: HashMap::new(),
            paths: HashMap::new(),
            cache: HashMap::new(),
            next_file: 0,
            providers: ProviderRegistry::built_in_schemas(),
        }
    }
}

impl AnalysisDatabase {
    pub fn with_provider_registry(providers: ProviderRegistry) -> Self {
        Self {
            providers,
            ..Self::default()
        }
    }

    pub fn open_file(&mut self, path: impl Into<String>, text: impl Into<String>) -> FileId {
        let path = path.into();
        if let Some(file) = self.paths.get(&path).copied() {
            self.set_file_text(file, text);
            return file;
        }
        let file = FileId::new(self.next_file);
        self.next_file += 1;
        let text: Arc<str> = Arc::from(text.into());
        self.files.insert(
            file,
            SourceFile {
                path: path.clone(),
                revision: SourceRevision::of(&text),
                text,
            },
        );
        self.paths.insert(path, file);
        file
    }

    pub fn set_file_text(&mut self, file: FileId, text: impl Into<String>) {
        let text: Arc<str> = Arc::from(text.into());
        if let Some(source) = self.files.get_mut(&file) {
            let revision = SourceRevision::of(&text);
            if source.revision != revision {
                source.text = text;
                source.revision = revision;
                self.cache.remove(&file);
            }
        }
    }

    pub fn file_for_path(&self, path: &str) -> Option<FileId> {
        self.paths.get(path).copied()
    }

    pub fn close_file(&mut self, file: FileId) {
        if let Some(source) = self.files.remove(&file) {
            self.paths.remove(&source.path);
        }
        self.cache.remove(&file);
    }

    pub fn source(&self, file: FileId) -> Result<Arc<str>, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| Arc::clone(&source.text))
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn path(&self, file: FileId) -> Result<&str, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| source.path.as_str())
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn source_revision(&self, file: FileId) -> Result<SourceRevision, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| source.revision)
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn parse(&mut self, file: FileId) -> Result<Parse, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(self.cache[&file].parse.clone())
    }

    pub fn hir_file(&mut self, file: FileId) -> Result<Arc<HirFile>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].hir))
    }

    pub fn diagnostics(&mut self, file: FileId) -> Result<Arc<Vec<Diagnostic>>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].diagnostics))
    }

    pub fn type_facts(&mut self, file: FileId) -> Result<Arc<Vec<TypeFact>>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].type_facts))
    }

    pub fn type_at(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<TypeFact>, AnalysisError> {
        Ok(self
            .type_facts(file)?
            .iter()
            .filter(|fact| fact.range.contains(offset) || fact.range.end() == offset)
            .min_by_key(|fact| fact.range.len())
            .cloned())
    }

    pub fn test_plan(&mut self, file: FileId) -> Result<Arc<TestPlan>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].plan))
    }

    pub fn provider_schema_hashes(&self) -> BTreeMap<String, String> {
        self.providers
            .schemas()
            .map(|schema| (schema.name.0.clone(), schema.hash()))
            .collect()
    }

    pub fn set_provider_registry(&mut self, providers: ProviderRegistry) {
        let current = self.provider_schema_hashes();
        let incoming = providers
            .schemas()
            .map(|schema| (schema.name.0.clone(), schema.hash()))
            .collect::<BTreeMap<_, _>>();
        self.providers = providers;
        if current != incoming {
            self.cache.clear();
        }
    }

    pub fn completions(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Vec<Completion>, AnalysisError> {
        self.ensure_queries(file)?;
        let parsed = self.cache[&file].parse.clone();
        let syntax = parsed.syntax();
        let offset_u32 = u32::from(offset);
        let containing = |node: &webtest_syntax::SyntaxNode| {
            let range = node.text_range();
            u32::from(range.start()) <= offset_u32 && u32::from(range.end()) >= offset_u32
        };
        if let Some(call) = syntax
            .descendants()
            .filter(|node| node.kind() == webtest_syntax::SyntaxKind::CallExpr && containing(node))
            .min_by_key(|node| node.text_range().len())
            && let Some((provider, operation)) = provider_operation_tokens(&call)
            && let Some(operation) = self
                .providers
                .schema(&provider)
                .and_then(|schema| schema.operation(&operation))
        {
            let present = call
                .children()
                .filter(|node| node.kind() == webtest_syntax::SyntaxKind::CallArg)
                .filter_map(|argument| named_argument_token(&argument))
                .collect::<HashSet<_>>();
            return Ok(operation
                .parameters
                .iter()
                .filter(|parameter| !parameter.positional && !present.contains(&parameter.name))
                .map(|parameter| Completion {
                    label: parameter.name.clone(),
                    detail: format!(
                        "{}{}",
                        parameter.ty,
                        if parameter.required {
                            ""
                        } else {
                            " (optional)"
                        }
                    ),
                    documentation: parameter.documentation.clone(),
                    kind: CompletionKind::Parameter,
                    insert_text: format!("{}: ", parameter.name),
                })
                .collect());
        }
        if let Some(member) = syntax
            .descendants()
            .filter(|node| {
                node.kind() == webtest_syntax::SyntaxKind::MemberExpr && containing(node)
            })
            .min_by_key(|node| node.text_range().len())
        {
            let tokens = meaningful_tokens(&member);
            if let Some(provider) = tokens.first().map(|token| token.text().to_string())
                && tokens
                    .iter()
                    .any(|token| token.kind() == webtest_syntax::SyntaxKind::Dot)
                && let Some(schema) = self.providers.schema(&provider)
            {
                return Ok(schema
                    .operations
                    .values()
                    .map(|operation| Completion {
                        label: operation.name.0.clone(),
                        detail: provider_signature(&provider, operation),
                        documentation: operation.documentation.clone(),
                        kind: CompletionKind::Function,
                        insert_text: format!("{}(", operation.name.0),
                    })
                    .collect());
            }
            if let Some(receiver) = member.children().next() {
                let facts = self.type_facts(file)?;
                let receiver_fact = facts
                    .iter()
                    .filter(|fact| {
                        fact.range.contains(receiver.text_range().start())
                            || fact.range == receiver.text_range()
                    })
                    .min_by_key(|fact| fact.range.len())
                    .or_else(|| {
                        // A half-typed `value.` is deliberately absent from HIR because the
                        // member token is missing. Recover the receiver's declaration fact from
                        // the lossless CST so completion remains useful while editing.
                        let receiver_name = meaningful_tokens(&receiver)
                            .first()
                            .map(|token| token.text().to_string())?;
                        syntax
                            .descendants()
                            .filter(|node| {
                                node.kind() == webtest_syntax::SyntaxKind::LetStmt
                                    && node.text_range().end() <= member.text_range().start()
                            })
                            .filter_map(|declaration| {
                                let tokens = meaningful_tokens(&declaration);
                                let name = tokens.get(1)?;
                                (name.text() == receiver_name).then_some(name.text_range())
                            })
                            .last()
                            .and_then(|range| facts.iter().find(|fact| fact.range == range))
                    });
                let Some(fact) = receiver_fact else {
                    return Ok(Vec::new());
                };
                let Type::Record(fields) = &fact.ty else {
                    return Ok(Vec::new());
                };
                return Ok(fields
                    .iter()
                    .map(|(name, field)| Completion {
                        label: name.clone(),
                        detail: field.ty.to_string(),
                        documentation: field.documentation.clone(),
                        kind: CompletionKind::Property,
                        insert_text: name.clone(),
                    })
                    .collect());
            }
        }
        Ok(Vec::new())
    }

    pub fn signature_help(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<Signature>, AnalysisError> {
        self.ensure_queries(file)?;
        let syntax = self.cache[&file].parse.syntax();
        let offset_u32 = u32::from(offset);
        let Some(call) = syntax
            .descendants()
            .filter(|node| {
                node.kind() == webtest_syntax::SyntaxKind::CallExpr
                    && u32::from(node.text_range().start()) <= offset_u32
                    && u32::from(node.text_range().end()) >= offset_u32
            })
            .min_by_key(|node| node.text_range().len())
        else {
            return Ok(None);
        };
        let Some((provider, operation_name)) = provider_operation_tokens(&call) else {
            return Ok(None);
        };
        let Some(operation) = self
            .providers
            .schema(&provider)
            .and_then(|schema| schema.operation(&operation_name))
        else {
            return Ok(None);
        };
        let active_parameter = call
            .children()
            .filter(|node| node.kind() == webtest_syntax::SyntaxKind::CallArg)
            .take_while(|node| u32::from(node.text_range().start()) <= offset_u32)
            .count()
            .saturating_sub(1)
            .min(operation.parameters.len().saturating_sub(1));
        Ok(Some(Signature {
            label: provider_signature(&provider, operation),
            documentation: operation.documentation.clone(),
            parameters: operation
                .parameters
                .iter()
                .map(|parameter| SignatureParameter {
                    label: format!("{}: {}", parameter.name, parameter.ty),
                    documentation: parameter.documentation.clone(),
                })
                .collect(),
            active_parameter,
        }))
    }

    pub fn documentation_at(
        &mut self,
        file: FileId,
        offset: TextSize,
    ) -> Result<Option<DocumentationFact>, AnalysisError> {
        self.ensure_queries(file)?;
        let syntax = self.cache[&file].parse.syntax();
        let offset_u32 = u32::from(offset);
        for call in syntax.descendants().filter(|node| {
            node.kind() == webtest_syntax::SyntaxKind::CallExpr
                && u32::from(node.text_range().start()) <= offset_u32
                && u32::from(node.text_range().end()) >= offset_u32
        }) {
            if let Some((provider, operation_name)) = provider_operation_tokens(&call)
                && let Some(operation) = self
                    .providers
                    .schema(&provider)
                    .and_then(|schema| schema.operation(&operation_name))
            {
                return Ok(Some(DocumentationFact {
                    range: call
                        .children()
                        .next()
                        .map_or(call.text_range(), |node| node.text_range()),
                    contents: format!(
                        "{}\n\nReturns `{}`. Retry-safe: {}.",
                        operation.documentation, operation.result, operation.retry_safe
                    ),
                }));
            }
        }
        Ok(None)
    }

    pub fn describe(
        &self,
        request: DescriptionRequest,
        project: Option<DescriptionProject>,
        limits: DescriptionLimits,
    ) -> DescriptionResponse {
        description::describe(&self.providers, request, project, limits)
    }

    fn ensure_queries(&mut self, file: FileId) -> Result<(), AnalysisError> {
        let source = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?;
        if self
            .cache
            .get(&file)
            .is_some_and(|cached| cached.revision == source.revision)
        {
            return Ok(());
        }

        let parsed = webtest_syntax::parse(&source.text);
        let mut diagnostics = parsed
            .errors()
            .iter()
            .map(|error| Diagnostic {
                range: error.range,
                severity: DiagnosticSeverity::Error,
                code: error.code,
                message: error.message.clone(),
                source: DiagnosticSource::Syntax,
                semantic_details: None,
                repair_hints: Vec::new(),
                reference_queries: syntax_reference_queries(&parsed, error),
            })
            .collect::<Vec<_>>();
        diagnostics.extend(invalid_duration_diagnostics(&parsed));
        let hir = Arc::new(webtest_hir::lower(file, &parsed));
        let compiled =
            Compiler::new(file, source.revision, &self.providers, diagnostics).compile(&hir);
        self.cache.insert(
            file,
            CachedQueries {
                revision: source.revision,
                parse: parsed,
                hir,
                diagnostics: Arc::new(compiled.diagnostics),
                type_facts: Arc::new(compiled.type_facts),
                plan: Arc::new(compiled.plan),
            },
        );
        Ok(())
    }
}

fn meaningful_tokens(node: &webtest_syntax::SyntaxNode) -> Vec<webtest_syntax::SyntaxToken> {
    node.descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| !token.kind().is_trivia())
        .collect()
}

fn provider_operation_tokens(node: &webtest_syntax::SyntaxNode) -> Option<(String, String)> {
    let tokens = meaningful_tokens(node);
    let dot = tokens
        .iter()
        .position(|token| token.kind() == webtest_syntax::SyntaxKind::Dot)?;
    Some((
        tokens.get(dot.checked_sub(1)?)?.text().into(),
        tokens.get(dot + 1)?.text().into(),
    ))
}

fn named_argument_token(node: &webtest_syntax::SyntaxNode) -> Option<String> {
    let tokens = meaningful_tokens(node);
    tokens
        .iter()
        .any(|token| token.kind() == webtest_syntax::SyntaxKind::Colon)
        .then(|| tokens.first().map(|token| token.text().into()))?
}

fn provider_signature(provider: &str, operation: &OperationSchema) -> String {
    let parameters = operation
        .parameters
        .iter()
        .map(|parameter| {
            format!(
                "{}{}: {}",
                parameter.name,
                if parameter.required { "" } else { "?" },
                parameter.ty
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{provider}.{}({parameters}) -> {}",
        operation.name.0, operation.result
    )
}

fn syntax_reference_queries(parsed: &Parse, error: &webtest_syntax::SyntaxError) -> Vec<String> {
    let mut queries = vec!["grammar".into()];
    match error.code {
        "syntax.expected_server_statement" => queries.push("scope.server".into()),
        "syntax.expected_browser_statement" => queries.push("scope.browser".into()),
        _ => return queries,
    }
    let offending = parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.text_range() == error.range)
        .map(|token| token.text().to_owned());
    let reference = match offending.as_deref() {
        Some("open") => Some("browser.open"),
        Some("evaluate") => Some("browser.evaluate"),
        Some("click") => Some("browser.click"),
        Some("fill") => Some("browser.fill"),
        Some("type") => Some("browser.type"),
        Some("press") => Some("browser.press"),
        Some("check") => Some("browser.check"),
        Some("uncheck") => Some("browser.uncheck"),
        Some("select") => Some("browser.select"),
        Some("hover") => Some("browser.hover"),
        Some("wait") => Some("browser.wait.locator"),
        _ => None,
    };
    if let Some(reference) = reference {
        queries.push(reference.into());
    }
    queries
}

fn invalid_duration_diagnostics(parsed: &Parse) -> Vec<Diagnostic> {
    parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| {
            let token = element.into_token()?;
            if token.kind() != webtest_syntax::SyntaxKind::Duration {
                return None;
            }
            let valid = token
                .text()
                .strip_suffix("ms")
                .or_else(|| token.text().strip_suffix('s'))
                .or_else(|| token.text().strip_suffix('m'))
                .and_then(|number| number.parse::<u64>().ok())
                .is_some_and(|number| number > 0);
            (!valid).then(|| Diagnostic {
                range: token.text_range(),
                severity: DiagnosticSeverity::Error,
                code: "semantic.invalid_duration",
                message: format!("invalid positive duration `{}`", token.text()),
                source: DiagnosticSource::Semantic,
                semantic_details: Some(serde_json::json!({ "literal": token.text() })),
                repair_hints: Vec::new(),
                reference_queries: vec!["grammar".into(), "type.Duration".into()],
            })
        })
        .collect()
}

struct CompileResult {
    diagnostics: Vec<Diagnostic>,
    type_facts: Vec<TypeFact>,
    plan: TestPlan,
}

#[derive(Clone)]
struct BindingState {
    name: String,
    ty: Type,
    domain: Capability,
    provider_operation: Option<String>,
}

struct TypedExpr {
    expression: PlanExpr,
    ty: Type,
    capability: Capability,
}

struct CompiledProviderCall {
    provider: String,
    operation: String,
    arguments: BTreeMap<String, PlanExpr>,
    result_type: Type,
    schema_hash: String,
    redacted_arguments: Vec<String>,
    redacted_result_fields: Vec<String>,
    retry_safe: bool,
}

struct Compiler<'a> {
    file: FileId,
    revision: SourceRevision,
    providers: &'a ProviderRegistry,
    diagnostics: Vec<Diagnostic>,
    type_facts: Vec<TypeFact>,
    bindings: HashMap<BindingId, BindingState>,
    names: HashMap<String, SyntaxOrigin>,
    declared_names: HashSet<String>,
    required: BTreeSet<Capability>,
    next_step: u32,
}

impl<'a> Compiler<'a> {
    fn new(
        file: FileId,
        revision: SourceRevision,
        providers: &'a ProviderRegistry,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        Self {
            file,
            revision,
            providers,
            diagnostics,
            type_facts: Vec::new(),
            bindings: HashMap::new(),
            names: HashMap::new(),
            declared_names: HashSet::new(),
            required: BTreeSet::new(),
            next_step: 0,
        }
    }

    fn compile(mut self, hir: &HirFile) -> CompileResult {
        let tests = hir
            .tests
            .iter()
            .map(|test| {
                self.bindings.clear();
                self.names.clear();
                self.declared_names.clear();
                for statement in &test.body {
                    collect_binding_names(statement, &mut self.declared_names);
                }
                let mut steps = Vec::new();
                for statement in &test.body {
                    self.compile_statement(statement, Capability::Pure, &mut steps);
                }
                PlannedTest {
                    id: test.id,
                    name: test.name.clone(),
                    steps,
                    origin: test.origin,
                }
            })
            .collect();
        CompileResult {
            diagnostics: self.diagnostics,
            type_facts: self.type_facts,
            plan: TestPlan {
                file: self.file,
                source_revision: self.revision,
                required_host_capabilities: self.required.into_iter().collect(),
                tests,
            },
        }
    }

    fn compile_statement(
        &mut self,
        statement: &HirStmt,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        match statement {
            HirStmt::Server(block) => {
                for statement in &block.statements {
                    self.compile_statement(statement, Capability::Server, steps);
                }
            }
            HirStmt::Browser(block) => {
                for statement in &block.statements {
                    self.compile_statement(statement, Capability::Browser, steps);
                }
            }
            HirStmt::Let(binding) => self.compile_let(binding, domain, steps),
            HirStmt::Expression(statement) => {
                if let Some(call) = self.provider_call(&statement.expression, domain) {
                    self.push_step(
                        steps,
                        statement.expression.origin,
                        TestOperation::ServerProviderCall(ServerProviderCall {
                            provider: call.provider,
                            operation: call.operation,
                            arguments: call.arguments,
                            result_binding: None,
                            result_name: None,
                            result_type: call.result_type,
                            schema_hash: call.schema_hash,
                            timeout: None,
                            redacted_arguments: call.redacted_arguments,
                            redacted_result_fields: call.redacted_result_fields,
                            retry_safe: call.retry_safe,
                        }),
                    );
                } else {
                    let value = self.infer_expr(&statement.expression, domain, None);
                    self.push_step(
                        steps,
                        statement.expression.origin,
                        TestOperation::EvaluatePure(EvaluatePureOperation {
                            expression: value.expression,
                            result_binding: None,
                            result_name: None,
                            result_type: value.ty,
                        }),
                    );
                }
            }
            HirStmt::Expect(expectation) => {
                self.compile_expectation(&expectation.expression, domain, steps)
            }
            HirStmt::BrowserOperation(operation) => {
                self.compile_browser_operation(operation, domain, steps)
            }
        }
    }

    fn compile_let(
        &mut self,
        binding: &webtest_hir::HirLet,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        if let Some(previous) = self.names.get(&binding.name) {
            self.error(
                binding.name_origin.range,
                "semantic.duplicate_binding",
                format!(
                    "binding `{}` is already declared at byte {}",
                    binding.name,
                    u32::from(previous.range.start())
                ),
            );
        } else {
            self.names.insert(binding.name.clone(), binding.name_origin);
        }

        if let Some(call) = self.provider_call(&binding.value, domain) {
            let mut result_type = call.result_type.clone();
            if let Some(annotation) = &binding.annotation {
                let expected = self.lower_type(annotation);
                if !expected.accepts(&result_type) {
                    self.type_mismatch(binding.value.origin.range, &expected, &result_type);
                }
                result_type = expected;
            }
            self.bindings.insert(
                binding.id,
                BindingState {
                    name: binding.name.clone(),
                    ty: result_type.clone(),
                    domain,
                    provider_operation: Some(format!("{}.{}", call.provider, call.operation)),
                },
            );
            self.type_fact(binding.name_origin.range, result_type.clone(), domain);
            self.push_step(
                steps,
                binding.value.origin,
                TestOperation::ServerProviderCall(ServerProviderCall {
                    provider: call.provider,
                    operation: call.operation,
                    arguments: call.arguments,
                    result_binding: Some(binding.id),
                    result_name: Some(binding.name.clone()),
                    result_type,
                    schema_hash: call.schema_hash,
                    timeout: None,
                    redacted_arguments: call.redacted_arguments,
                    redacted_result_fields: call.redacted_result_fields,
                    retry_safe: call.retry_safe,
                }),
            );
            return;
        }

        let annotation = binding
            .annotation
            .as_ref()
            .map(|annotation| self.lower_type(annotation));
        let mut value = self.infer_expr(&binding.value, domain, annotation.as_ref());
        if let Some(expected) = annotation {
            if value.ty == Type::Json && decodable_type(&expected) {
                let response_operation = self.response_operation(&binding.value);
                value.expression = PlanExpr::Decode {
                    value: Box::new(value.expression),
                    target: expected.clone(),
                    response_operation,
                };
                value.ty = expected;
            } else if !expected.accepts(&value.ty) {
                self.type_mismatch(binding.value.origin.range, &expected, &value.ty);
                value.ty = expected;
            } else {
                value.ty = expected;
            }
        }
        self.bindings.insert(
            binding.id,
            BindingState {
                name: binding.name.clone(),
                ty: value.ty.clone(),
                domain,
                provider_operation: None,
            },
        );
        self.type_fact(
            binding.name_origin.range,
            value.ty.clone(),
            value.capability,
        );
        self.push_step(
            steps,
            binding.value.origin,
            TestOperation::EvaluatePure(EvaluatePureOperation {
                expression: value.expression,
                result_binding: Some(binding.id),
                result_name: Some(binding.name.clone()),
                result_type: value.ty,
            }),
        );
    }

    fn compile_expectation(
        &mut self,
        expression: &HirExpr,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        let (matcher, actual, expected, value_type) = if let HirExprKind::Binary {
            operator,
            left,
            right,
        } = &expression.kind
        {
            if *operator == BinaryOperator::Matches {
                let actual = self.infer_expr(left, domain, None);
                let pattern = self.pattern_type(right);
                if actual.ty != Type::Json && !matches!(actual.ty, Type::Record(_)) {
                    self.error(
                        left.origin.range,
                        "semantic.invalid_matcher",
                        format!("`matches` requires Json or a record, got {}", actual.ty),
                    );
                }
                (
                    ValueMatcher::Matches,
                    actual.expression,
                    Some(PlanExpr::Type(pattern.clone())),
                    pattern,
                )
            } else {
                let left = self.infer_expr(left, domain, None);
                let right = self.infer_expr(right, domain, Some(&left.ty));
                self.validate_binary(*operator, &left.ty, &right.ty, expression.origin.range);
                (
                    matcher_for(*operator).unwrap_or(ValueMatcher::Truthy),
                    left.expression,
                    Some(right.expression),
                    left.ty,
                )
            }
        } else {
            let value = self.infer_expr(expression, domain, Some(&Type::Bool));
            if value.ty != Type::Bool && value.ty != Type::Unknown {
                self.type_mismatch(expression.origin.range, &Type::Bool, &value.ty);
            }
            (ValueMatcher::Truthy, value.expression, None, Type::Bool)
        };
        self.required.insert(Capability::Test);
        self.push_step(
            steps,
            expression.origin,
            TestOperation::Assertion(AssertionOperation::Value {
                matcher,
                actual,
                expected,
                value_type,
            }),
        );
    }

    fn response_operation(&self, expression: &HirExpr) -> Option<String> {
        let HirExprKind::Member {
            receiver, member, ..
        } = &expression.kind
        else {
            return None;
        };
        if member != "json" {
            return None;
        }
        let HirExprKind::Name(HirNameRef::Binding { id, .. }) = receiver.kind else {
            return None;
        };
        self.bindings
            .get(&id)
            .and_then(|binding| binding.provider_operation.clone())
    }

    fn compile_browser_operation(
        &mut self,
        operation: &HirBrowserOp,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        if domain != Capability::Browser {
            self.error_with_details(
                browser_origin(operation).range,
                "semantic.capability_mismatch",
                format!("Browser operation is not allowed in {domain} context"),
                serde_json::json!({
                    "required_capability": "Browser",
                    "actual_capability": domain.to_string(),
                    "construct": browser_reference(operation),
                }),
                Vec::new(),
                vec![
                    browser_reference(operation).into(),
                    "capability.Browser".into(),
                ],
            );
        }
        self.required.insert(Capability::Browser);
        let (operation, origin, assertion) = match operation {
            HirBrowserOp::Open(open) => {
                let url = self.infer_expr(&open.url, domain, Some(&Type::String));
                self.expect_type(open.url.origin.range, &Type::String, &url.ty);
                (
                    BrowserOperation::Navigate {
                        url: url.expression,
                    },
                    open.url.origin,
                    None,
                )
            }
            HirBrowserOp::Evaluate(evaluate) => (
                BrowserOperation::Evaluate {
                    expression: evaluate.expression.value.clone(),
                },
                evaluate.expression.origin,
                None,
            ),
            HirBrowserOp::Click(action) => (
                BrowserOperation::Click {
                    locator: locator_from_hir(&action.locator.kind),
                },
                action.locator.origin,
                None,
            ),
            HirBrowserOp::Fill(action) => {
                let value = self.browser_string_value(action, domain);
                (
                    BrowserOperation::Fill {
                        locator: locator_from_hir(&action.locator.kind),
                        value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Type(action) => {
                let value = self.browser_string_value(action, domain);
                (
                    BrowserOperation::Type {
                        locator: locator_from_hir(&action.locator.kind),
                        value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Press(action) => {
                let value = self.browser_string_value(action, domain);
                if let HirExprKind::Literal(HirLiteral::String(key)) = &action.value.kind
                    && !valid_key_chord(key)
                {
                    self.error(
                        action.value.origin.range,
                        "semantic.invalid_key",
                        format!("invalid key chord `{key}`"),
                    );
                }
                (
                    BrowserOperation::Press {
                        locator: locator_from_hir(&action.locator.kind),
                        key: value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Check(action) | HirBrowserOp::Uncheck(action) => (
                BrowserOperation::Check {
                    locator: locator_from_hir(&action.locator.kind),
                    checked: matches!(operation, HirBrowserOp::Check(_)),
                },
                action.locator.origin,
                None,
            ),
            HirBrowserOp::Select(action) => {
                let value = self.browser_string_value(action, domain);
                (
                    BrowserOperation::Select {
                        locator: locator_from_hir(&action.locator.kind),
                        option: value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Hover(action) => (
                BrowserOperation::Hover {
                    locator: locator_from_hir(&action.locator.kind),
                },
                action.locator.origin,
                None,
            ),
            HirBrowserOp::WaitLocator(wait) => (
                BrowserOperation::WaitForLocator {
                    locator: locator_from_hir(&wait.locator.kind),
                    state: locator_state_from_hir(wait.state),
                    timeout: wait.timeout,
                },
                wait.locator.origin,
                None,
            ),
            HirBrowserOp::WaitUrl(wait) => (
                BrowserOperation::WaitForUrl {
                    url: PlanExpr::Literal(Value::String(wait.url.value.clone())),
                    timeout: wait.timeout,
                },
                wait.url.origin,
                None,
            ),
            HirBrowserOp::ExpectLocator(expectation) => {
                let assertion = AssertionOperation::Locator {
                    locator: locator_from_hir(&expectation.locator.kind),
                    state: locator_state_from_hir(expectation.state),
                    timeout: expectation.timeout,
                };
                (
                    BrowserOperation::Hover {
                        locator: locator_from_hir(&expectation.locator.kind),
                    },
                    expectation.locator.origin,
                    Some(assertion),
                )
            }
            HirBrowserOp::ExpectUrl(expectation) => {
                let assertion = AssertionOperation::Url {
                    url: PlanExpr::Literal(Value::String(expectation.url.value.clone())),
                    timeout: expectation.timeout,
                };
                (
                    BrowserOperation::WaitForUrl {
                        url: PlanExpr::Literal(Value::String(expectation.url.value.clone())),
                        timeout: expectation.timeout,
                    },
                    expectation.url.origin,
                    Some(assertion),
                )
            }
        };
        self.push_step(
            steps,
            origin,
            assertion.map_or(TestOperation::Browser(operation), TestOperation::Assertion),
        );
    }

    fn browser_string_value(
        &mut self,
        action: &webtest_hir::HirValueAction,
        domain: Capability,
    ) -> PlanExpr {
        let value = self.infer_expr(&action.value, domain, Some(&Type::String));
        self.expect_type(action.value.origin.range, &Type::String, &value.ty);
        value.expression
    }

    fn provider_call(
        &mut self,
        expression: &HirExpr,
        domain: Capability,
    ) -> Option<CompiledProviderCall> {
        let HirExprKind::Call { callee, arguments } = &expression.kind else {
            return None;
        };
        let HirExprKind::Member {
            receiver,
            member: operation,
            ..
        } = &callee.kind
        else {
            return None;
        };
        let HirExprKind::Name(HirNameRef::Unresolved(provider)) = &receiver.kind else {
            return None;
        };
        let Some(schema) = self.providers.schema(provider) else {
            if provider == "app" {
                self.error(
                    receiver.origin.range,
                    "semantic.reserved_provider",
                    "`app` is reserved for the application bridge".into(),
                );
            } else {
                let known = self
                    .providers
                    .schemas()
                    .map(|schema| schema.name.0.clone())
                    .collect::<Vec<_>>();
                let candidates = nearest_strings(&known, provider, 5);
                self.error_with_details(
                    receiver.origin.range,
                    "semantic.unknown_provider",
                    format!("unknown provider `{provider}`"),
                    serde_json::json!({"requested": provider, "known_providers": known}),
                    text_hints(
                        RepairHintKind::NameCandidate,
                        candidates,
                        receiver.origin.range,
                    ),
                    vec!["provider".into()],
                );
            }
            return Some(CompiledProviderCall {
                provider: provider.clone(),
                operation: operation.clone(),
                arguments: BTreeMap::new(),
                result_type: Type::Unknown,
                schema_hash: String::new(),
                redacted_arguments: Vec::new(),
                redacted_result_fields: Vec::new(),
                retry_safe: false,
            });
        };
        let Some(operation_schema) = schema.operation(operation) else {
            let known = schema.operations.keys().cloned().collect::<Vec<_>>();
            let candidates = nearest_strings(&known, operation, 5);
            self.error_with_details(
                callee.origin.range,
                "semantic.unknown_provider_operation",
                format!("provider `{provider}` has no operation `{operation}`"),
                serde_json::json!({
                    "provider": provider,
                    "requested": operation,
                    "known_operations": known,
                }),
                text_hints(
                    RepairHintKind::NameCandidate,
                    candidates,
                    callee.origin.range,
                ),
                vec![format!("provider.{provider}")],
            );
            return Some(CompiledProviderCall {
                provider: provider.clone(),
                operation: operation.clone(),
                arguments: BTreeMap::new(),
                result_type: Type::Unknown,
                schema_hash: schema.hash(),
                redacted_arguments: Vec::new(),
                redacted_result_fields: Vec::new(),
                retry_safe: false,
            });
        };
        if domain != operation_schema.capability {
            self.error(
                expression.origin.range,
                "semantic.capability_mismatch",
                format!(
                    "{}.{} requires {} capability but is used in {domain} context",
                    provider, operation, operation_schema.capability
                ),
            );
        }
        self.required.insert(operation_schema.capability);
        let values =
            self.provider_arguments(operation_schema, arguments, domain, expression.origin.range);
        Some(CompiledProviderCall {
            provider: provider.clone(),
            operation: operation.clone(),
            arguments: values,
            result_type: operation_schema.result.clone(),
            schema_hash: schema.hash(),
            redacted_arguments: operation_schema
                .parameters
                .iter()
                .filter(|parameter| parameter.secret)
                .map(|parameter| parameter.name.clone())
                .collect(),
            redacted_result_fields: secret_record_fields(&operation_schema.result),
            retry_safe: operation_schema.retry_safe,
        })
    }

    fn provider_arguments(
        &mut self,
        schema: &OperationSchema,
        arguments: &[webtest_hir::HirCallArgument],
        domain: Capability,
        call_range: TextRange,
    ) -> BTreeMap<String, PlanExpr> {
        let mut values = BTreeMap::new();
        let positional: Vec<_> = schema
            .parameters
            .iter()
            .filter(|parameter| parameter.positional)
            .collect();
        let mut next_positional = 0;
        let mut body_argument = None;
        for argument in arguments {
            let parameter = if let Some(name) = &argument.name {
                schema
                    .parameters
                    .iter()
                    .find(|parameter| &parameter.name == name)
            } else {
                let parameter = positional.get(next_positional).copied();
                next_positional += 1;
                parameter
            };
            let Some(parameter) = parameter else {
                let requested = argument.name.clone();
                let known = schema
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>();
                let candidates = requested
                    .as_deref()
                    .map(|requested| nearest_strings(&known, requested, 5))
                    .unwrap_or_default();
                self.error_with_details(
                    argument.origin.range,
                    "semantic.unknown_argument",
                    argument.name.as_ref().map_or_else(
                        || "too many positional arguments".into(),
                        |name| format!("unknown argument `{name}`"),
                    ),
                    serde_json::json!({
                        "requested": requested,
                        "known_arguments": known,
                    }),
                    text_hints(
                        RepairHintKind::ArgumentCandidate,
                        candidates,
                        argument.origin.range,
                    ),
                    vec!["provider".into()],
                );
                continue;
            };
            if values.contains_key(&parameter.name) {
                self.error(
                    argument.origin.range,
                    "semantic.duplicate_argument",
                    format!("argument `{}` is provided more than once", parameter.name),
                );
                continue;
            }
            if matches!(parameter.name.as_str(), "json" | "text" | "bytes" | "form") {
                if let Some(previous) = &body_argument {
                    self.error(
                        argument.origin.range,
                        "semantic.conflicting_arguments",
                        format!(
                            "HTTP body arguments `{previous}` and `{}` cannot be combined",
                            parameter.name
                        ),
                    );
                } else {
                    body_argument = Some(parameter.name.clone());
                }
            }
            let value = self.infer_expr(&argument.value, domain, Some(&parameter.ty));
            self.expect_type(argument.value.origin.range, &parameter.ty, &value.ty);
            values.insert(parameter.name.clone(), value.expression);
        }
        for parameter in schema
            .parameters
            .iter()
            .filter(|parameter| parameter.required)
        {
            if !values.contains_key(&parameter.name) {
                self.error(
                    call_range,
                    "semantic.missing_argument",
                    format!("missing required argument `{}`", parameter.name),
                );
            }
        }
        values
    }

    fn infer_expr(
        &mut self,
        expression: &HirExpr,
        domain: Capability,
        expected: Option<&Type>,
    ) -> TypedExpr {
        let result = match &expression.kind {
            HirExprKind::Literal(literal) => match literal {
                HirLiteral::String(value) => typed(Value::String(value.clone()), Type::String),
                HirLiteral::Int(value) => typed(Value::Int(*value), Type::Int),
                HirLiteral::Float(value) => typed(Value::Float(*value), Type::Float),
                HirLiteral::Bool(value) => typed(Value::Bool(*value), Type::Bool),
                HirLiteral::Null => typed(Value::Null, Type::Null),
                HirLiteral::Duration(value) => typed(
                    Value::DurationMillis(value.as_millis().min(u128::from(u64::MAX)) as u64),
                    Type::Duration,
                ),
            },
            HirExprKind::Name(HirNameRef::Binding { id, name }) => {
                if let Some(binding) = self.bindings.get(id).cloned() {
                    if domain == Capability::Browser
                        && binding.domain == Capability::Server
                        && !binding.ty.is_transferable()
                    {
                        self.error(
                            expression.origin.range,
                            "semantic.non_transferable_value",
                            format!(
                                "binding `{}` has non-transferable type {} and cannot cross from Server to Browser",
                                binding.name, binding.ty
                            ),
                        );
                    }
                    TypedExpr {
                        expression: PlanExpr::Binding(*id),
                        ty: binding.ty,
                        capability: Capability::Pure,
                    }
                } else {
                    self.error(
                        expression.origin.range,
                        "semantic.use_before_definition",
                        format!("binding `{name}` is used before its value is available"),
                    );
                    unknown_expr()
                }
            }
            HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                if self.declared_names.contains(name) {
                    self.error(
                        expression.origin.range,
                        "semantic.use_before_definition",
                        format!("binding `{name}` is used before its declaration"),
                    );
                } else {
                    let known = self.declared_names.iter().cloned().collect::<Vec<_>>();
                    let candidates = nearest_strings(&known, name, 5);
                    self.error_with_details(
                        expression.origin.range,
                        "semantic.unknown_name",
                        format!("unknown name `{name}`"),
                        serde_json::json!({"requested": name, "known_names": known}),
                        text_hints(
                            RepairHintKind::NameCandidate,
                            candidates,
                            expression.origin.range,
                        ),
                        Vec::new(),
                    );
                }
                unknown_expr()
            }
            HirExprKind::List(items) => {
                let item_expected = match expected {
                    Some(Type::List(inner)) => Some(inner.as_ref()),
                    _ => None,
                };
                if items.is_empty() && item_expected.is_none() {
                    self.error(
                        expression.origin.range,
                        "semantic.empty_list_needs_type",
                        "empty list requires a contextual type".into(),
                    );
                }
                let compiled: Vec<_> = items
                    .iter()
                    .map(|item| self.infer_expr(item, domain, item_expected))
                    .collect();
                let item_type = item_expected
                    .cloned()
                    .or_else(|| compiled.first().map(|it| it.ty.clone()))
                    .unwrap_or(Type::Unknown);
                for item in &compiled {
                    self.expect_type(expression.origin.range, &item_type, &item.ty);
                }
                TypedExpr {
                    expression: PlanExpr::List(
                        compiled.into_iter().map(|item| item.expression).collect(),
                    ),
                    ty: Type::List(Box::new(item_type)),
                    capability: Capability::Pure,
                }
            }
            HirExprKind::Record(fields) => {
                let mut values = BTreeMap::new();
                let mut types = BTreeMap::new();
                for field in fields {
                    let expected_field = match expected {
                        Some(Type::Record(fields)) => fields.get(&field.name).map(|it| &it.ty),
                        _ => None,
                    };
                    if values.contains_key(&field.name) {
                        self.error(
                            field.origin.range,
                            "semantic.duplicate_record_field",
                            format!("record field `{}` is provided more than once", field.name),
                        );
                    }
                    let value = self.infer_expr(&field.value, domain, expected_field);
                    values.insert(field.name.clone(), value.expression);
                    types.insert(
                        field.name.clone(),
                        RecordField {
                            ty: value.ty,
                            optional: false,
                            documentation: String::new(),
                            secret: false,
                        },
                    );
                }
                TypedExpr {
                    expression: PlanExpr::Record(values),
                    ty: Type::Record(types),
                    capability: Capability::Pure,
                }
            }
            HirExprKind::Member {
                receiver,
                member,
                member_origin,
            } => {
                let receiver = self.infer_expr(receiver, domain, None);
                if let Some(ty) = receiver.ty.member(member) {
                    TypedExpr {
                        expression: PlanExpr::Member {
                            receiver: Box::new(receiver.expression),
                            member: member.clone(),
                        },
                        ty,
                        capability: receiver.capability,
                    }
                } else {
                    if receiver.ty != Type::Unknown {
                        let known = known_members(&receiver.ty);
                        let candidates = nearest_strings(&known, member, 5);
                        let message = if let Some(best) = candidates.first() {
                            format!(
                                "type {} has no member `{member}`; did you mean `{best}`?",
                                receiver.ty
                            )
                        } else {
                            format!("type {} has no member `{member}`", receiver.ty)
                        };
                        self.error_with_details(
                            member_origin.range,
                            "semantic.unknown_member",
                            message,
                            serde_json::json!({
                                "requested": member,
                                "receiver_type": receiver.ty.to_string(),
                                "known_members": known,
                            }),
                            text_hints(
                                RepairHintKind::MemberCandidate,
                                candidates,
                                member_origin.range,
                            ),
                            vec![format!("type.{}", receiver_type_reference(&receiver.ty))],
                        );
                    }
                    unknown_expr()
                }
            }
            HirExprKind::Call { .. } => {
                if let Some(call) = self.provider_call(expression, domain) {
                    self.error(
                        expression.origin.range,
                        "semantic.effectful_expression",
                        format!(
                            "provider call `{}.{}` must be the direct value of a binding or a statement",
                            call.provider, call.operation
                        ),
                    );
                    TypedExpr {
                        expression: PlanExpr::Literal(Value::Null),
                        ty: call.result_type,
                        capability: Capability::Server,
                    }
                } else {
                    self.error(
                        expression.origin.range,
                        "semantic.unknown_function",
                        "bare function calls do not resolve to providers".into(),
                    );
                    unknown_expr()
                }
            }
            HirExprKind::Unary { operator, operand } => {
                let operand = self.infer_expr(operand, domain, None);
                let ty = match operator {
                    UnaryOperator::Not if operand.ty == Type::Bool => Type::Bool,
                    UnaryOperator::Negate if numeric(&operand.ty) => operand.ty.clone(),
                    UnaryOperator::Not => {
                        self.type_mismatch(expression.origin.range, &Type::Bool, &operand.ty);
                        Type::Unknown
                    }
                    UnaryOperator::Negate => {
                        self.error(
                            expression.origin.range,
                            "semantic.invalid_unary_operand",
                            format!("numeric negation does not accept {}", operand.ty),
                        );
                        Type::Unknown
                    }
                };
                TypedExpr {
                    expression: PlanExpr::Unary {
                        operator: *operator,
                        operand: Box::new(operand.expression),
                    },
                    ty,
                    capability: operand.capability,
                }
            }
            HirExprKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.infer_expr(left, domain, None);
                let right = self.infer_expr(right, domain, Some(&left.ty));
                let ty =
                    self.validate_binary(*operator, &left.ty, &right.ty, expression.origin.range);
                TypedExpr {
                    expression: PlanExpr::Binary {
                        operator: *operator,
                        left: Box::new(left.expression),
                        right: Box::new(right.expression),
                    },
                    ty,
                    capability: Capability::Pure,
                }
            }
            HirExprKind::Missing => unknown_expr(),
        };
        self.type_fact(
            expression.origin.range,
            result.ty.clone(),
            result.capability,
        );
        result
    }

    fn validate_binary(
        &mut self,
        operator: BinaryOperator,
        left: &Type,
        right: &Type,
        range: TextRange,
    ) -> Type {
        if matches!(left, Type::Unknown) || matches!(right, Type::Unknown) {
            return Type::Unknown;
        }
        match operator {
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                if !left.accepts(right) && !right.accepts(left) {
                    self.error(
                        range,
                        "semantic.incompatible_equality",
                        format!("cannot compare {left} with {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if !(numeric(left) && numeric(right)
                    || left == &Type::String && right == &Type::String)
                {
                    self.error(
                        range,
                        "semantic.invalid_comparison",
                        format!("ordered comparison does not accept {left} and {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Add => {
                if left == &Type::String && right == &Type::String {
                    Type::String
                } else if numeric(left) && numeric(right) {
                    numeric_result(left, right)
                } else {
                    self.error(
                        range,
                        "semantic.invalid_binary_operands",
                        format!("addition does not accept {left} and {right}"),
                    );
                    Type::Unknown
                }
            }
            BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide => {
                if numeric(left) && numeric(right) {
                    numeric_result(left, right)
                } else {
                    self.error(
                        range,
                        "semantic.invalid_binary_operands",
                        format!("numeric operator does not accept {left} and {right}"),
                    );
                    Type::Unknown
                }
            }
            BinaryOperator::And | BinaryOperator::Or => {
                if left != &Type::Bool || right != &Type::Bool {
                    self.error(
                        range,
                        "semantic.invalid_boolean_operands",
                        format!("boolean operator requires Bool operands, got {left} and {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Contains => {
                let valid = (left == &Type::String && right == &Type::String)
                    || matches!(left, Type::List(inner) if inner.accepts(right));
                if !valid {
                    self.error(
                        range,
                        "semantic.invalid_matcher",
                        format!("`contains` does not accept {left} and {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Matches => Type::Bool,
        }
    }

    fn pattern_type(&mut self, expression: &HirExpr) -> Type {
        match &expression.kind {
            HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                named_type(name).unwrap_or_else(|| {
                    self.error(
                        expression.origin.range,
                        "semantic.unknown_type",
                        format!("unknown type `{name}` in match pattern"),
                    );
                    Type::Unknown
                })
            }
            HirExprKind::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.clone(),
                            RecordField {
                                ty: self.pattern_type(&field.value),
                                optional: false,
                                documentation: String::new(),
                                secret: false,
                            },
                        )
                    })
                    .collect(),
            ),
            HirExprKind::List(items) if items.len() == 1 => {
                Type::List(Box::new(self.pattern_type(&items[0])))
            }
            _ => {
                self.error(
                    expression.origin.range,
                    "semantic.invalid_type_pattern",
                    "expected a type name, record shape, or one-element list shape".into(),
                );
                Type::Unknown
            }
        }
    }

    fn lower_type(&mut self, ty: &HirType) -> Type {
        match &ty.kind {
            HirTypeKind::Named(name) => named_type(name).unwrap_or_else(|| {
                self.error(
                    ty.origin.range,
                    "semantic.unknown_type",
                    format!("unknown type `{name}`"),
                );
                Type::Unknown
            }),
            HirTypeKind::Generic { name, argument } => {
                let argument = self.lower_type(argument);
                match name.as_str() {
                    "List" => Type::List(Box::new(argument)),
                    "Option" => Type::Option(Box::new(argument)),
                    "Response" => Type::Response(Box::new(argument)),
                    _ => {
                        self.error(
                            ty.origin.range,
                            "semantic.unknown_type",
                            format!("unknown generic type `{name}`"),
                        );
                        Type::Unknown
                    }
                }
            }
            HirTypeKind::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.clone(),
                            RecordField {
                                ty: self.lower_type(&field.ty),
                                optional: field.optional,
                                documentation: String::new(),
                                secret: false,
                            },
                        )
                    })
                    .collect(),
            ),
            HirTypeKind::Missing => Type::Unknown,
        }
    }

    fn push_step(
        &mut self,
        steps: &mut Vec<PlannedStep>,
        origin: SyntaxOrigin,
        operation: TestOperation,
    ) {
        steps.push(PlannedStep {
            id: StepId(self.next_step),
            operation,
            origin,
        });
        self.next_step += 1;
    }

    fn expect_type(&mut self, range: TextRange, expected: &Type, actual: &Type) {
        if !expected.accepts(actual) {
            self.type_mismatch(range, expected, actual);
        }
    }

    fn type_mismatch(&mut self, range: TextRange, expected: &Type, actual: &Type) {
        if expected != &Type::Unknown && actual != &Type::Unknown {
            self.error_with_details(
                range,
                "semantic.type_mismatch",
                format!("expected {expected}, got {actual}"),
                serde_json::json!({
                    "expected_type": expected.to_string(),
                    "actual_type": actual.to_string(),
                }),
                Vec::new(),
                vec![
                    format!("type.{}", receiver_type_reference(expected)),
                    format!("type.{}", receiver_type_reference(actual)),
                ],
            );
        }
    }

    fn type_fact(&mut self, range: TextRange, ty: Type, capability: Capability) {
        self.type_facts.push(TypeFact {
            range,
            ty,
            capability,
        });
    }

    fn error(&mut self, range: TextRange, code: &'static str, message: String) {
        self.diagnostics.push(Diagnostic {
            range,
            severity: DiagnosticSeverity::Error,
            code,
            message,
            source: DiagnosticSource::Semantic,
            semantic_details: None,
            repair_hints: Vec::new(),
            reference_queries: default_reference_queries(code),
        });
    }

    fn error_with_details(
        &mut self,
        range: TextRange,
        code: &'static str,
        message: String,
        semantic_details: serde_json::Value,
        repair_hints: Vec<RepairHint>,
        reference_queries: Vec<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            range,
            severity: DiagnosticSeverity::Error,
            code,
            message,
            source: DiagnosticSource::Semantic,
            semantic_details: Some(semantic_details),
            repair_hints,
            reference_queries,
        });
    }
}

fn text_hints(kind: RepairHintKind, candidates: Vec<String>, range: TextRange) -> Vec<RepairHint> {
    candidates
        .into_iter()
        .map(|candidate| {
            let mut hint = RepairHint::text(kind, candidate);
            hint.source_range = Some(webtest_feedback::ByteRange {
                start: range.start().into(),
                end: range.end().into(),
            });
            hint
        })
        .collect()
}

fn nearest_strings(values: &[String], requested: &str, limit: usize) -> Vec<String> {
    let mut values = values
        .iter()
        .map(|value| (string_edit_distance(value, requested), value.clone()))
        .collect::<Vec<_>>();
    values.sort();
    values
        .into_iter()
        .take(limit)
        .map(|(_, value)| value)
        .collect()
}

fn string_edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right.iter().enumerate() {
            current.push(
                (previous[right_index + 1] + 1)
                    .min(current[right_index] + 1)
                    .min(previous[right_index] + usize::from(left_char != *right_char)),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

fn known_members(ty: &Type) -> Vec<String> {
    match ty {
        Type::Record(fields) => fields.keys().cloned().collect(),
        Type::Response(_) => ["status", "headers", "body", "text", "json"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        Type::ProcessResult => [
            "exit_code",
            "stdout",
            "stderr",
            "stdout_bytes",
            "stderr_bytes",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        _ => Vec::new(),
    }
}

fn secret_record_fields(ty: &Type) -> Vec<String> {
    fn collect(ty: &Type, fields: &mut BTreeSet<String>) {
        match ty {
            Type::Record(record) => {
                for (name, field) in record {
                    if field.secret {
                        fields.insert(name.clone());
                    }
                    collect(&field.ty, fields);
                }
            }
            Type::List(item) | Type::Option(item) | Type::Response(item) => collect(item, fields),
            _ => {}
        }
    }
    let mut fields = BTreeSet::new();
    collect(ty, &mut fields);
    fields.into_iter().collect()
}

fn receiver_type_reference(ty: &Type) -> &'static str {
    match ty {
        Type::Unknown => "Json",
        Type::Null => "Null",
        Type::Bool => "Bool",
        Type::Int => "Int",
        Type::Float => "Float",
        Type::String => "String",
        Type::Duration => "Duration",
        Type::Url => "Url",
        Type::Json => "Json",
        Type::List(_) => "List",
        Type::Option(_) => "Option",
        Type::Record(_) => "Record",
        Type::StatusCode => "StatusCode",
        Type::Headers => "Headers",
        Type::Bytes => "Bytes",
        Type::Response(_) => "Response",
        Type::ProcessResult => "ProcessResult",
        Type::FilePath => "FilePath",
        Type::TempDirectory => "TempDirectory",
        Type::Locator => "Locator",
        Type::BrowserPage => "BrowserPage",
    }
}

fn default_reference_queries(code: &str) -> Vec<String> {
    match code {
        "semantic.unknown_provider" | "semantic.reserved_provider" => vec!["provider".into()],
        "semantic.unknown_provider_operation"
        | "semantic.unknown_argument"
        | "semantic.missing_argument"
        | "semantic.duplicate_argument"
        | "semantic.conflicting_arguments" => vec!["provider".into()],
        "semantic.capability_mismatch" => vec!["capability".into(), "language".into()],
        "semantic.type_mismatch"
        | "semantic.unknown_type"
        | "semantic.invalid_type_pattern"
        | "semantic.non_transferable_value" => vec!["type".into()],
        "semantic.invalid_matcher" => vec!["assertion.value".into()],
        _ => Vec::new(),
    }
}

fn collect_binding_names(statement: &HirStmt, names: &mut HashSet<String>) {
    match statement {
        HirStmt::Server(block) => {
            for statement in &block.statements {
                collect_binding_names(statement, names);
            }
        }
        HirStmt::Browser(block) => {
            for statement in &block.statements {
                collect_binding_names(statement, names);
            }
        }
        HirStmt::Let(binding) => {
            names.insert(binding.name.clone());
        }
        HirStmt::Expression(_) | HirStmt::Expect(_) | HirStmt::BrowserOperation(_) => {}
    }
}

fn typed(value: Value, ty: Type) -> TypedExpr {
    TypedExpr {
        expression: PlanExpr::Literal(value),
        ty,
        capability: Capability::Pure,
    }
}

fn unknown_expr() -> TypedExpr {
    typed(Value::Null, Type::Unknown)
}

fn named_type(name: &str) -> Option<Type> {
    Some(match name {
        "Null" => Type::Null,
        "Bool" => Type::Bool,
        "Int" => Type::Int,
        "Float" => Type::Float,
        "String" => Type::String,
        "Duration" => Type::Duration,
        "Url" => Type::Url,
        "Json" => Type::Json,
        "StatusCode" => Type::StatusCode,
        "Headers" => Type::Headers,
        "Bytes" => Type::Bytes,
        "ProcessResult" => Type::ProcessResult,
        "FilePath" => Type::FilePath,
        "TempDirectory" => Type::TempDirectory,
        "Locator" => Type::Locator,
        "BrowserPage" => Type::BrowserPage,
        _ => return None,
    })
}

fn decodable_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Null
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::String
            | Type::Json
            | Type::List(_)
            | Type::Option(_)
            | Type::Record(_)
    )
}

fn numeric(ty: &Type) -> bool {
    matches!(ty, Type::Int | Type::Float | Type::StatusCode)
}

fn numeric_result(left: &Type, right: &Type) -> Type {
    if left == &Type::Float || right == &Type::Float {
        Type::Float
    } else {
        Type::Int
    }
}

fn matcher_for(operator: BinaryOperator) -> Option<ValueMatcher> {
    Some(match operator {
        BinaryOperator::Equal => ValueMatcher::Equal,
        BinaryOperator::NotEqual => ValueMatcher::NotEqual,
        BinaryOperator::Less => ValueMatcher::Less,
        BinaryOperator::LessEqual => ValueMatcher::LessEqual,
        BinaryOperator::Greater => ValueMatcher::Greater,
        BinaryOperator::GreaterEqual => ValueMatcher::GreaterEqual,
        BinaryOperator::Contains => ValueMatcher::Contains,
        BinaryOperator::Matches => ValueMatcher::Matches,
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide
        | BinaryOperator::And
        | BinaryOperator::Or => return None,
    })
}

fn browser_origin(operation: &HirBrowserOp) -> SyntaxOrigin {
    match operation {
        HirBrowserOp::Open(value) => value.origin,
        HirBrowserOp::Evaluate(value) => value.origin,
        HirBrowserOp::Click(value)
        | HirBrowserOp::Check(value)
        | HirBrowserOp::Uncheck(value)
        | HirBrowserOp::Hover(value) => value.origin,
        HirBrowserOp::Fill(value)
        | HirBrowserOp::Type(value)
        | HirBrowserOp::Press(value)
        | HirBrowserOp::Select(value) => value.origin,
        HirBrowserOp::WaitLocator(value) | HirBrowserOp::ExpectLocator(value) => value.origin,
        HirBrowserOp::WaitUrl(value) | HirBrowserOp::ExpectUrl(value) => value.origin,
    }
}

fn browser_reference(operation: &HirBrowserOp) -> &'static str {
    match operation {
        HirBrowserOp::Open(_) => "browser.open",
        HirBrowserOp::Evaluate(_) => "browser.evaluate",
        HirBrowserOp::Click(_) => "browser.click",
        HirBrowserOp::Fill(_) => "browser.fill",
        HirBrowserOp::Type(_) => "browser.type",
        HirBrowserOp::Press(_) => "browser.press",
        HirBrowserOp::Check(_) => "browser.check",
        HirBrowserOp::Uncheck(_) => "browser.uncheck",
        HirBrowserOp::Select(_) => "browser.select",
        HirBrowserOp::Hover(_) => "browser.hover",
        HirBrowserOp::WaitLocator(_) => "browser.wait.locator",
        HirBrowserOp::WaitUrl(_) => "browser.wait.url",
        HirBrowserOp::ExpectLocator(_) => "assertion.locator_state",
        HirBrowserOp::ExpectUrl(_) => "assertion.url",
    }
}

fn valid_key_chord(value: &str) -> bool {
    let mut main = 0;
    for part in value.split('+') {
        match part {
            "Alt" | "Control" | "Ctrl" | "Meta" | "Command" | "Shift" => {}
            "Enter" | "Tab" | "Escape" | "Esc" | "Backspace" | "Delete" | "ArrowUp"
            | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp" | "PageDown"
            | "Space" => main += 1,
            value if value.chars().count() == 1 => main += 1,
            _ => return false,
        }
    }
    main == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(source: &str) -> (Vec<Diagnostic>, TestPlan) {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("file:///test.webtest", source);
        (
            database
                .diagnostics(file)
                .expect("diagnostics")
                .as_ref()
                .clone(),
            database.test_plan(file).expect("plan").as_ref().clone(),
        )
    }

    #[test]
    fn compiles_typed_server_to_browser_flow() {
        let source = r#"test "created user can sign in" {
            server {
                let response = http.post("/api/test/users", json: { email: "alice@example.com" })
                expect response.status == 201
                let user: { id: Int, email: String } = response.json
            }
            browser {
                open "/login"
                fill label("Email") with user.email
                click role("button", name: "Sign in")
                expect text("Welcome").visible
            }
        }"#;
        let (diagnostics, plan) = analyze(source);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(matches!(
            plan.tests[0].steps[0].operation,
            TestOperation::ServerProviderCall(_)
        ));
        assert!(matches!(
            plan.tests[0].steps[2].operation,
            TestOperation::EvaluatePure(EvaluatePureOperation {
                expression: PlanExpr::Decode { .. },
                ..
            })
        ));
        assert_eq!(
            plan.required_host_capabilities,
            vec![Capability::Server, Capability::Browser, Capability::Test]
        );
    }

    #[test]
    fn reports_provider_type_capability_and_transfer_errors() {
        let source = r#"test "bad" {
            server { let result = process.run("seed", args: [1]) }
            browser {
                let nope = http.get("/inside-browser")
                fill label("Output") with result.stdout
            }
        }"#;
        let (diagnostics, _) = analyze(source);
        let codes: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        assert!(
            codes.contains(&"semantic.type_mismatch"),
            "{diagnostics:#?}"
        );
        assert!(
            codes.contains(&"semantic.capability_mismatch"),
            "{diagnostics:#?}"
        );
        assert!(
            codes.contains(&"semantic.non_transferable_value"),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn distinguishes_use_before_definition_from_unknown_names() {
        let source = r#"test "names" {
            let first = later
            let later = 1
            expect missing == 1
        }"#;
        let (diagnostics, _) = analyze(source);
        let later = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "semantic.use_before_definition")
            .expect("use-before diagnostic");
        assert_eq!(
            &source[u32::from(later.range.start()) as usize..u32::from(later.range.end()) as usize],
            "later"
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "semantic.unknown_name")
        );
    }

    #[test]
    fn expression_precedence_is_preserved_in_the_typed_plan() {
        let (diagnostics, plan) = analyze(r#"test "math" { let value = 1 + 2 * 3 }"#);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let TestOperation::EvaluatePure(operation) = &plan.tests[0].steps[0].operation else {
            panic!("pure evaluation")
        };
        assert!(matches!(
            operation.expression,
            PlanExpr::Binary {
                operator: BinaryOperator::Add,
                ref right,
                ..
            } if matches!(
                right.as_ref(),
                PlanExpr::Binary {
                    operator: BinaryOperator::Multiply,
                    ..
                }
            )
        ));
    }

    #[test]
    fn static_diagnostics_and_queries_follow_source_updates() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file(
            "file:///test.webtest",
            "test \"x\" { browser { open click id(\"x\") } }",
        );
        let first_revision = database.source_revision(file).expect("first revision");
        assert!(
            database
                .diagnostics(file)
                .expect("invalid diagnostics")
                .iter()
                .any(|diagnostic| diagnostic.code == "syntax.expected_expression")
        );

        database.set_file_text(
            file,
            "test \"x\" { browser { open \"about:blank\" click id(\"x\") } }",
        );
        assert_ne!(
            database.source_revision(file).expect("second revision"),
            first_revision
        );
        assert!(
            database
                .diagnostics(file)
                .expect("valid diagnostics")
                .is_empty()
        );
        assert_eq!(database.test_plan(file).expect("plan").tests.len(), 1);
    }

    #[test]
    fn machine_diagnostics_preserve_typed_details_references_and_bounded_corrections() {
        let source = r#"test "machine" {
            server {
                let user: { id: Int, email: String } = { id: 1, email: "a@example.test" }
                let typo = user.emial
                let response = htp.get("http://example.test")
                let other = http.gte("http://example.test")
                let final = http.get("http://example.test", heders: {})
            }
        }"#;
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("machine.webtest", source);
        let diagnostics = database.diagnostics(file).expect("diagnostics");
        let member = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "semantic.unknown_member")
            .expect("member diagnostic");
        assert_eq!(
            member.semantic_details.as_ref().expect("details")["requested"],
            "emial"
        );
        assert!(
            member
                .repair_hints
                .iter()
                .any(|hint| { hint.replacement == RepairReplacement::text("email") })
        );
        assert!(member.reference_queries.contains(&"type.Record".into()));

        let provider = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "semantic.unknown_provider")
            .expect("provider diagnostic");
        assert!(
            provider
                .repair_hints
                .iter()
                .any(|hint| { hint.replacement == RepairReplacement::text("http") })
        );
        let operation = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "semantic.unknown_provider_operation")
            .expect("operation diagnostic");
        assert!(
            operation
                .repair_hints
                .iter()
                .any(|hint| { hint.replacement == RepairReplacement::text("get") })
        );
        let argument = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "semantic.unknown_argument")
            .expect("argument diagnostic");
        assert!(
            argument
                .repair_hints
                .iter()
                .any(|hint| { hint.replacement == RepairReplacement::text("headers") })
        );
    }

    #[test]
    fn illegal_browser_action_in_server_scope_links_to_the_authoritative_reference() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file(
            "illegal.webtest",
            r#"test "illegal" { server { click role("button", name: "Sign in") } }"#,
        );
        let diagnostics = database.diagnostics(file).expect("diagnostics");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "syntax.expected_server_statement")
            .expect("server-scope syntax diagnostic");
        assert!(
            diagnostic
                .reference_queries
                .contains(&"scope.server".into())
        );
        assert!(
            diagnostic
                .reference_queries
                .contains(&"browser.click".into())
        );
    }
}
