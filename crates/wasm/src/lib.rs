//! Stable DTO-oriented facade for browser/editor builds of the language core.

use serde::Serialize;
use webtest_analysis::{AnalysisDatabase, DiagnosticSeverity};
use webtest_app_bridge::AppManifest;
use webtest_model::Capability;
use webtest_plan::TestPlan;
use webtest_provider::ProviderRegistry;

#[derive(Clone, Debug, Serialize)]
pub struct PortableDiagnostic {
    diagnostic_schema_version: u32,
    repair_hint_schema_version: u32,
    start: u32,
    end: u32,
    code: &'static str,
    severity: String,
    message: String,
    source_revision: String,
    semantic_details: Option<serde_json::Value>,
    repair_hints: Vec<webtest_feedback::RepairHint>,
    reference_queries: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PortableCompilation {
    diagnostics: Vec<PortableDiagnostic>,
    plan: Option<TestPlan>,
    required_host_capabilities: Vec<Capability>,
    executable_on_wasm: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PortableDocumentation {
    start: u32,
    end: u32,
    contents: String,
}

pub fn diagnostics(source: &str) -> Vec<PortableDiagnostic> {
    diagnostics_with_database(source, AnalysisDatabase::default())
}

pub fn diagnostics_with_manifest(
    source: &str,
    manifest_json: &str,
) -> Result<Vec<PortableDiagnostic>, String> {
    Ok(diagnostics_with_database(
        source,
        database_with_manifest(manifest_json)?,
    ))
}

fn diagnostics_with_database(
    source: &str,
    mut database: AnalysisDatabase,
) -> Vec<PortableDiagnostic> {
    let file = database.open_file("memory://document.webtest", source);
    database
        .diagnostics(file)
        .map(|diagnostics| {
            diagnostics
                .iter()
                .map(|diagnostic| portable_diagnostic(source, diagnostic))
                .collect()
        })
        .unwrap_or_default()
}

pub fn format(source: &str) -> String {
    webtest_format::format_file(&webtest_syntax::parse(source))
}

pub fn compile(source: &str) -> PortableCompilation {
    compile_with_database(source, AnalysisDatabase::default())
}

pub fn compile_with_manifest(
    source: &str,
    manifest_json: &str,
) -> Result<PortableCompilation, String> {
    Ok(compile_with_database(
        source,
        database_with_manifest(manifest_json)?,
    ))
}

fn compile_with_database(source: &str, mut database: AnalysisDatabase) -> PortableCompilation {
    let file = database.open_file("memory://document.webtest", source);
    let diagnostics = database.diagnostics(file).ok();
    let has_errors = diagnostics.as_ref().is_some_and(|diagnostics| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    });
    let diagnostics = diagnostics
        .map(|diagnostics| {
            diagnostics
                .iter()
                .map(|diagnostic| portable_diagnostic(source, diagnostic))
                .collect()
        })
        .unwrap_or_default();
    let plan = (!has_errors)
        .then(|| {
            database
                .test_plan(file)
                .ok()
                .map(|plan| plan.as_ref().clone())
        })
        .flatten();
    let required_host_capabilities = plan
        .as_ref()
        .map(|plan| plan.required_host_capabilities.clone())
        .unwrap_or_default();
    PortableCompilation {
        diagnostics,
        executable_on_wasm: required_host_capabilities.is_empty()
            || required_host_capabilities == [Capability::Pure, Capability::Test],
        required_host_capabilities,
        plan,
    }
}

fn portable_diagnostic(
    source: &str,
    diagnostic: &webtest_analysis::Diagnostic,
) -> PortableDiagnostic {
    PortableDiagnostic {
        diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        start: diagnostic.range.start().into(),
        end: diagnostic.range.end().into(),
        code: diagnostic.code,
        severity: match diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Information => "information",
            DiagnosticSeverity::Hint => "hint",
        }
        .into(),
        message: diagnostic.message.clone(),
        source_revision: webtest_text::SourceRevision::of(source)
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        semantic_details: diagnostic.semantic_details.clone(),
        repair_hints: diagnostic.repair_hints.clone(),
        reference_queries: diagnostic.reference_queries.clone(),
    }
}

pub fn describe(
    query: Option<&str>,
    search: Option<&str>,
) -> webtest_analysis::DescriptionResponse {
    let request = search.map_or_else(
        || {
            query.map_or(webtest_analysis::DescriptionRequest::Index, |query| {
                webtest_analysis::DescriptionRequest::Query(query.into())
            })
        },
        |search| webtest_analysis::DescriptionRequest::Search(search.into()),
    );
    AnalysisDatabase::default().describe(
        request,
        None,
        webtest_analysis::DescriptionLimits::default(),
    )
}

pub fn describe_with_manifest(
    query: Option<&str>,
    search: Option<&str>,
    manifest_json: &str,
) -> Result<webtest_analysis::DescriptionResponse, String> {
    let request = search.map_or_else(
        || {
            query.map_or(webtest_analysis::DescriptionRequest::Index, |query| {
                webtest_analysis::DescriptionRequest::Query(query.into())
            })
        },
        |search| webtest_analysis::DescriptionRequest::Search(search.into()),
    );
    Ok(database_with_manifest(manifest_json)?.describe(
        request,
        None,
        webtest_analysis::DescriptionLimits::default(),
    ))
}

fn database_with_manifest(manifest_json: &str) -> Result<AnalysisDatabase, String> {
    let manifest =
        AppManifest::from_json_normalized(manifest_json).map_err(|error| error.to_string())?;
    let mut providers = ProviderRegistry::built_in_schemas();
    providers.register_schema(manifest.provider_schema());
    Ok(AnalysisDatabase::with_provider_registry(providers))
}

pub fn completions(source: &str, byte_offset: u32) -> Vec<webtest_analysis::Completion> {
    completions_with_database(source, byte_offset, AnalysisDatabase::default())
}

pub fn completions_with_manifest(
    source: &str,
    byte_offset: u32,
    manifest_json: &str,
) -> Result<Vec<webtest_analysis::Completion>, String> {
    Ok(completions_with_database(
        source,
        byte_offset,
        database_with_manifest(manifest_json)?,
    ))
}

fn completions_with_database(
    source: &str,
    byte_offset: u32,
    mut database: AnalysisDatabase,
) -> Vec<webtest_analysis::Completion> {
    let file = database.open_file("memory://document.webtest", source);
    database
        .completions(
            file,
            webtest_text::TextSize::from(clamp_byte_offset(source, byte_offset)),
        )
        .unwrap_or_default()
}

pub fn signature_help(source: &str, byte_offset: u32) -> Option<webtest_analysis::Signature> {
    signature_help_with_database(source, byte_offset, AnalysisDatabase::default())
}

pub fn signature_help_with_manifest(
    source: &str,
    byte_offset: u32,
    manifest_json: &str,
) -> Result<Option<webtest_analysis::Signature>, String> {
    Ok(signature_help_with_database(
        source,
        byte_offset,
        database_with_manifest(manifest_json)?,
    ))
}

fn signature_help_with_database(
    source: &str,
    byte_offset: u32,
    mut database: AnalysisDatabase,
) -> Option<webtest_analysis::Signature> {
    let file = database.open_file("memory://document.webtest", source);
    database
        .signature_help(
            file,
            webtest_text::TextSize::from(clamp_byte_offset(source, byte_offset)),
        )
        .ok()
        .flatten()
}

pub fn hover(source: &str, byte_offset: u32) -> Option<PortableDocumentation> {
    hover_with_database(source, byte_offset, AnalysisDatabase::default())
}

pub fn hover_with_manifest(
    source: &str,
    byte_offset: u32,
    manifest_json: &str,
) -> Result<Option<PortableDocumentation>, String> {
    Ok(hover_with_database(
        source,
        byte_offset,
        database_with_manifest(manifest_json)?,
    ))
}

fn hover_with_database(
    source: &str,
    byte_offset: u32,
    mut database: AnalysisDatabase,
) -> Option<PortableDocumentation> {
    let file = database.open_file("memory://document.webtest", source);
    database
        .documentation_at(
            file,
            webtest_text::TextSize::from(clamp_byte_offset(source, byte_offset)),
        )
        .ok()
        .flatten()
        .map(|documentation| PortableDocumentation {
            start: documentation.range.start().into(),
            end: documentation.range.end().into(),
            contents: documentation.contents,
        })
}

fn clamp_byte_offset(source: &str, requested: u32) -> u32 {
    let mut offset = usize::try_from(requested)
        .unwrap_or(usize::MAX)
        .min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    u32::try_from(offset).unwrap_or(u32::MAX)
}

#[derive(Clone, Debug, Serialize)]
pub struct PortableHostCapability {
    supported: bool,
    code: &'static str,
    capability: &'static str,
    message: &'static str,
}

pub fn inspect_capability() -> PortableHostCapability {
    PortableHostCapability {
        supported: false,
        code: "unsupported_host_capability",
        capability: "native_browser",
        message: "semantic page inspection requires the native WebTest browser host",
    }
}

#[cfg(target_arch = "wasm32")]
mod exports {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(js_name = diagnostics)]
    pub fn diagnostics_js(source: &str) -> String {
        serde_json::to_string(&super::diagnostics(source)).unwrap_or_else(|_| "[]".into())
    }

    #[wasm_bindgen(js_name = diagnosticsWithAppSchema)]
    pub fn diagnostics_with_app_schema_js(source: &str, manifest: &str) -> String {
        serde_json::to_string(&super::diagnostics_with_manifest(source, manifest))
            .unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = format)]
    pub fn format_js(source: &str) -> String {
        super::format(source)
    }

    #[wasm_bindgen(js_name = compile)]
    pub fn compile_js(source: &str) -> String {
        serde_json::to_string(&super::compile(source)).unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = compileWithAppSchema)]
    pub fn compile_with_app_schema_js(source: &str, manifest: &str) -> String {
        serde_json::to_string(&super::compile_with_manifest(source, manifest))
            .unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = describe)]
    pub fn describe_js(query: Option<String>, search: Option<String>) -> String {
        serde_json::to_string(&super::describe(query.as_deref(), search.as_deref()))
            .unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = describeWithAppSchema)]
    pub fn describe_with_app_schema_js(
        query: Option<String>,
        search: Option<String>,
        manifest: &str,
    ) -> String {
        serde_json::to_string(&super::describe_with_manifest(
            query.as_deref(),
            search.as_deref(),
            manifest,
        ))
        .unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = completions)]
    pub fn completions_js(source: &str, byte_offset: u32) -> String {
        serde_json::to_string(&super::completions(source, byte_offset))
            .unwrap_or_else(|_| "[]".into())
    }

    #[wasm_bindgen(js_name = completionsWithAppSchema)]
    pub fn completions_with_app_schema_js(
        source: &str,
        byte_offset: u32,
        manifest: &str,
    ) -> String {
        serde_json::to_string(&super::completions_with_manifest(
            source,
            byte_offset,
            manifest,
        ))
        .unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = signatureHelp)]
    pub fn signature_help_js(source: &str, byte_offset: u32) -> String {
        serde_json::to_string(&super::signature_help(source, byte_offset))
            .unwrap_or_else(|_| "null".into())
    }

    #[wasm_bindgen(js_name = signatureHelpWithAppSchema)]
    pub fn signature_help_with_app_schema_js(
        source: &str,
        byte_offset: u32,
        manifest: &str,
    ) -> String {
        serde_json::to_string(&super::signature_help_with_manifest(
            source,
            byte_offset,
            manifest,
        ))
        .unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = hover)]
    pub fn hover_js(source: &str, byte_offset: u32) -> String {
        serde_json::to_string(&super::hover(source, byte_offset)).unwrap_or_else(|_| "null".into())
    }

    #[wasm_bindgen(js_name = hoverWithAppSchema)]
    pub fn hover_with_app_schema_js(source: &str, byte_offset: u32, manifest: &str) -> String {
        serde_json::to_string(&super::hover_with_manifest(source, byte_offset, manifest))
            .unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = inspectCapability)]
    pub fn inspect_capability_js() -> String {
        serde_json::to_string(&super::inspect_capability()).unwrap_or_else(|_| "{}".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_native_capability_nodes_but_marks_them_non_executable() {
        let result =
            compile("test \"x\" { server { let response = http.get(\"http://example.test\") } }");
        assert!(result.diagnostics.is_empty());
        assert!(result.plan.is_some());
        assert!(
            result
                .required_host_capabilities
                .contains(&Capability::Server)
        );
        assert!(!result.executable_on_wasm);
    }

    #[test]
    fn portable_and_native_plans_match_with_per_test_capabilities() {
        let source = r#"test "server" {
    server { let response = http.get("http://example.test") }
}
test "pure" { let value = 1 }
test "browser" { browser { open "/" } }
test "optional" {
    let value: { required: String, optional?: String } = { required: "yes" }
    expect value.optional == null
}"#;
        let portable = compile(source).plan.expect("portable plan");
        let mut database = AnalysisDatabase::default();
        let file = database.open_file("memory://document.webtest", source);
        let native = database.test_plan(file).expect("native plan");
        assert_eq!(portable, *native);
        assert_eq!(
            portable
                .tests
                .iter()
                .map(|test| test.required_host_capabilities.clone())
                .collect::<Vec<_>>(),
            [
                vec![Capability::Server],
                vec![],
                vec![Capability::Browser],
                vec![Capability::Test]
            ]
        );
        let webtest_plan::TestOperation::Assertion(webtest_plan::AssertionOperation::Value {
            actual,
            ..
        }) = &portable.tests[3].steps()[1].operation
        else {
            panic!("optional assertion")
        };
        assert!(matches!(
            actual,
            webtest_plan::PlanExpr::Member {
                missing_is_null: true,
                ..
            }
        ));
    }

    #[test]
    fn descriptions_and_static_repair_diagnostics_match_the_native_core() {
        for query in [
            "locator.role",
            "browser.fill",
            "browser.wait.locator",
            "assertion.value",
            "type.Int",
            "type.Option",
            "type.Record",
            "json.typed_decode",
        ] {
            let portable =
                serde_json::to_value(describe(Some(query), None)).expect("portable description");
            let native = serde_json::to_value(AnalysisDatabase::default().describe(
                webtest_analysis::DescriptionRequest::Query(query.into()),
                None,
                webtest_analysis::DescriptionLimits::default(),
            ))
            .expect("native description");
            assert_eq!(portable, native, "{query}");
        }

        let source = r#"test "x" {
    server {
        let user: { email: String } = { email: "a@example.test" }
        let value = user.emial
    }
}"#;
        let diagnostic = diagnostics(source)
            .into_iter()
            .find(|diagnostic| diagnostic.code == "semantic.unknown_member")
            .expect("unknown member diagnostic");
        assert_eq!(diagnostic.diagnostic_schema_version, 2);
        assert_eq!(diagnostic.repair_hint_schema_version, 1);
        assert_eq!(
            diagnostic.semantic_details.expect("details")["requested"],
            "emial"
        );
        assert_eq!(
            diagnostic.repair_hints[0].replacement,
            webtest_feedback::RepairReplacement::text("email")
        );
        assert!(diagnostic.reference_queries.contains(&"type.Record".into()));
        assert_eq!(diagnostic.source_revision.len(), 64);
        assert!(diagnostic.end > diagnostic.start);
    }

    #[test]
    fn native_inspection_is_explicitly_unsupported_on_the_portable_facade() {
        let capability = serde_json::to_value(inspect_capability()).expect("capability JSON");
        assert_eq!(capability["supported"], false);
        assert_eq!(capability["code"], "unsupported_host_capability");
        assert_eq!(capability["capability"], "native_browser");
    }

    #[test]
    fn offline_app_manifest_has_native_wasm_analysis_and_description_parity() {
        let manifest = include_str!("../../../protocol/examples/app-schema.json");
        let source =
            r#"test "x" { server { let user = app.create_user(email: "a@example.com") } }"#;
        let diagnostics = diagnostics_with_manifest(source, manifest).expect("diagnostics");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let compilation = compile_with_manifest(source, manifest).expect("compilation");
        let plan = compilation.plan.expect("portable plan");
        let webtest_plan::TestOperation::ServerProviderCall(call) =
            &plan.tests[0].steps()[0].operation
        else {
            panic!("app call")
        };
        assert_eq!(
            call.schema_hash,
            "blake3:b1254e79ab8984797e49f26190f9fa181239cb0d4c0d279f4d627b7d101e1e2a"
        );
        let description = describe_with_manifest(Some("provider.app.create_user"), None, manifest)
            .expect("description");
        let webtest_analysis::DescriptionResponse::Construct(construct) = description else {
            panic!("construct")
        };
        assert_eq!(construct.name, "app.create_user");
        assert_eq!(construct.retry_safe, Some(false));
        assert_eq!(
            construct.parameters[0].documentation,
            "Grant administrative access."
        );
        assert_eq!(
            construct.parameters[0].default,
            Some(serde_json::json!(false))
        );

        let completion_source = r#"test "x" { server { let user = app. } }"#;
        let completion_offset = completion_source.find("app.").expect("app") as u32 + 4;
        let completions = completions_with_manifest(completion_source, completion_offset, manifest)
            .expect("completions");
        assert!(
            completions
                .iter()
                .any(|completion| completion.label == "create_user")
        );
        let member_source = r#"test "x" { server {
    let user = app.create_user(email: "a@example.com")
    let email = user.
} }"#;
        let member_offset = member_source.find("user.").expect("member") as u32 + 5;
        let members = completions_with_manifest(member_source, member_offset, manifest)
            .expect("member completions");
        assert!(members.iter().any(|completion| completion.label == "email"));

        let call_offset = source.find("email:").expect("argument") as u32;
        let signature = signature_help_with_manifest(source, call_offset, manifest)
            .expect("signature help")
            .expect("signature");
        assert!(signature.label.starts_with("app.create_user("));
        let documentation = hover_with_manifest(
            source,
            source.find("create_user").expect("operation") as u32,
            manifest,
        )
        .expect("hover")
        .expect("documentation");
        assert!(documentation.contents.contains("Create a user"));
    }
}
