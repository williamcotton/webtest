//! Stable DTO-oriented facade for browser/editor builds of the language core.

use serde::Serialize;
use webtest_analysis::AnalysisDatabase;

#[derive(Serialize)]
pub struct PortableDiagnostic {
    start: u32,
    end: u32,
    code: &'static str,
    message: String,
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
}
