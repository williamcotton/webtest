//! Output observations come from typed completed provider results. The native
//! capture driver retains ownership of pipes, output limits, and process reaping.
use webtest_model::Value;
use webtest_observation::{CapturedOutput, OutputStream};

/// Bound each journal stream independently of the provider's capture limit.
/// This limits the redacted UTF-8 payload, including replacement markers.
pub(crate) const OUTPUT_TEXT_MAX_BYTES: usize = 64 * 1024;

pub(super) fn provider_output(
    value: &Value,
    fields: &[String],
    secrets: &[String],
) -> Vec<CapturedOutput> {
    let Value::ProcessResult(result) = value else {
        return Vec::new();
    };
    // Replacing a shorter secret first must not expose the suffix of a longer one.
    let mut secrets = secrets.to_vec();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    [
        (
            OutputStream::Stdout,
            "stdout",
            &result.stdout,
            &result.stdout_bytes,
        ),
        (
            OutputStream::Stderr,
            "stderr",
            &result.stderr,
            &result.stderr_bytes,
        ),
    ]
    .into_iter()
    .map(|(stream, name, original, bytes)| {
        let mut text = if fields.iter().any(|field| field.eq_ignore_ascii_case(name)) {
            "[redacted]".into()
        } else {
            crate::redaction::redact_text(original.clone(), &secrets)
        };
        let redacted = &text != original;
        let redacted_byte_length = text.len() as u64;
        let mut end = text.len().min(OUTPUT_TEXT_MAX_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        CapturedOutput {
            stream,
            truncated: end as u64 != redacted_byte_length,
            text,
            captured_byte_length: bytes.len() as u64,
            redacted_byte_length,
            lossy: std::str::from_utf8(bytes).is_err(),
            redacted,
        }
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtest_model::ProcessResultValue;

    fn process(stdout: Vec<u8>, stderr: Vec<u8>) -> Value {
        Value::ProcessResult(ProcessResultValue {
            exit_code: 0,
            stdout: String::from_utf8_lossy(&stdout).into(),
            stderr: String::from_utf8_lossy(&stderr).into(),
            stdout_bytes: stdout,
            stderr_bytes: stderr,
        })
    }

    #[test]
    fn output_redacts_complete_secrets_before_utf8_safe_truncation() {
        let secret = "token-and-private-suffix";
        let raw = format!(
            "{}{secret}{}",
            "a".repeat(OUTPUT_TEXT_MAX_BYTES - 2),
            "é".repeat(20)
        );
        let value = process(
            raw.as_bytes().to_vec(),
            b"token-and-private-suffix".to_vec(),
        );
        let output = provider_output(&value, &[], &["token".into(), secret.into()]);
        let stdout = &output[0];
        assert_eq!(stdout.stream, OutputStream::Stdout);
        assert_eq!(stdout.captured_byte_length, raw.len() as u64);
        assert!(stdout.redacted && stdout.truncated);
        assert!(stdout.text.len() <= OUTPUT_TEXT_MAX_BYTES);
        assert!(stdout.text.ends_with("[r"), "redaction preceded truncation");
        assert!(!stdout.text.contains("token"));
        assert_eq!(
            output[1].text, "[redacted]",
            "longer secrets are masked first"
        );
        assert!(!output[1].truncated);
        for captured in output {
            let bytes = serde_json::to_vec(&captured).unwrap();
            assert_eq!(
                serde_json::from_slice::<CapturedOutput>(&bytes).unwrap(),
                captured
            );
        }
    }

    #[test]
    fn output_preserves_empty_binary_and_unicode_length_facts_and_field_suppression() {
        let raw = format!("{}é", "a".repeat(OUTPUT_TEXT_MAX_BYTES - 1));
        let outputs = provider_output(&process(raw.as_bytes().to_vec(), vec![0xff]), &[], &[]);
        assert_eq!(outputs[0].text.len(), OUTPUT_TEXT_MAX_BYTES - 1);
        assert_eq!(
            outputs[0].redacted_byte_length,
            OUTPUT_TEXT_MAX_BYTES as u64 + 1
        );
        assert!(outputs[0].truncated);
        assert!(!outputs[0].redacted && !outputs[0].lossy);
        assert_eq!(outputs[1].text, "�");
        assert_eq!(outputs[1].captured_byte_length, 1);
        assert_eq!(outputs[1].redacted_byte_length, 3);
        assert!(outputs[1].lossy);
        let outputs = provider_output(
            &process(vec![], b"private".to_vec()),
            &["STDERR".into()],
            &[],
        );
        assert_eq!(outputs[0].text, "");
        assert_eq!(outputs[0].captured_byte_length, 0);
        assert_eq!(outputs[1].text, "[redacted]");
        assert!(outputs[1].redacted);
    }
}
