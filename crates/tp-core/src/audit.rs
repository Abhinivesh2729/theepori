//! Append-only, hash-chained audit log.
//!
//! Records go to `audit/YYYY-MM.jsonl` (the source of truth) and are indexed
//! in `audit_index` for queries. Each record's hash covers the previous hash
//! and the record itself, so editing, deleting or reordering lines breaks the
//! chain. Appends are serialized across processes by SQLite's write lock.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tp_proto::{Actor, ActorKind, AuditRecord, AuditVerify, Outcome, Provenance};

use crate::paths::FactoryPaths;
use crate::redact::Redactor;
use crate::{Error, Result};

pub const SCHEMA: u32 = 1;
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// An event to record. Build with [`NewEvent::new`] and the setters.
#[derive(Debug, Clone)]
pub struct NewEvent {
    pub actor: Actor,
    pub action: String,
    pub resource: Option<String>,
    pub outcome: Outcome,
    pub provenance: Provenance,
    pub project: Option<String>,
    pub task: Option<String>,
    pub authorization: Option<String>,
    pub details: Value,
}

impl NewEvent {
    pub fn new(kind: ActorKind, actor_id: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            actor: Actor { kind, id: actor_id.into() },
            action: action.into(),
            resource: None,
            outcome: Outcome::Ok,
            provenance: Provenance::Observed,
            project: None,
            task: None,
            authorization: None,
            details: Value::Null,
        }
    }
    pub fn system(action: impl Into<String>) -> Self {
        Self::new(ActorKind::System, "tpd", action)
    }
    pub fn resource(mut self, r: impl Into<String>) -> Self {
        self.resource = Some(r.into());
        self
    }
    pub fn outcome(mut self, o: Outcome) -> Self {
        self.outcome = o;
        self
    }
    pub fn claimed(mut self) -> Self {
        self.provenance = Provenance::Claimed;
        self
    }
    pub fn project(mut self, p: impl Into<String>) -> Self {
        self.project = Some(p.into());
        self
    }
    pub fn task(mut self, t: impl Into<String>) -> Self {
        self.task = Some(t.into());
        self
    }
    pub fn authorization(mut self, a: impl Into<String>) -> Self {
        self.authorization = Some(a.into());
        self
    }
    pub fn details(mut self, d: Value) -> Self {
        self.details = d;
        self
    }
}

pub struct AuditLog {
    dir: PathBuf,
    redactor: Redactor,
}

impl AuditLog {
    pub fn new(paths: &FactoryPaths) -> Self {
        Self { dir: paths.audit_dir(), redactor: Redactor::new() }
    }

    pub fn with_redactor(mut self, r: Redactor) -> Self {
        self.redactor = r;
        self
    }

    pub fn append(&self, conn: &mut Connection, ev: NewEvent) -> Result<AuditRecord> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.recover_tail(&tx)?;
        let (seq, prev_hash) = head(&tx)?;
        let mut rec = AuditRecord {
            schema: SCHEMA,
            seq: seq + 1,
            ts: crate::now_rfc3339(),
            actor: ev.actor,
            action: ev.action,
            resource: ev.resource.map(|r| self.redactor.redact(&r)),
            outcome: ev.outcome,
            provenance: ev.provenance,
            project: ev.project,
            task: ev.task,
            authorization: ev.authorization,
            details: self.redactor.redact_json(&ev.details),
            prev_hash,
            hash: String::new(),
        };
        rec.hash = hash_record(&rec)?;
        let line = serde_json::to_string(&rec)?;
        let file = self.file_for(&rec.ts);
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&file)
            .map_err(|e| Error::io(&file, e))?;
        f.write_all(format!("{line}\n").as_bytes()).map_err(|e| Error::io(&file, e))?;
        f.sync_data().map_err(|e| Error::io(&file, e))?;
        index(&tx, &rec, &file, &line)?;
        tx.commit()?;
        Ok(rec)
    }

    pub fn tail(&self, conn: &Connection, limit: u32) -> Result<Vec<AuditRecord>> {
        let mut stmt =
            conn.prepare("SELECT record FROM audit_index ORDER BY seq DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        out.reverse();
        Ok(out)
    }

    pub fn count(&self, conn: &Connection) -> Result<u64> {
        Ok(conn.query_row("SELECT COUNT(*) FROM audit_index", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Recomputes the whole chain from the JSONL files and checks the index.
    pub fn verify(&self, conn: &Connection) -> Result<AuditVerify> {
        let mut prev = GENESIS.to_string();
        let mut seq = 0u64;
        for file in self.files()? {
            let f = fs::File::open(&file).map_err(|e| Error::io(&file, e))?;
            for (n, line) in BufReader::new(f).lines().enumerate() {
                let line = line.map_err(|e| Error::io(&file, e))?;
                let at = format!("{}:{}", file.display(), n + 1);
                let rec: AuditRecord = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => return Ok(broken(seq, format!("{at}: unreadable record: {e}"))),
                };
                if rec.seq != seq + 1 {
                    return Ok(broken(seq, format!("{at}: seq {} follows {seq}", rec.seq)));
                }
                if rec.prev_hash != prev {
                    return Ok(broken(seq, format!("{at}: prev_hash does not match record {seq}")));
                }
                if hash_record(&rec)? != rec.hash {
                    return Ok(broken(seq, format!("{at}: record {} was modified", rec.seq)));
                }
                prev = rec.hash;
                seq = rec.seq;
            }
        }
        let (db_seq, db_hash) = head(conn)?;
        if db_seq != seq || (seq > 0 && db_hash != prev) {
            return Ok(broken(
                seq,
                format!("index head is record {db_seq} but the files end at record {seq}; run `tp doctor`"),
            ));
        }
        Ok(AuditVerify { ok: true, records: seq, problem: None })
    }

    fn file_for(&self, ts: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", ts.get(..7).unwrap_or("unknown")))
    }

    fn files(&self) -> Result<Vec<PathBuf>> {
        let mut files = match fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
                .collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(Error::io(&self.dir, e)),
        };
        files.sort();
        Ok(files)
    }

    /// A crash between writing a line and committing its index row leaves
    /// one unindexed record at the end of the newest file. Index it so the
    /// next append continues the chain instead of forking it.
    fn recover_tail(&self, tx: &Transaction<'_>) -> Result<()> {
        let Some(file) = self.files()?.pop() else { return Ok(()) };
        let Some(line) = last_line(&file)? else { return Ok(()) };
        let rec: AuditRecord = serde_json::from_str(&line)?;
        let (seq, prev) = head(tx)?;
        if rec.seq == seq + 1 && rec.prev_hash == prev && hash_record(&rec)? == rec.hash {
            index(tx, &rec, &file, &line)?;
        }
        Ok(())
    }
}

fn broken(records: u64, problem: String) -> AuditVerify {
    AuditVerify { ok: false, records, problem: Some(problem) }
}

fn head(conn: &Connection) -> Result<(u64, String)> {
    Ok(conn
        .query_row("SELECT seq, hash FROM audit_index ORDER BY seq DESC LIMIT 1", [], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?))
        })
        .optional()?
        .unwrap_or((0, GENESIS.to_string())))
}

fn index(conn: &Connection, rec: &AuditRecord, file: &Path, line: &str) -> Result<()> {
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    conn.execute(
        "INSERT INTO audit_index (seq, ts, action, project, task, file, hash, record)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![rec.seq as i64, rec.ts, rec.action, rec.project, rec.task, name, rec.hash, line],
    )?;
    Ok(())
}

/// sha256(prev_hash || "\n" || json(record with empty hash)), hex.
pub fn hash_record(rec: &AuditRecord) -> Result<String> {
    let mut unsigned = rec.clone();
    unsigned.hash = String::new();
    let body = serde_json::to_vec(&unsigned)?;
    let mut h = Sha256::new();
    h.update(rec.prev_hash.as_bytes());
    h.update(b"\n");
    h.update(&body);
    Ok(hex::encode(h.finalize()))
}

fn last_line(path: &Path) -> Result<Option<String>> {
    let mut f = fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
    if len == 0 {
        return Ok(None);
    }
    // Records are small; 64 KiB comfortably holds the last one.
    let start = len.saturating_sub(64 * 1024);
    f.seek(SeekFrom::Start(start)).map_err(|e| Error::io(path, e))?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).map_err(|e| Error::io(path, e))?;
    Ok(buf.trim_end().rsplit('\n').next().map(str::to_owned).filter(|l| !l.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use serde_json::json;

    fn setup() -> (tempfile::TempDir, FactoryPaths, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let paths = FactoryPaths::new(dir.path());
        let mut conn = db::open(&dir.path().join("s.db")).unwrap();
        db::migrate(&mut conn).unwrap();
        (dir, paths, conn)
    }

    #[test]
    fn chain_links_and_verifies() {
        let (_d, paths, mut conn) = setup();
        let log = AuditLog::new(&paths);
        let a = log.append(&mut conn, NewEvent::system("factory.setup")).unwrap();
        let b = log.append(&mut conn, NewEvent::system("project.added").project("app")).unwrap();
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!(a.prev_hash, GENESIS);
        assert_eq!(b.prev_hash, a.hash);
        let v = log.verify(&conn).unwrap();
        assert!(v.ok, "{v:?}");
        assert_eq!(v.records, 2);
        assert_eq!(log.tail(&conn, 1).unwrap(), vec![b]);
    }

    #[test]
    fn continues_across_reopen() {
        let (d, paths, mut conn) = setup();
        AuditLog::new(&paths).append(&mut conn, NewEvent::system("one")).unwrap();
        drop(conn);
        let mut conn = db::open(&d.path().join("s.db")).unwrap();
        let rec = AuditLog::new(&paths).append(&mut conn, NewEvent::system("two")).unwrap();
        assert_eq!(rec.seq, 2);
        assert!(AuditLog::new(&paths).verify(&conn).unwrap().ok);
    }

    #[test]
    fn detects_tampering() {
        let (_d, paths, mut conn) = setup();
        let log = AuditLog::new(&paths);
        for a in ["a", "b", "c"] {
            log.append(&mut conn, NewEvent::system(a)).unwrap();
        }
        let file = log.files().unwrap().pop().unwrap();
        let text = fs::read_to_string(&file).unwrap();
        fs::write(&file, text.replacen("\"action\":\"b\"", "\"action\":\"x\"", 1)).unwrap();
        let v = log.verify(&conn).unwrap();
        assert!(!v.ok);
        assert!(v.problem.unwrap().contains("record 2 was modified"));
    }

    #[test]
    fn detects_deleted_line() {
        let (_d, paths, mut conn) = setup();
        let log = AuditLog::new(&paths);
        for a in ["a", "b", "c"] {
            log.append(&mut conn, NewEvent::system(a)).unwrap();
        }
        let file = log.files().unwrap().pop().unwrap();
        let lines: Vec<_> = fs::read_to_string(&file).unwrap().lines().map(String::from).collect();
        fs::write(&file, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(!log.verify(&conn).unwrap().ok);
    }

    #[test]
    fn recovers_unindexed_tail() {
        let (_d, paths, mut conn) = setup();
        let log = AuditLog::new(&paths);
        log.append(&mut conn, NewEvent::system("a")).unwrap();
        log.append(&mut conn, NewEvent::system("b")).unwrap();
        // Simulate a crash after the line was written but before the commit.
        conn.execute("DELETE FROM audit_index WHERE seq = 2", []).unwrap();
        let c = log.append(&mut conn, NewEvent::system("c")).unwrap();
        assert_eq!(c.seq, 3);
        assert!(log.verify(&conn).unwrap().ok);
    }

    #[test]
    fn redacts_details_and_resource() {
        let (_d, paths, mut conn) = setup();
        let rec = AuditLog::new(&paths)
            .append(
                &mut conn,
                NewEvent::system("x")
                    .resource("token=abcdefghijklmnopqrst")
                    .details(json!({"cmd": "export API_KEY=abcdefghijklmnop"})),
            )
            .unwrap();
        assert_eq!(rec.resource.as_deref(), Some("token=[REDACTED]"));
        assert_eq!(rec.details["cmd"], "export API_KEY=[REDACTED]");
    }
}
