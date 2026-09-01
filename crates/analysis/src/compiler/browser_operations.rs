//! Browser operation/assertion validation and plan lowering.

use super::Compiler;
use webtest_hir::{HirBrowserOp, HirExprKind, HirLiteral, HirValueAction};
use webtest_plan::{
    AssertionOperation, BrowserOperation, PlanExpr, PlannedStep, TestOperation, locator_from_hir,
    locator_state_from_hir,
};
use webtest_provider::{Capability, Type, Value};
use webtest_text::SyntaxOrigin;

impl Compiler<'_> {
    pub(super) fn compile_browser_operation(
        &mut self,
        operation: &HirBrowserOp,
        domain: Capability,
        steps: &mut Vec<PlannedStep>,
    ) {
        if domain != Capability::Browser {
            self.error_with_details(
                browser_origin(operation).range,
                "semantic.capability_mismatch",
                format!("Browser operation is not allowed in {domain} context"),
                serde_json::json!({
                    "required_capability": "Browser",
                    "actual_capability": domain.to_string(),
                    "construct": browser_reference(operation),
                }),
                Vec::new(),
                vec![
                    browser_reference(operation).into(),
                    "capability.Browser".into(),
                ],
            );
        }
        self.record_capability(Capability::Browser);
        let (operation, origin, assertion) = match operation {
            HirBrowserOp::Open(open) => {
                let url = self.infer_expr(&open.url, domain, Some(&Type::String));
                self.expect_type(open.url.origin.range, &Type::String, &url.ty);
                (
                    BrowserOperation::Navigate {
                        url: url.expression,
                    },
                    open.url.origin,
                    None,
                )
            }
            HirBrowserOp::Evaluate(evaluate) => (
                BrowserOperation::Evaluate {
                    expression: evaluate.expression.value.clone(),
                },
                evaluate.expression.origin,
                None,
            ),
            HirBrowserOp::Click(action) => (
                BrowserOperation::Click {
                    locator: locator_from_hir(&action.locator.kind),
                },
                action.locator.origin,
                None,
            ),
            HirBrowserOp::Fill(action) => {
                let value = self.browser_string_value(action, domain);
                (
                    BrowserOperation::Fill {
                        locator: locator_from_hir(&action.locator.kind),
                        value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Type(action) => {
                let value = self.browser_string_value(action, domain);
                (
                    BrowserOperation::Type {
                        locator: locator_from_hir(&action.locator.kind),
                        value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Press(action) => {
                let value = self.browser_string_value(action, domain);
                if let HirExprKind::Literal(HirLiteral::String(key)) = &action.value.kind
                    && !valid_key_chord(key)
                {
                    self.error(
                        action.value.origin.range,
                        "semantic.invalid_key",
                        format!("invalid key chord `{key}`"),
                    );
                }
                (
                    BrowserOperation::Press {
                        locator: locator_from_hir(&action.locator.kind),
                        key: value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Check(action) | HirBrowserOp::Uncheck(action) => (
                BrowserOperation::Check {
                    locator: locator_from_hir(&action.locator.kind),
                    checked: matches!(operation, HirBrowserOp::Check(_)),
                },
                action.locator.origin,
                None,
            ),
            HirBrowserOp::Select(action) => {
                let value = self.browser_string_value(action, domain);
                (
                    BrowserOperation::Select {
                        locator: locator_from_hir(&action.locator.kind),
                        option: value,
                    },
                    action.locator.origin,
                    None,
                )
            }
            HirBrowserOp::Hover(action) => (
                BrowserOperation::Hover {
                    locator: locator_from_hir(&action.locator.kind),
                },
                action.locator.origin,
                None,
            ),
            HirBrowserOp::WaitLocator(wait) => (
                BrowserOperation::WaitForLocator {
                    locator: locator_from_hir(&wait.locator.kind),
                    state: locator_state_from_hir(wait.state),
                    timeout: wait.timeout,
                },
                wait.locator.origin,
                None,
            ),
            HirBrowserOp::WaitUrl(wait) => (
                BrowserOperation::WaitForUrl {
                    url: PlanExpr::Literal(Value::String(wait.url.value.clone())),
                    timeout: wait.timeout,
                },
                wait.url.origin,
                None,
            ),
            HirBrowserOp::ExpectLocator(expectation) => {
                let assertion = AssertionOperation::Locator {
                    locator: locator_from_hir(&expectation.locator.kind),
                    state: locator_state_from_hir(expectation.state),
                    timeout: expectation.timeout,
                };
                (
                    BrowserOperation::Hover {
                        locator: locator_from_hir(&expectation.locator.kind),
                    },
                    expectation.locator.origin,
                    Some(assertion),
                )
            }
            HirBrowserOp::ExpectUrl(expectation) => {
                let assertion = AssertionOperation::Url {
                    url: PlanExpr::Literal(Value::String(expectation.url.value.clone())),
                    timeout: expectation.timeout,
                };
                (
                    BrowserOperation::WaitForUrl {
                        url: PlanExpr::Literal(Value::String(expectation.url.value.clone())),
                        timeout: expectation.timeout,
                    },
                    expectation.url.origin,
                    Some(assertion),
                )
            }
        };
        self.push_step(
            steps,
            origin,
            assertion.map_or(TestOperation::Browser(operation), TestOperation::Assertion),
        );
    }

    fn browser_string_value(&mut self, action: &HirValueAction, domain: Capability) -> PlanExpr {
        let value = self.infer_expr(&action.value, domain, Some(&Type::String));
        self.expect_type(action.value.origin.range, &Type::String, &value.ty);
        value.expression
    }
}

fn browser_origin(operation: &HirBrowserOp) -> SyntaxOrigin {
    match operation {
        HirBrowserOp::Open(value) => value.origin,
        HirBrowserOp::Evaluate(value) => value.origin,
        HirBrowserOp::Click(value)
        | HirBrowserOp::Check(value)
        | HirBrowserOp::Uncheck(value)
        | HirBrowserOp::Hover(value) => value.origin,
        HirBrowserOp::Fill(value)
        | HirBrowserOp::Type(value)
        | HirBrowserOp::Press(value)
        | HirBrowserOp::Select(value) => value.origin,
        HirBrowserOp::WaitLocator(value) | HirBrowserOp::ExpectLocator(value) => value.origin,
        HirBrowserOp::WaitUrl(value) | HirBrowserOp::ExpectUrl(value) => value.origin,
    }
}

fn browser_reference(operation: &HirBrowserOp) -> &'static str {
    match operation {
        HirBrowserOp::Open(_) => "browser.open",
        HirBrowserOp::Evaluate(_) => "browser.evaluate",
        HirBrowserOp::Click(_) => "browser.click",
        HirBrowserOp::Fill(_) => "browser.fill",
        HirBrowserOp::Type(_) => "browser.type",
        HirBrowserOp::Press(_) => "browser.press",
        HirBrowserOp::Check(_) => "browser.check",
        HirBrowserOp::Uncheck(_) => "browser.uncheck",
        HirBrowserOp::Select(_) => "browser.select",
        HirBrowserOp::Hover(_) => "browser.hover",
        HirBrowserOp::WaitLocator(_) => "browser.wait.locator",
        HirBrowserOp::WaitUrl(_) => "browser.wait.url",
        HirBrowserOp::ExpectLocator(_) => "assertion.locator_state",
        HirBrowserOp::ExpectUrl(_) => "assertion.url",
    }
}

fn valid_key_chord(value: &str) -> bool {
    let mut main = 0;
    for part in value.split('+') {
        match part {
            "Alt" | "Control" | "Ctrl" | "Meta" | "Command" | "Shift" => {}
            "Enter" | "Tab" | "Escape" | "Esc" | "Backspace" | "Delete" | "ArrowUp"
            | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp" | "PageDown"
            | "Space" => main += 1,
            value if value.chars().count() == 1 => main += 1,
            _ => return false,
        }
    }
    main == 1
}

#[cfg(test)]
mod tests {
    use super::valid_key_chord;

    #[test]
    fn key_chords_have_exactly_one_main_key() {
        assert!(valid_key_chord("Control+Shift+Enter"));
        assert!(valid_key_chord("é"));
        assert!(!valid_key_chord("Control+Shift"));
        assert!(!valid_key_chord("Enter+Tab"));
        assert!(!valid_key_chord("Control+Unknown"));
    }
}
