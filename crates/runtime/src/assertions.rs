use std::collections::HashMap;

use webtest_browser::Page;
use webtest_hir::BindingId;
use webtest_observation::ValueDiff;
use webtest_plan::{AssertionOperation, PlanExpr, ValueMatcher};
use webtest_provider::Value;

use crate::{
    AssertionFailure, RunnerOptions, StepError,
    evaluation::{
        compare_values, decode_value, display_value, evaluate, string_value, value_contains,
        values_equal,
    },
    execution::{bounded_timeout, browser_locator, browser_state},
    url::resolve_browser_url,
};

pub(crate) async fn execute_assertion(
    page: Option<&mut (dyn Page + '_)>,
    assertion: &AssertionOperation,
    environment: &HashMap<BindingId, Value>,
    options: &RunnerOptions,
) -> Result<(), StepError> {
    match assertion {
        AssertionOperation::Locator {
            locator,
            state,
            timeout,
        } => page
            .ok_or_else(|| StepError::Internal("locator assertion has no browser page".into()))?
            .wait_for_locator(
                &browser_locator(locator),
                browser_state(*state),
                bounded_timeout(
                    timeout.unwrap_or(options.assertion_timeout),
                    options.test_timeout,
                ),
            )
            .await
            .map_err(StepError::Browser),
        AssertionOperation::Url { url, timeout } => {
            let url = string_value(evaluate(url, environment)?)?;
            let expected = resolve_browser_url(options.base_url.as_deref(), &url)
                .map_err(StepError::Browser)?;
            page.ok_or_else(|| StepError::Internal("URL assertion has no browser page".into()))?
                .wait_for_url(
                    &expected,
                    bounded_timeout(
                        timeout.unwrap_or(options.assertion_timeout),
                        options.test_timeout,
                    ),
                )
                .await
                .map_err(StepError::Browser)
        }
        AssertionOperation::Value {
            matcher,
            actual,
            expected,
            ..
        } => {
            let actual = evaluate(actual, environment)?;
            if *matcher == ValueMatcher::Matches {
                let expected_type = expected
                    .as_ref()
                    .and_then(|expression| match expression {
                        PlanExpr::Type(ty) => Some(ty),
                        _ => None,
                    })
                    .ok_or_else(|| StepError::Internal("matches assertion has no type".into()))?;
                decode_value(&actual, expected_type, "$", None)
                    .map(|_| ())
                    .map_err(StepError::Decode)
            } else {
                let expected = expected
                    .as_ref()
                    .map(|expected| evaluate(expected, environment))
                    .transpose()?;
                if assertion_matches(*matcher, &actual, expected.as_ref()) {
                    Ok(())
                } else {
                    Err(StepError::Assertion(Box::new(AssertionFailure {
                        matcher: *matcher,
                        message: assertion_message(*matcher, &actual, expected.as_ref()),
                        diff: value_diff(*matcher, &actual, expected.as_ref()),
                        expected,
                        actual,
                    })))
                }
            }
        }
    }
}

fn assertion_matches(matcher: ValueMatcher, actual: &Value, expected: Option<&Value>) -> bool {
    match matcher {
        ValueMatcher::Truthy => matches!(actual, Value::Bool(true)),
        ValueMatcher::Equal => expected.is_some_and(|expected| values_equal(actual, expected)),
        ValueMatcher::NotEqual => expected.is_some_and(|expected| !values_equal(actual, expected)),
        ValueMatcher::Less => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering < 0),
        ValueMatcher::LessEqual => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering <= 0),
        ValueMatcher::Greater => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering > 0),
        ValueMatcher::GreaterEqual => expected
            .and_then(|expected| compare_values(actual, expected))
            .is_some_and(|ordering| ordering >= 0),
        ValueMatcher::Contains => expected.is_some_and(|expected| value_contains(actual, expected)),
        ValueMatcher::Matches => false,
    }
}

pub(crate) fn assertion_message(
    matcher: ValueMatcher,
    actual: &Value,
    expected: Option<&Value>,
) -> String {
    match expected {
        Some(expected) => format!(
            "assertion {matcher:?} failed: expected {}, got {}",
            bounded_display_value(expected),
            bounded_display_value(actual)
        ),
        None => format!(
            "assertion {matcher:?} failed for {}",
            bounded_display_value(actual)
        ),
    }
}

pub(crate) fn value_diff(
    matcher: ValueMatcher,
    actual: &Value,
    expected: Option<&Value>,
) -> ValueDiff {
    if matcher == ValueMatcher::Contains {
        return ValueDiff::Contains {
            expected_item: expected.map(bounded_display_value).unwrap_or_default(),
            actual: bounded_display_value(actual),
        };
    }
    if matcher == ValueMatcher::Equal {
        match (actual, expected) {
            (Value::String(actual), Some(Value::String(expected))) => {
                let actual_chars: Vec<_> = actual.chars().collect();
                let expected_chars: Vec<_> = expected.chars().collect();
                let common_prefix_chars = actual_chars
                    .iter()
                    .zip(&expected_chars)
                    .take_while(|(actual, expected)| actual == expected)
                    .count();
                return ValueDiff::String {
                    common_prefix_chars,
                    expected_segment: bounded_char_segment(&expected_chars, common_prefix_chars),
                    actual_segment: bounded_char_segment(&actual_chars, common_prefix_chars),
                };
            }
            (Value::List(actual), Some(Value::List(expected))) => {
                let common = actual.len().min(expected.len());
                let mut differing_indices: Vec<_> = (0..common)
                    .filter(|index| !values_equal(&actual[*index], &expected[*index]))
                    .take(20)
                    .collect();
                differing_indices.extend(
                    (common..actual.len().max(expected.len()))
                        .take(20usize.saturating_sub(differing_indices.len())),
                );
                return ValueDiff::List {
                    expected_len: expected.len(),
                    actual_len: actual.len(),
                    differing_indices,
                };
            }
            (Value::Record(actual), Some(Value::Record(expected))) => {
                let missing_fields = expected
                    .keys()
                    .filter(|name| !actual.contains_key(*name))
                    .take(20)
                    .cloned()
                    .collect();
                let unexpected_fields = actual
                    .keys()
                    .filter(|name| !expected.contains_key(*name))
                    .take(20)
                    .cloned()
                    .collect();
                let mismatched_fields = expected
                    .iter()
                    .filter(|(name, expected)| {
                        actual
                            .get(*name)
                            .is_some_and(|actual| !values_equal(actual, expected))
                    })
                    .map(|(name, _)| name.clone())
                    .take(20)
                    .collect();
                return ValueDiff::Record {
                    missing_fields,
                    unexpected_fields,
                    mismatched_fields,
                };
            }
            _ => {}
        }
    }
    ValueDiff::Scalar {
        expected: expected.map(bounded_display_value),
        actual: bounded_display_value(actual),
    }
}

fn bounded_char_segment(characters: &[char], difference: usize) -> String {
    const CONTEXT: usize = 24;
    const LIMIT: usize = 80;
    let start = difference.saturating_sub(CONTEXT);
    let mut segment: String = characters.iter().skip(start).take(LIMIT).collect();
    if start > 0 {
        segment.insert_str(0, "...");
    }
    if start + LIMIT < characters.len() {
        segment.push_str("...");
    }
    segment
}

fn bounded_display_value(value: &Value) -> String {
    const LIMIT: usize = 240;
    let value = display_value(value);
    let mut characters = value.chars();
    let mut bounded: String = characters.by_ref().take(LIMIT).collect();
    if characters.next().is_some() {
        bounded.push_str("...");
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assertion_diffs_are_bounded_structural_and_unicode_safe() {
        let string = value_diff(
            ValueMatcher::Equal,
            &Value::String("prefix-β-actual".into()),
            Some(&Value::String("prefix-β-expected".into())),
        );
        assert!(matches!(
            string,
            ValueDiff::String {
                common_prefix_chars: 9,
                ..
            }
        ));

        let record = value_diff(
            ValueMatcher::Equal,
            &Value::Record(
                [
                    ("id".into(), Value::String("wrong".into())),
                    ("extra".into(), Value::Bool(true)),
                ]
                .into_iter()
                .collect(),
            ),
            Some(&Value::Record(
                [
                    ("id".into(), Value::Int(7)),
                    ("email".into(), Value::String("a@example.test".into())),
                ]
                .into_iter()
                .collect(),
            )),
        );
        assert_eq!(
            record,
            ValueDiff::Record {
                missing_fields: vec!["email".into()],
                unexpected_fields: vec!["extra".into()],
                mismatched_fields: vec!["id".into()],
            }
        );
    }

    #[test]
    fn all_value_matchers_preserve_current_semantics() {
        assert!(assertion_matches(
            ValueMatcher::Truthy,
            &Value::Bool(true),
            None
        ));
        assert!(assertion_matches(
            ValueMatcher::Equal,
            &Value::Int(2),
            Some(&Value::Float(2.0))
        ));
        assert!(assertion_matches(
            ValueMatcher::NotEqual,
            &Value::Int(2),
            Some(&Value::Int(3))
        ));
        assert!(assertion_matches(
            ValueMatcher::Less,
            &Value::Int(2),
            Some(&Value::Int(3))
        ));
        assert!(assertion_matches(
            ValueMatcher::LessEqual,
            &Value::Int(2),
            Some(&Value::Int(2))
        ));
        assert!(assertion_matches(
            ValueMatcher::Greater,
            &Value::String("b".into()),
            Some(&Value::String("a".into()))
        ));
        assert!(assertion_matches(
            ValueMatcher::GreaterEqual,
            &Value::Int(2),
            Some(&Value::Int(2))
        ));
        assert!(assertion_matches(
            ValueMatcher::Contains,
            &Value::List(vec![Value::Int(1), Value::Float(2.0)]),
            Some(&Value::Int(2))
        ));
        assert!(!assertion_matches(
            ValueMatcher::Matches,
            &Value::Int(2),
            None
        ));
    }

    #[test]
    fn list_and_record_diff_collections_are_bounded() {
        let list = value_diff(
            ValueMatcher::Equal,
            &Value::List((0..100).map(Value::Int).collect()),
            Some(&Value::List((100..200).map(Value::Int).collect())),
        );
        let ValueDiff::List {
            differing_indices, ..
        } = list
        else {
            panic!("list diff")
        };
        assert_eq!(differing_indices.len(), 20);

        let actual = Value::Record(
            (0..50)
                .map(|index| (format!("actual-{index}"), Value::Int(index)))
                .collect(),
        );
        let expected = Value::Record(
            (0..50)
                .map(|index| (format!("expected-{index}"), Value::Int(index)))
                .collect(),
        );
        let ValueDiff::Record {
            missing_fields,
            unexpected_fields,
            mismatched_fields,
        } = value_diff(ValueMatcher::Equal, &actual, Some(&expected))
        else {
            panic!("record diff")
        };
        assert_eq!(missing_fields.len(), 20);
        assert_eq!(unexpected_fields.len(), 20);
        assert!(mismatched_fields.is_empty());
    }
}
