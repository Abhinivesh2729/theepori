//! GitHub through the `gh` CLI, run deterministically by tpd (ADR N4).
//! Uses gh's own stored login; tp never reads the token.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::policy::CiState;
use crate::{Error, Result};

type Runner = Arc<dyn Fn(&[String]) -> Result<String> + Send + Sync>;

#[derive(Clone)]
pub struct Gh {
    run: Runner,
}

impl std::fmt::Debug for Gh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gh")
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub url: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct WorkflowRun {
    #[serde(rename = "databaseId")]
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub conclusion: String,
    #[serde(default)]
    pub status: String,
    #[serde(rename = "headSha", default)]
    pub head_sha: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrStatus {
    pub number: u64,
    pub url: String,
    /// OPEN | MERGED | CLOSED
    pub state: String,
    pub mergeable: bool,
    pub ci: CiState,
    pub human_review_required: bool,
    pub head_sha: String,
    pub failing_checks: Vec<String>,
}

impl Gh {
    /// Real `gh` on PATH (or the given binary).
    pub fn cli(bin: Option<PathBuf>) -> Self {
        let bin = bin
            .or_else(|| crate::doctor::which("gh"))
            .unwrap_or_else(|| PathBuf::from("gh"));
        Self {
            run: Arc::new(move |args: &[String]| {
                let out = Command::new(&bin)
                    .args(args)
                    .env("GH_PROMPT_DISABLED", "1")
                    .env("NO_COLOR", "1")
                    .output()
                    .map_err(|e| {
                        Error::Invalid(format!("gh: {e}; install it with `brew install gh`"))
                    })?;
                if !out.status.success() {
                    return Err(Error::Invalid(format!(
                        "gh {} failed: {}",
                        args.first().map(String::as_str).unwrap_or_default(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }),
        }
    }

    /// For tests: answers from a closure.
    pub fn fake(f: impl Fn(&[String]) -> Result<String> + Send + Sync + 'static) -> Self {
        Self { run: Arc::new(f) }
    }

    fn call(&self, args: &[&str]) -> Result<String> {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        (self.run)(&a)
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, args: &[&str]) -> Result<T> {
        let raw = self.call(args)?;
        serde_json::from_str(&raw)
            .map_err(|e| Error::Invalid(format!("gh {}: unexpected output: {e}", args[0])))
    }

    pub fn open_issues(&self, repo: &str, labels: &[String]) -> Result<Vec<Issue>> {
        let mut args = vec![
            "issue",
            "list",
            "-R",
            repo,
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,body,url,labels,updatedAt",
        ];
        let joined = labels.join(",");
        if !labels.is_empty() {
            args.extend(["--label", joined.as_str()]);
        }
        self.json(&args)
    }

    /// Recent completed runs on a branch.
    pub fn runs(&self, repo: &str, branch: &str) -> Result<Vec<WorkflowRun>> {
        self.json(&[
            "run",
            "list",
            "-R",
            repo,
            "--branch",
            branch,
            "--limit",
            "10",
            "--json",
            "databaseId,name,conclusion,status,headSha,url",
        ])
    }

    pub fn failed_log(&self, repo: &str, run_id: u64) -> Result<String> {
        let log = self.call(&[
            "run",
            "view",
            &run_id.to_string(),
            "-R",
            repo,
            "--log-failed",
        ])?;
        let lines: Vec<&str> = log.lines().collect();
        Ok(lines[lines.len().saturating_sub(120)..].join("\n"))
    }

    pub fn pr_for_branch(&self, repo: &str, branch: &str) -> Result<Option<(u64, String)>> {
        let v: Vec<Value> = self.json(&[
            "pr",
            "list",
            "-R",
            repo,
            "--head",
            branch,
            "--state",
            "all",
            "--json",
            "number,url",
        ])?;
        Ok(v.first().and_then(|p| {
            Some((
                p.get("number")?.as_u64()?,
                p.get("url")?.as_str()?.to_owned(),
            ))
        }))
    }

    /// Creates a PR (idempotent: returns the existing one for the branch).
    pub fn create_pr(
        &self,
        repo: &str,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<(u64, String)> {
        if let Some(p) = self.pr_for_branch(repo, branch)? {
            return Ok(p);
        }
        let url = self.call(&[
            "pr", "create", "-R", repo, "--head", branch, "--base", base, "--title", title,
            "--body", body,
        ])?;
        let url = url
            .lines()
            .rev()
            .find(|l| l.contains("/pull/"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        let number = url
            .rsplit('/')
            .next()
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| {
                Error::Invalid(format!("gh pr create: no PR URL in output ({url:?})"))
            })?;
        Ok((number, url))
    }

    pub fn pr_status(&self, repo: &str, number: u64) -> Result<PrStatus> {
        let v: Value = self.json(&[
            "pr", "view", &number.to_string(), "-R", repo, "--json",
            "number,url,state,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,headRefOid",
        ])?;
        Ok(parse_pr_status(&v))
    }

    pub fn merge_pr(&self, repo: &str, number: u64) -> Result<()> {
        self.call(&[
            "pr",
            "merge",
            &number.to_string(),
            "-R",
            repo,
            "--squash",
            "--delete-branch",
        ])?;
        Ok(())
    }

    pub fn comment_pr(&self, repo: &str, number: u64, body: &str) -> Result<()> {
        self.call(&[
            "pr",
            "comment",
            &number.to_string(),
            "-R",
            repo,
            "--body",
            body,
        ])?;
        Ok(())
    }

    pub fn comment_issue(&self, repo: &str, number: u64, body: &str) -> Result<()> {
        self.call(&[
            "issue",
            "comment",
            &number.to_string(),
            "-R",
            repo,
            "--body",
            body,
        ])?;
        Ok(())
    }

    /// Open Dependabot alerts (empty when the API is not enabled for the repo).
    pub fn dependabot_alerts(&self, repo: &str) -> Result<Vec<Value>> {
        match self.json::<Vec<Value>>(&[
            "api",
            &format!("repos/{repo}/dependabot/alerts?state=open&per_page=100"),
        ]) {
            Ok(v) => Ok(v),
            Err(e) if e.to_string().contains("403") || e.to_string().contains("404") => {
                Ok(Vec::new())
            }
            Err(e) => Err(e),
        }
    }

    pub fn auth_ok(&self) -> bool {
        self.call(&["auth", "status"]).is_ok()
    }
}

pub fn parse_pr_status(v: &Value) -> PrStatus {
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut failing = Vec::new();
    let mut pending = false;
    let checks = v
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for c in &checks {
        let name = c
            .get("name")
            .or_else(|| c.get("context"))
            .and_then(Value::as_str)
            .unwrap_or("check")
            .to_string();
        let conclusion = c
            .get("conclusion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status = c.get("status").and_then(Value::as_str).unwrap_or_default();
        let state = c.get("state").and_then(Value::as_str).unwrap_or_default();
        match (conclusion, status, state) {
            (
                "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE",
                _,
                _,
            )
            | (_, _, "FAILURE" | "ERROR") => failing.push(name),
            ("", "QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED", _)
            | (_, _, "PENDING" | "EXPECTED") => pending = true,
            _ => {}
        }
    }
    let ci = if !failing.is_empty() {
        CiState::Failing
    } else if pending {
        CiState::Pending
    } else if checks.is_empty() {
        CiState::None
    } else {
        CiState::Passing
    };
    PrStatus {
        number: v.get("number").and_then(Value::as_u64).unwrap_or(0),
        url: s("url"),
        state: s("state"),
        mergeable: s("mergeable") != "CONFLICTING" && s("mergeStateStatus") != "DIRTY",
        ci,
        human_review_required: s("reviewDecision") == "REVIEW_REQUIRED"
            || s("reviewDecision") == "CHANGES_REQUESTED",
        head_sha: s("headRefOid"),
        failing_checks: failing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pr_status_rollup() {
        let st = parse_pr_status(&json!({
            "number": 3, "url": "u", "state": "OPEN", "mergeable": "MERGEABLE", "reviewDecision": "",
            "statusCheckRollup": [
                {"name": "ci", "conclusion": "SUCCESS", "status": "COMPLETED"},
                {"context": "lint", "state": "SUCCESS"}
            ]
        }));
        assert_eq!(st.ci, CiState::Passing);
        assert!(st.mergeable && !st.human_review_required);
        let st = parse_pr_status(
            &json!({"statusCheckRollup": [{"name": "ci", "status": "IN_PROGRESS", "conclusion": ""}], "reviewDecision": "REVIEW_REQUIRED"}),
        );
        assert_eq!(st.ci, CiState::Pending);
        assert!(st.human_review_required);
        let st = parse_pr_status(
            &json!({"statusCheckRollup": [{"name": "ci", "conclusion": "FAILURE"}], "mergeable": "CONFLICTING"}),
        );
        assert_eq!(st.ci, CiState::Failing);
        assert_eq!(st.failing_checks, vec!["ci"]);
        assert!(!st.mergeable);
        assert_eq!(parse_pr_status(&json!({})).ci, CiState::None);
    }

    #[test]
    fn create_pr_is_idempotent() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let c2 = calls.clone();
        let gh = Gh::fake(move |a| {
            c2.lock().unwrap().push(a.join(" "));
            Ok(match a[1].as_str() {
                "list"
                    if c2
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|c| c.starts_with("pr create"))
                        .count()
                        == 0 =>
                {
                    "[]".into()
                }
                "list" => r#"[{"number": 9, "url": "https://github.com/o/r/pull/9"}]"#.into(),
                "create" => "https://github.com/o/r/pull/9\n".into(),
                _ => "".into(),
            })
        });
        assert_eq!(gh.create_pr("o/r", "tp/T1", "main", "t", "b").unwrap().0, 9);
        assert_eq!(gh.create_pr("o/r", "tp/T1", "main", "t", "b").unwrap().0, 9);
        let creates = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("pr create"))
            .count();
        assert_eq!(creates, 1);
    }
}
