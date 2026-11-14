//! Bounded, deterministic structural patterns. No provider/browser or source interpretation.
use crate::{
    PlanExpr,
    evaluation::{self, PureEvaluationError},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use webtest_model::{PatternId, Type, Value};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Matcher {
    Any,
    Literal {
        value: PlanExpr,
    },
    Type {
        ty: Type,
    },
    Record {
        exact: bool,
        fields: BTreeMap<String, Matcher>,
    },
    List {
        items: Vec<Matcher>,
    },
    Each {
        item: Box<Matcher>,
    },
    Contains {
        item: Box<Matcher>,
    },
    Optional {
        item: Box<Matcher>,
    },
    Absent,
    Regex {
        source: String,
    },
    Range {
        lower: PlanExpr,
        upper: PlanExpr,
    },
    OneOf {
        items: Vec<Matcher>,
    },
    AllOf {
        items: Vec<Matcher>,
    },
    Not {
        item: Box<Matcher>,
    },
    Reference {
        id: PatternId,
        name: String,
        body: Box<Matcher>,
    },
}
pub use webtest_model::{MatchReport, MismatchReason, PatternMismatch};
pub const MAX_PATTERN_DEPTH: usize = 64;
pub const MAX_PATTERN_VISITS: usize = 100_000;
pub fn validate_regex(source: &str) -> Result<regex::Regex, String> {
    if source.len() > 4096 {
        return Err("regex source exceeds 4096 bytes".into());
    }
    regex::RegexBuilder::new(source)
        .unicode(true)
        .size_limit(262144)
        .dfa_size_limit(262144)
        .build()
        .map_err(|e| e.to_string())
}
struct MatchContext<'a> {
    environment: &'a dyn evaluation::EvaluationBindings,
    budget: usize,
    regexes: HashMap<String, regex::Regex>,
}
impl Matcher {
    pub fn expressions(&self) -> Vec<&PlanExpr> {
        match self {
            Self::Literal { value } => vec![value],
            Self::Range { lower, upper } => vec![lower, upper],
            Self::Record { fields, .. } => fields.values().flat_map(Self::expressions).collect(),
            Self::List { items } | Self::OneOf { items } | Self::AllOf { items } => {
                items.iter().flat_map(Self::expressions).collect()
            }
            Self::Each { item }
            | Self::Contains { item }
            | Self::Optional { item }
            | Self::Not { item }
            | Self::Reference { body: item, .. } => item.expressions(),
            _ => Vec::new(),
        }
    }
    pub fn expressions_mut(&mut self) -> Vec<&mut PlanExpr> {
        match self {
            Self::Literal { value } => vec![value],
            Self::Range { lower, upper } => vec![lower, upper],
            Self::Record { fields, .. } => fields
                .values_mut()
                .flat_map(Self::expressions_mut)
                .collect(),
            Self::List { items } | Self::OneOf { items } | Self::AllOf { items } => {
                items.iter_mut().flat_map(Self::expressions_mut).collect()
            }
            Self::Each { item }
            | Self::Contains { item }
            | Self::Optional { item }
            | Self::Not { item }
            | Self::Reference { body: item, .. } => item.expressions_mut(),
            _ => Vec::new(),
        }
    }
    pub fn matches(
        &self,
        value: &Value,
        environment: &dyn evaluation::EvaluationBindings,
    ) -> Result<MatchReport, PureEvaluationError> {
        let mut report = MatchReport::default();
        let mut context = MatchContext {
            environment,
            budget: MAX_PATTERN_VISITS,
            regexes: HashMap::new(),
        };
        self.visit(value, &mut context, "$", 0, &mut report)?;
        Ok(report)
    }
    fn visit(
        &self,
        value: &Value,
        context: &mut MatchContext<'_>,
        path: &str,
        depth: usize,
        report: &mut MatchReport,
    ) -> Result<(), PureEvaluationError> {
        if depth > MAX_PATTERN_DEPTH || context.budget == 0 {
            return Err(limit("pattern evaluation budget exceeded"));
        }
        context.budget -= 1;
        let mut mismatch = |reason| {
            if report.mismatches.len() < 32 {
                report.mismatches.push(PatternMismatch {
                    path: path.chars().take(512).collect(),
                    reason,
                });
            } else {
                report.truncated = true;
            }
        };
        match self {
            Self::Any => (),
            Self::Literal { value: expected } => {
                if !evaluation::values_equal(
                    value,
                    &evaluation::evaluate(expected, context.environment)?,
                ) {
                    mismatch(MismatchReason::WrongLiteral)
                }
            }
            Self::Type { ty } => {
                if evaluation::decode_value(value, ty, path, None).is_err() {
                    mismatch(MismatchReason::WrongType)
                }
            }
            Self::Reference { body, .. } | Self::Optional { item: body } => {
                body.visit(value, context, path, depth + 1, report)?
            }
            Self::Absent => mismatch(MismatchReason::ExpectedAbsent),
            Self::Record { exact, fields } => {
                if let Value::Record(values) = value {
                    for (name, matcher) in fields {
                        let path = format!("{path}.{name}");
                        match (values.get(name), matcher) {
                            (None, Self::Optional { .. } | Self::Absent) => (),
                            (None, _) => push(report, &path, MismatchReason::MissingField),
                            (Some(_), Self::Absent) => {
                                push(report, &path, MismatchReason::ExpectedAbsent)
                            }
                            (Some(value), _) => {
                                matcher.visit(value, context, &path, depth + 1, report)?
                            }
                        }
                    }
                    if *exact {
                        for name in values.keys().filter(|name| !fields.contains_key(*name)) {
                            push(
                                report,
                                &format!("{path}.{name}"),
                                MismatchReason::UnexpectedField,
                            );
                        }
                    }
                } else {
                    mismatch(MismatchReason::WrongType)
                }
            }
            Self::List { items } => {
                if let Value::List(values) = value {
                    if items.len() != values.len() {
                        mismatch(MismatchReason::WrongLength)
                    }
                    for (i, (matcher, value)) in items.iter().zip(values).enumerate() {
                        matcher.visit(
                            value,
                            context,
                            &format!("{path}[{i}]"),
                            depth + 1,
                            report,
                        )?;
                    }
                } else {
                    mismatch(MismatchReason::WrongType)
                }
            }
            Self::Each { item } => {
                if let Value::List(values) = value {
                    for (i, value) in values.iter().enumerate() {
                        item.visit(value, context, &format!("{path}[{i}]"), depth + 1, report)?;
                    }
                } else {
                    mismatch(MismatchReason::WrongType)
                }
            }
            Self::Contains { item } => match value {
                Value::List(values) => {
                    let mut found = false;
                    for value in values {
                        let mut child = MatchReport::default();
                        item.visit(value, context, path, depth + 1, &mut child)?;
                        if child.mismatches.is_empty() {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        mismatch(MismatchReason::NoContainedItem)
                    }
                }
                Value::String(text) => {
                    if let Self::Literal { value: expected } = item.as_ref() {
                        match evaluation::evaluate(expected, context.environment)? {
                            Value::String(expected) if text.contains(&expected) => (),
                            _ => mismatch(MismatchReason::NoContainedItem),
                        }
                    } else {
                        mismatch(MismatchReason::WrongType)
                    }
                }
                _ => mismatch(MismatchReason::WrongType),
            },
            Self::Regex { source } => {
                if let Value::String(text) = value {
                    if text.len() > 262144 {
                        return Err(limit("regex input exceeds 262144 bytes"));
                    }
                    if !context.regexes.contains_key(source) {
                        if context.regexes.len() >= 64 {
                            return Err(limit("pattern exceeds 64 compiled regular expressions"));
                        }
                        context.regexes.insert(
                            source.clone(),
                            validate_regex(source).map_err(PureEvaluationError::Internal)?,
                        );
                    }
                    if !context
                        .regexes
                        .get(source)
                        .is_some_and(|regex| regex.is_match(text))
                    {
                        mismatch(MismatchReason::RegexMismatch)
                    }
                } else {
                    mismatch(MismatchReason::WrongType)
                }
            }
            Self::Range { lower, upper } => {
                let lower = evaluation::evaluate(lower, context.environment)?;
                let upper = evaluation::evaluate(upper, context.environment)?;
                let compatible = matches!(
                    (value, &lower, &upper),
                    (
                        Value::DurationMillis(_),
                        Value::DurationMillis(_),
                        Value::DurationMillis(_)
                    ) | (
                        Value::Int(_) | Value::Float(_),
                        Value::Int(_) | Value::Float(_),
                        Value::Int(_) | Value::Float(_)
                    )
                );
                if !compatible {
                    mismatch(MismatchReason::WrongType);
                    return Ok(());
                }
                let ordered = |a: &Value, b: &Value| match (a, b) {
                    (Value::DurationMillis(a), Value::DurationMillis(b)) => Ok(Some(a.cmp(b))),
                    _ => evaluation::compare_values(a, b),
                };
                if !ordered(value, &lower)?.is_some_and(|x| x.is_ge())
                    || !ordered(value, &upper)?.is_some_and(|x| x.is_le())
                {
                    mismatch(MismatchReason::OutOfRange)
                }
            }
            Self::OneOf { items } => {
                let mut found = false;
                let mut alternatives = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let mut child = MatchReport::default();
                    item.visit(value, context, path, depth + 1, &mut child)?;
                    if child.mismatches.is_empty() {
                        found = true;
                        break;
                    }
                    if alternatives.len() < 8 {
                        let truncated = child.truncated || child.mismatches.len() > 4;
                        alternatives.push(webtest_model::PatternAlternative {
                            index,
                            mismatches: child.mismatches.into_iter().take(4).collect(),
                            truncated,
                        });
                    }
                }
                if !found {
                    mismatch(MismatchReason::NoAlternative);
                    for alternative in alternatives {
                        if report.alternatives.len() < 8 {
                            report.alternatives.push(alternative)
                        } else {
                            report.truncated = true
                        }
                    }
                }
            }
            Self::AllOf { items } => {
                for item in items {
                    item.visit(value, context, path, depth + 1, report)?;
                }
            }
            Self::Not { item } => {
                let mut child = MatchReport::default();
                item.visit(value, context, path, depth + 1, &mut child)?;
                if child.mismatches.is_empty() {
                    mismatch(MismatchReason::NegationMatched)
                }
            }
        }
        Ok(())
    }
}
fn push(report: &mut MatchReport, path: &str, reason: MismatchReason) {
    if report.mismatches.len() < 32 {
        report.mismatches.push(PatternMismatch {
            path: path.chars().take(512).collect(),
            reason,
        });
    } else {
        report.truncated = true;
    }
}

fn limit(message: &str) -> PureEvaluationError {
    PureEvaluationError::Evaluation(evaluation::EvaluationFailure {
        kind: evaluation::EvaluationFailureKind::PatternLimitExceeded,
        message: message.into(),
    })
}

impl Matcher {
    pub fn validate(&self) -> bool {
        let mut pending = vec![(self, 0usize, false)];
        let mut count = 0;
        while let Some((matcher, depth, field)) = pending.pop() {
            count += 1;
            if depth > MAX_PATTERN_DEPTH || count > MAX_PATTERN_VISITS {
                return false;
            }
            match matcher {
                Self::Regex { source } if validate_regex(source).is_err() => {
                    return false;
                }
                Self::Type { ty } if !ty.is_transferable() => {
                    return false;
                }
                Self::Optional { item } => {
                    if !field {
                        return false;
                    }
                    pending.push((item, depth + 1, false));
                }
                Self::Absent if !field => {
                    return false;
                }
                Self::Record { fields, .. } => {
                    pending.extend(fields.values().map(|matcher| (matcher, depth + 1, true)))
                }
                Self::OneOf { items } | Self::AllOf { items } => {
                    if items.is_empty() || items.len() > 64 {
                        return false;
                    }
                    pending.extend(items.iter().map(|matcher| (matcher, depth + 1, false)));
                }
                Self::List { items } => {
                    pending.extend(items.iter().map(|matcher| (matcher, depth + 1, false)))
                }
                Self::Each { item }
                | Self::Contains { item }
                | Self::Not { item }
                | Self::Reference { body: item, .. } => pending.push((item, depth + 1, field)),
                _ => (),
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matcher_roundtrip_and_bounded_alternative_reasons() {
        let matcher = Matcher::OneOf {
            items: vec![
                Matcher::Record {
                    exact: true,
                    fields: BTreeMap::from([("name".into(), Matcher::Type { ty: Type::String })]),
                },
                Matcher::Range {
                    lower: PlanExpr::Literal(Value::Int(1)),
                    upper: PlanExpr::Literal(Value::Int(3)),
                },
            ],
        };
        assert!(matcher.validate());
        let bytes = serde_json::to_vec(&matcher).unwrap();
        let decoded: Matcher = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(matcher, decoded);
        assert!(
            matcher
                .matches(&Value::Int(2), &HashMap::new())
                .unwrap()
                .mismatches
                .is_empty()
        );
        let result = matcher
            .matches(&Value::Record(BTreeMap::new()), &HashMap::new())
            .unwrap();
        assert_eq!(result.mismatches[0].reason, MismatchReason::NoAlternative);
        assert_eq!(result.alternatives[0].mismatches[0].path, "$.name");
        assert!(result.alternatives.len() <= 8);
        assert!(!Matcher::Regex { source: "[".into() }.validate());
        assert!(
            !Matcher::Optional {
                item: Box::new(Matcher::Any)
            }
            .validate()
        );
    }
    #[test]
    fn regex_work_is_bounded_and_reused_over_a_collection() {
        let matcher = Matcher::Each {
            item: Box::new(Matcher::Regex {
                source: "^[a-z]+$".into(),
            }),
        };
        let values = Value::List(vec![Value::String("hello".into()); 10000]);
        assert!(
            matcher
                .matches(&values, &HashMap::new())
                .unwrap()
                .mismatches
                .is_empty()
        );
        let too_large = Value::String("a".repeat(262145));
        assert!(
            matches!(Matcher::Regex{source:"a".into()}.matches(&too_large,&HashMap::new()),Err(PureEvaluationError::Evaluation(failure)) if failure.kind==evaluation::EvaluationFailureKind::PatternLimitExceeded)
        );
    }
}
