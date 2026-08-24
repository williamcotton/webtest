//! Stable DTO-oriented facade for browser/editor builds of the language core.

use serde::Serialize;
use webtest_analysis::{AnalysisDatabase, DiagnosticSeverity};
use webtest_plan::TestPlan;
use webtest_provider::Capability;

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

pub fn diagnostics(source: &str) -> Vec<PortableDiagnostic> {
    let mut database = AnalysisDatabase::default();
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
    let mut database = AnalysisDatabase::default();
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

    #[wasm_bindgen(js_name = format)]
    pub fn format_js(source: &str) -> String {
        super::format(source)
    }

    #[wasm_bindgen(js_name = compile)]
    pub fn compile_js(source: &str) -> String {
        serde_json::to_string(&super::compile(source)).unwrap_or_else(|_| "{}".into())
    }

    #[wasm_bindgen(js_name = describe)]
    pub fn describe_js(query: Option<String>, search: Option<String>) -> String {
        serde_json::to_string(&super::describe(query.as_deref(), search.as_deref()))
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
    fn descriptions_and_static_repair_diagnostics_match_the_native_core() {
        let portable = serde_json::to_value(describe(Some("locator.role"), None))
            .expect("portable description");
        let native = serde_json::to_value(AnalysisDatabase::default().describe(
            webtest_analysis::DescriptionRequest::Query("locator.role".into()),
            None,
            webtest_analysis::DescriptionLimits::default(),
        ))
        .expect("native description");
        assert_eq!(portable, native);

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
        assert_eq!(diagnostic.diagnostic_schema_version, 1);
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
}
