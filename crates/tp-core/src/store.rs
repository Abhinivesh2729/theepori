//! Typed access to the engine tables. IDs on the wire are prefixed strings
//! (`T12`, `R3`, `A7`, `D2`) over integer primary keys.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tp_proto::*;

use crate::{Error, Result, now_rfc3339};

// ---------------------------------------------------------------- ids & enums

pub fn task_id(n: i64) -> String {
    format!("T{n}")
}
pub fn run_id(n: i64) -> String {
    format!("R{n}")
}
pub fn approval_id(n: i64) -> String {
    format!("A{n}")
}
pub fn deploy_id(n: i64) -> String {
    format!("D{n}")
}

pub fn parse_id(prefix: char, id: &str) -> Result<i64> {
    id.strip_prefix(prefix)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Invalid(format!("{id:?} is not a valid {prefix}-id")))
}

/// serde enum <-> its snake_case string.
pub fn enc<T: Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub fn dec<T: DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_value(Value::String(s.to_owned())).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn opt_json(s: Option<String>) -> Value {
    s.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------- kv

pub fn kv_get(c: &Connection, key: &str) -> Result<Option<String>> {
    Ok(
        c.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()?,
    )
}

pub fn kv_set(c: &Connection, key: &str, value: &str) -> Result<()> {
    c.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn controls(c: &Connection) -> Result<Controls> {
    Ok(kv_get(c, "controls")?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Controls {
            mode: Mode::Normal,
            auto_merge: true,
            auto_deploy: false,
            remote_access: false,
        }))
}

pub fn set_controls(c: &Connection, ctl: &Controls) -> Result<()> {
    kv_set(c, "controls", &serde_json::to_string(ctl)?)
}

// ---------------------------------------------------------------- tasks

pub struct NewTask {
    pub project: String,
    pub title: String,
    pub body: String,
    pub category: Category,
    pub priority: u8,
    pub risk: Risk,
    pub state: TaskState,
    pub source: String,
    pub source_url: Option<String>,
    pub dedupe_key: Option<String>,
    pub acceptance: Vec<String>,
}

/// Inserts a task unless one with the same dedupe key exists; returns
/// (task, created).
pub fn insert_task(c: &Connection, t: NewTask) -> Result<(TaskSummary, bool)> {
    if let Some(k) = &t.dedupe_key {
        if let Some(n) = c
            .query_row("SELECT n FROM tasks WHERE dedupe_key = ?1", [k], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            return Ok((get_task(c, n)?, false));
        }
    }
    let now = now_rfc3339();
    c.execute(
        "INSERT INTO tasks (project, title, body, category, priority, risk, state, source, source_url,
                            dedupe_key, acceptance, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        params![
            t.project,
            t.title,
            t.body,
            enc(t.category),
            t.priority,
            enc(t.risk),
            enc(t.state),
            t.source,
            t.source_url,
            t.dedupe_key,
            serde_json::to_string(&t.acceptance)?,
            now
        ],
    )?;
    Ok((get_task(c, c.last_insert_rowid())?, true))
}

const TASK_COLS: &str = "n, project, title, category, priority, risk, state, source, source_url,
    attempts, pr_url, jira_key, blocked_reason, created_at, updated_at";

fn row_task(r: &Row<'_>) -> rusqlite::Result<TaskSummary> {
    Ok(TaskSummary {
        id: task_id(r.get(0)?),
        project: r.get(1)?,
        title: r.get(2)?,
        category: dec(&r.get::<_, String>(3)?)?,
        priority: r.get(4)?,
        risk: dec(&r.get::<_, String>(5)?)?,
        state: dec(&r.get::<_, String>(6)?)?,
        source: r.get(7)?,
        source_url: r.get(8)?,
        attempts: r.get(9)?,
        pr_url: r.get(10)?,
        jira_key: r.get(11)?,
        blocked_reason: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
    })
}

pub fn get_task(c: &Connection, n: i64) -> Result<TaskSummary> {
    c.query_row(
        &format!("SELECT {TASK_COLS} FROM tasks WHERE n = ?1"),
        [n],
        row_task,
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("task {}", task_id(n))))
}

pub fn list_tasks(c: &Connection, q: &TaskQuery) -> Result<Vec<TaskSummary>> {
    let mut sql = format!("SELECT {TASK_COLS} FROM tasks WHERE 1=1");
    let mut args: Vec<String> = Vec::new();
    if let Some(p) = &q.project {
        args.push(p.clone());
        sql.push_str(&format!(" AND project = ?{}", args.len()));
    }
    if q.open_only {
        sql.push_str(" AND state NOT IN ('done','cancelled','rolled_back')");
    }
    if !q.states.is_empty() {
        let list: Vec<String> = q.states.iter().map(|s| format!("'{}'", enc(*s))).collect();
        sql.push_str(&format!(" AND state IN ({})", list.join(",")));
    }
    sql.push_str(&format!(
        " ORDER BY CASE WHEN state IN ('done','cancelled') THEN 1 ELSE 0 END, priority, n DESC LIMIT {}",
        q.limit.unwrap_or(500).min(5000)
    ));
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(rusqlite::params_from_iter(args.iter()), row_task)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Raw task fields the engine needs beyond the summary.
#[derive(Debug, Clone)]
pub struct TaskRow {
    pub n: i64,
    pub summary: TaskSummary,
    pub body: String,
    pub acceptance: Vec<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub session_id: Option<String>,
    pub pr_number: Option<i64>,
}

pub fn task_row(c: &Connection, n: i64) -> Result<TaskRow> {
    let summary = get_task(c, n)?;
    let (body, acc, branch, worktree, session_id, pr_number): (String, String, _, _, _, _) = c.query_row(
        "SELECT body, acceptance, branch, worktree, session_id, pr_number FROM tasks WHERE n = ?1",
        [n],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )?;
    Ok(TaskRow {
        n,
        summary,
        body,
        acceptance: serde_json::from_str(&acc).unwrap_or_default(),
        branch,
        worktree,
        session_id,
        pr_number,
    })
}

pub fn set_task_state(
    c: &Connection,
    n: i64,
    state: TaskState,
    reason: Option<&str>,
) -> Result<TaskSummary> {
    c.execute(
        "UPDATE tasks SET state = ?2, blocked_reason = ?3, updated_at = ?4 WHERE n = ?1",
        params![n, enc(state), reason, now_rfc3339()],
    )?;
    get_task(c, n)
}

/// Sets one column of a task (column names are fixed by callers).
pub fn set_task_field(
    c: &Connection,
    n: i64,
    column: &str,
    value: &dyn rusqlite::ToSql,
) -> Result<()> {
    const ALLOWED: &[&str] = &[
        "branch",
        "worktree",
        "session_id",
        "pr_number",
        "pr_url",
        "jira_key",
        "risk",
        "priority",
        "category",
        "acceptance",
        "attempts",
        "plan_id",
        "title",
        "body",
    ];
    if !ALLOWED.contains(&column) {
        return Err(Error::Invalid(format!("task column {column}")));
    }
    c.execute(
        &format!("UPDATE tasks SET {column} = ?2, updated_at = ?3 WHERE n = ?1"),
        params![n, value, now_rfc3339()],
    )?;
    Ok(())
}

pub fn count_tasks(c: &Connection, open_only: bool) -> Result<u32> {
    let sql = if open_only {
        "SELECT COUNT(*) FROM tasks WHERE state NOT IN ('done','cancelled','rolled_back')"
    } else {
        "SELECT COUNT(*) FROM tasks"
    };
    Ok(c.query_row(sql, [], |r| r.get(0))?)
}

// ---------------------------------------------------------------- runs

pub struct NewRun<'a> {
    pub task_n: Option<i64>,
    pub project: Option<&'a str>,
    pub kind: RunKind,
    pub runtime: &'a str,
    pub model: Option<&'a str>,
}

pub fn insert_run(c: &Connection, r: NewRun<'_>) -> Result<i64> {
    c.execute(
        "INSERT INTO runs (task_n, project, kind, state, runtime, model, started_at)
         VALUES (?1, ?2, ?3, 'starting', ?4, ?5, ?6)",
        params![
            r.task_n,
            r.project,
            enc(r.kind),
            r.runtime,
            r.model,
            now_rfc3339()
        ],
    )?;
    Ok(c.last_insert_rowid())
}

const RUN_COLS: &str =
    "n, task_n, kind, state, runtime, session_id, model, started_at, ended_at, cost_usd,
    input_tokens, output_tokens, num_turns, summary, error";

fn row_run(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: run_id(r.get(0)?),
        task_id: r.get::<_, Option<i64>>(1)?.map(task_id),
        kind: dec(&r.get::<_, String>(2)?)?,
        state: dec(&r.get::<_, String>(3)?)?,
        runtime: r.get(4)?,
        session_id: r.get(5)?,
        model: r.get(6)?,
        started_at: r.get(7)?,
        ended_at: r.get(8)?,
        cost_usd: r.get(9)?,
        input_tokens: r.get::<_, Option<i64>>(10)?.map(|v| v as u64),
        output_tokens: r.get::<_, Option<i64>>(11)?.map(|v| v as u64),
        num_turns: r.get(12)?,
        summary: r.get(13)?,
        error: r.get(14)?,
    })
}

pub fn get_run(c: &Connection, n: i64) -> Result<Run> {
    c.query_row(
        &format!("SELECT {RUN_COLS} FROM runs WHERE n = ?1"),
        [n],
        row_run,
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("run {}", run_id(n))))
}

pub fn runs_for_task(c: &Connection, task_n: i64) -> Result<Vec<Run>> {
    let mut st = c.prepare(&format!(
        "SELECT {RUN_COLS} FROM runs WHERE task_n = ?1 ORDER BY n"
    ))?;
    let rows = st.query_map([task_n], row_run)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Runs in starting/running state (for recovery and the status line).
pub fn active_runs(c: &Connection) -> Result<Vec<(i64, Option<i64>, Option<i64>, RunKind)>> {
    let mut st =
        c.prepare("SELECT n, task_n, pid, kind FROM runs WHERE state IN ('starting','running')")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            dec(&r.get::<_, String>(3)?)?,
        ))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn run_started(c: &Connection, n: i64, pid: u32) -> Result<()> {
    c.execute(
        "UPDATE runs SET state = 'running', pid = ?2, heartbeat_at = ?3 WHERE n = ?1",
        params![n, pid, now_rfc3339()],
    )?;
    Ok(())
}

pub fn run_session(c: &Connection, n: i64, session: &str, capabilities: &Value) -> Result<()> {
    c.execute(
        "UPDATE runs SET session_id = ?2, capabilities = ?3 WHERE n = ?1",
        params![n, session, capabilities.to_string()],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub state: Option<RunState>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<String>,
    pub num_turns: Option<u32>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub model: Option<String>,
}

pub fn finish_run(c: &Connection, n: i64, o: &RunOutcome) -> Result<Run> {
    let state = o.state.unwrap_or(RunState::Failed);
    c.execute(
        "UPDATE runs SET state = ?2, ended_at = ?3, input_tokens = ?4, output_tokens = ?5,
            cache_read_tokens = ?6, cache_write_tokens = ?7, cost_usd = ?8, cost_basis = ?9, num_turns = ?10,
            summary = ?11, error = ?12, model = COALESCE(?13, model), pid = NULL
         WHERE n = ?1",
        params![
            n,
            enc(state),
            now_rfc3339(),
            o.input_tokens.map(|v| v as i64),
            o.output_tokens.map(|v| v as i64),
            o.cache_read_tokens.map(|v| v as i64),
            o.cache_write_tokens.map(|v| v as i64),
            o.cost_usd,
            o.cost_basis,
            o.num_turns,
            o.summary,
            o.error,
            o.model
        ],
    )?;
    get_run(c, n)
}

pub fn run_task_project(c: &Connection, n: i64) -> Result<(Option<i64>, Option<String>, RunKind)> {
    Ok(c.query_row(
        "SELECT task_n, project, kind FROM runs WHERE n = ?1",
        [n],
        |r| Ok((r.get(0)?, r.get(1)?, dec(&r.get::<_, String>(2)?)?)),
    )?)
}

pub fn append_run_event(
    c: &Connection,
    run_n: i64,
    kind: &str,
    text: &str,
    data: &Value,
) -> Result<RunEvent> {
    let seq: i64 = c.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM run_events WHERE run_n = ?1",
        [run_n],
        |r| r.get(0),
    )?;
    let ts = now_rfc3339();
    let data_s = if data.is_null() {
        None
    } else {
        Some(data.to_string())
    };
    c.execute(
        "INSERT INTO run_events (run_n, seq, ts, kind, text, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![run_n, seq, ts, kind, text, data_s],
    )?;
    c.execute(
        "UPDATE runs SET heartbeat_at = ?2 WHERE n = ?1",
        params![run_n, ts],
    )?;
    Ok(RunEvent {
        run_id: run_id(run_n),
        seq: seq as u64,
        ts,
        kind: kind.into(),
        text: text.into(),
        data: data.clone(),
    })
}

pub fn run_events(c: &Connection, run_n: i64, after: u64, limit: u32) -> Result<Vec<RunEvent>> {
    let mut st = c.prepare(
        "SELECT seq, ts, kind, text, data FROM run_events WHERE run_n = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows = st.query_map(params![run_n, after as i64, limit.min(5000)], |r| {
        Ok(RunEvent {
            run_id: run_id(run_n),
            seq: r.get::<_, i64>(0)? as u64,
            ts: r.get(1)?,
            kind: r.get(2)?,
            text: r.get(3)?,
            data: opt_json(r.get(4)?),
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ---------------------------------------------------------------- approvals

pub struct NewApproval {
    pub kind: ApprovalKind,
    pub task_n: Option<i64>,
    pub project: Option<String>,
    pub title: String,
    pub what: String,
    pub why: String,
    pub impact: String,
    pub risks: String,
    pub risk: Risk,
    pub reversible: bool,
    pub evidence: String,
    pub recommendation: String,
    pub if_rejected: String,
    pub urgent: bool,
    pub payload: Value,
    pub dedupe_key: Option<String>,
}

/// Returns (approval, created). A pending approval with the same dedupe key is reused.
pub fn insert_approval(c: &Connection, a: NewApproval) -> Result<(Approval, bool)> {
    if let Some(k) = &a.dedupe_key {
        if let Some(n) = c
            .query_row("SELECT n FROM approvals WHERE dedupe_key = ?1", [k], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            return Ok((get_approval(c, n)?, false));
        }
    }
    c.execute(
        "INSERT INTO approvals (kind, state, task_n, project, title, what, why, impact, risks, risk, reversible,
            evidence, recommendation, if_rejected, urgent, payload, dedupe_key, created_at)
         VALUES (?1, 'pending', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            enc(a.kind),
            a.task_n,
            a.project,
            a.title,
            a.what,
            a.why,
            a.impact,
            a.risks,
            enc(a.risk),
            a.reversible,
            a.evidence,
            a.recommendation,
            a.if_rejected,
            a.urgent,
            a.payload.to_string(),
            a.dedupe_key,
            now_rfc3339()
        ],
    )?;
    Ok((get_approval(c, c.last_insert_rowid())?, true))
}

const APPROVAL_COLS: &str =
    "n, kind, state, task_n, project, title, what, why, impact, risks, risk, reversible,
    evidence, recommendation, if_rejected, urgent, created_at, decided_at, decided_by, note";

fn row_approval(r: &Row<'_>) -> rusqlite::Result<Approval> {
    Ok(Approval {
        id: approval_id(r.get(0)?),
        kind: dec(&r.get::<_, String>(1)?)?,
        state: dec(&r.get::<_, String>(2)?)?,
        task_id: r.get::<_, Option<i64>>(3)?.map(task_id),
        project: r.get(4)?,
        title: r.get(5)?,
        what: r.get(6)?,
        why: r.get(7)?,
        impact: r.get(8)?,
        risks: r.get(9)?,
        risk: dec(&r.get::<_, String>(10)?)?,
        reversible: r.get(11)?,
        evidence: r.get(12)?,
        recommendation: r.get(13)?,
        if_rejected: r.get(14)?,
        urgent: r.get(15)?,
        created_at: r.get(16)?,
        decided_at: r.get(17)?,
        decided_by: r.get(18)?,
        note: r.get(19)?,
    })
}

pub fn get_approval(c: &Connection, n: i64) -> Result<Approval> {
    c.query_row(
        &format!("SELECT {APPROVAL_COLS} FROM approvals WHERE n = ?1"),
        [n],
        row_approval,
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("approval {}", approval_id(n))))
}

pub fn approval_payload(c: &Connection, n: i64) -> Result<Value> {
    Ok(opt_json(c.query_row(
        "SELECT payload FROM approvals WHERE n = ?1",
        [n],
        |r| r.get(0),
    )?))
}

pub fn list_approvals(
    c: &Connection,
    state: Option<ApprovalState>,
    task_n: Option<i64>,
) -> Result<Vec<Approval>> {
    let mut sql = format!("SELECT {APPROVAL_COLS} FROM approvals WHERE 1=1");
    if let Some(s) = state {
        sql.push_str(&format!(" AND state = '{}'", enc(s)));
    }
    if let Some(t) = task_n {
        sql.push_str(&format!(" AND task_n = {t}"));
    }
    sql.push_str(" ORDER BY urgent DESC, n DESC LIMIT 500");
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map([], row_approval)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn decide_approval(
    c: &Connection,
    n: i64,
    state: ApprovalState,
    by: &str,
    note: Option<&str>,
) -> Result<Approval> {
    let cur = get_approval(c, n)?;
    if cur.state != ApprovalState::Pending && cur.state != ApprovalState::Deferred {
        return Err(Error::Conflict(format!(
            "approval {} is already {:?}",
            cur.id, cur.state
        )));
    }
    c.execute(
        "UPDATE approvals SET state = ?2, decided_at = ?3, decided_by = ?4, note = ?5 WHERE n = ?1",
        params![n, enc(state), now_rfc3339(), by, note],
    )?;
    get_approval(c, n)
}

/// Latest approval of `kind` for a task, if any.
pub fn task_approval(c: &Connection, task_n: i64, kind: ApprovalKind) -> Result<Option<Approval>> {
    Ok(list_approvals(c, None, Some(task_n))?
        .into_iter()
        .find(|a| a.kind == kind))
}

pub fn count_pending_approvals(c: &Connection) -> Result<u32> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM approvals WHERE state = 'pending'",
        [],
        |r| r.get(0),
    )?)
}

// ---------------------------------------------------------------- plans

pub fn insert_plan(
    c: &Connection,
    id: &str,
    date: &str,
    summary: &str,
    items: &[(i64, u32, String)],
) -> Result<()> {
    c.execute(
        "UPDATE plans SET state = 'superseded' WHERE state = 'draft' AND id != ?1",
        [id],
    )?;
    c.execute(
        "INSERT INTO plans (id, date, state, summary, created_at) VALUES (?1, ?2, 'draft', ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET summary = excluded.summary, state = 'draft'",
        params![id, date, summary, now_rfc3339()],
    )?;
    c.execute("DELETE FROM plan_items WHERE plan_id = ?1", [id])?;
    for (task_n, rank, why) in items {
        c.execute(
            "INSERT INTO plan_items (plan_id, task_n, rank, rationale) VALUES (?1, ?2, ?3, ?4)",
            params![id, task_n, rank, why],
        )?;
        c.execute(
            "UPDATE tasks SET plan_id = ?2 WHERE n = ?1",
            params![task_n, id],
        )?;
    }
    Ok(())
}

pub fn set_plan(
    c: &Connection,
    id: &str,
    state: Option<&str>,
    approval: Option<&str>,
) -> Result<()> {
    if let Some(s) = state {
        c.execute("UPDATE plans SET state = ?2 WHERE id = ?1", params![id, s])?;
    }
    if let Some(a) = approval {
        c.execute(
            "UPDATE plans SET approval_id = ?2 WHERE id = ?1",
            params![id, a],
        )?;
    }
    Ok(())
}

pub fn get_plan(c: &Connection, id: &str) -> Result<Option<Plan>> {
    let Some((date, state, summary, approval_id, created_at)) = c
        .query_row(
            "SELECT date, state, summary, approval_id, created_at FROM plans WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut st = c.prepare(
        "SELECT task_n, rank, rationale FROM plan_items WHERE plan_id = ?1 ORDER BY rank",
    )?;
    let rows: Vec<(i64, u32, String)> = st
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut items = Vec::new();
    for (n, rank, rationale) in rows {
        items.push(PlanItem {
            rank,
            task: get_task(c, n)?,
            rationale,
        });
    }
    Ok(Some(Plan {
        id: id.into(),
        date,
        state,
        summary,
        items,
        approval_id,
        created_at,
    }))
}

pub fn latest_plan(c: &Connection) -> Result<Option<Plan>> {
    let id: Option<String> = c
        .query_row(
            "SELECT id FROM plans WHERE state != 'superseded' ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => get_plan(c, &id),
        None => Ok(None),
    }
}

pub fn plan_task_ns(c: &Connection, id: &str) -> Result<Vec<i64>> {
    let mut st = c.prepare("SELECT task_n FROM plan_items WHERE plan_id = ?1 ORDER BY rank")?;
    let rows = st.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ---------------------------------------------------------------- signals

pub struct NewSignal<'a> {
    pub project: &'a str,
    pub source: &'a str,
    pub kind: &'a str,
    pub external_id: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub url: Option<&'a str>,
    pub labels: &'a [String],
}

/// Upserts a signal by (source, external id); returns (n, is_new, task_n).
pub fn upsert_signal(c: &Connection, s: NewSignal<'_>) -> Result<(i64, bool, Option<i64>)> {
    let key = format!("{}:{}:{}", s.source, s.project, s.external_id);
    let now = now_rfc3339();
    if let Some((n, task)) = c
        .query_row(
            "SELECT n, task_n FROM signals WHERE dedupe_key = ?1",
            [&key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    {
        c.execute(
            "UPDATE signals SET last_seen = ?2, title = ?3, body = ?4, labels = ?5 WHERE n = ?1",
            params![n, now, s.title, s.body, serde_json::to_string(s.labels)?],
        )?;
        return Ok((n, false, task));
    }
    c.execute(
        "INSERT INTO signals (project, source, kind, external_id, dedupe_key, title, body, url, labels, first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![s.project, s.source, s.kind, s.external_id, key, s.title, s.body, s.url, serde_json::to_string(s.labels)?, now],
    )?;
    Ok((c.last_insert_rowid(), true, None))
}

pub fn link_signal(c: &Connection, n: i64, task_n: i64) -> Result<()> {
    c.execute(
        "UPDATE signals SET task_n = ?2 WHERE n = ?1",
        params![n, task_n],
    )?;
    Ok(())
}

pub fn list_signals(c: &Connection, project: Option<&str>, limit: u32) -> Result<Vec<Signal>> {
    let mut st = c.prepare(
        "SELECT n, project, source, kind, external_id, title, url, task_n, first_seen, last_seen FROM signals
         WHERE (?1 IS NULL OR project = ?1) ORDER BY last_seen DESC LIMIT ?2",
    )?;
    let rows = st.query_map(params![project, limit], |r| {
        Ok(Signal {
            id: r.get(0)?,
            project: r.get(1)?,
            source: r.get(2)?,
            kind: r.get(3)?,
            external_id: r.get(4)?,
            title: r.get(5)?,
            url: r.get(6)?,
            task_id: r.get::<_, Option<i64>>(7)?.map(task_id),
            first_seen: r.get(8)?,
            last_seen: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ---------------------------------------------------------------- verification & findings

pub fn insert_verification(
    c: &Connection,
    task_n: i64,
    attempt: u32,
    v: &Verification,
) -> Result<()> {
    c.execute(
        "INSERT INTO verifications (task_n, attempt, name, kind, status, duration_ms, summary, evidence_path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![task_n, attempt, v.name, v.kind, enc(v.status), v.duration_ms as i64, v.summary, v.evidence_path, now_rfc3339()],
    )?;
    Ok(())
}

pub fn verifications(
    c: &Connection,
    task_n: i64,
    attempt: Option<u32>,
) -> Result<Vec<Verification>> {
    let mut st = c.prepare(
        "SELECT n, attempt, name, kind, status, duration_ms, summary, evidence_path, created_at FROM verifications
         WHERE task_n = ?1 AND (?2 IS NULL OR attempt = ?2) ORDER BY n",
    )?;
    let rows = st.query_map(params![task_n, attempt], |r| {
        Ok(Verification {
            id: r.get(0)?,
            task_id: task_id(task_n),
            attempt: r.get(1)?,
            name: r.get(2)?,
            kind: r.get(3)?,
            status: dec(&r.get::<_, String>(4)?)?,
            duration_ms: r.get::<_, i64>(5)? as u64,
            summary: r.get(6)?,
            evidence_path: r.get(7)?,
            created_at: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn insert_finding(
    c: &Connection,
    task_n: i64,
    attempt: u32,
    severity: &str,
    file: Option<&str>,
    line: Option<u32>,
    message: &str,
) -> Result<()> {
    c.execute(
        "INSERT INTO findings (task_n, attempt, severity, file, line, message, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open')",
        params![task_n, attempt, severity, file, line, message],
    )?;
    Ok(())
}

pub fn resolve_findings(c: &Connection, task_n: i64) -> Result<()> {
    c.execute(
        "UPDATE findings SET status = 'resolved' WHERE task_n = ?1 AND status = 'open'",
        [task_n],
    )?;
    Ok(())
}

pub fn findings(c: &Connection, task_n: i64) -> Result<Vec<Finding>> {
    let mut st = c.prepare("SELECT n, severity, file, line, message, status FROM findings WHERE task_n = ?1 ORDER BY n")?;
    let rows = st.query_map([task_n], |r| {
        Ok(Finding {
            id: r.get(0)?,
            task_id: task_id(task_n),
            severity: r.get(1)?,
            file: r.get(2)?,
            line: r.get(3)?,
            message: r.get(4)?,
            status: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ---------------------------------------------------------------- outbox

pub struct OutboxItem {
    pub n: i64,
    pub target: String,
    pub op: String,
    pub project: String,
    pub task_n: Option<i64>,
    pub payload: Value,
    pub key: String,
    pub attempts: u32,
}

/// Queues an intent once per idempotency key; returns whether it was new.
pub fn enqueue(
    c: &Connection,
    target: &str,
    op: &str,
    project: &str,
    task_n: Option<i64>,
    payload: &Value,
    key: &str,
) -> Result<bool> {
    let now = now_rfc3339();
    let n = c.execute(
        "INSERT OR IGNORE INTO outbox (target, op, project, task_n, payload, idempotency_key, state, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)",
        params![target, op, project, task_n, payload.to_string(), key, now],
    )?;
    Ok(n == 1)
}

pub fn due_outbox(c: &Connection, target: &str, limit: u32) -> Result<Vec<OutboxItem>> {
    let mut st = c.prepare(
        "SELECT n, target, op, project, task_n, payload, idempotency_key, attempts FROM outbox
         WHERE target = ?1 AND state = 'pending' AND (next_attempt_at IS NULL OR next_attempt_at <= ?2)
         ORDER BY n LIMIT ?3",
    )?;
    let rows = st.query_map(params![target, now_rfc3339(), limit], |r| {
        Ok(OutboxItem {
            n: r.get(0)?,
            target: r.get(1)?,
            op: r.get(2)?,
            project: r.get(3)?,
            task_n: r.get(4)?,
            payload: opt_json(r.get(5)?),
            key: r.get(6)?,
            attempts: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn outbox_done(c: &Connection, n: i64, result: &Value) -> Result<()> {
    c.execute(
        "UPDATE outbox SET state = 'synced', result = ?2, updated_at = ?3 WHERE n = ?1",
        params![n, result.to_string(), now_rfc3339()],
    )?;
    Ok(())
}

/// Records a failure; after `max` attempts the intent is blocked.
pub fn outbox_failed(c: &Connection, n: i64, err: &str, max: u32, retry_at: &str) -> Result<bool> {
    let attempts: u32 = c.query_row("SELECT attempts + 1 FROM outbox WHERE n = ?1", [n], |r| {
        r.get(0)
    })?;
    let blocked = attempts >= max;
    c.execute(
        "UPDATE outbox SET attempts = ?2, last_error = ?3, state = ?4, next_attempt_at = ?5, updated_at = ?6 WHERE n = ?1",
        params![n, attempts, err, if blocked { "blocked" } else { "pending" }, retry_at, now_rfc3339()],
    )?;
    Ok(blocked)
}

pub fn outbox_result(c: &Connection, key: &str) -> Result<Option<Value>> {
    Ok(c.query_row(
        "SELECT result FROM outbox WHERE idempotency_key = ?1 AND state = 'synced'",
        [key],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()?
    .flatten()
    .and_then(|s| serde_json::from_str(&s).ok()))
}

// ---------------------------------------------------------------- deployments

pub fn insert_deployment(
    c: &Connection,
    project: &str,
    env: &str,
    task_n: Option<i64>,
    commit: &str,
    auth: &str,
) -> Result<i64> {
    c.execute(
        "INSERT INTO deployments (project, environment, task_n, commit_sha, state, authorization, started_at)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6)",
        params![project, env, task_n, commit, auth, now_rfc3339()],
    )?;
    Ok(c.last_insert_rowid())
}

pub fn finish_deployment(
    c: &Connection,
    n: i64,
    state: &str,
    health: Option<&str>,
    version_ok: Option<bool>,
    rolled_back: bool,
) -> Result<Deployment> {
    c.execute(
        "UPDATE deployments SET state = ?2, health = ?3, version_ok = ?4, rolled_back = ?5, finished_at = ?6 WHERE n = ?1",
        params![n, state, health, version_ok, rolled_back, now_rfc3339()],
    )?;
    Ok(list_deployments(c, None, None)?
        .into_iter()
        .find(|d| d.id == deploy_id(n))
        .ok_or_else(|| Error::NotFound(deploy_id(n)))?)
}

pub fn list_deployments(
    c: &Connection,
    project: Option<&str>,
    task_n: Option<i64>,
) -> Result<Vec<Deployment>> {
    let mut st = c.prepare(
        "SELECT n, project, environment, task_n, commit_sha, state, health, version_ok, rolled_back, authorization,
                started_at, finished_at FROM deployments
         WHERE (?1 IS NULL OR project = ?1) AND (?2 IS NULL OR task_n = ?2) ORDER BY n DESC LIMIT 200",
    )?;
    let rows = st.query_map(params![project, task_n], |r| {
        Ok(Deployment {
            id: deploy_id(r.get(0)?),
            project: r.get(1)?,
            environment: r.get(2)?,
            task_id: r.get::<_, Option<i64>>(3)?.map(task_id),
            commit: r.get(4)?,
            state: r.get(5)?,
            health: r.get(6)?,
            version_ok: r.get(7)?,
            rolled_back: r.get(8)?,
            authorization: r.get(9)?,
            started_at: r.get(10)?,
            finished_at: r.get(11)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ---------------------------------------------------------------- usage, budgets, chat

#[allow(clippy::too_many_arguments)]
pub fn record_usage(
    c: &Connection,
    run_n: Option<i64>,
    project: Option<&str>,
    task_n: Option<i64>,
    kind: RunKind,
    model: Option<&str>,
    o: &RunOutcome,
) -> Result<()> {
    c.execute(
        "INSERT INTO usage (run_n, project, task_n, kind, source, model, input_tokens, output_tokens,
            cache_read_tokens, cache_write_tokens, cost_usd, cost_basis, ts)
         VALUES (?1, ?2, ?3, ?4, 'claude-code/stream-json', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            run_n,
            project,
            task_n,
            enc(kind),
            model,
            o.input_tokens.unwrap_or(0) as i64,
            o.output_tokens.unwrap_or(0) as i64,
            o.cache_read_tokens.unwrap_or(0) as i64,
            o.cache_write_tokens.unwrap_or(0) as i64,
            o.cost_usd,
            o.cost_basis.clone().unwrap_or_else(|| "unreported".into()),
            now_rfc3339()
        ],
    )?;
    Ok(())
}

pub fn list_budgets(c: &Connection) -> Result<Vec<Budget>> {
    let mut st =
        c.prepare("SELECT scope, period, limit_usd FROM budgets ORDER BY scope, period")?;
    let rows = st.query_map([], |r| {
        Ok(Budget {
            scope: r.get(0)?,
            period: r.get(1)?,
            limit_usd: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn set_budget(c: &Connection, b: &Budget) -> Result<()> {
    if b.period != "day" && b.period != "month" {
        return Err(Error::Invalid("budget period must be day or month".into()));
    }
    if b.limit_usd <= 0.0 {
        c.execute(
            "DELETE FROM budgets WHERE scope = ?1 AND period = ?2",
            params![b.scope, b.period],
        )?;
    } else {
        c.execute(
            "INSERT INTO budgets (scope, period, limit_usd) VALUES (?1, ?2, ?3)
             ON CONFLICT(scope, period) DO UPDATE SET limit_usd = excluded.limit_usd",
            params![b.scope, b.period, b.limit_usd],
        )?;
    }
    Ok(())
}

/// Reported spend since `since` (RFC 3339) for a scope ("global" = all).
pub fn spent_since(c: &Connection, scope: &str, since: &str) -> Result<f64> {
    Ok(c.query_row(
        "SELECT COALESCE(SUM(cost_usd), 0) FROM usage WHERE ts >= ?1 AND (?2 = 'global' OR project = ?2)",
        params![since, scope],
        |r| r.get(0),
    )?)
}

pub fn chat_add(c: &Connection, role: &str, text: &str) -> Result<ChatMessage> {
    let ts = now_rfc3339();
    c.execute(
        "INSERT INTO chat (role, text, ts) VALUES (?1, ?2, ?3)",
        params![role, text, ts],
    )?;
    Ok(ChatMessage {
        id: c.last_insert_rowid(),
        role: role.into(),
        text: text.into(),
        ts,
    })
}

pub fn chat_history(c: &Connection, limit: u32) -> Result<Vec<ChatMessage>> {
    let mut st = c.prepare("SELECT n, role, text, ts FROM chat ORDER BY n DESC LIMIT ?1")?;
    let rows = st.query_map([limit], |r| {
        Ok(ChatMessage {
            id: r.get(0)?,
            role: r.get(1)?,
            text: r.get(2)?,
            ts: r.get(3)?,
        })
    })?;
    let mut v: Vec<ChatMessage> = rows.collect::<std::result::Result<_, _>>()?;
    v.reverse();
    Ok(v)
}

pub fn record_intervention(
    c: &Connection,
    task_n: Option<i64>,
    kind: &str,
    minutes: f64,
) -> Result<()> {
    c.execute(
        "INSERT INTO interventions (task_n, kind, minutes, ts) VALUES (?1, ?2, ?3, ?4)",
        params![task_n, kind, minutes, now_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn conn_with_project(slug: &str) -> (tempfile::TempDir, Connection) {
        let d = tempfile::tempdir().unwrap();
        let mut c = crate::db::open(&d.path().join("s.db")).unwrap();
        crate::db::migrate(&mut c).unwrap();
        c.execute(
            "INSERT INTO projects (slug, name, repo_path, created_at) VALUES (?1, ?1, ?2, 'x')",
            params![slug, format!("/r/{slug}")],
        )
        .unwrap();
        (d, c)
    }

    pub fn task(c: &Connection, project: &str, title: &str, dedupe: Option<&str>) -> TaskSummary {
        insert_task(
            c,
            NewTask {
                project: project.into(),
                title: title.into(),
                body: String::new(),
                category: Category::Bug,
                priority: 4,
                risk: Risk::Low,
                state: TaskState::Triaged,
                source: "manual".into(),
                source_url: None,
                dedupe_key: dedupe.map(Into::into),
                acceptance: vec![],
            },
        )
        .unwrap()
        .0
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn tasks_dedupe_and_state() {
        let (_d, c) = conn_with_project("app");
        let a = task(&c, "app", "fix", Some("gh:app#1"));
        let b = task(&c, "app", "fix again", Some("gh:app#1"));
        assert_eq!(a.id, b.id);
        assert_eq!(a.id, "T1");
        let t = set_task_state(&c, 1, TaskState::Blocked, Some("why")).unwrap();
        assert_eq!(t.state, TaskState::Blocked);
        assert_eq!(t.blocked_reason.as_deref(), Some("why"));
        let open = list_tasks(
            &c,
            &TaskQuery {
                open_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(open.len(), 1);
        assert!(set_task_field(&c, 1, "state; DROP TABLE tasks", &1).is_err());
    }

    #[test]
    fn runs_events_usage() {
        let (_d, c) = conn_with_project("app");
        task(&c, "app", "x", None);
        let r = insert_run(
            &c,
            NewRun {
                task_n: Some(1),
                project: Some("app"),
                kind: RunKind::Implement,
                runtime: "claude",
                model: None,
            },
        )
        .unwrap();
        run_started(&c, r, 42).unwrap();
        append_run_event(&c, r, "text", "hello", &Value::Null).unwrap();
        append_run_event(&c, r, "tool_use", "Bash: ls", &serde_json::json!({"x": 1})).unwrap();
        assert_eq!(run_events(&c, r, 1, 10).unwrap().len(), 1);
        assert_eq!(active_runs(&c).unwrap().len(), 1);
        let out = RunOutcome {
            state: Some(RunState::Succeeded),
            cost_usd: Some(0.5),
            ..Default::default()
        };
        let run = finish_run(&c, r, &out).unwrap();
        assert_eq!(run.state, RunState::Succeeded);
        record_usage(
            &c,
            Some(r),
            Some("app"),
            Some(1),
            RunKind::Implement,
            None,
            &out,
        )
        .unwrap();
        assert!((spent_since(&c, "global", "2000").unwrap() - 0.5).abs() < 1e-9);
        assert!((spent_since(&c, "other", "2000").unwrap()).abs() < 1e-9);
    }

    #[test]
    fn approvals_dedupe_and_decide() {
        let (_d, c) = conn_with_project("app");
        let mk = || NewApproval {
            kind: ApprovalKind::Merge,
            task_n: None,
            project: Some("app".into()),
            title: "t".into(),
            what: "w".into(),
            why: "y".into(),
            impact: "i".into(),
            risks: "r".into(),
            risk: Risk::Medium,
            reversible: true,
            evidence: "e".into(),
            recommendation: "approve".into(),
            if_rejected: "nothing".into(),
            urgent: false,
            payload: Value::Null,
            dedupe_key: Some("merge:T1".into()),
        };
        let (a, new) = insert_approval(&c, mk()).unwrap();
        assert!(new);
        assert!(!insert_approval(&c, mk()).unwrap().1);
        let a = decide_approval(
            &c,
            parse_id('A', &a.id).unwrap(),
            ApprovalState::Approved,
            "me",
            None,
        )
        .unwrap();
        assert_eq!(a.state, ApprovalState::Approved);
        assert!(decide_approval(&c, 1, ApprovalState::Rejected, "me", None).is_err());
        assert_eq!(count_pending_approvals(&c).unwrap(), 0);
    }

    #[test]
    fn outbox_idempotency() {
        let (_d, c) = conn_with_project("app");
        let p = serde_json::json!({"a": 1});
        assert!(enqueue(&c, "jira", "create", "app", None, &p, "k1").unwrap());
        assert!(!enqueue(&c, "jira", "create", "app", None, &p, "k1").unwrap());
        let due = due_outbox(&c, "jira", 10).unwrap();
        assert_eq!(due.len(), 1);
        assert!(!outbox_failed(&c, due[0].n, "boom", 2, "2000-01-01").unwrap());
        assert!(outbox_failed(&c, due[0].n, "boom", 2, "2000-01-01").unwrap());
        assert!(due_outbox(&c, "jira", 10).unwrap().is_empty());
    }

    #[test]
    fn signals_upsert() {
        let (_d, c) = conn_with_project("app");
        let s = |t| NewSignal {
            project: "app",
            source: "github",
            kind: "issue",
            external_id: "7",
            title: t,
            body: "",
            url: None,
            labels: &[],
        };
        let (n, new, _) = upsert_signal(&c, s("a")).unwrap();
        assert!(new);
        let (n2, new2, _) = upsert_signal(&c, s("b")).unwrap();
        assert_eq!((n, false), (n2, new2));
        assert_eq!(list_signals(&c, None, 10).unwrap()[0].title, "b");
    }

    #[test]
    fn plans_round_trip() {
        let (_d, c) = conn_with_project("app");
        task(&c, "app", "a", None);
        task(&c, "app", "b", None);
        insert_plan(
            &c,
            "P2026-10-19",
            "2026-10-19",
            "two",
            &[(2, 1, "first".into()), (1, 2, "second".into())],
        )
        .unwrap();
        let p = latest_plan(&c).unwrap().unwrap();
        assert_eq!(p.items[0].task.id, "T2");
        assert_eq!(plan_task_ns(&c, "P2026-10-19").unwrap(), vec![2, 1]);
    }

    #[test]
    fn controls_default() {
        let (_d, c) = conn_with_project("app");
        let mut ctl = controls(&c).unwrap();
        assert_eq!(ctl.mode, Mode::Normal);
        ctl.mode = Mode::Safe;
        set_controls(&c, &ctl).unwrap();
        assert_eq!(controls(&c).unwrap().mode, Mode::Safe);
    }
}
