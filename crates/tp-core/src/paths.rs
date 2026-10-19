//! The factory directory layout (`~/factory`, or `$TP_FACTORY`).

use std::path::{Path, PathBuf};

use crate::{Error, Result};

pub const ENV_FACTORY: &str = "TP_FACTORY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactoryPaths {
    root: PathBuf,
}

impl FactoryPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$TP_FACTORY` if set and non-empty, else `~/factory`.
    pub fn resolve() -> Result<Self> {
        if let Some(v) = std::env::var_os(ENV_FACTORY).filter(|v| !v.is_empty()) {
            return Ok(Self::new(v));
        }
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                Error::Invalid("HOME is not set; set TP_FACTORY to the factory directory".into())
            })?;
        Ok(Self::new(Path::new(&home).join("factory")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config(&self) -> PathBuf {
        self.root.join("factory.yaml")
    }
    pub fn db(&self) -> PathBuf {
        self.root.join("state.db")
    }
    pub fn socket(&self) -> PathBuf {
        self.root.join("tpd.sock")
    }
    pub fn lock(&self) -> PathBuf {
        self.root.join("tpd.lock")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn audit_dir(&self) -> PathBuf {
        self.root.join("audit")
    }
    pub fn vault_dir(&self) -> PathBuf {
        self.root.join("vault")
    }
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory").join("global")
    }
    pub fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }
    pub fn project_dir(&self, slug: &str) -> PathBuf {
        self.projects_dir().join(slug)
    }

    /// Directories `tp setup` creates, in order.
    pub fn dirs(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.logs(),
            self.audit_dir(),
            self.vault_dir(),
            self.skills_dir(),
            self.memory_dir(),
            self.projects_dir(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_under_root() {
        let p = FactoryPaths::new("/x/factory");
        assert_eq!(p.config(), PathBuf::from("/x/factory/factory.yaml"));
        assert_eq!(
            p.project_dir("app"),
            PathBuf::from("/x/factory/projects/app")
        );
        assert!(p.dirs().iter().all(|d| d.starts_with("/x/factory")));
    }
}
