//! Explicit portable workspace inputs and queries. No host I/O or execution.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use webtest_analysis::{AnalysisDatabase, AnalysisSnapshot, WorkspaceConfiguration};
use webtest_text::{DocumentVersion, TextRange, TextSize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSource {
    pub path: String,
    pub text: String,
    #[serde(default)]
    pub version: Option<DocumentVersion>,
    #[serde(default)]
    pub uri: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInputs {
    pub configuration: WorkspaceConfiguration,
    pub sources: Vec<WorkspaceSource>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub app_manifest: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceQuery {
    CompileVariant {
        variant: webtest_model::TestVariantId,
    },
    ProjectDescription {
        limit: usize,
    },
    Diagnostics {
        path: String,
    },
    Compile {
        path: String,
    },
    Format {
        path: String,
    },
    Completions {
        path: String,
        offset: u32,
    },
    Signature {
        path: String,
        offset: u32,
    },
    Hover {
        path: String,
        offset: u32,
    },
    Definition {
        path: String,
        offset: u32,
    },
    References {
        path: String,
        offset: u32,
        include_declaration: bool,
        limit: usize,
    },
    Rename {
        path: String,
        offset: u32,
        name: String,
    },
    DocumentSymbols {
        path: String,
    },
    WorkspaceSymbols {
        query: String,
        limit: usize,
    },
    Folding {
        path: String,
    },
    Selection {
        path: String,
        offset: u32,
    },
    InlayHints {
        path: String,
    },
    CodeActions {
        path: String,
        start: u32,
        end: u32,
    },
    SemanticTokens {
        path: String,
    },
    Discovery {
        limit: usize,
    },
}
pub struct PortableWorkspace {
    database: AnalysisDatabase,
}
impl PortableWorkspace {
    pub fn new(inputs: WorkspaceInputs) -> Result<Self, String> {
        let mut workspace = Self {
            database: AnalysisDatabase::default(),
        };
        workspace.replace_inputs(inputs)?;
        Ok(workspace)
    }
    /// Validate the entire host transaction before mutating any tracked input.
    /// Unchanged files and schemas keep their Salsa memos.
    pub fn replace_inputs(&mut self, inputs: WorkspaceInputs) -> Result<u64, String> {
        let mut paths = BTreeSet::new();
        let mut bytes = 0usize;
        if inputs.sources.len() > 16_384 || inputs.aliases.len() > 16_384 {
            return Err("workspace input count exceeds the limit".into());
        }
        for source in &inputs.sources {
            if source.path.is_empty()
                || source.path.len() > 4096
                || source.path.contains('\0')
                || !paths.insert(source.path.clone())
                || source.uri.as_ref().is_some_and(|uri| uri.len() > 4096)
            {
                return Err("invalid or duplicate workspace source path".into());
            }
            bytes = bytes.saturating_add(source.text.len());
            if source.text.len() > 8 * 1024 * 1024 || bytes > 64 * 1024 * 1024 {
                return Err("workspace source byte budget exceeded".into());
            }
        }
        if inputs.configuration.root.len() > 4096
            || inputs.configuration.source_roots.len() > 128
            || inputs
                .configuration
                .source_roots
                .iter()
                .any(|root| root.len() > 4096)
            || inputs.configuration.compiler_features.len() > 128
            || inputs
                .configuration
                .compiler_features
                .iter()
                .any(|feature| feature.len() > 256)
            || inputs.configuration.toolchain_identity.len() > 4096
            || inputs
                .configuration
                .active_profile
                .as_ref()
                .is_some_and(|profile| profile.len() > 256)
            || serde_json::to_vec(&inputs.configuration.resolved_configuration)
                .map_or(true, |bytes| bytes.len() > 1024 * 1024)
        {
            return Err("workspace configuration exceeds the input limit".into());
        }
        for (alias, target) in &inputs.aliases {
            if alias.is_empty()
                || alias.len() > 4096
                || alias.contains('\0')
                || paths.contains(alias)
                || !paths.contains(target)
            {
                return Err("invalid workspace alias or canonical target".into());
            }
        }
        let mut providers = webtest_provider::ProviderRegistry::built_in_schemas();
        if let Some(manifest) = &inputs.app_manifest {
            if manifest.len() > 8 * 1024 * 1024 {
                return Err("application schema byte budget exceeded".into());
            }
            let manifest = match webtest_app_bridge::AppManifest::from_json_normalized(manifest) {
                Ok(manifest) => manifest,
                Err(error) => {
                    self.database.set_provider_registry(providers);
                    let mut configuration = self.database.workspace_configuration();
                    configuration.invalid_inputs = true;
                    self.database.configure_workspace(configuration);
                    return Err(error.to_string());
                }
            };
            providers.register_schema(manifest.provider_schema());
        }
        for (alias, target) in self.database.source_aliases() {
            if inputs.aliases.get(&alias) != Some(&target) {
                self.database.remove_alias(&alias);
            }
        }
        for (path, _) in self.database.source_inputs() {
            if !paths.contains(&path)
                && let Some(file) = self.database.file_for_path(&path)
            {
                self.database.close_file(file);
            }
        }
        self.database.configure_workspace(inputs.configuration);
        self.database.set_provider_registry(providers);
        for source in inputs.sources {
            let file = self.database.open_file(source.path, source.text);
            match source.version {
                Some(version) => self.database.set_document_version(file, version),
                None => self.database.clear_document_version(file),
            }
            .map_err(|error| error.to_string())?;
            self.database
                .set_document_uri(file, source.uri)
                .map_err(|error| error.to_string())?;
        }
        for (alias, target) in inputs.aliases {
            let file = self
                .database
                .file_for_path(&target)
                .ok_or("canonical alias target disappeared")?;
            self.database
                .alias_file(&alias, file)
                .map_err(|error| error.to_string())?;
        }
        Ok(self.database.workspace_revision())
    }
    pub fn revision(&self) -> u64 {
        self.database.workspace_revision()
    }
    pub fn snapshot(&self) -> AnalysisSnapshot {
        self.database.snapshot()
    }
    pub fn query(&self, request: WorkspaceQuery) -> Result<Value, String> {
        query(&mut self.snapshot(), request)
    }
}
fn encoded(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}
fn query(snapshot: &mut AnalysisSnapshot, request: WorkspaceQuery) -> Result<Value, String> {
    let file = |path: &str| {
        snapshot
            .file_for_path(path)
            .ok_or_else(|| format!("unknown workspace source `{path}`"))
    };
    let result = match request {
        WorkspaceQuery::CompileVariant { variant } => {
            let discovery = snapshot.discover_tests(10_000).map_err(|e| e.to_string())?;
            if discovery.truncated {
                return Err("test discovery exceeded the limit".into());
            }
            let module = discovery
                .modules
                .iter()
                .find(|module| {
                    module.tests.iter().any(|test| {
                        test.variants
                            .iter()
                            .any(|item| item.id == variant && item.runnable)
                    })
                })
                .ok_or("unknown or non-runnable test variant")?;
            let plan = snapshot
                .test_plan(module.file)
                .map_err(|e| e.to_string())?
                .select_variants(&BTreeSet::from([variant]));
            json!({"required_host_capabilities":plan.required_host_capabilities,"plan":plan,"executable_on_wasm":false})
        }
        WorkspaceQuery::ProjectDescription { limit } => encoded(
            snapshot
                .project_description(limit)
                .map_err(|error| error.to_string())?,
        )?,
        WorkspaceQuery::Discovery { limit } => encoded(
            snapshot
                .discover_tests(limit)
                .map_err(|error| error.to_string())?,
        )?,
        WorkspaceQuery::WorkspaceSymbols { query, limit } => encoded(
            snapshot
                .workspace_symbols(&query, limit)
                .map_err(|error| error.to_string())?,
        )?,
        request => {
            let path = match &request {
                WorkspaceQuery::Diagnostics { path }
                | WorkspaceQuery::Compile { path }
                | WorkspaceQuery::Format { path }
                | WorkspaceQuery::Completions { path, .. }
                | WorkspaceQuery::Signature { path, .. }
                | WorkspaceQuery::Hover { path, .. }
                | WorkspaceQuery::Definition { path, .. }
                | WorkspaceQuery::References { path, .. }
                | WorkspaceQuery::Rename { path, .. }
                | WorkspaceQuery::DocumentSymbols { path }
                | WorkspaceQuery::Folding { path }
                | WorkspaceQuery::Selection { path, .. }
                | WorkspaceQuery::InlayHints { path }
                | WorkspaceQuery::CodeActions { path, .. }
                | WorkspaceQuery::SemanticTokens { path } => path,
                _ => return Err("invalid workspace query".into()),
            };
            let file = file(path)?;
            let source = snapshot.source(file).map_err(|error| error.to_string())?;
            match request {
                WorkspaceQuery::Diagnostics { .. } => encoded(snapshot.diagnostics(file).map_err(|error| error.to_string())?.iter().map(|diagnostic| super::portable_diagnostic(&source, diagnostic)).collect::<Vec<_>>())?,
                WorkspaceQuery::Compile { .. } => {
                    let diagnostics = snapshot.diagnostics(file).map_err(|error| error.to_string())?;
                    let plan = if diagnostics.iter().any(|diagnostic| diagnostic.severity == webtest_analysis::DiagnosticSeverity::Error) { None } else { Some(snapshot.test_plan(file).map_err(|error| error.to_string())?) };
                    json!({"diagnostics":diagnostics.iter().map(|diagnostic| super::portable_diagnostic(&source, diagnostic)).collect::<Vec<_>>(), "plan":plan.as_deref(), "executable_on_wasm":false, "required_host_capabilities":plan.as_ref().map(|plan| &plan.required_host_capabilities)})
                }
                WorkspaceQuery::Format { .. } => encoded(webtest_format::format_file(&snapshot.parse(file).map_err(|error| error.to_string())?))?,
                WorkspaceQuery::Completions { offset, .. } => encoded(snapshot.completions(file, offset.into()).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::Signature { offset, .. } => encoded(snapshot.signature_help(file, offset.into()).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::Hover { offset, .. } => {
                    let documentation = snapshot.documentation_at(file, offset.into()).map_err(|error| error.to_string())?;
                    if let Some(fact) = documentation { json!({"start":u32::from(fact.range.start()),"end":u32::from(fact.range.end()),"contents":fact.contents}) }
                    else if let Some(fact) = snapshot.type_at(file, offset.into()).map_err(|error| error.to_string())? { json!({"start":u32::from(fact.range.start()),"end":u32::from(fact.range.end()),"contents":format!("{} ({})",fact.ty,fact.capability)}) }
                    else { Value::Null }
                }
                WorkspaceQuery::Definition { offset, .. } => encoded(snapshot.definition(file, offset.into()).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::References { offset, include_declaration, limit, .. } => encoded(snapshot.references(file, offset.into(), include_declaration, limit).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::Rename { offset, name, .. } => encoded(snapshot.rename(file, offset.into(), &name).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::DocumentSymbols { .. } => encoded(snapshot.document_symbols(file).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::Folding { .. } => encoded(snapshot.folding_ranges(file).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::Selection { offset, .. } => encoded(snapshot.selection_range(file, offset.into()).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::InlayHints { .. } => encoded(snapshot.inlay_hints(file).map_err(|error| error.to_string())?)?,
                WorkspaceQuery::CodeActions { start, end, .. } => {
                    if start > end { return Err("source range starts after its end".into()); }
                    encoded(snapshot.code_actions(file, TextRange::new(TextSize::from(start), TextSize::from(end))).map_err(|error| error.to_string())?)?
                }
                WorkspaceQuery::SemanticTokens { .. } => encoded(snapshot.semantic_tokens(file).map_err(|error| error.to_string())?.into_iter().map(|token| json!({"start":u32::from(token.range.start()),"end":u32::from(token.range.end()),"kind":token.kind,"modifiers":token.modifiers})).collect::<Vec<_>>())?,
                _ => return Err("invalid document query".into()),
            }
        }
    };
    Ok(
        json!({"workspace_schema_version":1,"workspace_revision":snapshot.workspace_revision(),"result":result}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inputs() -> WorkspaceInputs {
        WorkspaceInputs {
            configuration: WorkspaceConfiguration { root: "/workspace".into(), ..Default::default() },
            sources: vec![
                WorkspaceSource { path: "/workspace/helpers.webtest".into(), text: "/// User identity.\nexport fn identité(value: String) -> String = value".into(), version: Some(DocumentVersion(3)), uri: None },
                WorkspaceSource { path: "/workspace/root.webtest".into(), text: "import { identité as make } from \"./helpers\"\ntest \"same\" { let üser = make(value: \"a\") expect üser == \"a\" }".into(), version: Some(DocumentVersion(5)), uri: None },
            ], aliases: BTreeMap::new(), app_manifest: None,
        }
    }
    #[test]
    fn workspace_queries_have_native_parity_and_keep_incremental_inputs() {
        let inputs = inputs();
        let mut workspace = PortableWorkspace::new(inputs.clone()).unwrap();
        let revision = workspace.database.workspace_revision();
        let description = workspace
            .query(WorkspaceQuery::ProjectDescription { limit: 100 })
            .unwrap();
        assert_eq!(
            description["result"],
            encoded(workspace.database.project_description(100)).unwrap()
        );
        let path = "/workspace/root.webtest";
        let file = workspace.database.file_for_path(path).unwrap();
        let source = workspace.database.source(file).unwrap();
        let offset = source.find("make(value").unwrap() as u32;
        let definition = workspace
            .query(WorkspaceQuery::Definition {
                path: path.into(),
                offset,
            })
            .unwrap();
        assert_eq!(
            definition["result"],
            encoded(workspace.database.definition(file, offset.into()).unwrap()).unwrap()
        );
        assert_eq!(
            definition["result"]["target"]["path"],
            "/workspace/helpers.webtest"
        );
        let plan = workspace
            .query(WorkspaceQuery::Compile { path: path.into() })
            .unwrap();
        assert_eq!(
            plan["result"]["plan"],
            encoded(workspace.database.test_plan(file).unwrap().as_ref()).unwrap()
        );
        let rename = workspace
            .query(WorkspaceQuery::Rename {
                path: path.into(),
                offset,
                name: "renamed".into(),
            })
            .unwrap();
        assert_eq!(rename["result"]["documents"][0]["version"], 5);
        assert_eq!(
            rename["result"]["documents"][0]["edits"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let signature = workspace
            .query(WorkspaceQuery::Signature {
                path: path.into(),
                offset: offset + 5,
            })
            .unwrap();
        assert!(
            signature["result"]["documentation"]
                .as_str()
                .unwrap()
                .contains("User identity")
        );
        let tokens = workspace
            .query(WorkspaceQuery::SemanticTokens { path: path.into() })
            .unwrap();
        assert!(
            tokens["result"]
                .as_array()
                .unwrap()
                .iter()
                .any(|token| token["kind"] == "function")
        );
        let hints = workspace
            .query(WorkspaceQuery::InlayHints { path: path.into() })
            .unwrap();
        assert_eq!(hints["result"]["items"][0]["label"], ": String");
        let before = workspace.database.query_counts();
        assert_eq!(workspace.replace_inputs(inputs.clone()).unwrap(), revision);
        workspace
            .query(WorkspaceQuery::Compile { path: path.into() })
            .unwrap();
        assert_eq!(workspace.database.query_counts(), before);
        let mut changed = inputs;
        changed.sources[0].text = "export fn identité(value: String) -> Int = 7".into();
        workspace.replace_inputs(changed).unwrap();
        let diagnostics = workspace
            .query(WorkspaceQuery::Diagnostics { path: path.into() })
            .unwrap();
        assert!(!diagnostics["result"].as_array().unwrap().is_empty());
        assert!(
            workspace
                .query(WorkspaceQuery::Hover {
                    path: path.into(),
                    offset: source.find("üser").unwrap() as u32 + 1
                })
                .is_err()
        );
        assert!(
            workspace
                .query(WorkspaceQuery::CodeActions {
                    path: path.into(),
                    start: 9,
                    end: 1
                })
                .is_err()
        );
    }
    #[test]
    fn aliases_deletions_and_invalid_schema_replacements_invalidate_the_same_core() {
        let mut inputs = inputs();
        inputs.aliases.insert(
            "/workspace/alias.webtest".into(),
            "/workspace/helpers.webtest".into(),
        );
        inputs.sources[1].text = inputs.sources[1].text.replace("./helpers", "./alias");
        let mut workspace = PortableWorkspace::new(inputs.clone()).unwrap();
        let revision = workspace.database.workspace_revision();
        assert_eq!(workspace.replace_inputs(inputs.clone()).unwrap(), revision);
        let root = "/workspace/root.webtest";
        assert_eq!(
            workspace
                .query(WorkspaceQuery::Diagnostics { path: root.into() })
                .unwrap()["result"],
            json!([])
        );
        inputs.aliases.clear();
        workspace.replace_inputs(inputs.clone()).unwrap();
        assert!(
            !workspace
                .query(WorkspaceQuery::Diagnostics { path: root.into() })
                .unwrap()["result"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        inputs.app_manifest =
            Some(include_str!("../../../protocol/examples/app-schema.json").into());
        inputs.sources[1].text =
            "test \"app\" { server { let user = app.create_user(email: \"a@example.com\") } }"
                .into();
        workspace.replace_inputs(inputs.clone()).unwrap();
        assert_eq!(
            workspace
                .query(WorkspaceQuery::Diagnostics { path: root.into() })
                .unwrap()["result"],
            json!([])
        );
        inputs.app_manifest = Some("{".into());
        assert!(workspace.replace_inputs(inputs).is_err());
        assert!(
            !workspace
                .query(WorkspaceQuery::Diagnostics { path: root.into() })
                .unwrap()["result"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    use super::*;
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(js_name = Workspace)]
    pub struct Workspace(PortableWorkspace);
    #[wasm_bindgen(js_class = Workspace)]
    impl Workspace {
        #[wasm_bindgen(constructor)]
        pub fn new(inputs: &str) -> Result<Workspace, JsValue> {
            let inputs = decode_inputs(inputs).map_err(JsValue::from)?;
            PortableWorkspace::new(inputs)
                .map(Workspace)
                .map_err(JsValue::from)
        }
        #[wasm_bindgen(js_name = replaceInputs)]
        pub fn replace_inputs(&mut self, inputs: &str) -> Result<u64, JsValue> {
            self.0
                .replace_inputs(decode_inputs(inputs).map_err(JsValue::from)?)
                .map_err(JsValue::from)
        }
        pub fn query(&self, request: &str) -> Result<String, JsValue> {
            if request.len() > 256 * 1024 {
                return Err(JsValue::from_str("query byte budget exceeded"));
            }
            let request = serde_json::from_str(request)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            self.0
                .query(request)
                .and_then(|result| {
                    serde_json::to_string(&result).map_err(|error| error.to_string())
                })
                .map_err(JsValue::from)
        }
    }
    fn decode_inputs(inputs: &str) -> Result<WorkspaceInputs, String> {
        if inputs.len() > 80 * 1024 * 1024 {
            return Err("workspace input envelope byte budget exceeded".into());
        }
        serde_json::from_str(inputs).map_err(|error| error.to_string())
    }
}
