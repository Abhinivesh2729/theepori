//! SQLite state database with ordered, append-only migrations.

use std::path::Path;
use std::time::Duration;

pub use rusqlite::Connection;

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
    // 2: factory engine (M1-M10)
    "CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE tasks (
        n            INTEGER PRIMARY KEY AUTOINCREMENT,
        project      TEXT NOT NULL REFERENCES projects(slug),
        title        TEXT NOT NULL,
        body         TEXT NOT NULL DEFAULT '',
        category     TEXT NOT NULL,
        priority     INTEGER NOT NULL,
        risk         TEXT NOT NULL,
        state        TEXT NOT NULL,
        source       TEXT NOT NULL,
        source_url   TEXT,
        dedupe_key   TEXT UNIQUE,
        acceptance   TEXT NOT NULL DEFAULT '[]',
        attempts     INTEGER NOT NULL DEFAULT 0,
        branch       TEXT,
        worktree     TEXT,
        session_id   TEXT,
        pr_number    INTEGER,
        pr_url       TEXT,
        jira_key     TEXT,
        blocked_reason TEXT,
        plan_id      TEXT,
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL
    );
    CREATE INDEX tasks_state ON tasks(state);
    CREATE INDEX tasks_project ON tasks(project, state);
    CREATE TABLE runs (
        n            INTEGER PRIMARY KEY AUTOINCREMENT,
        task_n       INTEGER REFERENCES tasks(n),
        project      TEXT,
        kind         TEXT NOT NULL,
        state        TEXT NOT NULL,
        runtime      TEXT NOT NULL,
        session_id   TEXT,
        pid          INTEGER,
        model        TEXT,
        started_at   TEXT NOT NULL,
        ended_at     TEXT,
        heartbeat_at TEXT,
        input_tokens INTEGER,
        output_tokens INTEGER,
        cache_read_tokens INTEGER,
        cache_write_tokens INTEGER,
        cost_usd     REAL,
        cost_basis   TEXT,
        num_turns    INTEGER,
        summary      TEXT,
        error        TEXT,
        capabilities TEXT
    );
    CREATE INDEX runs_task ON runs(task_n);
    CREATE TABLE run_events (
        run_n INTEGER NOT NULL REFERENCES runs(n),
        seq   INTEGER NOT NULL,
        ts    TEXT NOT NULL,
        kind  TEXT NOT NULL,
        text  TEXT NOT NULL,
        data  TEXT,
        PRIMARY KEY (run_n, seq)
    );
    CREATE TABLE signals (
        n           INTEGER PRIMARY KEY AUTOINCREMENT,
        project     TEXT NOT NULL,
        source      TEXT NOT NULL,
        kind        TEXT NOT NULL,
        external_id TEXT NOT NULL,
        dedupe_key  TEXT NOT NULL UNIQUE,
        title       TEXT NOT NULL,
        body        TEXT NOT NULL DEFAULT '',
        url         TEXT,
        labels      TEXT NOT NULL DEFAULT '[]',
        task_n      INTEGER,
        first_seen  TEXT NOT NULL,
        last_seen   TEXT NOT NULL
    );
    CREATE TABLE plans (
        id          TEXT PRIMARY KEY,
        date        TEXT NOT NULL,
        state       TEXT NOT NULL,
        summary     TEXT NOT NULL,
        approval_id TEXT,
        created_at  TEXT NOT NULL
    );
    CREATE TABLE plan_items (
        plan_id   TEXT NOT NULL REFERENCES plans(id),
        task_n    INTEGER NOT NULL REFERENCES tasks(n),
        rank      INTEGER NOT NULL,
        rationale TEXT NOT NULL,
        PRIMARY KEY (plan_id, task_n)
    );
    CREATE TABLE approvals (
        n              INTEGER PRIMARY KEY AUTOINCREMENT,
        kind           TEXT NOT NULL,
        state          TEXT NOT NULL,
        task_n         INTEGER,
        project        TEXT,
        title          TEXT NOT NULL,
        what           TEXT NOT NULL,
        why            TEXT NOT NULL,
        impact         TEXT NOT NULL,
        risks          TEXT NOT NULL,
        risk           TEXT NOT NULL,
        reversible     INTEGER NOT NULL,
        evidence       TEXT NOT NULL,
        recommendation TEXT NOT NULL,
        if_rejected    TEXT NOT NULL,
        urgent         INTEGER NOT NULL DEFAULT 0,
        payload        TEXT,
        dedupe_key     TEXT UNIQUE,
        created_at     TEXT NOT NULL,
        decided_at     TEXT,
        decided_by     TEXT,
        note           TEXT
    );
    CREATE INDEX approvals_state ON approvals(state);
    CREATE TABLE verifications (
        n            INTEGER PRIMARY KEY AUTOINCREMENT,
        task_n       INTEGER NOT NULL,
        attempt      INTEGER NOT NULL,
        name         TEXT NOT NULL,
        kind         TEXT NOT NULL,
        status       TEXT NOT NULL,
        duration_ms  INTEGER NOT NULL,
        summary      TEXT NOT NULL,
        evidence_path TEXT,
        created_at   TEXT NOT NULL
    );
    CREATE TABLE findings (
        n        INTEGER PRIMARY KEY AUTOINCREMENT,
        task_n   INTEGER NOT NULL,
        attempt  INTEGER NOT NULL,
        severity TEXT NOT NULL,
        file     TEXT,
        line     INTEGER,
        message  TEXT NOT NULL,
        status   TEXT NOT NULL
    );
    CREATE TABLE outbox (
        n               INTEGER PRIMARY KEY AUTOINCREMENT,
        target          TEXT NOT NULL,
        op              TEXT NOT NULL,
        project         TEXT NOT NULL,
        task_n          INTEGER,
        payload         TEXT NOT NULL,
        idempotency_key TEXT NOT NULL UNIQUE,
        state           TEXT NOT NULL,
        attempts        INTEGER NOT NULL DEFAULT 0,
        last_error      TEXT,
        result          TEXT,
        next_attempt_at TEXT,
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL
    );
    CREATE TABLE deployments (
        n             INTEGER PRIMARY KEY AUTOINCREMENT,
        project       TEXT NOT NULL,
        environment   TEXT NOT NULL,
        task_n        INTEGER,
        commit_sha    TEXT NOT NULL,
        state         TEXT NOT NULL,
        health        TEXT,
        version_ok    INTEGER,
        rolled_back   INTEGER NOT NULL DEFAULT 0,
        authorization TEXT NOT NULL,
        started_at    TEXT NOT NULL,
        finished_at   TEXT
    );
    CREATE TABLE dep_items (
        n         INTEGER PRIMARY KEY AUTOINCREMENT,
        project   TEXT NOT NULL,
        ecosystem TEXT NOT NULL,
        package   TEXT NOT NULL,
        from_v    TEXT NOT NULL,
        to_v      TEXT NOT NULL,
        bump      TEXT NOT NULL,
        risk      TEXT NOT NULL,
        breaking  INTEGER NOT NULL,
        reasons   TEXT NOT NULL,
        rationale TEXT NOT NULL,
        state     TEXT NOT NULL,
        task_n    INTEGER,
        scanned_at TEXT NOT NULL,
        UNIQUE (project, ecosystem, package, to_v)
    );
    CREATE TABLE memory (
        n            INTEGER PRIMARY KEY AUTOINCREMENT,
        scope        TEXT NOT NULL,
        category     TEXT NOT NULL,
        text         TEXT NOT NULL,
        status       TEXT NOT NULL,
        confidence   REAL NOT NULL,
        provenance   TEXT NOT NULL,
        version      INTEGER NOT NULL DEFAULT 1,
        content_hash TEXT NOT NULL,
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL,
        UNIQUE (scope, content_hash)
    );
    CREATE VIRTUAL TABLE memory_fts USING fts5(text, category, scope, content='memory', content_rowid='n');
    CREATE TRIGGER memory_ai AFTER INSERT ON memory BEGIN
        INSERT INTO memory_fts(rowid, text, category, scope) VALUES (new.n, new.text, new.category, new.scope);
    END;
    CREATE TRIGGER memory_au AFTER UPDATE ON memory BEGIN
        INSERT INTO memory_fts(memory_fts, rowid, text, category, scope) VALUES ('delete', old.n, old.text, old.category, old.scope);
        INSERT INTO memory_fts(rowid, text, category, scope) VALUES (new.n, new.text, new.category, new.scope);
    END;
    CREATE TABLE usage (
        n             INTEGER PRIMARY KEY AUTOINCREMENT,
        run_n         INTEGER,
        project       TEXT,
        task_n        INTEGER,
        kind          TEXT NOT NULL,
        source        TEXT NOT NULL,
        model         TEXT,
        input_tokens  INTEGER NOT NULL DEFAULT 0,
        output_tokens INTEGER NOT NULL DEFAULT 0,
        cache_read_tokens INTEGER NOT NULL DEFAULT 0,
        cache_write_tokens INTEGER NOT NULL DEFAULT 0,
        cost_usd      REAL,
        cost_basis    TEXT NOT NULL,
        pricing_version TEXT,
        ts            TEXT NOT NULL
    );
    CREATE INDEX usage_ts ON usage(ts);
    CREATE TABLE budgets (
        scope     TEXT NOT NULL,
        period    TEXT NOT NULL,
        limit_usd REAL NOT NULL,
        PRIMARY KEY (scope, period)
    );
    CREATE TABLE chat (
        n    INTEGER PRIMARY KEY AUTOINCREMENT,
        role TEXT NOT NULL,
        text TEXT NOT NULL,
        ts   TEXT NOT NULL
    );
    CREATE TABLE interventions (
        n       INTEGER PRIMARY KEY AUTOINCREMENT,
        task_n  INTEGER,
        kind    TEXT NOT NULL,
        minutes REAL NOT NULL,
        ts      TEXT NOT NULL
    );",
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
