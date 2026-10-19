//! `tp mcp`: the MCP server agents get (stdio, newline-delimited JSON-RPC).
//!
//! Agents reach anything privileged only through these tools; tp decides,
//! executes and redacts. No tool ever returns a secret value.

use std::io::{BufRead, Write};

use serde_json::{Value, json};
use tp_proto::{ActorKind, ApprovalKind, Risk};

use crate::audit::{AuditLog, NewEvent};
use crate::paths::FactoryPaths;
use crate::redact::Redactor;
use crate::store::{self, NewApproval};
use crate::{Error, Result, db, memory, project_config, verify};

pub fn tools() -> Value {
    json!([
        {"name": "task_context", "description": "The task you are working on: title, description, acceptance criteria, risk, project instructions and verification checks.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "verify_run", "description": "Run the project's verification checks in your worktree (build, tests, lint) and get the results. Run it before you finish.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "memory_search", "description": "Search validated lessons from earlier work on this project and globally.",
         "inputSchema": {"type": "object", "required": ["query"], "properties": {"query": {"type": "string"}}}},
        {"name": "propose_memory", "description": "Propose a reusable lesson (a fact about this codebase or process). The founder validates it before it is used.",
         "inputSchema": {"type": "object", "required": ["category", "text"], "properties": {"category": {"type": "string"}, "text": {"type": "string"}, "global": {"type": "boolean"}}}},
        {"name": "report_blocked", "description": "Stop and report that you cannot finish without the founder (ambiguous requirements, missing access, failing environment). Do not guess.",
         "inputSchema": {"type": "object", "required": ["reason"], "properties": {"reason": {"type": "string"}}}},
        {"name": "request_approval", "description": "Ask the founder to approve an action you are not allowed to take (destructive, security-sensitive, outside scope). After calling it, stop and summarize.",
         "inputSchema": {"type": "object", "required": ["title", "what", "why"], "properties": {
            "title": {"type": "string"}, "what": {"type": "string"}, "why": {"type": "string"},
            "risk": {"enum": ["low", "medium", "high", "critical"]}, "reversible": {"type": "boolean"}}}}
    ])
}

struct Ctx {
    paths: FactoryPaths,
    run: String,
    task_n: Option<i64>,
    project: Option<String>,
    worktree: Option<std::path::PathBuf>,
}

fn ctx(paths: &FactoryPaths, run: &str) -> Result<Ctx> {
    let c = db::open(&paths.db())?;
    let rn = store::parse_id('R', run)?;
    let (task_n, project, _) = store::run_task_project(&c, rn)?;
    let worktree = crate::hook::load_scope(paths, run).ok().map(|s| s.worktree);
    Ok(Ctx {
        paths: paths.clone(),
        run: run.into(),
        task_n,
        project,
        worktree,
    })
}

fn audit(cx: &Ctx, action: &str, details: Value) -> Result<()> {
    let mut c = db::open(&cx.paths.db())?;
    let mut e = NewEvent::new(ActorKind::Agent, &cx.run, action)
        .claimed()
        .details(details);
    if let Some(t) = cx.task_n {
        e = e.task(store::task_id(t));
    }
    if let Some(p) = &cx.project {
        e = e.project(p);
    }
    AuditLog::new(&cx.paths).append(&mut c, e)?;
    Ok(())
}

/// Executes one tool call; returns (text, is_error).
pub fn call(paths: &FactoryPaths, run: &str, name: &str, args: &Value) -> (String, bool) {
    match call_inner(paths, run, name, args) {
        Ok(t) => (Redactor::new().redact(&t), false),
        Err(e) => (format!("error: {e}"), true),
    }
}

fn call_inner(paths: &FactoryPaths, run: &str, name: &str, args: &Value) -> Result<String> {
    let cx = ctx(paths, run)?;
    let c = db::open(&paths.db())?;
    let arg = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match name {
        "task_context" => {
            let n = cx
                .task_n
                .ok_or_else(|| Error::Invalid("this run has no task".into()))?;
            let r = store::task_row(&c, n)?;
            let pc = project_config::load_project(paths, &r.summary.project)?;
            let v = project_config::load_verify(paths, &r.summary.project)?;
            Ok(serde_json::to_string_pretty(&json!({
                "task": r.summary.id, "title": r.summary.title, "description": r.body,
                "acceptance_criteria": r.acceptance, "category": r.summary.category, "risk": r.summary.risk,
                "project": r.summary.project, "base_branch": pc.base_branch, "project_instructions": pc.instructions,
                "verification_checks": v.checks.iter().map(|c| json!({"name": c.name, "run": c.run})).collect::<Vec<_>>(),
            }))?)
        }
        "verify_run" => {
            let project = cx
                .project
                .clone()
                .ok_or_else(|| Error::Invalid("this run has no project".into()))?;
            let wt = cx
                .worktree
                .clone()
                .ok_or_else(|| Error::Invalid("no worktree for this run".into()))?;
            let v = project_config::load_verify(paths, &project)?;
            if v.checks.is_empty() {
                return Ok("No verification checks are configured for this project. Say so in your summary.".into());
            }
            let dir = paths.root().join("runs").join(&cx.run).join("verify");
            let res = verify::run_checks(&wt, &v.checks, &dir, &|_| Vec::new(), &Redactor::new());
            audit(
                &cx,
                "agent.verify_run",
                json!({"results": res.iter().map(|r| json!({"name": r.name, "status": r.status})).collect::<Vec<_>>()}),
            )?;
            Ok(res
                .iter()
                .map(|r| format!("## {} — {:?}\n{}", r.name, r.status, r.summary))
                .collect::<Vec<_>>()
                .join("\n\n"))
        }
        "memory_search" => {
            let hits = memory::search(&c, &arg("query"), cx.project.as_deref(), false)?;
            if hits.is_empty() {
                return Ok("No validated lessons match.".into());
            }
            Ok(hits
                .iter()
                .map(|m| format!("- [{}:{}] {}", m.scope, m.category, m.text))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "propose_memory" => {
            let scope = if args.get("global").and_then(Value::as_bool) == Some(true) {
                "global".to_string()
            } else {
                cx.project.clone().unwrap_or_else(|| "global".into())
            };
            let (m, new) = memory::propose(
                &c,
                &scope,
                &arg("category"),
                &arg("text"),
                json!({"run": cx.run, "task": cx.task_n.map(store::task_id)}),
                0.5,
            )?;
            audit(&cx, "memory.proposed", json!({"id": m.id, "new": new}))?;
            Ok(format!(
                "Lesson {} recorded as proposed; the founder validates it.",
                m.id
            ))
        }
        "report_blocked" => {
            store::kv_set(&c, &format!("run_blocked:{}", cx.run), &arg("reason"))?;
            audit(&cx, "agent.blocked", json!({"reason": arg("reason")}))?;
            Ok(
                "Recorded. Stop now and end with a short summary; the founder will be notified."
                    .into(),
            )
        }
        "request_approval" => {
            let risk: Risk = args
                .get("risk")
                .cloned()
                .and_then(|r| serde_json::from_value(r).ok())
                .unwrap_or(Risk::High);
            let (a, _) = store::insert_approval(&c, NewApproval {
                kind: ApprovalKind::Action,
                task_n: cx.task_n,
                project: cx.project.clone(),
                title: arg("title"),
                what: arg("what"),
                why: arg("why"),
                impact: "Requested by the coding agent; the factory resumes the agent with your decision.".into(),
                risks: format!("Agent-rated risk {risk:?} (tp treats agent ratings as a floor)."),
                risk,
                reversible: args.get("reversible").and_then(Value::as_bool).unwrap_or(false),
                evidence: format!("Run {}", cx.run),
                recommendation: "Review the request; approve only if the action is needed.".into(),
                if_rejected: "The agent is told to find another way or report blocked.".into(),
                urgent: false,
                payload: json!({"run": cx.run}),
                dedupe_key: Some(format!("action:{}:{}", cx.run, arg("title"))),
            })?;
            store::kv_set(&c, &format!("run_awaiting:{}", cx.run), &a.id)?;
            audit(
                &cx,
                "agent.approval_requested",
                json!({"approval": a.id, "title": a.title}),
            )?;
            Ok(format!(
                "Approval {} requested. Stop now and summarize what you have done; you will be resumed with the decision.",
                a.id
            ))
        }
        other => Err(Error::Invalid(format!("unknown tool {other}"))),
    }
}

/// Serves MCP over stdin/stdout until EOF.
pub fn serve(
    paths: &FactoryPaths,
    run: &str,
    input: impl BufRead,
    mut out: impl Write,
) -> Result<()> {
    for line in input.lines() {
        let line = line.map_err(|e| Error::Daemon(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = req.get("id").cloned();
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Some(json!({
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "tp", "version": crate::VERSION}
            })),
            "tools/list" => Some(json!({"tools": tools()})),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let (text, is_error) = call(paths, run, name, &args);
                Some(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
            }
            "ping" => Some(json!({})),
            _ => None,
        };
        let Some(id) = id else { continue }; // notification
        let resp = match result {
            Some(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            None => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method {method} not found")}})
            }
        };
        writeln!(out, "{resp}").map_err(|e| Error::Daemon(e.to_string()))?;
        out.flush().map_err(|e| Error::Daemon(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::task;

    #[test]
    fn protocol_and_tools() {
        let d = tempfile::tempdir().unwrap();
        let paths = FactoryPaths::new(d.path().join("f"));
        crate::setup::setup(&paths, "t").unwrap();
        let c = db::open(&paths.db()).unwrap();
        c.execute("INSERT INTO projects (slug, name, repo_path, created_at) VALUES ('app','app','/r','x')", []).unwrap();
        task(&c, "app", "Fix crash", None);
        let rn = store::insert_run(
            &c,
            store::NewRun {
                task_n: Some(1),
                project: Some("app"),
                kind: tp_proto::RunKind::Implement,
                runtime: "claude",
                model: None,
            },
        )
        .unwrap();
        let run = store::run_id(rn);
        let wt = d.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        crate::hook::write_scope(
            &paths,
            &run,
            &crate::policy::RunScope {
                kind: tp_proto::RunKind::Implement,
                worktree: wt,
                factory_root: paths.root().into(),
                home: "/h".into(),
                policy: Default::default(),
            },
        )
        .unwrap();
        project_config::write_atomic(
            &project_config::verify_yaml(&paths, "app"),
            "checks:\n  - {name: t, kind: test, run: 'echo ok'}\n",
        )
        .unwrap();

        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"task_context","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"verify_run","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"request_approval","arguments":{"title":"drop table","what":"x","why":"y"}}}"#,
        ]
        .join("\n");
        let mut out = Vec::new();
        serve(&paths, &run, input.as_bytes(), &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 5, "notification gets no reply");
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 6);
        assert!(
            lines[2]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Fix crash")
        );
        assert!(
            lines[3]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Passed")
        );
        assert!(
            lines[4]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Approval A1")
        );
        assert!(
            store::kv_get(&c, &format!("run_awaiting:{run}"))
                .unwrap()
                .is_some()
        );
    }
}
