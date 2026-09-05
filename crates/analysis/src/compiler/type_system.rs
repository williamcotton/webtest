//! Current HIR type lowering, compatibility, operator, matcher, and recovery rules.

use super::{Compiler, TypedExpr};
use crate::diagnostic::type_reference_name;
use webtest_hir::{HirExpr, HirExprKind, HirNameRef, HirType, HirTypeKind};
use webtest_model::{BinaryOperator, Capability, RecordField, Type, Value};
use webtest_plan::{PlanExpr, ValueMatcher};
use webtest_text::TextRange;

impl Compiler<'_> {
    pub(super) fn validate_binary(
        &mut self,
        operator: BinaryOperator,
        left: &Type,
        right: &Type,
        range: TextRange,
    ) -> Type {
        if matches!(left, Type::Unknown) || matches!(right, Type::Unknown) {
            return Type::Unknown;
        }
        match operator {
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                if !left.accepts(right) && !right.accepts(left) {
                    self.error(
                        range,
                        "semantic.incompatible_equality",
                        format!("cannot compare {left} with {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if !(numeric(left) && numeric(right)
                    || left == &Type::String && right == &Type::String)
                {
                    self.error(
                        range,
                        "semantic.invalid_comparison",
                        format!("ordered comparison does not accept {left} and {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Add => {
                if left == &Type::String && right == &Type::String {
                    Type::String
                } else if numeric(left) && numeric(right) {
                    numeric_result(left, right)
                } else {
                    self.error(
                        range,
                        "semantic.invalid_binary_operands",
                        format!("addition does not accept {left} and {right}"),
                    );
                    Type::Unknown
                }
            }
            BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide => {
                if numeric(left) && numeric(right) {
                    numeric_result(left, right)
                } else {
                    self.error(
                        range,
                        "semantic.invalid_binary_operands",
                        format!("numeric operator does not accept {left} and {right}"),
                    );
                    Type::Unknown
                }
            }
            BinaryOperator::And | BinaryOperator::Or => {
                if left != &Type::Bool || right != &Type::Bool {
                    self.error(
                        range,
                        "semantic.invalid_boolean_operands",
                        format!("boolean operator requires Bool operands, got {left} and {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Contains => {
                let valid = (left == &Type::String && right == &Type::String)
                    || matches!(left, Type::List(inner) if inner.accepts(right));
                if !valid {
                    self.error(
                        range,
                        "semantic.invalid_matcher",
                        format!("`contains` does not accept {left} and {right}"),
                    );
                }
                Type::Bool
            }
            BinaryOperator::Matches => Type::Bool,
        }
    }

    pub(super) fn pattern_type(&mut self, expression: &HirExpr) -> Type {
        match &expression.kind {
            HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                named_type(name).unwrap_or_else(|| {
                    self.error(
                        expression.origin.range,
                        "semantic.unknown_type",
                        format!("unknown type `{name}` in match pattern"),
                    );
                    Type::Unknown
                })
            }
            HirExprKind::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.clone(),
                            RecordField {
                                ty: self.pattern_type(&field.value),
                                optional: false,
                                documentation: String::new(),
                                secret: false,
                            },
                        )
                    })
                    .collect(),
            ),
            HirExprKind::List(items) if items.len() == 1 => {
                Type::List(Box::new(self.pattern_type(&items[0])))
            }
            _ => {
                self.error(
                    expression.origin.range,
                    "semantic.invalid_type_pattern",
                    "expected a type name, record shape, or one-element list shape".into(),
                );
                Type::Unknown
            }
        }
    }

    pub(super) fn lower_type(&mut self, ty: &HirType) -> Type {
        match &ty.kind {
            HirTypeKind::Named(name) => named_type(name).unwrap_or_else(|| {
                self.error(
                    ty.origin.range,
                    "semantic.unknown_type",
                    format!("unknown type `{name}`"),
                );
                Type::Unknown
            }),
            HirTypeKind::Generic { name, argument } => {
                let argument = self.lower_type(argument);
                match name.as_str() {
                    "List" => Type::List(Box::new(argument)),
                    "Option" => Type::Option(Box::new(argument)),
                    "Response" => Type::Response(Box::new(argument)),
                    _ => {
                        self.error(
                            ty.origin.range,
                            "semantic.unknown_type",
                            format!("unknown generic type `{name}`"),
                        );
                        Type::Unknown
                    }
                }
            }
            HirTypeKind::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.clone(),
                            RecordField {
                                ty: self.lower_type(&field.ty),
                                optional: field.optional,
                                documentation: String::new(),
                                secret: false,
                            },
                        )
                    })
                    .collect(),
            ),
            HirTypeKind::Missing => Type::Unknown,
        }
    }

    pub(super) fn expect_type(&mut self, range: TextRange, expected: &Type, actual: &Type) {
        if !expected.accepts(actual) {
            self.type_mismatch(range, expected, actual);
        }
    }

    pub(super) fn type_mismatch(&mut self, range: TextRange, expected: &Type, actual: &Type) {
        if expected != &Type::Unknown && actual != &Type::Unknown {
            self.error_with_details(
                range,
                "semantic.type_mismatch",
                format!("expected {expected}, got {actual}"),
                serde_json::json!({
                    "expected_type": expected.to_string(),
                    "actual_type": actual.to_string(),
                }),
                Vec::new(),
                vec![
                    format!("type.{}", type_reference_name(expected)),
                    format!("type.{}", type_reference_name(actual)),
                ],
            );
        }
    }
}

pub(super) fn known_members(ty: &Type) -> Vec<String> {
    match ty {
        Type::Record(fields) => fields.keys().cloned().collect(),
        Type::Response(_) => ["status", "headers", "body", "text", "json"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        Type::ProcessResult => [
            "exit_code",
            "stdout",
            "stderr",
            "stdout_bytes",
            "stderr_bytes",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn typed(value: Value, ty: Type) -> TypedExpr {
    TypedExpr {
        expression: PlanExpr::Literal(value),
        ty,
        capability: Capability::Pure,
    }
}

pub(super) fn unknown_expr() -> TypedExpr {
    typed(Value::Null, Type::Unknown)
}

fn named_type(name: &str) -> Option<Type> {
    Some(match name {
        "Null" => Type::Null,
        "Bool" => Type::Bool,
        "Int" => Type::Int,
        "Float" => Type::Float,
        "String" => Type::String,
        "Duration" => Type::Duration,
        "Url" => Type::Url,
        "Json" => Type::Json,
        "StatusCode" => Type::StatusCode,
        "Headers" => Type::Headers,
        "Bytes" => Type::Bytes,
        "ProcessResult" => Type::ProcessResult,
        "FilePath" => Type::FilePath,
        "TempDirectory" => Type::TempDirectory,
        "Locator" => Type::Locator,
        "BrowserPage" => Type::BrowserPage,
        _ => return None,
    })
}

pub(super) fn decodable_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Null
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::String
            | Type::Json
            | Type::List(_)
            | Type::Option(_)
            | Type::Record(_)
    )
}

fn numeric(ty: &Type) -> bool {
    matches!(ty, Type::Int | Type::Float | Type::StatusCode)
}

fn numeric_result(left: &Type, right: &Type) -> Type {
    if left == &Type::Float || right == &Type::Float {
        Type::Float
    } else {
        Type::Int
    }
}

pub(super) fn matcher_for(operator: BinaryOperator) -> Option<ValueMatcher> {
    Some(match operator {
        BinaryOperator::Equal => ValueMatcher::Equal,
        BinaryOperator::NotEqual => ValueMatcher::NotEqual,
        BinaryOperator::Less => ValueMatcher::Less,
        BinaryOperator::LessEqual => ValueMatcher::LessEqual,
        BinaryOperator::Greater => ValueMatcher::Greater,
        BinaryOperator::GreaterEqual => ValueMatcher::GreaterEqual,
        BinaryOperator::Contains => ValueMatcher::Contains,
        BinaryOperator::Matches => ValueMatcher::Matches,
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide
        | BinaryOperator::And
        | BinaryOperator::Or => return None,
    })
}
