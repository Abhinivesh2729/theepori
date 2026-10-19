//! Verification (PRD §12): deterministic checks run by tp, and an
//! independent read-only reviewer. Agent claims never count as evidence.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tp_proto::{CheckStatus, Verification};

use crate::project_config::Check;
use crate::redact::Redactor;
use crate::runtime::{RunResult, RunSpec, Runtime, run_blocking};
use crate::{Error, Result};

/// Runs one shell command with a timeout; returns (exit code or None on timeout, combined output).
pub fn run_shell(
    cwd: &Path,
    cmd: &str,
    env: &[(String, String)],
    timeout: Duration,
) -> Result<(Option<i32>, String)> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("exec 2>&1; {cmd}"))
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Invalid(format!("run {cmd:?}: {e}")))?;
    let mut out = child
        .stdout
        .take()
        .ok_or_else(|| Error::Daemon("no stdout".into()))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().or(Some(-1)),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(Error::Daemon(format!("wait: {e}"))),
        }
    };
    let buf = reader.join().unwrap_or_default();
    Ok((code, String::from_utf8_lossy(&buf).into_owned()))
}

/// Runs the checks in order; evidence logs go to `evidence_dir`.
pub fn run_checks(
    worktree: &Path,
    checks: &[Check],
    evidence_dir: &Path,
    env_for: &dyn Fn(Option<&str>) -> Vec<(String, String)>,
    redactor: &Redactor,
) -> Vec<Verification> {
    let _ = std::fs::create_dir_all(evidence_dir);
    checks
        .iter()
        .map(|c| {
            let start = Instant::now();
            let env = env_for(c.secrets.as_deref());
            let mut red = redactor.clone();
            red.add(env.iter().map(|(_, v)| v.clone()));
            let (status, summary, path) =
                match run_shell(worktree, &c.run, &env, Duration::from_secs(c.timeout_secs)) {
                    Ok((code, out)) => {
                        let out = red.redact(&out);
                        let path = evidence_dir.join(format!("{}.log", slug(&c.name)));
                        let _ = std::fs::write(
                            &path,
                            format!("$ {}\n{}\n[exit: {:?}]\n", c.run, out, code),
                        );
                        let tail: Vec<&str> = out
                            .lines()
                            .rev()
                            .take(12)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect();
                        match code {
                            Some(0) => (
                                CheckStatus::Passed,
                                format!("passed in {:.1}s", start.elapsed().as_secs_f64()),
                                Some(path),
                            ),
                            Some(c2) => (
                                CheckStatus::Failed,
                                format!("exit {c2}\n{}", tail.join("\n")),
                                Some(path),
                            ),
                            None => (
                                CheckStatus::Failed,
                                format!("timed out after {}s\n{}", c.timeout_secs, tail.join("\n")),
                                Some(path),
                            ),
                        }
                    }
                    Err(e) => (CheckStatus::Blocked, e.to_string(), None),
                };
            Verification {
                id: 0,
                task_id: String::new(),
                attempt: 0,
                name: c.name.clone(),
                kind: c.kind.clone(),
                status,
                duration_ms: start.elapsed().as_millis() as u64,
                summary,
                evidence_path: path.map(|p| p.display().to_string()),
                created_at: crate::now_rfc3339(),
            }
        })
        .collect()
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Text block describing failures, for the repair prompt.
pub fn failure_report(v: &[Verification]) -> String {
    v.iter()
        .filter(|x| x.status != CheckStatus::Passed)
        .map(|x| format!("### {} ({:?})\n{}", x.name, x.status, x.summary))
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---------------------------------------------------------------- review

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewFinding {
    /// blocking | major | minor | info
    pub severity: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Review {
    pub addresses_task: bool,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
    /// The reviewer's own risk rating; can only raise the computed risk.
    #[serde(default)]
    pub risk: Option<tp_proto::Risk>,
}

impl Review {
    pub fn blocking(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == "blocking")
            .count()
            + usize::from(!self.addresses_task)
    }
}

pub fn review_schema() -> Value {
    json!({
        "type": "object",
        "required": ["addresses_task", "summary", "findings"],
        "properties": {
            "addresses_task": {"type": "boolean"},
            "summary": {"type": "string"},
            "risk": {"enum": ["low", "medium", "high", "critical"]},
            "findings": {"type": "array", "items": {"type": "object", "required": ["severity", "message"], "properties": {
                "severity": {"enum": ["blocking", "major", "minor", "info"]},
                "file": {"type": "string"}, "line": {"type": "integer"}, "message": {"type": "string"}}}}
        }
    })
}

pub fn review_prompt(
    title: &str,
    body: &str,
    acceptance: &[String],
    diff: &str,
    checks: &str,
) -> String {
    format!(
        "You are an independent code reviewer. Another agent made the change below. Review it for correctness, \
         security, regressions and whether it actually addresses the task. Read files as needed (read-only). \
         Mark a finding `blocking` only for real defects that must be fixed before merge. Be concise.\n\n\
         Task: {title}\n{}\n\nAcceptance criteria:\n{}\n\nDeterministic check results:\n{checks}\n\nDiff:\n```diff\n{diff}\n```",
        crate::manager::truncate(body, 4000),
        acceptance
            .iter()
            .map(|a| format!("- {a}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn review(
    rt: &dyn Runtime,
    worktree: &Path,
    model: Option<&str>,
    settings: Option<PathBuf>,
    prompt: &str,
) -> Result<(Review, RunResult)> {
    let spec = RunSpec {
        cwd: worktree.into(),
        prompt: prompt.into(),
        model: Some(model.unwrap_or("haiku").into()),
        tools: Some(vec![
            "Read".into(),
            "Grep".into(),
            "Glob".into(),
            "Bash".into(),
        ]),
        allowed_tools: vec!["Read".into(), "Grep".into(), "Glob".into()],
        strict_mcp: true,
        settings,
        json_schema: Some(review_schema()),
        max_turns: Some(20),
        max_budget_usd: Some(1.0),
        ..Default::default()
    };
    let res = run_blocking(rt, &spec, Box::new(|_| {}))?;
    let v = res.structured.clone().ok_or_else(|| {
        Error::Daemon(format!(
            "reviewer returned no structured output: {}",
            res.text
        ))
    })?;
    let r: Review = serde_json::from_value(v)?;
    Ok((r, res))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, run: &str, timeout: u64) -> Check {
        Check {
            name: name.into(),
            kind: "test".into(),
            run: run.into(),
            timeout_secs: timeout,
            required: true,
            secrets: None,
        }
    }

    #[test]
    fn checks_pass_fail_timeout_and_redact() {
        let d = tempfile::tempdir().unwrap();
        let ev = d.path().join("ev");
        let env =
            |_: Option<&str>| vec![("MY_SECRET".to_string(), "supersecretvalue123".to_string())];
        let v = run_checks(
            d.path(),
            &[
                check("ok", "echo fine", 10),
                check("bad", "echo $MY_SECRET; exit 3", 10),
                check("slow", "sleep 5", 1),
            ],
            &ev,
            &env,
            &Redactor::new(),
        );
        assert_eq!(v[0].status, CheckStatus::Passed);
        assert_eq!(v[1].status, CheckStatus::Failed);
        assert!(v[1].summary.contains("exit 3"));
        assert!(!v[1].summary.contains("supersecretvalue123"));
        let log = std::fs::read_to_string(v[1].evidence_path.as_ref().unwrap()).unwrap();
        assert!(!log.contains("supersecretvalue123"));
        assert_eq!(v[2].status, CheckStatus::Failed);
        assert!(v[2].summary.contains("timed out"));
        assert!(failure_report(&v).contains("### bad"));
    }

    #[test]
    fn review_blocking_count() {
        let r = Review {
            addresses_task: false,
            summary: "".into(),
            findings: vec![ReviewFinding {
                severity: "blocking".into(),
                file: None,
                line: None,
                message: "x".into(),
            }],
            risk: None,
        };
        assert_eq!(r.blocking(), 2);
    }
}
