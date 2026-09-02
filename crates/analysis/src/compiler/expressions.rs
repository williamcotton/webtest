//! Recursive expression inference and `PlanExpr` construction.

use std::collections::BTreeMap;

use super::{Compiler, TypedExpr};
use crate::diagnostic::{nearest_strings, text_hints, type_reference_name};
use webtest_feedback::RepairHintKind;
use webtest_hir::{HirExpr, HirExprKind, HirLiteral, HirNameRef, UnaryOperator};
use webtest_plan::PlanExpr;
use webtest_provider::{Capability, RecordField, Type, Value};

impl Compiler<'_> {
    pub(super) fn infer_expr(
        &mut self,
        expression: &HirExpr,
        domain: Capability,
        expected: Option<&Type>,
    ) -> TypedExpr {
        let result = match &expression.kind {
            HirExprKind::Literal(literal) => match literal {
                HirLiteral::String(value) => {
                    super::type_system::typed(Value::String(value.clone()), Type::String)
                }
                HirLiteral::Int(value) => super::type_system::typed(Value::Int(*value), Type::Int),
                HirLiteral::Float(value) => {
                    super::type_system::typed(Value::Float(*value), Type::Float)
                }
                HirLiteral::Bool(value) => {
                    super::type_system::typed(Value::Bool(*value), Type::Bool)
                }
                HirLiteral::Null => super::type_system::typed(Value::Null, Type::Null),
                HirLiteral::Duration(value) => super::type_system::typed(
                    Value::DurationMillis(value.as_millis().min(u128::from(u64::MAX)) as u64),
                    Type::Duration,
                ),
            },
            HirExprKind::Name(HirNameRef::Binding { id, name }) => {
                if let Some(binding) = self.bindings.get(id).cloned() {
                    if domain == Capability::Browser
                        && binding.domain == Capability::Server
                        && !binding.ty.is_transferable()
                    {
                        self.error(
                            expression.origin.range,
                            "semantic.non_transferable_value",
                            format!(
                                "binding `{}` has non-transferable type {} and cannot cross from Server to Browser",
                                binding.name, binding.ty
                            ),
                        );
                    }
                    TypedExpr {
                        expression: PlanExpr::Binding(*id),
                        ty: binding.ty,
                        capability: Capability::Pure,
                    }
                } else {
                    self.error(
                        expression.origin.range,
                        "semantic.use_before_definition",
                        format!("binding `{name}` is used before its value is available"),
                    );
                    super::type_system::unknown_expr()
                }
            }
            HirExprKind::Name(HirNameRef::Unresolved(name)) => {
                if self.declared_names.contains(name) {
                    self.error(
                        expression.origin.range,
                        "semantic.use_before_definition",
                        format!("binding `{name}` is used before its declaration"),
                    );
                } else {
                    let known = self.declared_names.iter().cloned().collect::<Vec<_>>();
                    let candidates = nearest_strings(&known, name, 5);
                    self.error_with_details(
                        expression.origin.range,
                        "semantic.unknown_name",
                        format!("unknown name `{name}`"),
                        serde_json::json!({"requested": name, "known_names": known}),
                        text_hints(
                            RepairHintKind::NameCandidate,
                            candidates,
                            expression.origin.range,
                        ),
                        Vec::new(),
                    );
                }
                super::type_system::unknown_expr()
            }
            HirExprKind::List(items) => {
                let item_expected = match expected {
                    Some(Type::List(inner)) => Some(inner.as_ref()),
                    _ => None,
                };
                if items.is_empty() && item_expected.is_none() {
                    self.error(
                        expression.origin.range,
                        "semantic.empty_list_needs_type",
                        "empty list requires a contextual type".into(),
                    );
                }
                let compiled: Vec<_> = items
                    .iter()
                    .map(|item| self.infer_expr(item, domain, item_expected))
                    .collect();
                let item_type = item_expected
                    .cloned()
                    .or_else(|| compiled.first().map(|it| it.ty.clone()))
                    .unwrap_or(Type::Unknown);
                for item in &compiled {
                    self.expect_type(expression.origin.range, &item_type, &item.ty);
                }
                TypedExpr {
                    expression: PlanExpr::List(
                        compiled.into_iter().map(|item| item.expression).collect(),
                    ),
                    ty: Type::List(Box::new(item_type)),
                    capability: Capability::Pure,
                }
            }
            HirExprKind::Record(fields) => {
                let mut values = BTreeMap::new();
                let mut types = BTreeMap::new();
                for field in fields {
                    let expected_field = match expected {
                        Some(Type::Record(fields)) => fields.get(&field.name).map(|it| &it.ty),
                        _ => None,
                    };
                    if values.contains_key(&field.name) {
                        self.error(
                            field.origin.range,
                            "semantic.duplicate_record_field",
                            format!("record field `{}` is provided more than once", field.name),
                        );
                    }
                    let value = self.infer_expr(&field.value, domain, expected_field);
                    values.insert(field.name.clone(), value.expression);
                    types.insert(
                        field.name.clone(),
                        RecordField {
                            ty: value.ty,
                            optional: false,
                            documentation: String::new(),
                            secret: false,
                        },
                    );
                }
                TypedExpr {
                    expression: PlanExpr::Record(values),
                    ty: Type::Record(types),
                    capability: Capability::Pure,
                }
            }
            HirExprKind::Member {
                receiver,
                member,
                member_origin,
            } => {
                let receiver = self.infer_expr(receiver, domain, None);
                let missing_is_null = receiver.ty.member_missing_is_null(member);
                if let Some(ty) = receiver.ty.member(member) {
                    TypedExpr {
                        expression: PlanExpr::Member {
                            receiver: Box::new(receiver.expression),
                            member: member.clone(),
                            missing_is_null,
                        },
                        ty,
                        capability: receiver.capability,
                    }
                } else {
                    if receiver.ty != Type::Unknown {
                        let known = super::type_system::known_members(&receiver.ty);
                        let candidates = nearest_strings(&known, member, 5);
                        let message = if let Some(best) = candidates.first() {
                            format!(
                                "type {} has no member `{member}`; did you mean `{best}`?",
                                receiver.ty
                            )
                        } else {
                            format!("type {} has no member `{member}`", receiver.ty)
                        };
                        self.error_with_details(
                            member_origin.range,
                            "semantic.unknown_member",
                            message,
                            serde_json::json!({
                                "requested": member,
                                "receiver_type": receiver.ty.to_string(),
                                "known_members": known,
                            }),
                            text_hints(
                                RepairHintKind::MemberCandidate,
                                candidates,
                                member_origin.range,
                            ),
                            vec![format!("type.{}", type_reference_name(&receiver.ty))],
                        );
                    }
                    super::type_system::unknown_expr()
                }
            }
            HirExprKind::Call { .. } => {
                if let Some(call) = self.provider_call(expression, domain) {
                    self.error(
                        expression.origin.range,
                        "semantic.effectful_expression",
                        format!(
                            "provider call `{}.{}` must be the direct value of a binding or a statement",
                            call.provider, call.operation
                        ),
                    );
                    TypedExpr {
                        expression: PlanExpr::Literal(Value::Null),
                        ty: call.result_type,
                        capability: Capability::Server,
                    }
                } else {
                    self.error(
                        expression.origin.range,
                        "semantic.unknown_function",
                        "bare function calls do not resolve to providers".into(),
                    );
                    super::type_system::unknown_expr()
                }
            }
            HirExprKind::Unary { operator, operand } => {
                let operand = self.infer_expr(operand, domain, None);
                let ty = match operator {
                    UnaryOperator::Not if operand.ty == Type::Bool => Type::Bool,
                    UnaryOperator::Negate
                        if matches!(operand.ty, Type::Int | Type::Float | Type::StatusCode) =>
                    {
                        operand.ty.clone()
                    }
                    UnaryOperator::Not => {
                        self.type_mismatch(expression.origin.range, &Type::Bool, &operand.ty);
                        Type::Unknown
                    }
                    UnaryOperator::Negate => {
                        self.error(
                            expression.origin.range,
                            "semantic.invalid_unary_operand",
                            format!("numeric negation does not accept {}", operand.ty),
                        );
                        Type::Unknown
                    }
                };
                TypedExpr {
                    expression: PlanExpr::Unary {
                        operator: *operator,
                        operand: Box::new(operand.expression),
                    },
                    ty,
                    capability: operand.capability,
                }
            }
            HirExprKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.infer_expr(left, domain, None);
                let right = self.infer_expr(right, domain, Some(&left.ty));
                let ty =
                    self.validate_binary(*operator, &left.ty, &right.ty, expression.origin.range);
                TypedExpr {
                    expression: PlanExpr::Binary {
                        operator: *operator,
                        left: Box::new(left.expression),
                        right: Box::new(right.expression),
                    },
                    ty,
                    capability: Capability::Pure,
                }
            }
            HirExprKind::Missing => super::type_system::unknown_expr(),
        };
        self.type_fact(
            expression.origin.range,
            result.ty.clone(),
            result.capability,
        );
        result
    }
}
