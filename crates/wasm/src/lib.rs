//! Stable DTO-oriented facade for browser/editor builds of the language core.

use serde::Serialize;
use webtest_analysis::{AnalysisDatabase, DiagnosticSeverity};
use webtest_plan::TestPlan;
use webtest_provider::Capability;

#[derive(Clone, Debug, Serialize)]
pub struct PortableDiagnostic {
    start: u32,
    end: u32,
    code: &'static str,
    message: String,
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
                .map(|diagnostic| PortableDiagnostic {
                    start: diagnostic.range.start().into(),
                    end: diagnostic.range.end().into(),
                    code: diagnostic.code,
                    message: diagnostic.message.clone(),
                })
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
                .map(|diagnostic| PortableDiagnostic {
                    start: diagnostic.range.start().into(),
                    end: diagnostic.range.end().into(),
                    code: diagnostic.code,
                    message: diagnostic.message.clone(),
                })
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
}
