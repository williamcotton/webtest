//! Explicit in-memory Milestone H performance fixture; no browser or providers.
use std::{collections::HashMap, time::Instant};
use webtest_analysis::AnalysisDatabase;
use webtest_model::Value;
use webtest_plan::pattern::Matcher;
fn main() {
    let rows = (0..1000)
        .map(|n| format!("case \"row-{n}\" {{value: {n}}}"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        "pattern Row = subset {{value: range(0, 1000)}} test \"rows\" cases [{rows}] as input {{expect input matches Row}}"
    );
    let start = Instant::now();
    let mut db = AnalysisDatabase::default();
    let file = db.open_file("benchmark.webtest", source);
    assert!(db.diagnostics(file).unwrap().is_empty());
    let plan = db.test_plan(file).unwrap();
    plan.validate_tree().unwrap();
    let compile = start.elapsed();
    let matcher = Matcher::Each {
        item: Box::new(Matcher::Regex {
            source: "^[a-z]+$".into(),
        }),
    };
    let values = Value::List(vec![Value::String("hello".into()); 10000]);
    let start = Instant::now();
    assert!(
        matcher
            .matches(&values, &HashMap::new())
            .unwrap()
            .mismatches
            .is_empty()
    );
    println!(
        "{}",
        serde_json::json!({"variants":plan.tests.len(),"compile_ms":compile.as_secs_f64()*1000.0,"regex_values":10000,"match_ms":start.elapsed().as_secs_f64()*1000.0,"profile":if cfg!(debug_assertions){"debug"}else{"release"}})
    );
}
