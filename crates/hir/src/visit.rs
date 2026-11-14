//! Shared structural traversal for semantic indexes and dependency queries.
use super::*;
impl HirStmt {
    pub fn children(&self) -> &[HirStmt] {
        match self {
            Self::GuardScope(value) => &value.statements,
            Self::ActorBlock(value) => &value.statements,
            Self::Check(value) => &value.statements,
            Self::Timeout(value) => &value.statements,
            Self::Retry(value) => &value.statements,
            Self::Parallel(value) => &value.branches,
            Self::Race(value) => &value.branches,
            Self::Server(value) => &value.statements,
            Self::Browser(value) => &value.statements,
            _ => &[],
        }
    }
    pub fn origin(&self) -> SyntaxOrigin {
        match self {
            Self::GuardScope(value) => value.origin,
            Self::Select(value) => value.origin,
            Self::Actor(value) => value.origin,
            Self::ActorBlock(value) => value.origin,
            Self::Check(value) => value.origin,
            Self::Verdict(value) => value.origin,
            Self::Assume(value) => value.origin,
            Self::Timeout(value) => value.origin,
            Self::Retry(value) => value.origin,
            Self::Parallel(value) => value.origin,
            Self::Race(value) => value.origin,
            Self::Server(value) => value.origin,
            Self::Browser(value) => value.origin,
            Self::Let(value) => value.origin,
            Self::Provide(value) | Self::Expression(value) => value.origin,
            Self::Expect(value) => value.origin,
            Self::BrowserOperation(value) => value.origin(),
        }
    }
}
impl HirBrowserOp {
    pub fn origin(&self) -> SyntaxOrigin {
        match self {
            Self::Open(value) => value.origin,
            Self::Evaluate(value) => value.origin,
            Self::Click(value) | Self::Check(value) | Self::Uncheck(value) | Self::Hover(value) => {
                value.origin
            }
            Self::Fill(value) | Self::Type(value) | Self::Press(value) | Self::Select(value) => {
                value.origin
            }
            Self::WaitLocator(value) | Self::ExpectLocator(value) => value.origin,
            Self::WaitUrl(value) | Self::ExpectUrl(value) => value.origin,
        }
    }
}
pub fn visit_expression<'a>(expression: &'a HirExpr, visit: &mut impl FnMut(&'a HirExpr)) {
    visit(expression);
    match &expression.kind {
        HirExprKind::FixtureUse(call) => visit_expression(call, visit),
        HirExprKind::List(values) => {
            for value in values {
                visit_expression(value, visit);
            }
        }
        HirExprKind::Record(fields) => {
            for field in fields {
                visit_expression(&field.value, visit);
            }
        }
        HirExprKind::PatternRecord {
            record: receiver, ..
        }
        | HirExprKind::Member { receiver, .. } => visit_expression(receiver, visit),
        HirExprKind::Call { callee, arguments } => {
            visit_expression(callee, visit);
            for argument in arguments {
                visit_expression(&argument.value, visit);
            }
        }
        HirExprKind::Unary { operand, .. } => visit_expression(operand, visit),
        HirExprKind::Binary { left, right, .. } => {
            visit_expression(left, visit);
            visit_expression(right, visit);
        }
        HirExprKind::Literal(_) | HirExprKind::Name(_) | HirExprKind::Missing => {}
    }
}
pub fn visit_statement_expressions<'a>(
    statement: &'a HirStmt,
    visit: &mut impl FnMut(&'a HirExpr),
) {
    match statement {
        HirStmt::GuardScope(v) => visit_expression(&v.activations, visit),
        HirStmt::Select(v) => {
            visit_expression(&v.actor, visit);
            if let Some(cursor) = &v.cursor {
                visit_expression(cursor, visit);
            }
            for arm in &v.arms {
                if let Some(filter) = &arm.filter {
                    visit_expression(filter, visit);
                }
                for stmt in &arm.statements {
                    visit_statement_expressions(stmt, visit);
                }
            }
            for body in &v.timeouts {
                for stmt in body {
                    visit_statement_expressions(stmt, visit);
                }
            }
        }
        HirStmt::ActorBlock(v) => visit_expression(&v.actor, visit),
        HirStmt::Verdict(v) => {
            if let Some(expr) = &v.message {
                visit_expression(expr, visit);
            }
        }
        HirStmt::Assume(v) => {
            visit_expression(&v.condition, visit);
            if let Some(expr) = &v.verdict.message {
                visit_expression(expr, visit);
            }
        }
        HirStmt::Let(value) => visit_expression(&value.value, visit),
        HirStmt::Provide(value) | HirStmt::Expression(value) => {
            visit_expression(&value.expression, visit)
        }
        HirStmt::Expect(value) => visit_expression(&value.expression, visit),
        HirStmt::BrowserOperation(HirBrowserOp::Open(value)) => visit_expression(&value.url, visit),
        HirStmt::BrowserOperation(
            HirBrowserOp::Fill(value)
            | HirBrowserOp::Type(value)
            | HirBrowserOp::Press(value)
            | HirBrowserOp::Select(value),
        ) => visit_expression(&value.value, visit),
        _ => {}
    }
    if let HirStmt::BrowserOperation(operation) = statement
        && let Some(locator) = operation.locator()
        && let HirLocatorKind::Dynamic { value, name, .. } = &locator.kind
    {
        visit_expression(value, visit);
        if let Some(name) = name {
            visit_expression(name, visit);
        }
    }
    for child in statement.children() {
        visit_statement_expressions(child, visit);
    }
}

impl HirBrowserOp {
    pub fn locator(&self) -> Option<&HirLocator> {
        match self {
            Self::Click(v) | Self::Check(v) | Self::Uncheck(v) | Self::Hover(v) => Some(&v.locator),
            Self::Fill(v) | Self::Type(v) | Self::Press(v) | Self::Select(v) => Some(&v.locator),
            Self::WaitLocator(v) | Self::ExpectLocator(v) => Some(&v.locator),
            _ => None,
        }
    }
}
