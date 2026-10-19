use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Where a secret applies: a project and an environment within it.
///
/// Conventions:
/// - project `"global"` holds secrets shared by every project
///   ([`Scope::GLOBAL`]);
/// - environment `"default"` is the environment used when none is named
///   ([`Scope::DEFAULT_ENV`]).
///
/// [`crate::Unlocked::env_for`] layers scopes from least to most specific:
/// `global/default` < `global/<env>` < `<project>/default` < `<project>/<env>`.
///
/// Components are 1-64 characters from `A-Z a-z 0-9 . _ -` and start with a
/// letter or digit, so they can never contain the `|` separator used in
/// associated data or a path separator.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub project: String,
    pub environment: String,
}

impl Scope {
    pub const GLOBAL: &'static str = "global";
    pub const DEFAULT_ENV: &'static str = "default";

    pub fn new(project: impl Into<String>, environment: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            environment: environment.into(),
        }
    }

    /// `global/default`: shared by every project and environment.
    pub fn global() -> Self {
        Self::new(Self::GLOBAL, Self::DEFAULT_ENV)
    }

    /// `<project>/default`.
    pub fn project(project: impl Into<String>) -> Self {
        Self::new(project, Self::DEFAULT_ENV)
    }

    pub fn is_global(&self) -> bool {
        self.project == Self::GLOBAL
    }

    pub fn validate(&self) -> Result<()> {
        validate_component(&self.project)?;
        validate_component(&self.environment)
    }

    /// The scopes [`crate::Unlocked::env_for`] reads, least specific first,
    /// without duplicates.
    pub fn layers(&self) -> Vec<Scope> {
        let candidates = [
            Scope::global(),
            Scope::new(Self::GLOBAL, self.environment.clone()),
            Scope::project(self.project.clone()),
            self.clone(),
        ];
        let mut out: Vec<Scope> = Vec::with_capacity(4);
        for c in candidates {
            if !out.contains(&c) {
                out.push(c);
            }
        }
        out
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.project, self.environment)
    }
}

fn validate_component(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidScope(s.to_string()))
    }
}

/// Secret names are environment variable names: `[A-Za-z_][A-Za-z0-9_]*`,
/// at most 256 bytes (same rule as Panuval's env/apiKey/token items).
pub fn validate_name(name: &str) -> Result<()> {
    let b = name.as_bytes();
    let ok = !b.is_empty()
        && b.len() <= 256
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_');
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_are_ordered_and_deduplicated() {
        let s = Scope::new("thinai", "prod");
        assert_eq!(
            s.layers(),
            vec![
                Scope::global(),
                Scope::new("global", "prod"),
                Scope::project("thinai"),
                s.clone()
            ]
        );
        assert_eq!(Scope::global().layers(), vec![Scope::global()]);
        assert_eq!(
            Scope::project("kavi").layers(),
            vec![Scope::global(), Scope::project("kavi")]
        );
    }

    #[test]
    fn validation() {
        assert!(Scope::new("thinai", "prod-1.2_x").validate().is_ok());
        for bad in ["", "-x", "a|b", "a/b", "தமிழ்", &"x".repeat(65)] {
            assert!(Scope::new(bad, "prod").validate().is_err(), "{bad:?}");
            assert!(Scope::new("p", bad).validate().is_err(), "{bad:?}");
        }
        assert!(validate_name("OPENAI_API_KEY").is_ok());
        assert!(validate_name("_x1").is_ok());
        for bad in ["", "1X", "A-B", "A B", "A|B", "é"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }
}
