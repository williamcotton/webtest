use webtest_browser::BrowserError;

pub fn resolve_browser_url(base_url: Option<&str>, value: &str) -> Result<String, BrowserError> {
    if is_absolute_url(value) {
        return Ok(normalize_url(value));
    }
    let base = base_url.ok_or_else(|| BrowserError::NavigationFailed {
        url: value.into(),
        reason: "relative URL requires browser.base_url".into(),
    })?;
    let resolved = if value.starts_with('/') {
        let scheme_end = base.find("://").map(|index| index + 3).unwrap_or(0);
        let authority_end = base[scheme_end..]
            .find('/')
            .map(|index| scheme_end + index)
            .unwrap_or(base.len());
        format!("{}{}", &base[..authority_end], value)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), value)
    };
    Ok(normalize_url(&resolved))
}

fn is_absolute_url(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && !rest.is_empty()
            && scheme.chars().next().is_some_and(char::is_alphabetic)
            && scheme.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            })
    })
}

fn normalize_url(value: &str) -> String {
    let Some(scheme) = value.find("://") else {
        return value.into();
    };
    let after_scheme = scheme + 3;
    if !value[after_scheme..].contains(['/', '?', '#']) {
        format!("{value}/")
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_normalizes_absolute_urls() {
        assert_eq!(
            resolve_browser_url(Some("http://example.test/base"), "/login").unwrap(),
            "http://example.test/login"
        );
        assert_eq!(
            resolve_browser_url(None, "http://example.test").unwrap(),
            "http://example.test/"
        );
        assert!(resolve_browser_url(None, "/login").is_err());
    }
}
