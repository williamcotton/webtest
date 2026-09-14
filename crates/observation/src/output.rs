//! Bounded text projections of acknowledged output, never raw process pipes.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// A completed stream snapshot. Lengths distinguish captured bytes from decoded,
/// redacted UTF-8 text. Truncation applies only after redaction; raw bytes are not
/// persisted here. The producing operation and occurrence are in the event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedOutput {
    pub stream: OutputStream,
    pub text: String,
    pub captured_byte_length: u64,
    pub redacted_byte_length: u64,
    pub lossy: bool,
    pub redacted: bool,
    pub truncated: bool,
}
