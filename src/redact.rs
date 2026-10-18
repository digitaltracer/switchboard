//! Secret redaction: masks obvious credentials in text and JSON before Switchboard logs or sends them.

use std::sync::LazyLock;

use regex::{Regex, RegexSet};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const MASK: &str = "[redacted]";

/// Object keys whose string values are always masked, matched after dropping `-` and `_`.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "accesskey",
    "privatekey",
    "authorization",
    "cookie",
    "credential",
];

struct Rule {
    re: Regex,
    replace: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |re: &str, replace| Rule {
        re: Regex::new(re).expect("valid redaction pattern"),
        replace,
    };
    vec![
        rule(
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(-----END [A-Z ]*PRIVATE KEY-----|$)",
            "[redacted private key]",
        ),
        rule(
            r"(?i)\b([a-z][a-z0-9+.-]*://[^/\s:@]+):[^@\s/]+@",
            "${1}:[redacted]@",
        ),
        rule(
            r"(?i)\b(bearer)(\s+)[A-Za-z0-9._~+/=-]{8,}",
            "${1}${2}[redacted]",
        ),
        rule(
            r#"(?i)\b(authorization"?\s*[:=]\s*"?(?:basic|digest|token)\s+)[^\s"',]+"#,
            "${1}[redacted]",
        ),
        rule(
            r#"(?i)(--?[a-z0-9-]*(?:password|passwd|secret|token|api-?key|access-?key)[a-z0-9-]*)(=|\s+)("[^"]*"|'[^']*'|[^\s"']+)"#,
            "${1}${2}[redacted]",
        ),
        rule(
            r#"(?i)\b([a-z0-9_]*(?:secret|token|password|passwd|api_?key|access_?key|private_?key|credentials?)[a-z0-9_]*)("?\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s"',;&|]+)"#,
            "${1}${2}[redacted]",
        ),
        rule(
            r"\b(?:sk-(?:ant-|proj-)?[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|glpat-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
            MASK,
        ),
    ]
});

static ANY: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new(RULES.iter().map(|r| r.re.as_str())).expect("valid set"));

pub fn redact_str(s: &str) -> String {
    if !ANY.is_match(s) {
        return s.to_owned();
    }
    let mut out = s.to_owned();
    for rule in RULES.iter() {
        out = rule.re.replace_all(&out, rule.replace).into_owned();
    }
    out
}

fn secret_key(key: &str) -> bool {
    let k: String = key
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect();
    SECRET_KEYS.iter().any(|s| k.contains(s))
}

/// A copy of `v` with secrets masked; numbers and booleans are kept.
pub fn redact(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact_str(s)),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = match v {
                        Value::String(_) if secret_key(k) => Value::String(MASK.into()),
                        other => redact(other),
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A copy of any serializable value with secrets masked in its strings.
pub fn redact_as<T: Serialize + DeserializeOwned>(v: T) -> T {
    let json = serde_json::to_value(&v).expect("serializable");
    serde_json::from_value(redact(&json)).unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn masks_common_secrets_in_text() {
        let cases = [
            (
                "curl -H 'Authorization: Bearer abcdefghijklmnop' https://x",
                "curl -H 'Authorization: Bearer [redacted]' https://x",
            ),
            (
                "export GITHUB_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ && make",
                "export GITHUB_TOKEN=[redacted] && make",
            ),
            (
                "psql postgres://app:hunter2@db:5432/x",
                "psql postgres://app:[redacted]@db:5432/x",
            ),
            (
                "mysql --password=s3cr3t -u root",
                "mysql --password=[redacted] -u root",
            ),
            ("login --token abc123def456", "login --token [redacted]"),
            ("key sk-ant-api03-abcdefghijklmnopqrst", "key [redacted]"),
            ("AWS AKIAABCDEFGHIJKLMNOP here", "AWS [redacted] here"),
            (r#"{"api_key": "zzz-123"}"#, r#"{"api_key": [redacted]}"#),
            (
                "Authorization: Basic Zm9vOmJhcg==",
                "Authorization: Basic [redacted]",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(redact_str(input), want, "{input}");
        }
        let pem = "a\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\nBBBB\n-----END OPENSSH PRIVATE KEY-----\nb";
        assert_eq!(redact_str(pem), "a\n[redacted private key]\nb");
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        for s in [
            "cargo test --lib hub",
            "git commit -m 'fix token parsing'",
            "Basic configuration for the bearer of news",
            "ls -la /tmp",
            "max_tokens 100",
        ] {
            assert_eq!(redact_str(s), s);
        }
    }

    #[test]
    fn masks_secret_keys_in_json_but_keeps_numbers() {
        let v = json!({
            "command": "echo hi",
            "env": {"DB_PASSWORD": "pw", "HOME": "/home/x"},
            "headers": [{"Authorization": "Basic Zm9vOmJhcg=="}],
            "max_tokens": 100,
        });
        assert_eq!(
            redact(&v),
            json!({
                "command": "echo hi",
                "env": {"DB_PASSWORD": "[redacted]", "HOME": "/home/x"},
                "headers": [{"Authorization": "[redacted]"}],
                "max_tokens": 100,
            })
        );
    }
}
