//! Reproducible in-memory host for the checked-in workspace benchmark manifest.
//! Queries receive explicit inputs; only this executable reads time/configuration.
use std::time::Instant;
use webtest_analysis::AnalysisDatabase;
use webtest_text::TextSize;

fn main() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../benchmarks/workspace/workspace.json"))
            .expect("benchmark manifest");
    let count = manifest["files"].as_u64().unwrap() as usize;
    let declarations = manifest["declarations_per_file"].as_u64().unwrap() as usize;
    assert_eq!(declarations, 5);
    let mut db = AnalysisDatabase::default();
    db.configure_workspace(webtest_analysis::WorkspaceConfiguration {
        root: "/benchmark".into(),
        ..Default::default()
    });
    let mut files = Vec::new();
    for index in 0..count {
        let mut source = if index > 0 {
            format!(
                "import {{ helper as previous_helper, fixture_value as previous_fixture }} from \"./file{:04}.webtest\"\n",
                index - 1
            )
        } else {
            String::new()
        };
        let helper = if index % 8 == 0 {
            "value"
        } else {
            "previous_helper(value) + 1"
        };
        let setup = if index % 8 == 0 {
            "provide 7"
        } else {
            "let seed = use previous_fixture() provide seed + 1"
        };
        source.push_str(&format!("export fn helper(value: Int) -> Int = {helper}\nexport fixture fixture_value() -> Int scope suite shareable {{ setup {{ {setup} }} teardown {{ expect 1 == 1 }} }}\n"));
        source.push_str(&format!("test \"helper {index}\" {{ let value = helper({index}) expect value == {} }}\ntest \"fixture {index}\" {{ let seed = use fixture_value() expect seed == {} }}\n", index + index % 8, 7 + index % 8));
        if index % 97 == 0 {
            source.push_str("test \"half typed\" { let value =\n");
        } else {
            source.push_str("test \"provider\" { server { let response = http.get(\"https://example.test\") expect response.status == 200 } }\n");
        }
        files.push(db.open_file(format!("/benchmark/file{index:04}.webtest"), source));
    }
    let started = Instant::now();
    let graph_started = Instant::now();
    for file in &files {
        db.module_graph(*file).unwrap();
    }
    let graph_ms = graph_started.elapsed().as_secs_f64() * 1000.0;
    let mut diagnostics = 0;
    for file in &files {
        diagnostics += db.diagnostics(*file).unwrap().len();
    }
    let initial_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut completion_us = Vec::new();
    for index in 0..1000 {
        let file = files[index % count];
        let length = db.source(file).unwrap().len() as u32;
        let started = Instant::now();
        let _ = db.completions(file, TextSize::from(length)).unwrap();
        completion_us.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    completion_us.sort_by(f64::total_cmp);
    let unrelated = db.test_plan(files[1]).unwrap();
    let dependent = db.test_plan(files[9]).unwrap();
    let edit_file = files[8];
    let leaf_source = db.source(edit_file).unwrap();
    let before_edits = db.query_counts();
    let started = Instant::now();
    for revision in 0..100 {
        let file = edit_file;
        db.set_file_text(
            file,
            leaf_source.replace(
                "export fn helper(value: Int) -> Int = value",
                &format!("export fn helper(value: Int) -> Int = value + {revision}"),
            ),
        );
        db.diagnostics(file).unwrap();
    }
    let edit_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(std::sync::Arc::ptr_eq(
        &unrelated,
        &db.test_plan(files[1]).unwrap()
    ));
    let after_edits = db.query_counts();
    assert_eq!(after_edits.parse - before_edits.parse, 100);
    assert_eq!(after_edits.hir - before_edits.hir, 100);
    assert_eq!(after_edits.compile - before_edits.compile, 100);
    assert!(!std::sync::Arc::ptr_eq(
        &dependent,
        &db.test_plan(files[9]).unwrap()
    ));
    let after_dependent = db.query_counts();
    assert_eq!(after_dependent.parse, after_edits.parse);
    assert_eq!(after_dependent.hir, after_edits.hir);
    assert_eq!(after_dependent.compile, after_edits.compile + 1);
    if std::env::args().any(|argument| argument == "--check") {
        let budgets: serde_json::Value =
            serde_json::from_str(include_str!("../../../benchmarks/workspace/budgets.json"))
                .unwrap();
        for (name, observed) in [
            ("initial_ms", initial_ms),
            ("completion_p95_us", completion_us[950]),
            ("hundred_edits_ms", edit_ms),
        ] {
            assert!(
                observed <= budgets[name].as_f64().unwrap(),
                "{name} exceeded its budget: {observed}"
            );
        }
    }
    println!(
        "{}",
        serde_json::json!({ "files": count, "declarations": count * declarations, "diagnostics": diagnostics, "initial_ms": initial_ms, "graph_ms": graph_ms, "completion_p95_us": completion_us[950], "hundred_edits_ms": edit_ms, "profile": if cfg!(debug_assertions) { "debug" } else { "release" } })
    );
}
