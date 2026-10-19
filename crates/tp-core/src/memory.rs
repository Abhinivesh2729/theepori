//! Institutional memory (PRD §18): reusable lessons with scope and
//! provenance, separate from the audit log. Agents propose; only the founder
//! validates. Import never silently overrides newer or conflicting knowledge.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tp_proto::MemoryItem;

use crate::redact::Redactor;
use crate::{Error, Result, now_rfc3339};

pub const STATUSES: &[&str] = &["proposed", "validated", "retired"];

fn hash(scope: &str, text: &str) -> String {
    let norm: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    hex::encode(Sha256::digest(format!("{scope}\n{norm}").as_bytes()))
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryItem> {
    Ok(MemoryItem {
        id: r.get(0)?,
        scope: r.get(1)?,
        category: r.get(2)?,
        text: r.get(3)?,
        status: r.get(4)?,
        confidence: r.get(5)?,
        provenance: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or(Value::Null),
        version: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

const COLS: &str =
    "n, scope, category, text, status, confidence, provenance, version, created_at, updated_at";

/// Adds a proposed lesson (or returns the existing one with the same text).
pub fn propose(
    c: &Connection,
    scope: &str,
    category: &str,
    text: &str,
    provenance: Value,
    confidence: f64,
) -> Result<(MemoryItem, bool)> {
    if text.trim().len() < 8 {
        return Err(Error::Invalid("a lesson needs at least a sentence".into()));
    }
    if Redactor::new().contains_secret(text) {
        return Err(Error::Invalid("lessons must not contain secrets".into()));
    }
    let h = hash(scope, text);
    if let Some(n) = c
        .query_row(
            "SELECT n FROM memory WHERE scope = ?1 AND content_hash = ?2",
            params![scope, h],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok((get(c, n)?, false));
    }
    let now = now_rfc3339();
    c.execute(
        "INSERT INTO memory (scope, category, text, status, confidence, provenance, content_hash, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'proposed', ?4, ?5, ?6, ?7, ?7)",
        params![scope, category, text.trim(), confidence.clamp(0.0, 1.0), provenance.to_string(), h, now],
    )?;
    Ok((get(c, c.last_insert_rowid())?, true))
}

pub fn get(c: &Connection, n: i64) -> Result<MemoryItem> {
    c.query_row(&format!("SELECT {COLS} FROM memory WHERE n = ?1"), [n], row)
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("memory {n}")))
}

pub fn set_status(c: &Connection, n: i64, status: &str) -> Result<MemoryItem> {
    if !STATUSES.contains(&status) {
        return Err(Error::Invalid(format!(
            "status must be one of {STATUSES:?}"
        )));
    }
    c.execute(
        "UPDATE memory SET status = ?2, version = version + 1, updated_at = ?3 WHERE n = ?1",
        params![n, status, now_rfc3339()],
    )?;
    get(c, n)
}

pub fn list(c: &Connection, scope: Option<&str>, status: Option<&str>) -> Result<Vec<MemoryItem>> {
    let mut st = c.prepare(&format!(
        "SELECT {COLS} FROM memory WHERE (?1 IS NULL OR scope = ?1) AND (?2 IS NULL OR status = ?2) ORDER BY updated_at DESC LIMIT 1000"
    ))?;
    let rows = st.query_map(params![scope, status], row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Full-text search over validated (and, optionally, proposed) lessons in a
/// project scope plus global.
pub fn search(
    c: &Connection,
    query: &str,
    scope: Option<&str>,
    include_proposed: bool,
) -> Result<Vec<MemoryItem>> {
    let terms: Vec<String> = query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(|w| format!("\"{w}\""))
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut st = c.prepare(&format!(
        "SELECT {} FROM memory m JOIN memory_fts f ON f.rowid = m.n
         WHERE memory_fts MATCH ?1 AND (?2 IS NULL OR m.scope = ?2 OR m.scope = 'global')
           AND (m.status = 'validated' OR (?3 AND m.status = 'proposed'))
         ORDER BY rank LIMIT 20",
        COLS.split(", ")
            .map(|c| format!("m.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let rows = st.query_map(params![terms.join(" OR "), scope, include_proposed], row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ---------------------------------------------------------------- export / import

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExport {
    pub schema: u32,
    pub exported_at: String,
    pub items: Vec<MemoryItem>,
}

pub fn export(c: &Connection) -> Result<MemoryExport> {
    Ok(MemoryExport {
        schema: 1,
        exported_at: now_rfc3339(),
        items: list(c, None, None)?,
    })
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct MemoryImport {
    pub new: u32,
    pub duplicates: u32,
    pub conflicts: Vec<String>,
}

fn jaccard(a: &str, b: &str) -> f64 {
    let set = |s: &str| {
        s.to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<std::collections::HashSet<_>>()
    };
    let (x, y) = (set(a), set(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    x.intersection(&y).count() as f64 / x.union(&y).count() as f64
}

/// Merges an export. Exact duplicates are skipped; items that look like a
/// different version of an existing lesson are imported as `proposed` and
/// reported as conflicts (the existing lesson is untouched).
pub fn import(c: &Connection, data: &MemoryExport, apply: bool) -> Result<MemoryImport> {
    if data.schema != 1 {
        return Err(Error::Invalid(format!(
            "memory export schema {} is not supported",
            data.schema
        )));
    }
    let mut rep = MemoryImport::default();
    let existing = list(c, None, None)?;
    for it in &data.items {
        if Redactor::new().contains_secret(&it.text) {
            rep.conflicts.push(format!(
                "skipped an item in {} that looks like it contains a secret",
                it.scope
            ));
            continue;
        }
        let h = hash(&it.scope, &it.text);
        let dup = c
            .query_row(
                "SELECT 1 FROM memory WHERE scope = ?1 AND content_hash = ?2",
                params![it.scope, h],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if dup {
            rep.duplicates += 1;
            continue;
        }
        let similar = existing.iter().find(|e| {
            e.scope == it.scope && e.category == it.category && jaccard(&e.text, &it.text) > 0.6
        });
        let status = if let Some(e) = similar {
            rep.conflicts.push(format!(
                "memory {} ({}) differs from an imported lesson; imported copy kept as proposed",
                e.id, e.scope
            ));
            "proposed"
        } else if it.status == "validated" {
            "validated"
        } else {
            it.status.as_str()
        };
        rep.new += 1;
        if apply {
            let prov = json!({"imported": true, "original": it.provenance, "original_id": it.id, "original_updated_at": it.updated_at});
            c.execute(
                "INSERT INTO memory (scope, category, text, status, confidence, provenance, version, content_hash, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![it.scope, it.category, it.text, status, it.confidence, prov.to_string(), it.version, h, it.created_at, now_rfc3339()],
            )?;
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> (tempfile::TempDir, Connection) {
        let d = tempfile::tempdir().unwrap();
        let mut c = crate::db::open(&d.path().join("s.db")).unwrap();
        crate::db::migrate(&mut c).unwrap();
        (d, c)
    }

    #[test]
    fn propose_validate_search() {
        let (_d, c) = conn();
        let (m, new) = propose(
            &c,
            "app",
            "build",
            "Run cargo build with --locked in CI",
            json!({"task": "T1"}),
            0.7,
        )
        .unwrap();
        assert!(new);
        assert!(
            !propose(
                &c,
                "app",
                "build",
                "run  cargo build with --locked in ci",
                Value::Null,
                0.5
            )
            .unwrap()
            .1
        );
        assert!(
            search(&c, "cargo locked", Some("app"), false)
                .unwrap()
                .is_empty(),
            "proposed lessons are not served"
        );
        set_status(&c, m.id, "validated").unwrap();
        assert_eq!(
            search(&c, "cargo locked", Some("app"), false)
                .unwrap()
                .len(),
            1
        );
        assert!(
            search(&c, "cargo", Some("other"), false)
                .unwrap()
                .is_empty()
        );
        assert!(
            propose(
                &c,
                "app",
                "x",
                "the token=abcdefghijklmnopqrstuv works",
                Value::Null,
                0.5
            )
            .is_err()
        );
    }

    #[test]
    fn import_dedupes_and_flags_conflicts() {
        let (_d, c) = conn();
        propose(
            &c,
            "app",
            "build",
            "Always run the migrations before tests start",
            Value::Null,
            0.9,
        )
        .unwrap();
        let (_d2, c2) = conn();
        propose(
            &c2,
            "app",
            "build",
            "Always run the migrations before tests start",
            Value::Null,
            0.9,
        )
        .unwrap();
        propose(
            &c2,
            "app",
            "build",
            "Always run the migrations before the tests start up",
            Value::Null,
            0.9,
        )
        .unwrap();
        propose(
            &c2,
            "global",
            "process",
            "Batch dependency upgrades weekly per project",
            Value::Null,
            0.8,
        )
        .unwrap();
        let ex = export(&c2).unwrap();
        let preview = import(&c, &ex, false).unwrap();
        assert_eq!(
            (preview.new, preview.duplicates, preview.conflicts.len()),
            (2, 1, 1)
        );
        assert_eq!(
            list(&c, None, None).unwrap().len(),
            1,
            "preview changes nothing"
        );
        import(&c, &ex, true).unwrap();
        assert_eq!(list(&c, None, None).unwrap().len(), 3);
        assert_eq!(
            import(&c, &ex, true).unwrap().new,
            0,
            "re-import is idempotent"
        );
    }
}
