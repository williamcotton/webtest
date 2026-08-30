use serde::{Deserialize, Serialize};
use serde_json::Value;
use webtest_browser::BrowserError;

#[derive(Serialize)]
pub(crate) struct Command<'a> {
    pub(crate) id: u64,
    pub(crate) method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) params: Option<&'a Value>,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<&'a str>,
}

#[derive(Deserialize)]
pub(crate) struct IncomingMessage {
    pub(crate) id: Option<u64>,
    #[serde(rename = "sessionId")]
    pub(crate) session_id: Option<String>,
    pub(crate) result: Option<Value>,
    pub(crate) error: Option<CdpError>,
    pub(crate) method: Option<String>,
    pub(crate) params: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct CdpError {
    pub(crate) code: i64,
    pub(crate) message: String,
}

pub(crate) fn bounded_text(value: &str) -> String {
    value.chars().take(256).collect()
}

pub(crate) fn string_field(
    value: &Value,
    field: &str,
    method: &str,
) -> Result<String, BrowserError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| BrowserError::Protocol {
            method: method.into(),
            message: format!("response did not contain `{field}`"),
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn command_envelopes_preserve_optional_fields_and_session_names() {
        let bare = Command {
            id: 1,
            method: "Browser.getVersion",
            params: None,
            session_id: None,
        };
        assert_eq!(
            serde_json::to_value(bare).expect("serialize command"),
            json!({"id": 1, "method": "Browser.getVersion"})
        );

        let params = json!({"expression": "1 + 1"});
        let page = Command {
            id: 2,
            method: "Runtime.evaluate",
            params: Some(&params),
            session_id: Some("page-session"),
        };
        assert_eq!(
            serde_json::to_value(page).expect("serialize page command"),
            json!({
                "id": 2,
                "method": "Runtime.evaluate",
                "params": {"expression": "1 + 1"},
                "sessionId": "page-session"
            })
        );
    }

    #[test]
    fn incoming_envelopes_and_required_string_fields_remain_typed() {
        let response: IncomingMessage = serde_json::from_value(json!({
            "id": 4,
            "sessionId": "session",
            "error": {"code": -32000, "message": "failed"}
        }))
        .expect("deserialize response");
        assert_eq!(response.id, Some(4));
        assert_eq!(response.session_id.as_deref(), Some("session"));
        let error = response.error.expect("CDP error");
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "failed");

        assert_eq!(
            string_field(
                &json!({"targetId": "target"}),
                "targetId",
                "Target.createTarget"
            )
            .expect("target ID"),
            "target"
        );
        assert!(matches!(
            string_field(&json!({}), "targetId", "Target.createTarget"),
            Err(BrowserError::Protocol { method, message })
                if method == "Target.createTarget" && message.contains("targetId")
        ));
    }

    #[test]
    fn protocol_diagnostic_text_is_bounded_by_characters() {
        let text = "é".repeat(300);
        let bounded = bounded_text(&text);
        assert_eq!(bounded.chars().count(), 256);
        assert!(bounded.is_char_boundary(bounded.len()));
    }
}
