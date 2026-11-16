//! Keep secrets out of anything KodAct prints.

use serde_json::Value;

/// Keys whose values are secrets (`KODACT_API_KEY`, `apiKeyHelper` output, tokens, passwords, ...).
pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["key", "token", "secret", "password", "passwd", "credential", "auth", "cookie", "session_id"]
        .iter()
        .any(|w| k.contains(w))
        && !k.ends_with("helper")
}

/// A copy of `v` with secret-looking values replaced by `"<redacted>"`.
pub fn redact(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, val)| {
                    let out = if is_secret_key(k) && (val.is_string() || val.is_number()) {
                        Value::String("<redacted>".into())
                    } else {
                        redact(val)
                    };
                    (k.clone(), out)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(redact).collect()),
        Value::String(s) if looks_like_secret(s) => Value::String("<redacted>".into()),
        // A secret inside a value whose key says nothing ("Authorization: Bearer ..." in custom headers).
        Value::String(s) => Value::String(redact_text(s)),
        other => other.clone(),
    }
}

/// Mask secrets inside free text (a shell command, a tool result, a log line):
/// private key blocks, well-known key prefixes, bearer tokens, and `key=value`
/// (or `"key": "value"`) assignments whose name says it is a secret.
pub fn redact_text(s: &str) -> String {
    use std::sync::OnceLock;
    static RES: OnceLock<[regex::Regex; 4]> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            regex::Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----").unwrap(),
            regex::Regex::new(
                r"\b(sk-[A-Za-z0-9_\-]{16,}|[sr]k_(?:live|test)_[A-Za-z0-9]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_\-]{20,}|xox[abpr]-[A-Za-z0-9\-]{10,}|AKIA[A-Z0-9]{16}|AIza[A-Za-z0-9_\-]{35}|npm_[A-Za-z0-9]{36})",
            )
            .unwrap(),
            regex::Regex::new(r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=\-]{8,}").unwrap(),
            regex::Regex::new(
                r#"(?i)\b([A-Z0-9_]*(?:api[_-]?key|token|secret|password|passwd|credential)[A-Z0-9_]*)(["']?\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s"',;]+)"#,
            )
            .unwrap(),
        ]
    });
    let s = res[0].replace_all(s, "<redacted private key>");
    let s = res[1].replace_all(&s, "<redacted>");
    let s = res[2].replace_all(&s, "$1 <redacted>");
    res[3]
        .replace_all(&s, |c: &regex::Captures| {
            // Keep the quotes around a quoted value, so JSON stays readable.
            let v = &c[3];
            let q = if v.starts_with('"') && c[2].starts_with('"') { "\"" } else { "" };
            format!("{}{}{q}<redacted>{q}", &c[1], &c[2])
        })
        .into_owned()
}

/// [`redact`], and [`redact_text`] on every string: for copies of transcripts
/// and logs that leave the session (`/feedback`).
pub fn redact_deep(v: &Value) -> Value {
    match redact(v) {
        Value::String(s) => Value::String(redact_text(&s)),
        Value::Array(a) => Value::Array(a.iter().map(redact_deep).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, redact_deep(&v))).collect()),
        other => other,
    }
}

/// Values that are secrets whatever their key: well-known key prefixes and bearer tokens.
fn looks_like_secret(s: &str) -> bool {
    let t = s.trim();
    ["sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA"].iter().any(|p| t.starts_with(p)) && t.len() >= 20
        || t.to_ascii_lowercase().starts_with("bearer ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_secrets_inside_text() {
        let t = "export KODACT_API_KEY=abc123 && curl -H 'Authorization: Bearer abcdefgh123' x; key sk-abcdefghijklmnop1234 ok";
        let r = redact_text(t);
        assert_eq!(
            r,
            "export KODACT_API_KEY=<redacted> && curl -H 'Authorization: Bearer <redacted>' x; key <redacted> ok"
        );
        assert_eq!(redact_text("password: \"hunter2 two\" done"), "password: <redacted> done");
        assert_eq!(redact_text("tokens used: 1200"), "tokens used: 1200", "no assignment, no masking");
        let v = json!({"message": {"content": [{"text": "my token=s3cr3t"}]}, "apiKey": "x"});
        let d = redact_deep(&v);
        assert_eq!(d["message"]["content"][0]["text"], "my token=<redacted>");
        assert_eq!(d["apiKey"], "<redacted>");
    }

    #[test]
    fn redacts_by_key_and_by_value() {
        let v = json!({
            "model": "opus",
            "env": {"KODACT_API_KEY": "abc", "OTHER": "sk-abcdefghijklmnopqrstuvwxyz", "PATH": "/bin"},
            "apiKeyHelper": "cat ~/.key",
            "headers": ["Authorization: Bearer xyz", "Bearer abcdef"],
        });
        let r = redact(&v);
        assert_eq!(r["model"], "opus");
        assert_eq!(r["env"]["KODACT_API_KEY"], "<redacted>");
        assert_eq!(r["env"]["OTHER"], "<redacted>");
        assert_eq!(r["env"]["PATH"], "/bin");
        assert_eq!(r["apiKeyHelper"], "cat ~/.key", "the helper command is not itself a secret");
        assert_eq!(r["headers"][1], "<redacted>");
    }

    #[test]
    fn redacts_headers_quoted_keys_and_private_keys() {
        // A secret inside a value whose key says nothing (custom headers).
        let v = json!({"env": {"KODACT_CUSTOM_HEADERS": "Authorization: Bearer abcdefgh12345\nX-Team: core"}});
        assert_eq!(redact(&v)["env"]["KODACT_CUSTOM_HEADERS"], "Authorization: Bearer <redacted>\nX-Team: core");
        // Azure's and gateways' own key headers, as sent for chat completions (`openai.authHeader`).
        let v = json!({"env": {"KODACT_CUSTOM_HEADERS": "api-key: abcdef123456\nX-API-Key: zyx987654\nX-Team: core"}});
        assert_eq!(
            redact(&v)["env"]["KODACT_CUSTOM_HEADERS"],
            "api-key: <redacted>\nX-API-Key: <redacted>\nX-Team: core"
        );
        // JSON or YAML text quoting the key name.
        assert_eq!(
            redact_text(r#"{"api_key": "abc123", "region": "eu"}"#),
            r#"{"api_key": "<redacted>", "region": "eu"}"#
        );
        assert_eq!(redact_text("client_secret: 'xyz'"), "client_secret: <redacted>");
        // A private key someone's tool read.
        let pem =
            "id:\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAA\n-----END OPENSSH PRIVATE KEY-----\nend";
        assert_eq!(redact_text(pem), "id:\n<redacted private key>\nend");
        // More well-known token shapes.
        for t in [
            "AIzaSyA1234567890abcdefghijklmnopqrstuv",
            "npm_abcdefghijklmnopqrstuvwxyz0123456789",
            "glpat-abcdefghij0123456789",
            "sk_live_abcdefghijklmnop1234",
        ] {
            assert_eq!(redact_text(&format!("token {t} here")), "token <redacted> here", "{t}");
        }
    }
}
