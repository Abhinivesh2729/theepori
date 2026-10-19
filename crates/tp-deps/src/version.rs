//! Lenient version parsing and comparison across ecosystems.
//!
//! Strict semver first; then a fallback that handles Go's `v` prefix,
//! two-component versions (`1.2`), four-component versions (`1.2.3.4`) and
//! PEP 440 pre-releases (`1.0rc1`). Build metadata (pub's `+3`, Go's
//! `+incompatible`) is ignored for ordering, as semver requires.

use std::cmp::Ordering;

use semver::{BuildMetadata, Prerelease, Version};

use crate::Bump;

/// Parse a version string, accepting common non-semver forms.
pub fn parse(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V', '=']).unwrap_or(s).trim();
    if let Ok(v) = Version::parse(s) {
        return Some(v);
    }
    lenient(s)
}

fn lenient(s: &str) -> Option<Version> {
    // Drop build metadata and PEP 440 local versions.
    let s = s.split('+').next()?;
    let mut nums: Vec<u64> = Vec::new();
    let mut rest = s;
    loop {
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if end == 0 {
            break;
        }
        nums.push(rest[..end].parse().ok()?);
        rest = &rest[end..];
        match rest.strip_prefix('.') {
            Some(r) if r.starts_with(|c: char| c.is_ascii_digit()) => rest = r,
            _ => break,
        }
    }
    if nums.is_empty() {
        return None;
    }
    let mut v = Version::new(
        nums[0],
        nums.get(1).copied().unwrap_or(0),
        nums.get(2).copied().unwrap_or(0),
    );
    let tail = rest
        .trim_start_matches(['.', '-', '_'])
        .to_ascii_lowercase();
    let is_pre = [
        "a", "b", "c", "rc", "alpha", "beta", "pre", "preview", "dev",
    ]
    .iter()
    .any(|p| {
        tail.strip_prefix(p).is_some_and(|r| {
            r.is_empty() || r.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        })
    });
    if is_pre {
        let ident: String = tail
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '.' })
            .collect();
        v.pre = Prerelease::new(ident.trim_matches('.')).unwrap_or(Prerelease::EMPTY);
    } else if nums.len() > 3 || !tail.is_empty() {
        // 4th component / post-release: keep for display only.
        v.build = BuildMetadata::new("lenient").unwrap_or(BuildMetadata::EMPTY);
    }
    Some(v)
}

/// Compare two version strings; `None` when either cannot be parsed.
/// Build metadata is ignored, so `1.0.0+1` equals `1.0.0`.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (a, b) = (parse(a)?, parse(b)?);
    Some(a.cmp_precedence(&b))
}

/// True when `a` is strictly newer than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    compare(a, b) == Some(Ordering::Greater)
}

/// Classify the move from `from` to `to` using caret (Cargo/npm/pub) rules:
/// for `0.y.z` a minor change is breaking (Major), and for `0.0.z` any
/// change is breaking.
pub fn bump(from: &str, to: &str) -> Bump {
    let (Some(f), Some(t)) = (parse(from), parse(to)) else {
        return Bump::Unknown;
    };
    let breaking = f.major != t.major
        || (f.major == 0 && f.minor != t.minor)
        || (f.major == 0 && f.minor == 0 && f.patch != t.patch);
    if breaking {
        Bump::Major
    } else if f.minor != t.minor {
        Bump::Minor
    } else {
        // Patch or pre-release change (or equal precedence).
        Bump::Patch
    }
}

/// True when the version is a pre-release.
pub fn is_prerelease(s: &str) -> bool {
    parse(s).is_some_and(|v| !v.pre.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ecosystem_forms() {
        assert_eq!(parse("1.2.3"), Some(Version::new(1, 2, 3)));
        assert_eq!(parse("v1.2.3"), Some(Version::new(1, 2, 3)));
        assert_eq!(parse("1.2"), Some(Version::new(1, 2, 0)));
        assert_eq!(parse("2023.10"), Some(Version::new(2023, 10, 0)));
        assert!(parse("v0.0.0-20210101000000-abcdef123456").is_some());
        assert_eq!(parse("1.2.3+4").map(|v| v.patch), Some(3));
        assert!(parse("v2.0.0+incompatible").is_some());
        assert!(is_prerelease("1.0rc1"));
        assert!(is_prerelease("2.0.0-beta.1"));
        assert!(!is_prerelease("2.0.0.post1"));
        assert_eq!(parse("garbage"), None);
    }

    #[test]
    fn compares() {
        assert!(newer("1.10.0", "1.9.9"));
        assert!(newer("v1.2.4", "v1.2.3"));
        assert!(newer("1.0", "1.0rc1"));
        assert_eq!(compare("1.0.0+1", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare("x", "1.0.0"), None);
    }

    #[test]
    fn bump_classes() {
        assert_eq!(bump("1.2.3", "1.2.4"), Bump::Patch);
        assert_eq!(bump("1.2.3", "1.3.0"), Bump::Minor);
        assert_eq!(bump("1.2.3", "2.0.0"), Bump::Major);
        assert_eq!(bump("0.3.1", "0.4.0"), Bump::Major);
        assert_eq!(bump("0.3.1", "0.3.2"), Bump::Patch);
        assert_eq!(bump("0.0.1", "0.0.2"), Bump::Major);
        assert_eq!(bump("v1.4.0", "v1.5.2"), Bump::Minor);
        assert_eq!(bump("nope", "1.0.0"), Bump::Unknown);
    }
}
