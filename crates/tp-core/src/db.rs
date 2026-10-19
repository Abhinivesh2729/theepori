//! SQLite state database with ordered, append-only migrations.

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::Result;

/// Each entry runs once, in order; `PRAGMA user_version` records progress.
/// Never edit a shipped migration; append a new one.
const MIGRATIONS: &[&str] = &[
    // 1: project registry + audit index
    "CREATE TABLE projects (
        slug       TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        repo_path  TEXT NOT NULL UNIQUE,
        jira_key   TEXT,
        created_at TEXT NOT NULL
    );
    CREATE TABLE audit_index (
        seq        INTEGER PRIMARY KEY,
        ts         TEXT NOT NULL,
        action     TEXT NOT NULL,
        project    TEXT,
        task       TEXT,
        file       TEXT NOT NULL,
        hash       TEXT NOT NULL,
        record     TEXT NOT NULL
    );
    CREATE INDEX audit_index_project ON audit_index(project, seq);
    CREATE INDEX audit_index_action ON audit_index(action, seq);",
];

pub fn latest_version() -> u32 {
    MIGRATIONS.len() as u32
}

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

pub fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// Applies pending migrations; returns how many ran.
pub fn migrate(conn: &mut Connection) -> Result<u32> {
    let current = version(conn)?;
    let mut ran = 0;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as u32)?;
        tx.commit()?;
        ran += 1;
    }
    Ok(ran)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = open(&dir.path().join("s.db")).unwrap();
        assert_eq!(migrate(&mut c).unwrap(), latest_version());
        assert_eq!(migrate(&mut c).unwrap(), 0);
        assert_eq!(version(&c).unwrap(), latest_version());
    }
}
