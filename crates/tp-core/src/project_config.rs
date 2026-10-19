//! Per-project configuration: `projects/<slug>/project.yaml` and `verify.yaml`.
//!
//! Project files are untrusted relative to the global policy: they can only
//! tighten it ([`effective_policy`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{Policy, RiskLimit};
use crate::paths::FactoryPaths;
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ProjectConfig {
    pub schema: u32,
    pub base_branch: String,
    /// `owner/name`; detected from the `origin` remote when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    pub intake: Intake,
    pub policy: PolicyOverrides,
    /// Extra instructions appended to every coding run for this project.
    pub instructions: String,
    pub deploy: Vec<DeployEnv>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            schema: 1,
            base_branch: "main".into(),
            github: None,
            intake: Intake::default(),
            policy: PolicyOverrides::default(),
            instructions: String::new(),
            deploy: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Intake {
    pub github_issues: bool,
    /// Only issues carrying one of these labels (empty = all open issues).
    pub labels: Vec<String>,
    pub ci_failures: bool,
    pub dependencies: bool,
}

impl Default for Intake {
    fn default() -> Self {
        Self {
            github_issues: true,
            labels: Vec::new(),
            ci_failures: true,
            dependencies: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PolicyOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_approve_max_risk: Option<RiskLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_merge_max_risk: Option<RiskLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_deploy_max_risk: Option<RiskLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_web: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny_commands: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny_paths: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub high_risk_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeployEnv {
    pub name: String,
    /// Shell command run in the project checkout at the merged commit.
    pub command: String,
    #[serde(default)]
    pub production: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_url: Option<String>,
    /// URL whose body must contain the deployed commit SHA (short or full).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_command: Option<String>,
    /// Vault environment whose secrets are injected (default: name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct VerifyConfig {
    pub schema: u32,
    pub checks: Vec<Check>,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            schema: 1,
            checks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    /// build | test | lint | typecheck | security | web | device | custom
    pub kind: String,
    pub run: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "yes")]
    pub required: bool,
    /// Vault environment to inject (none by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<String>,
}

fn default_timeout() -> u64 {
    900
}
fn yes() -> bool {
    true
}

pub fn project_yaml(paths: &FactoryPaths, slug: &str) -> std::path::PathBuf {
    paths.project_dir(slug).join("project.yaml")
}
pub fn verify_yaml(paths: &FactoryPaths, slug: &str) -> std::path::PathBuf {
    paths.project_dir(slug).join("verify.yaml")
}

fn load_yaml<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T> {
    match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(T::default()),
        Ok(raw) => serde_yaml::from_str(&raw).map_err(|e| Error::Config {
            path: path.into(),
            message: e.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(Error::io(path, e)),
    }
}

pub fn load_project(paths: &FactoryPaths, slug: &str) -> Result<ProjectConfig> {
    load_yaml(&project_yaml(paths, slug))
}

pub fn load_verify(paths: &FactoryPaths, slug: &str) -> Result<VerifyConfig> {
    load_yaml(&verify_yaml(paths, slug))
}

pub fn parse_project(text: &str) -> Result<ProjectConfig> {
    serde_yaml::from_str(text).map_err(|e| Error::Invalid(format!("project.yaml: {e}")))
}

pub fn parse_verify(text: &str) -> Result<VerifyConfig> {
    let v: VerifyConfig =
        serde_yaml::from_str(text).map_err(|e| Error::Invalid(format!("verify.yaml: {e}")))?;
    for c in &v.checks {
        if c.name.trim().is_empty() || c.run.trim().is_empty() {
            return Err(Error::Invalid(
                "verify.yaml: every check needs a name and run".into(),
            ));
        }
    }
    Ok(v)
}

pub fn write_atomic(path: &Path, body: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| Error::io(d, e))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
}

/// Writes default project.yaml / verify.yaml for a new project (detecting
/// the base branch, GitHub repo and build commands) unless they exist.
pub fn init_project_files(paths: &FactoryPaths, slug: &str, repo: &Path) -> Result<()> {
    let py = project_yaml(paths, slug);
    if !py.exists() {
        let cfg = ProjectConfig {
            base_branch: crate::git::default_branch(repo).unwrap_or_else(|| "main".into()),
            github: crate::git::github_repo(repo),
            ..ProjectConfig::default()
        };
        let body = serde_yaml::to_string(&cfg).map_err(|e| Error::Invalid(e.to_string()))?;
        write_atomic(
            &py,
            &format!(
                "# Thee Pori project config. Project settings may only tighten the global policy.\n{body}"
            ),
        )?;
    }
    let vy = verify_yaml(paths, slug);
    if !vy.exists() {
        let v = VerifyConfig {
            schema: 1,
            checks: detect_checks(repo),
        };
        let body = serde_yaml::to_string(&v).map_err(|e| Error::Invalid(e.to_string()))?;
        write_atomic(
            &vy,
            &format!(
                "# Deterministic checks run by tpd (never by the agent). Missing evidence = not passed.\n{body}"
            ),
        )?;
    }
    Ok(())
}

/// Best-effort default checks from the repository's build files.
pub fn detect_checks(repo: &Path) -> Vec<Check> {
    let check = |name: &str, kind: &str, run: &str| Check {
        name: name.into(),
        kind: kind.into(),
        run: run.into(),
        timeout_secs: default_timeout(),
        required: true,
        secrets: None,
    };
    let mut out = Vec::new();
    if repo.join("Cargo.toml").exists() {
        out.push(check("cargo build", "build", "cargo build --all-targets"));
        out.push(check("cargo test", "test", "cargo test"));
    }
    if let Ok(raw) = std::fs::read_to_string(repo.join("package.json")) {
        let scripts = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.get("scripts").cloned())
            .unwrap_or_default();
        let has = |s: &str| scripts.get(s).is_some();
        let install = if repo.join("package-lock.json").exists() {
            "npm ci && "
        } else {
            "npm install && "
        };
        if has("build") {
            out.push(check(
                "npm build",
                "build",
                &format!("{install}npm run build"),
            ));
        }
        if has("lint") {
            out.push(check("npm lint", "lint", "npm run lint"));
        }
        if has("test") {
            out.push(check("npm test", "test", "npm test"));
        }
    }
    if repo.join("go.mod").exists() {
        out.push(check("go build", "build", "go build ./..."));
        out.push(check("go test", "test", "go test ./..."));
    }
    if repo.join("pubspec.yaml").exists() {
        out.push(check("flutter analyze", "lint", "flutter analyze"));
        out.push(check("flutter test", "test", "flutter test"));
    }
    if repo.join("pyproject.toml").exists() || repo.join("requirements.txt").exists() {
        out.push(check("pytest", "test", "python3 -m pytest -q"));
    }
    out
}

/// Global policy tightened by project overrides. Never loosens anything.
pub fn effective_policy(global: &Policy, o: &PolicyOverrides) -> Policy {
    let mut p = global.clone();
    let tighten = |g: RiskLimit, o: Option<RiskLimit>| o.map_or(g, |o| g.min(o));
    p.auto_approve_max_risk = tighten(p.auto_approve_max_risk, o.auto_approve_max_risk);
    p.auto_merge_max_risk = tighten(p.auto_merge_max_risk, o.auto_merge_max_risk);
    p.auto_deploy_max_risk = tighten(p.auto_deploy_max_risk, o.auto_deploy_max_risk);
    if let Some(w) = o.allow_web {
        p.allow_web = p.allow_web && w;
    }
    p.deny_commands.extend(o.deny_commands.iter().cloned());
    p.deny_paths.extend(o.deny_paths.iter().cloned());
    p.high_risk_paths.extend(o.high_risk_paths.iter().cloned());
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_only_tighten() {
        let g = Policy {
            auto_merge_max_risk: RiskLimit::Low,
            allow_web: false,
            ..Policy::default()
        };
        let o = PolicyOverrides {
            auto_merge_max_risk: Some(RiskLimit::High),
            auto_deploy_max_risk: Some(RiskLimit::Medium),
            allow_web: Some(true),
            deny_commands: vec!["make deploy".into()],
            ..Default::default()
        };
        let e = effective_policy(&g, &o);
        assert_eq!(e.auto_merge_max_risk, RiskLimit::Low);
        assert_eq!(e.auto_deploy_max_risk, RiskLimit::None);
        assert!(!e.allow_web);
        assert!(e.deny_commands.contains(&"make deploy".to_string()));
        let o = PolicyOverrides {
            auto_merge_max_risk: Some(RiskLimit::None),
            ..Default::default()
        };
        assert_eq!(
            effective_policy(&g, &o).auto_merge_max_risk,
            RiskLimit::None
        );
    }

    #[test]
    fn detects_checks() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(
            d.path().join("package.json"),
            r#"{"scripts":{"test":"x","build":"y"}}"#,
        )
        .unwrap();
        let names: Vec<_> = detect_checks(d.path())
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            names,
            ["cargo build", "cargo test", "npm build", "npm test"]
        );
    }

    #[test]
    fn verify_parse_validates() {
        assert!(parse_verify("checks:\n  - {name: t, kind: test, run: ''}\n").is_err());
        let v = parse_verify("checks:\n  - {name: t, kind: test, run: 'true'}\n").unwrap();
        assert_eq!(v.checks[0].timeout_secs, 900);
        assert!(v.checks[0].required);
    }
}
