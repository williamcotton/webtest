//! Public diagnostics and shared deterministic repair/reference primitives.

use webtest_feedback::{ByteRange, RepairHint, RepairHintKind};
use webtest_provider::Type;
use webtest_text::TextRange;

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

pub(crate) fn text_hints(
    kind: RepairHintKind,
    candidates: Vec<String>,
    range: TextRange,
) -> Vec<RepairHint> {
    candidates
        .into_iter()
        .map(|candidate| {
            let mut hint = RepairHint::text(kind, candidate);
            hint.source_range = Some(ByteRange {
                start: range.start().into(),
                end: range.end().into(),
            });
            hint
        })
        .collect()
}

pub(crate) fn nearest_strings(values: &[String], requested: &str, limit: usize) -> Vec<String> {
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

pub(crate) fn type_reference_name(ty: &Type) -> &'static str {
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

pub(crate) fn default_reference_queries(code: &str) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_feedback::RepairReplacement;
    use webtest_text::TextSize;

    #[test]
    fn nearest_strings_use_unicode_scalars_deterministic_ties_and_bounds() {
        let values = vec![
            "éclair".into(),
            "éclat".into(),
            "école".into(),
            "élan".into(),
            "étude".into(),
            "éveil".into(),
        ];
        let first = nearest_strings(&values, "éclai", 5);
        assert_eq!(first.len(), 5);
        assert_eq!(first, nearest_strings(&values, "éclai", 5));
        assert_eq!(first.first().map(String::as_str), Some("éclair"));
    }

    #[test]
    fn text_hints_preserve_replacement_ranges() {
        let range = TextRange::new(TextSize::from(3), TextSize::from(7));
        let hints = text_hints(RepairHintKind::NameCandidate, vec!["name".into()], range);
        assert_eq!(hints[0].replacement, RepairReplacement::text("name"));
        assert_eq!(hints[0].source_range, Some(ByteRange { start: 3, end: 7 }));
    }
}
