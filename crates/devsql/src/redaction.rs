use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn redact_sensitive_text(text: &str) -> String {
    let mut redacted = json_secret_regex()
        .replace_all(text, "${1}<redacted>${2}")
        .into_owned();
    redacted = authorization_regex()
        .replace_all(&redacted, "${1}${2}${3}<redacted>")
        .into_owned();
    redacted = bearer_regex()
        .replace_all(&redacted, "Bearer <redacted>")
        .into_owned();
    redacted = known_token_regex()
        .replace_all(&redacted, "<redacted>")
        .into_owned();
    redacted = url_userinfo_regex()
        .replace_all(&redacted, "${1}<redacted>@")
        .into_owned();
    redacted = url_query_regex()
        .replace_all(&redacted, "${1}<redacted>")
        .into_owned();
    key_value_regex()
        .replace_all(&redacted, "${1}${2}<redacted>")
        .into_owned()
}

fn json_secret_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)("(?:authorization|api[_-]?key|access[_-]?token|refresh[_-]?token|secret|password|cookie)"\s*:\s*")[^"]*(")"#,
        )
        .expect("valid JSON secret regex")
    })
}

fn bearer_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+").expect("valid bearer regex")
    })
}

fn authorization_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b(authorization)(\s*[:=]\s*)(Bearer\s+)?[^\s,;&]+")
            .expect("valid authorization regex")
    })
}

fn known_token_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:sk-[A-Za-z0-9_-]{8,}|github_pat_[A-Za-z0-9_]{8,}|gh[pousr]_[A-Za-z0-9]{8,}|xox[baprs]-[A-Za-z0-9-]{8,})\b",
        )
        .expect("valid token regex")
    })
}

fn url_userinfo_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)(https?://)[^/\s:@]+:[^@\s/]+@").expect("valid URL userinfo regex")
    })
}

fn url_query_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)([?&](?:authorization|api[_-]?key|token|access[_-]?token|refresh[_-]?token|secret|password|cookie)=)[^&#\s]+",
        )
        .expect("valid URL query regex")
    })
}

fn key_value_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)\b((?:api[_-]?key|access[_-]?token|refresh[_-]?token|secret|password|cookie)\b)(\s*[:=]\s*)[^\s,;&]+",
        )
        .expect("valid key-value regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_bearer_and_known_token_prefixes() {
        let input =
            "Authorization: Bearer abc.def.ghi sk-abcdefgh12345678 github_pat_abcdefgh12345678";
        let redacted = redact_sensitive_text(input);

        assert!(!redacted.contains("abc.def.ghi"));
        assert!(!redacted.contains("sk-abcdefgh12345678"));
        assert!(!redacted.contains("github_pat_abcdefgh12345678"));
        assert!(redacted.contains("<redacted>"));
    }

    #[test]
    fn redacts_structured_secret_values_and_sensitive_urls() {
        let input = concat!(
            r#"{"password":"hello world","api_key":"abc123"} "#,
            "https://user:pass@example.com/path?access_token=url-secret&safe=yes"
        );
        let redacted = redact_sensitive_text(input);

        for secret in ["hello world", "abc123", "user:pass", "url-secret"] {
            assert!(!redacted.contains(secret), "{secret} leaked in {redacted}");
        }
        assert!(redacted.contains("safe=yes"));
    }
}
