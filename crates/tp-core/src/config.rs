//! `factory.yaml`: global factory configuration (schema-versioned).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FactoryConfig {
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub planning: Planning,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub jira: Jira,
    #[serde(default)]
    pub audit: Audit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Planning {
    /// Local time (HH:MM) the daily work plan is drafted.
    pub daily_plan_at: String,
}

impl Default for Planning {
    fn default() -> Self {
        Self { daily_plan_at: "08:00".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_concurrent_runs: u32,
    pub max_runs_per_project: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_concurrent_runs: 2, max_runs_per_project: 1 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Jira {
    /// Atlassian site cloudId, passed to steward runs to save a lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub retention_days: u32,
}

impl Default for Audit {
    fn default() -> Self {
        Self { retention_days: 30 }
    }
}

impl Default for FactoryConfig {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            name: "factory".into(),
            planning: Planning::default(),
            limits: Limits::default(),
            jira: Jira::default(),
            audit: Audit::default(),
        }
    }
}

const HEADER: &str = "# Thee Pori factory configuration. Edit with care; `tp doctor` validates it.\n";

impl FactoryConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let cfg: Self = serde_yaml::from_str(&raw).map_err(|e| Error::Config {
            path: path.into(),
            message: e.to_string(),
        })?;
        cfg.validate().map_err(|message| Error::Config { path: path.into(), message })?;
        Ok(cfg)
    }

    /// Writes atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate().map_err(|message| Error::Config { path: path.into(), message })?;
        let body = serde_yaml::to_string(self).map_err(|e| Error::Invalid(e.to_string()))?;
        let tmp = path.with_extension("yaml.tmp");
        std::fs::write(&tmp, format!("{HEADER}{body}")).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.schema != SCHEMA_VERSION {
            return Err(format!(
                "schema {} is not supported (expected {SCHEMA_VERSION}); upgrade tp or restore a matching factory.yaml",
                self.schema
            ));
        }
        if self.name.trim().is_empty() {
            return Err("name must not be empty".into());
        }
        if !valid_hhmm(&self.planning.daily_plan_at) {
            return Err(format!(
                "planning.daily_plan_at {:?} must be HH:MM (24h)",
                self.planning.daily_plan_at
            ));
        }
        if self.limits.max_concurrent_runs == 0 || self.limits.max_runs_per_project == 0 {
            return Err("limits must be at least 1".into());
        }
        if self.limits.max_runs_per_project > self.limits.max_concurrent_runs {
            return Err("limits.max_runs_per_project cannot exceed limits.max_concurrent_runs".into());
        }
        Ok(())
    }
}

fn valid_hhmm(s: &str) -> bool {
    let Some((h, m)) = s.split_once(':') else { return false };
    h.len() == 2
        && m.len() == 2
        && h.parse::<u8>().is_ok_and(|h| h < 24)
        && m.parse::<u8>().is_ok_and(|m| m < 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("factory.yaml");
        FactoryConfig::default().save(&p).unwrap();
        assert_eq!(FactoryConfig::load(&p).unwrap(), FactoryConfig::default());
        assert!(std::fs::read_to_string(&p).unwrap().starts_with("# Thee Pori"));
    }

    #[test]
    fn rejects_unknown_schema_and_fields() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("factory.yaml");
        std::fs::write(&p, "schema: 2\nname: x\n").unwrap();
        assert!(FactoryConfig::load(&p).unwrap_err().to_string().contains("schema 2"));
        std::fs::write(&p, "schema: 1\nname: x\nbogus: 1\n").unwrap();
        assert!(FactoryConfig::load(&p).unwrap_err().to_string().contains("bogus"));
    }

    #[test]
    fn validates_values() {
        let mut c = FactoryConfig::default();
        c.planning.daily_plan_at = "25:00".into();
        assert!(c.validate().is_err());
        c.planning.daily_plan_at = "07:30".into();
        c.limits.max_runs_per_project = 5;
        assert!(c.validate().is_err());
    }
}
