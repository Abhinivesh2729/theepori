//! `tp setup`: create or repair the factory. Safe to run any number of times.

use std::os::unix::fs::PermissionsExt;

use serde::Serialize;
use tp_proto::ActorKind;

use crate::audit::{AuditLog, NewEvent};
use crate::config::FactoryConfig;
use crate::paths::FactoryPaths;
use crate::{Error, Result, db};

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SetupReport {
    /// Paths created by this run (empty when nothing changed).
    pub created: Vec<String>,
    pub migrations_applied: u32,
}

impl SetupReport {
    pub fn changed(&self) -> bool {
        !self.created.is_empty() || self.migrations_applied > 0
    }
}

pub fn setup(paths: &FactoryPaths, actor: &str) -> Result<SetupReport> {
    let mut report = SetupReport::default();
    for dir in paths.dirs() {
        if !dir.exists() {
            std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
            report.created.push(dir.display().to_string());
        }
    }
    // The factory holds state and (later) the vault: owner-only.
    for dir in [paths.root().to_path_buf(), paths.vault_dir()] {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| Error::io(&dir, e))?;
    }
    let cfg_path = paths.config();
    if cfg_path.exists() {
        FactoryConfig::load(&cfg_path)?;
    } else {
        FactoryConfig::default().save(&cfg_path)?;
        report.created.push(cfg_path.display().to_string());
    }
    let db_path = paths.db();
    if !db_path.exists() {
        report.created.push(db_path.display().to_string());
    }
    let mut conn = db::open(&db_path)?;
    report.migrations_applied = db::migrate(&mut conn)?;
    if report.changed() {
        AuditLog::new(paths).append(
            &mut conn,
            NewEvent::new(ActorKind::Human, actor, "factory.setup")
                .details(serde_json::to_value(&report)?),
        )?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_run_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = FactoryPaths::new(dir.path().join("factory"));
        let first = setup(&paths, "test").unwrap();
        assert!(first.changed());
        assert!(paths.config().exists());

        let snapshot = |p: &FactoryPaths| {
            let mut entries: Vec<_> = walk(p.root());
            entries.sort();
            entries
        };
        let before = snapshot(&paths);
        let second = setup(&paths, "test").unwrap();
        assert!(!second.changed(), "{second:?}");
        assert_eq!(before, snapshot(&paths));

        let conn = db::open(&paths.db()).unwrap();
        assert_eq!(AuditLog::new(&paths).count(&conn).unwrap(), 1);
        let mode = std::fs::metadata(paths.root())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn refuses_invalid_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let paths = FactoryPaths::new(dir.path());
        std::fs::write(paths.config(), "schema: 9\nname: x\n").unwrap();
        assert!(setup(&paths, "test").is_err());
    }

    /// (relative path, size) for every file except SQLite's WAL side files,
    /// whose size changes on open.
    fn walk(root: &std::path::Path) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let rel = p.strip_prefix(root).unwrap().display().to_string();
                if p.is_dir() {
                    out.push((rel, 0));
                    stack.push(p);
                } else if !rel.ends_with("-wal") && !rel.ends_with("-shm") && !rel.ends_with(".db")
                {
                    out.push((rel, std::fs::metadata(&p).unwrap().len()));
                }
            }
        }
        out
    }
}
