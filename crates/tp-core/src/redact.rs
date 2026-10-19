//! Masks credentials before text is logged, audited or persisted.
//! Port of the Go `internal/secrets` package (legacy/go-agent).

use std::sync::OnceLock;

use regex::Regex;

pub const MARK: &str = "[REDACTED]";

fn patterns() -> &'static [Regex] {
    static P: OnceLock<Vec<Regex>> = OnceLock::new();
    P.get_or_init(|| {
        [
            r"sk-ant-[A-Za-z0-9_\-]{20,}",
            r"sk-(proj-|svcacct-)?[A-Za-z0-9_\-]{20,}",
            r"gh[pousr]_[A-Za-z0-9]{30,}",
            r"github_pat_[A-Za-z0-9_]{40,}",
            r"glpat-[A-Za-z0-9_\-]{20,}",
            r"xox[baprs]-[A-Za-z0-9\-]{10,}",
            r"AKIA[0-9A-Z]{16}",
            r"AIza[0-9A-Za-z_\-]{35}",
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            // Must stay last: keeps the key name, masks the value.
            r#"(?i)(api[_-]?key|secret|token|password|passwd)(["']?\s*[:=]\s*["']?)([A-Za-z0-9_\-./+]{12,})"#,
        ]
        .iter()
        .map(|p| Regex::new(p).expect("static redaction pattern"))
        .collect()
    })
}

/// Masks known secret values and common credential shapes.
#[derive(Debug, Default, Clone)]
pub struct Redactor {
    values: Vec<String>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Also masks these literal values (ignored when shorter than 8 chars).
    pub fn add<I, S>(&mut self, values: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.values
            .extend(values.into_iter().map(Into::into).filter(|v| v.len() >= 8));
        self.values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    }

    pub fn redact(&self, s: &str) -> String {
        if s.is_empty() {
            return String::new();
        }
        let mut out = s.to_owned();
        for v in &self.values {
            out = out.replace(v.as_str(), MARK);
        }
        let pats = patterns();
        let last = pats.len() - 1;
        for (i, re) in pats.iter().enumerate() {
            let rep = if i == last {
                "${1}${2}[REDACTED]"
            } else {
                MARK
            };
            out = re.replace_all(&out, rep).into_owned();
        }
        out
    }

    pub fn contains_secret(&self, s: &str) -> bool {
        self.redact(s) != s
    }

    /// Redacts every string inside a JSON value.
    pub fn redact_json(&self, v: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match v {
            Value::String(s) => Value::String(self.redact(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.redact_json(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), self.redact_json(x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Shows only the first 3 and last 4 characters of a credential.
pub fn mask(v: &str) -> String {
    let chars: Vec<char> = v.chars().collect();
    if chars.len() <= 8 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_shapes() {
        let r = Redactor::new();
        for s in [
            "key sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWX end",
            "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            "AKIAABCDEFGHIJKLMNOP",
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----",
        ] {
            let out = r.redact(s);
            assert!(out.contains(MARK), "{s} -> {out}");
            assert!(r.contains_secret(s));
        }
    }

    #[test]
    fn keeps_key_name_for_assignments() {
        let r = Redactor::new();
        assert_eq!(r.redact("API_KEY=abcdefghijklmnop"), "API_KEY=[REDACTED]");
        assert_eq!(
            r.redact(r#""password": "hunter2hunter2""#),
            r#""password": "[REDACTED]""#
        );
    }

    #[test]
    fn literal_values_and_json() {
        let mut r = Redactor::new();
        r.add(["short", "my-literal-secret-value"]);
        assert_eq!(
            r.redact("x my-literal-secret-value y short"),
            "x [REDACTED] y short"
        );
        let v = serde_json::json!({"a": ["my-literal-secret-value"], "n": 3});
        assert_eq!(
            r.redact_json(&v),
            serde_json::json!({"a": ["[REDACTED]"], "n": 3})
        );
    }

    #[test]
    fn plain_text_untouched() {
        assert!(!Redactor::new().contains_secret("cargo test --workspace"));
    }

    #[test]
    fn mask_shows_edges() {
        assert_eq!(mask("abcdefghijkl"), "abc…ijkl");
        assert_eq!(mask("short"), "•••••");
    }
}
