//! Severity derivation: CVSS v3 base score and qualitative labels.

use crate::Severity;

/// Map a CVSS base score to the CVSS qualitative rating.
/// `0.0` (no impact) and out-of-range scores map to `Unknown`.
pub fn severity_from_score(score: f64) -> Severity {
    if !(score > 0.0 && score <= 10.0) {
        Severity::Unknown
    } else if score >= 9.0 {
        Severity::Critical
    } else if score >= 7.0 {
        Severity::High
    } else if score >= 4.0 {
        Severity::Medium
    } else {
        Severity::Low
    }
}

/// Map a database label (GHSA `MODERATE`, RustSec/OSV `high`, ...) to a severity.
pub(crate) fn severity_from_label(label: &str) -> Severity {
    match label.trim().to_ascii_lowercase().as_str() {
        "critical" => Severity::Critical,
        "high" | "important" => Severity::High,
        "moderate" | "medium" => Severity::Medium,
        "low" | "minor" => Severity::Low,
        _ => Severity::Unknown,
    }
}

/// CVSS v3.0/v3.1 base score from a vector string such as
/// `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H` (9.8).
/// Returns `None` if any base metric is missing or invalid.
pub fn cvss3_base_score(vector: &str) -> Option<f64> {
    let mut parts = vector.trim().split('/');
    if !parts.next()?.starts_with("CVSS:3") {
        return None;
    }
    let mut m = std::collections::HashMap::new();
    for p in parts {
        let (k, v) = p.split_once(':')?;
        m.insert(k, v);
    }
    let changed = match *m.get("S")? {
        "U" => false,
        "C" => true,
        _ => return None,
    };
    let av = match *m.get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let ac = match *m.get("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let pr = match (*m.get("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match *m.get("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let cia = |k: &str| -> Option<f64> {
        match *m.get(k)? {
            "H" => Some(0.56),
            "L" => Some(0.22),
            "N" => Some(0.0),
            _ => None,
        }
    };
    let (c, i, a) = (cia("C")?, cia("I")?, cia("A")?);
    let iss = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);
    let impact = if changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02f64).powi(15)
    } else {
        6.42 * iss
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploitability = 8.22 * av * ac * pr * ui;
    let raw = if changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };
    Some(roundup(raw))
}

/// CVSS v3.1 Roundup: smallest one-decimal number >= input, robust to
/// floating-point noise.
fn roundup(x: f64) -> f64 {
    let int_input = (x * 100_000.0).round() as i64;
    if int_input % 10_000 == 0 {
        int_input as f64 / 100_000.0
    } else {
        ((int_input / 10_000) + 1) as f64 / 10.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cvss3_known_vectors() {
        let cases = [
            ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H", 9.8),
            ("CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:C/C:L/I:L/A:N", 6.1),
            ("CVSS:3.0/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H", 7.5),
            ("CVSS:3.1/AV:L/AC:H/PR:H/UI:R/S:U/C:L/I:N/A:N", 1.8),
            ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H", 10.0),
            ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N", 0.0),
        ];
        for (v, want) in cases {
            assert_eq!(cvss3_base_score(v), Some(want), "{v}");
        }
        assert_eq!(cvss3_base_score("CVSS:2.0/AV:N"), None);
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L"), None);
    }

    #[test]
    fn labels_and_scores() {
        assert_eq!(severity_from_score(9.8), Severity::Critical);
        assert_eq!(severity_from_score(7.0), Severity::High);
        assert_eq!(severity_from_score(6.1), Severity::Medium);
        assert_eq!(severity_from_score(1.6), Severity::Low);
        assert_eq!(severity_from_score(0.0), Severity::Unknown);
        assert_eq!(severity_from_label("MODERATE"), Severity::Medium);
        assert_eq!(severity_from_label("critical"), Severity::Critical);
        assert_eq!(severity_from_label("whatever"), Severity::Unknown);
    }
}
