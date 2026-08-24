use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::BridgeMessage;

pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("application bridge protocol failure ({code}): {message}")]
pub struct ProtocolError {
    pub code: &'static str,
    pub message: String,
}

impl ProtocolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub async fn read_frame(
    reader: &mut (impl AsyncBufRead + Unpin),
    max_message_bytes: usize,
) -> Result<Option<BridgeMessage>, ProtocolError> {
    let mut bytes = Vec::new();
    let mut terminated = false;
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|error| ProtocolError::new("io", error.to_string()))?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let payload = newline.map_or(available, |index| &available[..index]);
        if bytes.len().saturating_add(payload.len()) > max_message_bytes {
            return Err(ProtocolError::new(
                "frame_too_large",
                format!("frame exceeded {max_message_bytes} bytes"),
            ));
        }
        bytes.extend_from_slice(payload);
        reader.consume(consumed);
        if newline.is_some() {
            terminated = true;
            break;
        }
    }
    if !terminated {
        return Err(ProtocolError::new(
            "truncated_frame",
            "bridge frame ended without a line terminator",
        ));
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let source = std::str::from_utf8(&bytes)
        .map_err(|_| ProtocolError::new("invalid_utf8", "frame was not UTF-8"))?;
    let value: serde_json::Value = serde_json::from_str(source)
        .map_err(|error| ProtocolError::new("invalid_json", error.to_string()))?;
    if !value.is_object() {
        return Err(ProtocolError::new(
            "non_object_message",
            "protocol messages must be JSON objects",
        ));
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| ProtocolError::new("invalid_message", error.to_string()))
}

pub async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &BridgeMessage,
    max_message_bytes: usize,
) -> Result<(), ProtocolError> {
    let encoded = serde_json::to_vec(message)
        .map_err(|error| ProtocolError::new("encode", error.to_string()))?;
    if encoded.len() > max_message_bytes {
        return Err(ProtocolError::new(
            "frame_too_large",
            format!("frame exceeded {max_message_bytes} bytes"),
        ));
    }
    writer
        .write_all(&encoded)
        .await
        .map_err(|error| ProtocolError::new("io", error.to_string()))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|error| ProtocolError::new("io", error.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|error| ProtocolError::new("io", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn line_framing_round_trips_unicode_and_escaped_newlines() {
        let message = BridgeMessage::Error {
            id: 4,
            code: "fixture.failed".into(),
            message: "héllo\nworld".into(),
            retryable: false,
            data: serde_json::json!({}),
            debug: None,
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &message, DEFAULT_MAX_MESSAGE_BYTES)
            .await
            .expect("write");
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
        let mut reader = tokio::io::BufReader::new(bytes.as_slice());
        assert_eq!(
            read_frame(&mut reader, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .expect("read"),
            Some(message)
        );
    }

    #[tokio::test]
    async fn rejects_non_objects_and_oversized_frames() {
        let mut reader = tokio::io::BufReader::new(b"[]\n".as_slice());
        assert_eq!(
            read_frame(&mut reader, 10).await.expect_err("object").code,
            "non_object_message"
        );
        let mut reader = tokio::io::BufReader::new(b"{\"type\":\"ping\",\"id\":1}\n".as_slice());
        assert_eq!(
            read_frame(&mut reader, 4).await.expect_err("size").code,
            "frame_too_large"
        );
        let mut reader = tokio::io::BufReader::new(b"{\"type\":\"ping\",\"id\":1}".as_slice());
        assert_eq!(
            read_frame(&mut reader, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .expect_err("terminator")
                .code,
            "truncated_frame"
        );
    }
}
