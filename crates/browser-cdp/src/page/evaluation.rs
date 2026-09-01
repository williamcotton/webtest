use std::time::Duration;

use serde_json::{Value, json};
use webtest_browser::BrowserError;

use super::CdpPage;

pub(super) async fn evaluate_expression(
    page: &CdpPage,
    expression: String,
) -> Result<Value, BrowserError> {
    page.connection
        .command(
            "Runtime.evaluate",
            Some(json!({ "expression": expression, "returnByValue": true })),
            Some(&page.session_id),
        )
        .await
}

pub(super) async fn evaluate_expression_with_timeout(
    page: &CdpPage,
    expression: String,
    timeout: Duration,
) -> Result<Value, BrowserError> {
    page.connection
        .command_with_timeout(
            "Runtime.evaluate",
            Some(json!({ "expression": expression, "returnByValue": true })),
            Some(&page.session_id),
            timeout,
        )
        .await
}

pub(super) async fn evaluate(page: &CdpPage, expression: &str) -> Result<(), BrowserError> {
    let evaluation = evaluate_expression(page, expression.to_owned()).await?;
    validate_evaluation(expression, evaluation)
}

pub(super) async fn evaluate_with_timeout(
    page: &CdpPage,
    expression: &str,
    timeout: Duration,
) -> Result<(), BrowserError> {
    let evaluation = evaluate_expression_with_timeout(page, expression.to_owned(), timeout).await?;
    validate_evaluation(expression, evaluation)
}

fn validate_evaluation(expression: &str, evaluation: Value) -> Result<(), BrowserError> {
    if let Some(message) = evaluation.get("errorText").and_then(Value::as_str) {
        return Err(BrowserError::EvaluationFailed {
            expression: expression.to_owned(),
            message: message.to_owned(),
        });
    }
    if let Some(message) = evaluation
        .pointer("/exceptionDetails/exception/description")
        .and_then(Value::as_str)
        .or_else(|| {
            evaluation
                .pointer("/exceptionDetails/text")
                .and_then(Value::as_str)
        })
    {
        return Err(BrowserError::EvaluationFailed {
            expression: expression.to_owned(),
            message: message.to_owned(),
        });
    }
    Ok(())
}

pub(super) fn evaluation_value(result: &Value) -> Option<&Value> {
    result.pointer("/result/value")
}

pub(super) fn invalid_evaluation(message: &str) -> BrowserError {
    BrowserError::Protocol {
        method: "Runtime.evaluate".into(),
        message: message.into(),
    }
}
