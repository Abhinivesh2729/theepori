//! `tp hook pre|post`: Claude Code PreToolUse / PostToolUse handlers.
//!
//! Policy is enforced here, outside the model. Any error denies (fail closed).

use std::path::Path;

use serde_json::{Value, json};
use tp_proto::{ActorKind, Outcome};

use crate::audit::{AuditLog, NewEvent};
use crate::paths::FactoryPaths;
use crate::policy::{RunScope, decide_tool};
use crate::{Error, Result, db};

pub fn scope_path(paths: &FactoryPaths, run_id: &str) -> std::path::PathBuf {
    paths.root().join("runs").join(run_id).join("scope.json")
}

pub fn write_scope(paths: &FactoryPaths, run_id: &str, scope: &RunScope) -> Result<()> {
    let p = scope_path(paths, run_id);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| Error::io(d, e))?;
    }
    std::fs::write(&p, serde_json::to_vec_pretty(scope)?).map_err(|e| Error::io(&p, e))
}

pub fn load_scope(paths: &FactoryPaths, run_id: &str) -> Result<RunScope> {
    let p = scope_path(paths, run_id);
    let raw = std::fs::read(&p).map_err(|e| Error::io(&p, e))?;
    Ok(serde_json::from_slice(&raw)?)
}

fn deny_json(reason: &str) -> Value {
    json!({ "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": format!("Thee Pori policy: {reason}")
    }})
}

/// Returns the JSON to print (None = no opinion / allow through normal rules).
pub fn pre(paths: &FactoryPaths, run_id: &str, input: &str) -> Value {
    match pre_inner(paths, run_id, input) {
        Ok(v) => v,
        Err(e) => deny_json(&format!("policy check failed ({e}); denied by default")),
    }
}

fn pre_inner(paths: &FactoryPaths, run_id: &str, input: &str) -> Result<Value> {
    let ev: Value = serde_json::from_str(input)?;
    let tool = ev
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool_input = ev.get("tool_input").cloned().unwrap_or(Value::Null);
    let scope = load_scope(paths, run_id)?;
    let d = decide_tool(&scope, tool, &tool_input);
    if d.allow {
        return Ok(Value::Null);
    }
    audit_tool(
        paths,
        run_id,
        tool,
        &tool_input,
        Outcome::Denied,
        Some((d.rule, &d.reason)),
    )?;
    Ok(deny_json(&d.reason))
}

pub fn post(paths: &FactoryPaths, run_id: &str, input: &str) -> Result<()> {
    let ev: Value = serde_json::from_str(input)?;
    let tool = ev
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool_input = ev.get("tool_input").cloned().unwrap_or(Value::Null);
    audit_tool(paths, run_id, tool, &tool_input, Outcome::Ok, None)
}

fn audit_tool(
    paths: &FactoryPaths,
    run_id: &str,
    tool: &str,
    input: &Value,
    outcome: Outcome,
    rule: Option<(&str, &str)>,
) -> Result<()> {
    let mut conn = db::open(&paths.db())?;
    let (task, project) = run_context(&conn, run_id);
    let mut e = NewEvent::new(
        ActorKind::Agent,
        run_id,
        if outcome == Outcome::Denied {
            "tool.denied"
        } else {
            "tool.used"
        },
    )
    .resource(crate::runtime::tool_summary(tool, input))
    .outcome(outcome)
    .details(json!({ "tool": tool, "rule": rule.map(|r| r.0), "reason": rule.map(|r| r.1) }));
    if let Some(t) = task {
        e = e.task(t);
    }
    if let Some(p) = project {
        e = e.project(p);
    }
    AuditLog::new(paths).append(&mut conn, e)?;
    Ok(())
}

fn run_context(conn: &rusqlite::Connection, run_id: &str) -> (Option<String>, Option<String>) {
    let Ok(n) = crate::store::parse_id('R', run_id) else {
        return (None, None);
    };
    crate::store::run_task_project(conn, n)
        .map(|(t, p, _)| (t.map(crate::store::task_id), p))
        .unwrap_or((None, None))
}

/// Used by tests and `tp doctor` to prove the hook denies when misconfigured.
pub fn self_test(paths: &FactoryPaths) -> bool {
    let out = pre(
        paths,
        "R0-missing",
        r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#,
    );
    out.pointer("/hookSpecificOutput/permissionDecision")
        .and_then(Value::as_str)
        == Some("deny")
        && !Path::new(&scope_path(paths, "R0-missing")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_by_policy_and_fails_closed() {
        let d = tempfile::tempdir().unwrap();
        let paths = FactoryPaths::new(d.path().join("f"));
        crate::setup::setup(&paths, "t").unwrap();
        assert!(self_test(&paths), "missing scope must deny");
        let scope = RunScope {
            kind: tp_proto::RunKind::Implement,
            worktree: paths.root().join("projects/a/worktrees/T1"),
            factory_root: paths.root().into(),
            home: "/Users/me".into(),
            policy: crate::config::Policy::default(),
        };
        write_scope(&paths, "R1", &scope).unwrap();
        let ok = pre(
            &paths,
            "R1",
            r#"{"tool_name":"Bash","tool_input":{"command":"cargo test"}}"#,
        );
        assert!(ok.is_null());
        let no = pre(
            &paths,
            "R1",
            r#"{"tool_name":"Bash","tool_input":{"command":"git push origin x"}}"#,
        );
        assert_eq!(
            no.pointer("/hookSpecificOutput/permissionDecision")
                .unwrap(),
            "deny"
        );
        let garbage = pre(&paths, "R1", "not json");
        assert_eq!(
            garbage
                .pointer("/hookSpecificOutput/permissionDecision")
                .unwrap(),
            "deny"
        );
        post(
            &paths,
            "R1",
            r#"{"tool_name":"Bash","tool_input":{"command":"cargo test"}}"#,
        )
        .unwrap();
        let conn = db::open(&paths.db()).unwrap();
        let tail = AuditLog::new(&paths).tail(&conn, 5).unwrap();
        let actions: Vec<_> = tail.iter().map(|r| r.action.as_str()).collect();
        assert!(
            actions.contains(&"tool.denied") && actions.contains(&"tool.used"),
            "{actions:?}"
        );
    }
}
