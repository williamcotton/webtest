use url::Url;
use webtest_browser::PageEvidence;

pub(super) fn redact_and_truncate(value: &str, secrets: &[String], max_bytes: usize) -> String {
    let redacted = secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(value.to_owned(), |value, secret| {
            value.replace(secret, "[redacted]")
        });
    truncate_utf8(&redacted, max_bytes)
}

pub(super) fn redact_url_query(value: &str, sensitive: &[String]) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return truncate_utf8(value, MAX_INSPECTION_URL_BYTES);
    };
    let pairs = url
        .query_pairs()
        .map(|(name, value)| {
            let replacement = if sensitive
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&name))
            {
                "[redacted]".to_owned()
            } else {
                value.into_owned()
            };
            (name.into_owned(), replacement)
        })
        .collect::<Vec<_>>();
    if !pairs.is_empty() {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    truncate_utf8(url.as_str(), MAX_INSPECTION_URL_BYTES)
}

const MAX_INSPECTION_URL_BYTES: usize = 4_096;

pub(super) fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.into();
    }
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

pub(super) fn redact_evidence(
    evidence: &mut PageEvidence,
    redactions: &[String],
    query_parameters: &[String],
) {
    let redact = |value: &mut String| {
        for secret in redactions.iter().filter(|secret| !secret.is_empty()) {
            *value = value.replace(secret, "<redacted>");
        }
    };
    if let Some(value) = &mut evidence.current_url {
        *value = redact_url_query(value, query_parameters);
        redact(value)
    }
    if let Some(value) = &mut evidence.title {
        redact(value)
    }
    if let Some(value) = &mut evidence.dom_snapshot {
        redact(value)
    }
    for value in &mut evidence.console_errors {
        redact(value)
    }
    for candidate in &mut evidence.candidates {
        if let Some(value) = &mut candidate.name {
            redact(value)
        }
        if let Some(value) = &mut candidate.text {
            redact(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use webtest_browser::CandidateEvidence;

    use super::*;

    #[test]
    fn truncation_preserves_utf8_boundaries_without_a_suffix() {
        assert_eq!(truncate_utf8("ééé", 3), "é");
        assert_eq!(truncate_utf8("abc", 3), "abc");
        assert_eq!(truncate_utf8("abc", 0), "");
    }

    #[test]
    fn page_outputs_preserve_tokens_and_case_insensitive_query_redaction() {
        assert_eq!(
            redact_and_truncate("prefix secret suffix", &["secret".into()], 128),
            "prefix [redacted] suffix"
        );
        let url = redact_url_query(
            "http://example.test/?Token=secret&view=full&code=private",
            &["token".into(), "CODE".into()],
        );
        assert!(!url.contains("secret"));
        assert!(!url.contains("private"));
        assert!(url.contains("view=full"));

        let mut evidence = PageEvidence {
            current_url: Some("http://example.test/?token=secret".into()),
            title: Some("secret".into()),
            dom_snapshot: Some("<body>secret</body>".into()),
            console_errors: vec!["secret".into()],
            candidates: vec![CandidateEvidence {
                tag: "div".into(),
                name: Some("secret".into()),
                text: Some("secret".into()),
                ..CandidateEvidence::default()
            }],
            ..PageEvidence::default()
        };
        redact_evidence(
            &mut evidence,
            &["secret".into(), String::new()],
            &["token".into()],
        );
        assert!(!format!("{evidence:?}").contains("secret"));
        assert!(format!("{evidence:?}").contains("<redacted>"));
    }
}
