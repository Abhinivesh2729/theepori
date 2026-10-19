//! Deterministic policy: command classification (ported from the Go agent),
//! per-tool decisions for agent runs (enforced by the PreToolUse hook),
//! change-risk classification and the merge/deploy gates.
//!
//! Nothing here consults an LLM. An agent's own risk estimate can only raise
//! the level.

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tp_proto::{Category, CheckStatus, Mode, Risk, RunKind};

use crate::config::{Policy, RiskLimit};

// ---------------------------------------------------------------- commands

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CmdRisk {
    Read,
    Exec,
    Destructive,
}

fn destructive_rules() -> &'static [(Regex, &'static str)] {
    static R: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    R.get_or_init(|| {
        [
            (r"\brm\s+(-[A-Za-z]*[rRf][A-Za-z]*\s+)+", "recursive/forced delete"),
            (r"\bgit\s+push\b", "publishes commits to a remote"),
            (r"\bgit\s+reset\s+--hard\b", "discards uncommitted work"),
            (r"\bgit\s+clean\s+-[A-Za-z]*f", "deletes untracked files"),
            (r"\bgit\s+(checkout|restore)\s+(--\s+)?\.(\s|$)", "discards working-tree changes"),
            (r"\bgit\s+branch\s+-D\b", "force-deletes a branch"),
            (r"\bgit\s+(rebase|filter-branch|filter-repo)\b", "rewrites history"),
            (r"\bgit\s+stash\s+(drop|clear)\b", "deletes stashed work"),
            (r"\bsudo\b", "runs as root"),
            (r"\b(mkfs|fdisk|diskutil\s+erase)", "formats disks"),
            (r"\bdd\s+.*\bof=", "raw disk write"),
            (r"\b(chmod|chown)\s+-R\b", "recursive permission change"),
            (r"\b(shutdown|reboot|halt)\b", "stops the machine"),
            (r"(curl|wget)\b[^|]*\|\s*(sudo\s+)?(ba|z|fi)?sh\b", "executes a downloaded script"),
            (r"\bkubectl\s+(delete|apply|replace|scale|drain)\b", "changes a cluster"),
            (r"\bterraform\s+(apply|destroy|import)\b", "changes infrastructure"),
            (r"\bpulumi\s+(up|destroy)\b", "changes infrastructure"),
            (r"\bhelm\s+(install|upgrade|uninstall|delete|rollback)\b", "changes a cluster"),
            (r"\b(vercel|netlify)\b.*(--prod|deploy)", "deploys"),
            (r"\bwrangler\s+(deploy|publish|delete)\b", "deploys to Cloudflare"),
            (r"\bfirebase\s+(deploy|.*:delete)", "deploys to Firebase"),
            (r"\baws\s+\S+\s+(delete|terminate|rm|remove|destroy|put-bucket-policy)", "changes AWS resources"),
            (r"\bgcloud\b.*\b(delete|deploy)\b", "changes GCP resources"),
            (r"\bdocker\s+(system\s+prune|volume\s+(rm|prune)|rm|rmi)\b", "deletes containers/images/volumes"),
            (r"(?i)\b(drop\s+(table|database|schema)|truncate\s+table)\b", "destroys database data"),
            (r"\b(npm|pnpm|yarn)\s+publish\b|\bcargo\s+publish\b|\btwine\s+upload\b", "publishes a package"),
            (r"\bgh\s+(pr\s+(merge|close)|release\s+(create|delete)|repo\s+(delete|edit)|secret)\b", "changes GitHub state"),
            (r"\bfind\b.*\s-delete\b", "bulk delete"),
            (r"\bxargs\s+(-\S+\s+)*rm\b", "bulk delete"),
            (r">\s*/dev/(sd|disk|nvme)", "raw disk write"),
            (r":\(\)\s*\{", "fork bomb"),
            (r"\b(ssh-keygen|gpg\s+--delete|security\s+delete)", "credential change"),
            (r"\b(export|setx)\s+\w*(KEY|TOKEN|SECRET|PASSWORD)\w*=", "credential change"),
        ]
        .iter()
        .map(|(p, r)| (Regex::new(p).expect("static pattern"), *r))
        .collect()
    })
}

/// Commands no agent may run, whatever the policy says.
fn agent_forbidden() -> &'static [(Regex, &'static str)] {
    static R: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    R.get_or_init(|| {
        [
            (r"(^|[\s;&|(])gh(\s|$)", "GitHub CLI is reserved for tpd"),
            (
                r"\bgit\s+push\b",
                "pushing is done by tpd after verification",
            ),
            (
                r"(^|[\s;&|(])tp(d)?(\s|$)",
                "the factory CLI is not available to agents",
            ),
            (r"(^|[\s;&|(])security\s", "keychain access"),
            (r"\blaunchctl\b", "service control"),
            (r"\bcloudflared\b", "tunnels are managed by tpd"),
            (
                r"\bclaude\s+(config|mcp|update|setup-token)",
                "Claude Code configuration",
            ),
            (r"\b(printenv|env)\s*($|[|;&>])", "dumps the environment"),
        ]
        .iter()
        .map(|(p, r)| (Regex::new(p).expect("static pattern"), *r))
        .collect()
    })
}

fn split_segments(cmd: &str) -> Vec<&str> {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\|\||&&|[|;\n]").expect("static"))
        .split(cmd)
        .collect()
}

const READ_ONLY: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "wc", "grep", "rg", "echo", "which", "file", "stat", "du",
    "df", "tree", "date", "whoami", "uname", "sort", "uniq", "cut", "diff", "basename", "dirname",
    "realpath",
];
const READ_ONLY_GIT: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
    "branch",
    "remote",
    "describe",
    "shortlog",
    "tag",
];

pub fn classify_command(cmd: &str) -> (CmdRisk, &'static str) {
    for (re, why) in destructive_rules() {
        if re.is_match(cmd) {
            return (CmdRisk::Destructive, why);
        }
    }
    if is_read_only(cmd) {
        (CmdRisk::Read, "")
    } else {
        (CmdRisk::Exec, "")
    }
}

fn is_read_only(cmd: &str) -> bool {
    if cmd.contains('>') || cmd.contains('`') || cmd.contains("$(") || cmd.contains("<(") {
        return false;
    }
    for seg in split_segments(cmd) {
        let f: Vec<&str> = seg.split_whitespace().collect();
        let Some(first) = f.first() else { continue };
        if READ_ONLY.contains(first) {
            continue;
        }
        if *first == "find" {
            if seg.contains("-exec") || seg.contains("-delete") || seg.contains("-ok") {
                return false;
            }
            continue;
        }
        if *first == "git" && f.len() > 1 && READ_ONLY_GIT.contains(&f[1]) {
            let sub = f[1];
            if (sub == "branch" || sub == "tag") && f.len() > 2 {
                return false;
            }
            if sub == "remote" && f.len() > 2 && f[2] != "-v" {
                return false;
            }
            continue;
        }
        return false;
    }
    true
}

/// `*` wildcard match (anything, including `/`); otherwise substring match.
pub fn pattern_matches(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return text.contains(pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(idx) if i == 0 && idx != 0 && !pattern.starts_with('*') => return false,
            Some(idx) => pos += idx + part.len(),
            None => return false,
        }
    }
    pattern.ends_with('*') || pos == text.len() || parts.last().is_some_and(|p| p.is_empty())
}

// ---------------------------------------------------------------- tool calls

/// Where an agent run is allowed to act.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunScope {
    pub kind: RunKind,
    pub worktree: PathBuf,
    pub factory_root: PathBuf,
    pub home: PathBuf,
    pub policy: Policy,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDecision {
    pub allow: bool,
    pub reason: String,
    pub rule: &'static str,
}

impl ToolDecision {
    fn allow(rule: &'static str) -> Self {
        Self {
            allow: true,
            reason: String::new(),
            rule,
        }
    }
    fn deny(rule: &'static str, reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            reason: reason.into(),
            rule,
        }
    }
}

const READ_TOOLS: &[&str] = &["Read", "Grep", "Glob", "LS", "NotebookRead"];
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const HARMLESS: &[&str] = &[
    "TodoWrite",
    "ToolSearch",
    "Task",
    "Agent",
    "ExitPlanMode",
    "TaskOutput",
];

/// Decides one PreToolUse call. Deny wins; unknown tools are denied.
pub fn decide_tool(scope: &RunScope, tool: &str, input: &Value) -> ToolDecision {
    match scope.kind {
        RunKind::Steward => {
            return if tool.starts_with("mcp__claude_ai_Atlassian__") || tool == "ToolSearch" {
                ToolDecision::allow("steward.atlassian")
            } else {
                ToolDecision::deny(
                    "steward.only_jira",
                    format!("steward runs may only use Jira tools, not {tool}"),
                )
            };
        }
        RunKind::Manager => {
            return ToolDecision::deny("manager.no_tools", "manager reasoning runs have no tools");
        }
        RunKind::Review => {
            if READ_TOOLS.contains(&tool) {
                return check_path(scope, input, false);
            }
            if tool == "Bash" {
                let cmd = input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return if classify_command(cmd).0 == CmdRisk::Read {
                    ToolDecision::allow("review.read_only_bash")
                } else {
                    ToolDecision::deny("review.read_only", "the reviewer is read-only")
                };
            }
            if tool == "ToolSearch" || tool == "TodoWrite" {
                return ToolDecision::allow("review.harmless");
            }
            return ToolDecision::deny(
                "review.read_only",
                format!("the reviewer may not use {tool}"),
            );
        }
        RunKind::Implement | RunKind::Repair | RunKind::Intervene => {}
    }

    if tool.starts_with("mcp__tp__") {
        return ToolDecision::allow("mcp.tp");
    }
    if tool.starts_with("mcp__") {
        return ToolDecision::deny(
            "mcp.other",
            format!("{tool} is not available to coding runs"),
        );
    }
    if HARMLESS.contains(&tool) {
        return ToolDecision::allow("tool.harmless");
    }
    if tool == "WebFetch" || tool == "WebSearch" {
        return if scope.policy.allow_web {
            ToolDecision::allow("web.allowed")
        } else {
            ToolDecision::deny(
                "web.disabled",
                "web access is disabled by policy (policy.allow_web)",
            )
        };
    }
    if READ_TOOLS.contains(&tool) {
        return check_path(scope, input, false);
    }
    if EDIT_TOOLS.contains(&tool) {
        return check_path(scope, input, true);
    }
    if tool == "Bash" {
        let cmd = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return decide_bash(scope, cmd);
    }
    ToolDecision::deny("tool.unknown", format!("{tool} is not on the allow list"))
}

fn decide_bash(scope: &RunScope, cmd: &str) -> ToolDecision {
    for (re, why) in agent_forbidden() {
        if re.is_match(cmd) {
            return ToolDecision::deny("bash.forbidden", format!("{why}: `{}`", short(cmd)));
        }
    }
    for p in &scope.policy.deny_commands {
        if pattern_matches(p, cmd) {
            return ToolDecision::deny(
                "bash.policy_deny",
                format!("matches policy deny_commands `{p}`"),
            );
        }
    }
    for p in sensitive_paths(scope) {
        let s = p.to_string_lossy();
        if s.len() > 1 && cmd.contains(s.as_ref()) {
            return ToolDecision::deny(
                "bash.sensitive_path",
                format!("touches protected path {s}"),
            );
        }
    }
    if cmd.contains(".env") && !cmd.contains(".env.example") {
        return ToolDecision::deny("bash.dotenv", "agents may not read or write .env files");
    }
    match classify_command(cmd) {
        (CmdRisk::Destructive, why) => {
            if why == "recursive/forced delete" && rm_is_local(cmd) {
                return ToolDecision::allow("bash.local_delete");
            }
            ToolDecision::deny(
                "bash.destructive",
                format!("{why}; ask the founder with the request_approval tool instead"),
            )
        }
        _ => ToolDecision::allow("bash.sandboxed"),
    }
}

/// `rm` of plain relative paths (no `..`, `~`, `/`, globs at the top).
fn rm_is_local(cmd: &str) -> bool {
    let segs = split_segments(cmd);
    if segs.len() != 1 {
        return false;
    }
    let f: Vec<&str> = cmd.split_whitespace().collect();
    if f.first() != Some(&"rm") {
        return false;
    }
    let targets: Vec<&&str> = f[1..].iter().filter(|a| !a.starts_with('-')).collect();
    !targets.is_empty()
        && targets.iter().all(|t| {
            !t.starts_with('/')
                && !t.starts_with('~')
                && !t.contains("..")
                && !t.starts_with('*')
                && **t != "."
        })
}

/// Paths no agent may touch. The factory root itself is handled separately
/// because task worktrees live under it.
fn sensitive_paths(scope: &RunScope) -> Vec<PathBuf> {
    let h = &scope.home;
    let f = &scope.factory_root;
    vec![
        f.join("vault"),
        f.join("state.db"),
        f.join("audit"),
        f.join("runs"),
        f.join("factory.yaml"),
        f.join("tpd.sock"),
        h.join(".claude"),
        h.join(".ssh"),
        h.join(".aws"),
        h.join(".gnupg"),
        h.join(".config/gh"),
        h.join(".cloudflared"),
        h.join("Library/Keychains"),
    ]
}

fn input_path(input: &Value) -> Option<&str> {
    ["file_path", "path", "notebook_path"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
}

/// Lexically normalizes `p` against `base` (no filesystem access).
pub fn normalize(base: &Path, p: &str) -> PathBuf {
    let joined = if Path::new(p).is_absolute() {
        PathBuf::from(p)
    } else {
        base.join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn check_path(scope: &RunScope, input: &Value, edit: bool) -> ToolDecision {
    let Some(raw) = input_path(input) else {
        return if edit {
            ToolDecision::deny("path.missing", "edit without a path")
        } else {
            ToolDecision::allow("path.none")
        };
    };
    let path = normalize(&scope.worktree, raw);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.starts_with(".env") && name != ".env.example" && name != ".env.sample" {
        return ToolDecision::deny("path.dotenv", "agents may not read or write .env files");
    }
    for s in sensitive_paths(scope) {
        if path.starts_with(&s) {
            return ToolDecision::deny("path.protected", format!("{} is protected", s.display()));
        }
    }
    if path.starts_with(&scope.factory_root) && !path.starts_with(&scope.worktree) {
        return ToolDecision::deny(
            "path.factory",
            "factory state outside this task's worktree is protected",
        );
    }
    let text = path.to_string_lossy();
    for p in &scope.policy.deny_paths {
        if pattern_matches(p, &text) {
            return ToolDecision::deny(
                "path.policy_deny",
                format!("matches policy deny_paths `{p}`"),
            );
        }
    }
    if edit {
        if !path.starts_with(&scope.worktree) {
            return ToolDecision::deny(
                "path.outside_worktree",
                format!(
                    "edits must stay inside the task worktree ({})",
                    scope.worktree.display()
                ),
            );
        }
        if path.starts_with(scope.worktree.join(".git")) {
            return ToolDecision::deny("path.git_dir", "the .git directory is managed by tpd");
        }
    }
    ToolDecision::allow(if edit {
        "path.edit_in_worktree"
    } else {
        "path.read"
    })
}

fn short(s: &str) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s
    }
}

// ---------------------------------------------------------------- change risk

const DEP_FILES: &[&str] = &[
    "package.json",
    "package-lock.json",
    "Cargo.toml",
    "Cargo.lock",
    "go.mod",
    "go.sum",
    "pubspec.yaml",
    "pubspec.lock",
    "requirements.txt",
    "poetry.lock",
    "pyproject.toml",
    "yarn.lock",
    "pnpm-lock.yaml",
];

/// Deterministic risk of a change. `claimed` (from an agent) can only raise it.
pub fn change_risk(
    policy: &Policy,
    category: Category,
    files: &[tp_proto::DiffFile],
    claimed: Option<Risk>,
) -> (Risk, Vec<String>) {
    let mut risk = Risk::Low;
    let mut why = Vec::new();
    let mut raise = |r: Risk, reason: String, risk: &mut Risk| {
        if r > *risk {
            *risk = r;
        }
        why.push(reason);
    };
    if matches!(category, Category::Security | Category::Incident) {
        raise(Risk::High, format!("{category:?} work"), &mut risk);
    }
    for f in files {
        let lower = f.path.to_ascii_lowercase();
        if let Some(p) = policy
            .high_risk_paths
            .iter()
            .find(|p| lower.contains(&p.to_ascii_lowercase()))
        {
            raise(
                Risk::High,
                format!("{} touches high-risk area `{p}`", f.path),
                &mut risk,
            );
        }
        let base = f.path.rsplit('/').next().unwrap_or(&f.path);
        if DEP_FILES.contains(&base) {
            raise(
                Risk::Medium,
                format!("{} changes dependencies", f.path),
                &mut risk,
            );
        }
    }
    let lines: u32 = files.iter().map(|f| f.additions + f.deletions).sum();
    if lines > policy.medium_risk_lines {
        raise(Risk::Medium, format!("{lines} changed lines"), &mut risk);
    }
    if files.len() > 25 {
        raise(
            Risk::Medium,
            format!("{} files changed", files.len()),
            &mut risk,
        );
    }
    if let Some(c) = claimed.filter(|c| *c > risk) {
        raise(c, format!("agent rated it {c:?}"), &mut risk);
    }
    (risk, why)
}

// ---------------------------------------------------------------- gates

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiState {
    Passing,
    Pending,
    Failing,
    /// No checks configured on the PR.
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeFacts {
    pub mode: Mode,
    pub auto_merge_enabled: bool,
    pub risk: Risk,
    pub verifications: Vec<(String, CheckStatus, bool)>,
    pub open_blocking_findings: u32,
    pub review_required_by_policy: bool,
    pub review_ran: bool,
    pub ci: CiState,
    /// GitHub says a human review is required (branch protection).
    pub human_review_required: bool,
    pub mergeable: bool,
    pub founder_approved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDecision {
    /// May merge now.
    pub allowed: bool,
    /// Blocked only because a human must approve (create an approval).
    pub needs_founder: bool,
    /// Waiting on something that may resolve on its own (CI pending).
    pub wait: bool,
    pub reasons: Vec<String>,
    /// Policy rule or approval that authorizes the merge.
    pub authorization: String,
}

/// The merge gate (PRD §9). Evidence first, then authority.
pub fn merge_gate(policy: &Policy, f: &MergeFacts) -> GateDecision {
    let mut hard = Vec::new();
    if f.mode == Mode::Safe {
        hard.push("factory is in safe mode".to_string());
    }
    let required: Vec<_> = f.verifications.iter().filter(|(_, _, req)| *req).collect();
    if required.is_empty() {
        hard.push("no required verification evidence".into());
    }
    for (name, st, _) in &required {
        if *st != CheckStatus::Passed {
            hard.push(format!("check `{name}` is {st:?}"));
        }
    }
    if f.open_blocking_findings > 0 {
        hard.push(format!(
            "{} blocking review finding(s) open",
            f.open_blocking_findings
        ));
    }
    if f.review_required_by_policy && !f.review_ran {
        hard.push("independent review has not run".into());
    }
    if f.ci == CiState::Failing {
        hard.push("required CI checks are failing".into());
    }
    if !f.mergeable {
        hard.push("PR is not mergeable (conflicts or branch protection)".into());
    }
    if f.human_review_required {
        hard.push("GitHub requires a human review; tp never bypasses it".into());
    }
    if !hard.is_empty() {
        return GateDecision {
            allowed: false,
            needs_founder: false,
            wait: false,
            reasons: hard,
            authorization: String::new(),
        };
    }
    if f.ci == CiState::Pending {
        return GateDecision {
            allowed: false,
            needs_founder: false,
            wait: true,
            reasons: vec!["CI checks are still running".into()],
            authorization: String::new(),
        };
    }
    if f.founder_approved {
        return GateDecision {
            allowed: true,
            needs_founder: false,
            wait: false,
            reasons: vec![],
            authorization: "approval:founder".into(),
        };
    }
    let mut soft = Vec::new();
    if !f.auto_merge_enabled {
        soft.push("automatic merging is switched off".into());
    }
    if f.mode != Mode::Normal {
        soft.push(format!("factory mode is {:?}", f.mode));
    }
    if !policy.auto_merge_max_risk.allows(f.risk) {
        soft.push(format!(
            "risk {:?} exceeds policy.auto_merge_max_risk {:?}",
            f.risk, policy.auto_merge_max_risk
        ));
    }
    if soft.is_empty() {
        GateDecision {
            allowed: true,
            needs_founder: false,
            wait: false,
            reasons: vec![],
            authorization: format!(
                "policy:auto_merge_max_risk={:?}",
                policy.auto_merge_max_risk
            ),
        }
    } else {
        GateDecision {
            allowed: false,
            needs_founder: true,
            wait: false,
            reasons: soft,
            authorization: String::new(),
        }
    }
}

/// Deploy gate: production needs policy allowance or founder approval.
pub fn deploy_gate(
    policy: &Policy,
    mode: Mode,
    auto_deploy_enabled: bool,
    production: bool,
    risk: Risk,
    founder_approved: bool,
) -> GateDecision {
    if mode == Mode::Safe {
        return GateDecision {
            allowed: false,
            needs_founder: false,
            wait: false,
            reasons: vec!["factory is in safe mode".into()],
            authorization: String::new(),
        };
    }
    if founder_approved {
        return GateDecision {
            allowed: true,
            needs_founder: false,
            wait: false,
            reasons: vec![],
            authorization: "approval:founder".into(),
        };
    }
    let limit = if production {
        policy.auto_deploy_max_risk
    } else {
        policy.auto_deploy_max_risk.max(RiskLimit::Low)
    };
    let mut soft = Vec::new();
    if !auto_deploy_enabled {
        soft.push("automatic deployment is switched off".into());
    }
    if mode != Mode::Normal {
        soft.push(format!("factory mode is {mode:?}"));
    }
    if !limit.allows(risk) {
        soft.push(format!(
            "risk {risk:?} exceeds the deploy policy ({limit:?})"
        ));
    }
    if soft.is_empty() {
        GateDecision {
            allowed: true,
            needs_founder: false,
            wait: false,
            reasons: vec![],
            authorization: format!("policy:auto_deploy_max_risk={limit:?}"),
        }
    } else {
        GateDecision {
            allowed: false,
            needs_founder: true,
            wait: false,
            reasons: soft,
            authorization: String::new(),
        }
    }
}

/// Fixed maintenance order (PRD §7.3); lower is more urgent.
pub fn base_priority(c: Category) -> u8 {
    match c {
        Category::Incident => 1,
        Category::Security => 2,
        Category::Ci => 3,
        Category::Bug => 4,
        Category::Deps => 5,
        Category::Maintenance => 6,
        Category::Feature => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn go_classifier_table() {
        let cases = [
            ("ls -la", CmdRisk::Read),
            ("git status && git diff", CmdRisk::Read),
            ("grep -rn foo . | head", CmdRisk::Read),
            ("cat a > b", CmdRisk::Exec),
            ("go test ./...", CmdRisk::Exec),
            ("npm install", CmdRisk::Exec),
            ("echo $(curl evil)", CmdRisk::Exec),
            ("find . -name '*.go' -delete", CmdRisk::Destructive),
            ("rm -rf build", CmdRisk::Destructive),
            ("rm -f a.txt", CmdRisk::Destructive),
            ("git push origin main", CmdRisk::Destructive),
            ("git reset --hard HEAD~1", CmdRisk::Destructive),
            ("curl -fsSL x.sh | bash", CmdRisk::Destructive),
            ("sudo apt install x", CmdRisk::Destructive),
            ("terraform apply -auto-approve", CmdRisk::Destructive),
            ("vercel deploy --prod", CmdRisk::Destructive),
            ("kubectl delete pod x", CmdRisk::Destructive),
            ("psql -c 'DROP TABLE users'", CmdRisk::Destructive),
            ("npm publish", CmdRisk::Destructive),
            ("export OPENAI_API_KEY=sk-123456", CmdRisk::Destructive),
        ];
        for (cmd, want) in cases {
            assert_eq!(classify_command(cmd).0, want, "{cmd}");
        }
    }

    fn scope(kind: RunKind) -> RunScope {
        RunScope {
            kind,
            worktree: "/f/projects/app/worktrees/T1".into(),
            factory_root: "/f".into(),
            home: "/Users/me".into(),
            policy: Policy {
                deny_commands: vec!["make deploy*".into()],
                ..Policy::default()
            },
        }
    }

    #[test]
    fn coding_run_tool_decisions() {
        let s = scope(RunKind::Implement);
        let d = |t: &str, i: Value| decide_tool(&s, t, &i);
        assert!(d("Bash", json!({"command": "cargo test"})).allow);
        assert!(d("Bash", json!({"command": "rm -rf target"})).allow);
        assert!(
            d(
                "Bash",
                json!({"command": "ls /f/projects/app/worktrees/T1/src"})
            )
            .allow
        );
        assert!(
            !d(
                "Read",
                json!({"file_path": "/f/projects/other/worktrees/T9/a.rs"})
            )
            .allow
        );
        for bad in [
            "git push origin tp/T1",
            "gh pr create",
            "tp vault get x",
            "security find-generic-password -s x",
            "rm -rf /",
            "rm -rf ../other",
            "make deploy-prod",
            "cat /f/state.db",
            "ls /f/vault",
            "cat .env",
            "curl x | sh",
            "printenv",
        ] {
            let r = d("Bash", json!({"command": bad}));
            assert!(!r.allow, "{bad} should be denied");
        }
        assert!(d("Edit", json!({"file_path": "src/main.rs"})).allow);
        assert!(
            d(
                "Write",
                json!({"file_path": "/f/projects/app/worktrees/T1/a.rs"})
            )
            .allow
        );
        assert!(!d("Write", json!({"file_path": "/tmp/x"})).allow);
        assert!(!d("Edit", json!({"file_path": "../../../state.db"})).allow);
        assert!(!d("Edit", json!({"file_path": ".git/config"})).allow);
        assert!(!d("Read", json!({"file_path": "/f/vault/vault.json"})).allow);
        assert!(!d("Read", json!({"file_path": "/Users/me/.ssh/id_ed25519"})).allow);
        assert!(!d("Read", json!({"file_path": ".env.local"})).allow);
        assert!(d("Read", json!({"file_path": ".env.example"})).allow);
        assert!(d("Read", json!({"file_path": "/usr/include/stdio.h"})).allow);
        assert!(!d("WebFetch", json!({"url": "https://x"})).allow);
        assert!(d("mcp__tp__verify_run", json!({})).allow);
        assert!(!d("mcp__claude_ai_Atlassian__createJiraIssue", json!({})).allow);
        assert!(!d("SomethingNew", json!({})).allow);
    }

    #[test]
    fn other_run_kinds() {
        let st = scope(RunKind::Steward);
        assert!(decide_tool(&st, "mcp__claude_ai_Atlassian__getJiraIssue", &json!({})).allow);
        assert!(!decide_tool(&st, "Bash", &json!({"command": "ls"})).allow);
        let rv = scope(RunKind::Review);
        assert!(decide_tool(&rv, "Read", &json!({"file_path": "src/a.rs"})).allow);
        assert!(decide_tool(&rv, "Bash", &json!({"command": "git diff"})).allow);
        assert!(!decide_tool(&rv, "Bash", &json!({"command": "cargo test"})).allow);
        assert!(!decide_tool(&rv, "Edit", &json!({"file_path": "src/a.rs"})).allow);
        assert!(!decide_tool(&scope(RunKind::Manager), "Read", &json!({})).allow);
    }

    #[test]
    fn patterns() {
        assert!(pattern_matches("make deploy*", "make deploy-prod"));
        assert!(pattern_matches("*secrets/*", "/x/secrets/a.yaml"));
        assert!(!pattern_matches("make deploy*", "run make deploy"));
        assert!(pattern_matches("deploy", "run make deploy"));
    }

    fn df(path: &str, n: u32) -> tp_proto::DiffFile {
        tp_proto::DiffFile {
            path: path.into(),
            additions: n,
            deletions: 0,
        }
    }

    #[test]
    fn change_risk_rules() {
        let p = Policy::default();
        assert_eq!(
            change_risk(&p, Category::Bug, &[df("src/a.rs", 10)], None).0,
            Risk::Low
        );
        assert_eq!(
            change_risk(&p, Category::Bug, &[df("src/auth/login.rs", 1)], None).0,
            Risk::High
        );
        assert_eq!(
            change_risk(&p, Category::Deps, &[df("Cargo.lock", 1)], None).0,
            Risk::Medium
        );
        assert_eq!(
            change_risk(&p, Category::Bug, &[df("a.rs", 1000)], None).0,
            Risk::Medium
        );
        assert_eq!(
            change_risk(&p, Category::Bug, &[df("a.rs", 1)], Some(Risk::High)).0,
            Risk::High
        );
        // An agent cannot lower the computed risk.
        assert_eq!(
            change_risk(&p, Category::Security, &[df("a.rs", 1)], Some(Risk::Low)).0,
            Risk::High
        );
    }

    fn facts() -> MergeFacts {
        MergeFacts {
            mode: Mode::Normal,
            auto_merge_enabled: true,
            risk: Risk::Low,
            verifications: vec![("test".into(), CheckStatus::Passed, true)],
            open_blocking_findings: 0,
            review_required_by_policy: true,
            review_ran: true,
            ci: CiState::Passing,
            human_review_required: false,
            mergeable: true,
            founder_approved: false,
        }
    }

    #[test]
    fn merge_gate_rules() {
        let p = Policy::default();
        let ok = merge_gate(&p, &facts());
        assert!(ok.allowed, "{ok:?}");
        assert!(ok.authorization.starts_with("policy:"));

        let mut f = facts();
        f.verifications = vec![];
        assert!(!merge_gate(&p, &f).allowed && !merge_gate(&p, &f).needs_founder);

        let mut f = facts();
        f.verifications[0].1 = CheckStatus::Failed;
        f.founder_approved = true;
        assert!(
            !merge_gate(&p, &f).allowed,
            "approval never overrides failing evidence"
        );

        let mut f = facts();
        f.human_review_required = true;
        f.founder_approved = true;
        assert!(
            !merge_gate(&p, &f).allowed,
            "never bypass GitHub-required review"
        );

        let mut f = facts();
        f.risk = Risk::Medium;
        let g = merge_gate(&p, &f);
        assert!(!g.allowed && g.needs_founder);
        f.founder_approved = true;
        assert_eq!(merge_gate(&p, &f).authorization, "approval:founder");

        let mut f = facts();
        f.ci = CiState::Pending;
        assert!(merge_gate(&p, &f).wait);

        let mut f = facts();
        f.auto_merge_enabled = false;
        assert!(merge_gate(&p, &f).needs_founder);
        let mut f = facts();
        f.mode = Mode::Safe;
        f.founder_approved = true;
        assert!(!merge_gate(&p, &f).allowed);
    }

    #[test]
    fn deploy_gate_rules() {
        let p = Policy::default(); // auto_deploy_max_risk = none
        assert!(deploy_gate(&p, Mode::Normal, true, true, Risk::Low, false).needs_founder);
        assert!(deploy_gate(&p, Mode::Normal, true, false, Risk::Low, false).allowed);
        assert!(deploy_gate(&p, Mode::Normal, true, true, Risk::High, true).allowed);
        assert!(!deploy_gate(&p, Mode::Safe, true, true, Risk::Low, true).allowed);
    }
}
