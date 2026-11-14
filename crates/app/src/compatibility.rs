//! Composition of authoritative version constants for release/support tooling.
pub(crate) fn manifest() -> serde_json::Value {
    serde_json::json!({
        "compatibility_schema": 1,
        "webtest": env!("CARGO_PKG_VERSION"),
        "target": env!("WEBTEST_BUILD_TARGET"),
        "build_commit": env!("WEBTEST_BUILD_COMMIT"),
        "plan_format": webtest_plan::PLAN_FORMAT_VERSION,
        "runtime_semantics": webtest_plan::RUNTIME_SEMANTICS_VERSION,
        "event_schema": crate::report::REPORT_SCHEMA_VERSION,
        "native_journal": webtest_observation::EVENT_JOURNAL_SCHEMA_VERSION,
        "trace_format": webtest_trace::TRACE_FORMAT_VERSION,
        "observation_ipc": webtest_observation::OBSERVATION_IPC_VERSION,
        "inspection_schema": webtest_browser::INSPECTION_SCHEMA_VERSION,
        "description_schema": webtest_analysis::DESCRIPTION_SCHEMA_VERSION,
        "diagnostic_schema": webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
        "repair_hint_schema": webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
        "app_bridge_protocol": [webtest_app_bridge::PROTOCOL_VERSION],
        "wasm_editor_api": webtest_wasm::EDITOR_API_VERSION,
        "worker_protocol": webtest_wasm::WORKER_PROTOCOL_VERSION,
        "vscode_extension": ">=0.2.0 <0.3.0",
        "chrome_for_testing": webtest_browser_manager::TESTED_CHROME_VERSION
    })
}
