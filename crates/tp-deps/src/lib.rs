#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Dependency and security pipeline (SF-9, M8).
//!
//! Discovers manifests and lockfiles in a repository, reads the resolved
//! packages, looks up advisories (OSV.dev) and latest registry versions, and
//! drafts a *recommended* upgrade plan. The plan is deliberately conservative:
//! security fixes take the smallest version that resolves every advisory,
//! routine upgrades are proposed only for direct dependencies (unless policy
//! widens that), and every item carries a semver bump class, risk and a
//! human-readable rationale. Nothing here edits files or upgrades anything.
//!
//! Network access is isolated behind [`AdvisorySource`] and
//! [`RegistrySource`] so callers (and tests) can substitute their own.

mod discover;
mod error;
mod http;
mod osv;
mod parse;
mod plan;
mod registry;
mod severity;
pub mod version;

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use discover::discover;
pub use error::{Error, Result};
pub use osv::{BATCH_LIMIT, Osv, advisory_from_vuln};
pub use parse::packages;
pub use plan::{PlanPolicy, plan};
pub use registry::{Registries, collect_latest};
pub use severity::{cvss3_base_score, severity_from_score};

/// A package ecosystem. Serialized in lowercase (`npm`, `cargo`, `go`, `pub`, `pypi`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Npm,
    Cargo,
    Go,
    Pub,
    PyPI,
}

impl Ecosystem {
    /// The ecosystem name OSV.dev uses.
    pub fn osv_name(self) -> &'static str {
        match self {
            Ecosystem::Npm => "npm",
            Ecosystem::Cargo => "crates.io",
            Ecosystem::Go => "Go",
            Ecosystem::Pub => "Pub",
            Ecosystem::PyPI => "PyPI",
        }
    }

    /// Inverse of [`Ecosystem::osv_name`].
    pub fn from_osv_name(name: &str) -> Option<Self> {
        match name {
            "npm" => Some(Ecosystem::Npm),
            "crates.io" => Some(Ecosystem::Cargo),
            "Go" => Some(Ecosystem::Go),
            "Pub" => Some(Ecosystem::Pub),
            "PyPI" => Some(Ecosystem::PyPI),
            _ => None,
        }
    }

    /// Canonical form of a package name in this ecosystem (PEP 503 for PyPI,
    /// unchanged elsewhere). Used to match advisories and registry results.
    pub fn normalize_name(self, name: &str) -> String {
        match self {
            Ecosystem::PyPI => {
                let mut out = String::with_capacity(name.len());
                let mut prev_sep = false;
                for c in name.trim().chars() {
                    if matches!(c, '-' | '_' | '.') {
                        if !prev_sep {
                            out.push('-');
                        }
                        prev_sep = true;
                    } else {
                        out.extend(c.to_lowercase());
                        prev_sep = false;
                    }
                }
                out
            }
            _ => name.trim().to_string(),
        }
    }
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Ecosystem::Npm => "npm",
            Ecosystem::Cargo => "cargo",
            Ecosystem::Go => "go",
            Ecosystem::Pub => "pub",
            Ecosystem::PyPI => "pypi",
        })
    }
}

/// Latest known version per (ecosystem, package name), as consumed by [`plan`].
pub type LatestVersions = std::collections::HashMap<(Ecosystem, String), String>;

/// A discovered manifest and, when present, the lockfile that pins it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub path: PathBuf,
    pub ecosystem: Ecosystem,
    pub lockfile: Option<PathBuf>,
}

/// A resolved package version. `direct` is true when the manifest names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub ecosystem: Ecosystem,
    pub direct: bool,
}

/// Advisory severity, ordered from least to most severe.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Unknown,
    Low,
    Medium,
    High,
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Unknown => "unknown",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        })
    }
}

/// One vulnerability affecting one installed package version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advisory {
    pub id: String,
    pub aliases: Vec<String>,
    pub package: String,
    pub ecosystem: Ecosystem,
    /// The installed version this advisory was matched against.
    pub affected_version: String,
    pub severity: Severity,
    pub summary: String,
    /// Versions that fix the vulnerability, across all affected ranges.
    pub fixed_versions: Vec<String>,
}

/// Something that can report advisories for a set of packages.
pub trait AdvisorySource {
    fn query(&self, pkgs: &[Package]) -> Result<Vec<Advisory>>;
}

/// Something that can report the latest stable version of a package.
/// `Ok(None)` means the registry does not know the package.
pub trait RegistrySource {
    fn latest(&self, pkg: &Package) -> Result<Option<String>>;
}

/// Why an upgrade is recommended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "advisories")]
pub enum Reason {
    /// Fixes the listed advisory ids.
    Security(Vec<String>),
    /// A newer release exists.
    Outdated,
}

/// Semver distance between the installed and the proposed version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bump {
    Patch,
    Minor,
    Major,
    Unknown,
}

impl fmt::Display for Bump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Bump::Patch => "patch",
            Bump::Minor => "minor",
            Bump::Major => "major",
            Bump::Unknown => "unknown",
        })
    }
}

/// How risky applying the upgrade is (not how severe the vulnerability is).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

/// One recommended upgrade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeItem {
    pub package: String,
    pub ecosystem: Ecosystem,
    pub from: String,
    /// Proposed version. Equal to `from` only when an advisory has no fixed
    /// release yet (bump `Unknown`, rationale says so).
    pub to: String,
    pub reasons: Vec<Reason>,
    pub bump: Bump,
    pub risk: Risk,
    pub breaking: bool,
    /// Highest severity among the advisories this item addresses.
    pub severity: Option<Severity>,
    pub direct: bool,
    pub rationale: String,
}

impl UpgradeItem {
    /// Advisory ids this item addresses.
    pub fn advisory_ids(&self) -> Vec<&str> {
        self.reasons
            .iter()
            .flat_map(|r| match r {
                Reason::Security(ids) => ids.iter().map(String::as_str).collect(),
                Reason::Outdated => Vec::new(),
            })
            .collect()
    }

    pub fn is_security(&self) -> bool {
        self.reasons
            .iter()
            .any(|r| matches!(r, Reason::Security(_)))
    }
}
