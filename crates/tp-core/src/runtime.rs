//! Coding-runtime boundary (PRD §14). Factory logic talks to [`Runtime`];
//! [`Claude`] drives the installed Claude Code CLI through its documented
//! headless interface (`-p --output-format stream-json`). No private APIs.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::Serialize;
use serde_json::{Value, json};

use crate::policy::RunScope;
use crate::{Error, Result};

/// What a runtime can do. Gaps are explicit, never papered over.
#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub name: &'static str,
    pub streaming_output: bool,
    pub resume: bool,
    /// Interrupt ends the current turn but keeps the session.
    pub interrupt: bool,
    /// Live input into a running turn (not supported: we resume instead).
    pub live_input: bool,
    pub usage_reporting: bool,
    pub cost_reporting: &'static str,
    pub pre_tool_hooks: bool,
    pub sandbox: bool,
    pub structured_output: bool,
    pub remote_control: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct RunSpec {
    pub cwd: PathBuf,
    pub prompt: String,
    pub resume: Option<String>,
    pub model: Option<String>,
    /// Generated settings file (hooks, sandbox, deny rules).
    pub settings: Option<PathBuf>,
    pub mcp_config: Option<PathBuf>,
    /// Use only the MCP servers in `mcp_config` (drops claude.ai connectors).
    pub strict_mcp: bool,
    pub allowed_tools: Vec<String>,
    /// Restrict the built-in tool set (`Some(vec![])` = no tools).
    pub tools: Option<Vec<String>>,
    pub append_system_prompt: Option<String>,
    pub json_schema: Option<Value>,
    pub max_budget_usd: Option<f64>,
    pub max_turns: Option<u32>,
    pub permission_mode: Option<String>,
    pub env: Vec<(String, String)>,
    pub persist_session: bool,
}

/// One parsed line of the runtime's output stream.
#[derive(Debug, Clone)]
pub enum StreamItem {
    Init {
        session_id: String,
        model: Option<String>,
        info: Value,
    },
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        is_error: bool,
        excerpt: String,
    },
    Result(RunResult),
    Stderr(String),
    Other(Value),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RunResult {
    pub is_error: bool,
    pub subtype: String,
    pub text: String,
    pub structured: Option<Value>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<String>,
    pub num_turns: Option<u32>,
    pub permission_denials: Vec<Value>,
}

pub type Sink = Box<dyn FnMut(StreamItem) + Send>;

pub struct RunHandle {
    pub pid: u32,
    child: Arc<Mutex<Child>>,
    reader: Option<JoinHandle<Option<RunResult>>>,
}

impl RunHandle {
    /// SIGINT: Claude Code ends the turn and keeps the session (resumable).
    pub fn interrupt(&self) -> Result<()> {
        signal(self.pid, "INT")
    }

    pub fn kill(&self) -> Result<()> {
        self.child
            .lock()
            .map_err(|_| Error::Daemon("child lock".into()))?
            .kill()
            .map_err(|e| Error::Daemon(format!("kill: {e}")))
    }

    /// Waits for exit; returns the final result event if one arrived.
    pub fn wait(mut self) -> Result<(Option<RunResult>, Option<i32>)> {
        let result = self.reader.take().and_then(|h| h.join().ok()).flatten();
        let status = self
            .child
            .lock()
            .map_err(|_| Error::Daemon("child lock".into()))?
            .wait()
            .map_err(|e| Error::Daemon(format!("wait: {e}")))?;
        Ok((result, status.code()))
    }
}

pub fn signal(pid: u32, sig: &str) -> Result<()> {
    let ok = Command::new("kill")
        .args([format!("-{sig}"), pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(Error::Daemon(format!("could not signal pid {pid}")))
    }
}

pub fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub trait Runtime: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn start(&self, spec: &RunSpec, sink: Sink) -> Result<RunHandle>;
}

/// Claude Code CLI runtime.
#[derive(Debug, Clone)]
pub struct Claude {
    pub bin: PathBuf,
}

impl Claude {
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into() }
    }

    /// `runtime.claude_path`, else `claude` on PATH, else ~/.local/bin/claude.
    pub fn locate(configured: Option<&str>) -> Result<Self> {
        if let Some(p) = configured.filter(|p| !p.is_empty()) {
            return Ok(Self::new(p));
        }
        if let Some(p) = crate::doctor::which("claude") {
            return Ok(Self::new(p));
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let p = home.join(".local/bin/claude");
        if p.is_file() {
            return Ok(Self::new(p));
        }
        Err(Error::Invalid(
            "Claude Code CLI not found; install it or set runtime.claude_path in factory.yaml"
                .into(),
        ))
    }

    pub fn args(spec: &RunSpec) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "-p".into(),
            spec.prompt.clone(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            // Never load the repo's .claude settings: they could disable hooks (spike 2).
            "--setting-sources".into(),
            "user".into(),
        ];
        let mut push = |k: &str, v: String| {
            a.push(k.into());
            a.push(v);
        };
        if let Some(s) = &spec.settings {
            push("--settings", s.display().to_string());
        }
        if let Some(m) = &spec.mcp_config {
            push("--mcp-config", m.display().to_string());
        }
        if let Some(r) = &spec.resume {
            push("--resume", r.clone());
        }
        if let Some(m) = &spec.model {
            push("--model", m.clone());
        }
        if let Some(p) = &spec.append_system_prompt {
            push("--append-system-prompt", p.clone());
        }
        if let Some(s) = &spec.json_schema {
            push("--json-schema", s.to_string());
        }
        if let Some(b) = spec.max_budget_usd {
            push("--max-budget-usd", format!("{b:.2}"));
        }
        if let Some(t) = spec.max_turns {
            push("--max-turns", t.to_string());
        }
        if let Some(m) = &spec.permission_mode {
            push("--permission-mode", m.clone());
        }
        if spec.strict_mcp {
            a.push("--strict-mcp-config".into());
        }
        if !spec.persist_session {
            a.push("--no-session-persistence".into());
        }
        if let Some(tools) = &spec.tools {
            a.push("--tools".into());
            if tools.is_empty() {
                a.push(String::new());
            } else {
                a.extend(tools.iter().cloned());
            }
        }
        if !spec.allowed_tools.is_empty() {
            a.push("--allowedTools".into());
            a.extend(spec.allowed_tools.iter().cloned());
        }
        a
    }
}

impl Runtime for Claude {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            name: "claude-code",
            streaming_output: true,
            resume: true,
            interrupt: true,
            live_input: false,
            usage_reporting: true,
            cost_reporting: "list-price estimate (subscription login)",
            pre_tool_hooks: true,
            sandbox: true,
            structured_output: true,
            remote_control: "not used for managed sessions (no documented API)",
        }
    }

    fn start(&self, spec: &RunSpec, mut sink: Sink) -> Result<RunHandle> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(Self::args(spec))
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Invalid(format!("start {}: {e}", self.bin.display())))?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Daemon("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Daemon("no stderr".into()))?;
        let err_lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let err_buf = err_lines.clone();
        let err_thread = std::thread::spawn(move || {
            for line in BufReader::new(stderr)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Ok(mut b) = err_buf.lock() {
                    if b.len() < 200 {
                        b.push(line);
                    }
                }
            }
        });
        let reader = std::thread::spawn(move || {
            let mut result = None;
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    sink(StreamItem::Stderr(line));
                    continue;
                };
                for item in parse_line(&v) {
                    if let StreamItem::Result(r) = &item {
                        result = Some(r.clone());
                    }
                    sink(item);
                }
            }
            let _ = err_thread.join();
            if let Ok(lines) = err_lines.lock() {
                for l in lines.iter().filter(|l| !l.trim().is_empty()) {
                    sink(StreamItem::Stderr(l.clone()));
                }
            }
            result
        });
        Ok(RunHandle {
            pid,
            child: Arc::new(Mutex::new(child)),
            reader: Some(reader),
        })
    }
}

/// Splits one stream-json message into items.
pub fn parse_line(v: &Value) -> Vec<StreamItem> {
    let t = v.get("type").and_then(Value::as_str).unwrap_or_default();
    match t {
        "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            vec![StreamItem::Init {
                session_id: v
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                model: v.get("model").and_then(Value::as_str).map(Into::into),
                info: json!({
                    "tools": v.get("tools"),
                    "mcp_servers": v.get("mcp_servers"),
                    "capabilities": v.get("capabilities"),
                    "permissionMode": v.get("permissionMode"),
                    "claude_code_version": v.get("claude_code_version"),
                }),
            }]
        }
        "assistant" => content(v)
            .iter()
            .filter_map(|c| match c.get("type").and_then(Value::as_str) {
                Some("text") => c
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| StreamItem::Text(t.into())),
                Some("tool_use") => Some(StreamItem::ToolUse {
                    id: c
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    name: c
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    input: c.get("input").cloned().unwrap_or(Value::Null),
                }),
                _ => None,
            })
            .collect(),
        "user" => content(v)
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("tool_result"))
            .map(|c| StreamItem::ToolResult {
                id: c
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                is_error: c.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                excerpt: excerpt(c.get("content").unwrap_or(&Value::Null), 400),
            })
            .collect(),
        "result" => vec![StreamItem::Result(parse_result(v))],
        _ => vec![StreamItem::Other(v.clone())],
    }
}

fn content(v: &Value) -> Vec<Value> {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub fn parse_result(v: &Value) -> RunResult {
    let u = v.get("usage").cloned().unwrap_or(Value::Null);
    let n = |k: &str| u.get(k).and_then(Value::as_u64);
    let (model, basis) = v
        .get("modelUsage")
        .and_then(Value::as_object)
        .and_then(|m| {
            m.iter()
                .max_by(|a, b| {
                    let c = |x: &Value| x.get("costUSD").and_then(Value::as_f64).unwrap_or(0.0);
                    c(a.1).total_cmp(&c(b.1))
                })
                .map(|(k, x)| {
                    (
                        Some(k.clone()),
                        x.get("costBasis")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    )
                })
        })
        .unwrap_or((None, None));
    RunResult {
        is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        subtype: v
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        text: v
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        structured: v.get("structured_output").cloned().filter(|s| !s.is_null()),
        session_id: v.get("session_id").and_then(Value::as_str).map(Into::into),
        model,
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_write_tokens: n("cache_creation_input_tokens"),
        cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
        cost_basis: Some(basis.unwrap_or_else(|| "reported".into())),
        num_turns: v.get("num_turns").and_then(Value::as_u64).map(|x| x as u32),
        permission_denials: v
            .get("permission_denials")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }
}

/// A short one-line text of any tool-result content.
pub fn excerpt(v: &Value, max: usize) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let s = s.replace('\n', " ⏎ ");
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s
    }
}

/// One-line summary of a tool call for activity feeds.
pub fn tool_summary(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    let detail = match name {
        "Bash" => field("command").to_string(),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let p = field("file_path");
            if p.is_empty() {
                field("notebook_path").to_string()
            } else {
                p.to_string()
            }
        }
        "Grep" | "Glob" => field("pattern").to_string(),
        "WebFetch" => field("url").to_string(),
        "WebSearch" => field("query").to_string(),
        "Task" | "Agent" => field("description").to_string(),
        _ => excerpt(input, 160),
    };
    excerpt(&Value::String(format!("{name}: {detail}")), 200)
}

/// Starts a run and blocks until it ends (manager/steward/review calls).
pub fn run_blocking(rt: &dyn Runtime, spec: &RunSpec, sink: Sink) -> Result<RunResult> {
    let h = rt.start(spec, sink)?;
    let (res, code) = h.wait()?;
    res.ok_or_else(|| Error::Daemon(format!("runtime exited ({code:?}) without a result")))
}

// ---------------------------------------------------------------- per-run files

/// Writes the settings file for one run (outside the worktree) and returns its path.
pub fn write_settings(
    dir: &Path,
    scope: &RunScope,
    tp_bin: &Path,
    run_id: &str,
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let f = scope.factory_root.display().to_string();
    let home = scope.home.display().to_string();
    let hook = |event: &str| {
        json!([{ "matcher": "*", "hooks": [{
            "type": "command",
            "command": format!("{} hook {event} --factory {} --run {run_id}", sh_quote(&tp_bin.display().to_string()), sh_quote(&f)),
            "timeout": 30
        }]}])
    };
    let deny = vec![
        format!("Read(/{f}/vault/**)"),
        format!("Edit(/{f}/vault/**)"),
        format!("Read(/{f}/state.db)"),
        format!("Read(/{f}/runs/**)"),
        format!("Edit(/{f}/runs/**)"),
        format!("Edit(/{f}/audit/**)"),
        format!("Edit(/{home}/.claude/**)"),
        format!("Read(/{home}/.ssh/**)"),
        "Read(./.env)".into(),
        "Read(./.env.*)".into(),
        "Bash(git push:*)".into(),
        "Bash(gh:*)".into(),
    ];
    let mut write_allow = vec![scope.worktree.display().to_string()];
    // Toolchain caches that builds need.
    for d in [
        ".cargo",
        ".npm",
        ".cache",
        "go",
        ".gradle",
        ".pub-cache",
        "Library/Caches",
    ] {
        write_allow.push(scope.home.join(d).display().to_string());
    }
    let settings = json!({
        "hooks": { "PreToolUse": hook("pre"), "PostToolUse": hook("post") },
        "permissions": { "deny": deny, "defaultMode": "acceptEdits" },
        "sandbox": {
            "enabled": true,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": false,
            "filesystem": {
                "denyRead": [format!("{f}/vault"), format!("{f}/state.db"), format!("{home}/.ssh")],
                "allowWrite": write_allow
            }
        }
    });
    let path = dir.join("settings.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&settings)?)
        .map_err(|e| Error::io(&path, e))?;
    Ok(path)
}

/// MCP config exposing only tp's own server to the run.
pub fn write_mcp_config(
    dir: &Path,
    tp_bin: &Path,
    factory_root: &Path,
    run_id: &str,
) -> Result<PathBuf> {
    let cfg = json!({ "mcpServers": { "tp": {
        "type": "stdio",
        "command": tp_bin.display().to_string(),
        "args": ["mcp", "--factory", factory_root.display().to_string(), "--run", run_id]
    }}});
    let path = dir.join("mcp.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&cfg)?).map_err(|e| Error::io(&path, e))?;
    Ok(path)
}

fn sh_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};

    /// A fake `claude` that emits stream-json; it appends `fake-change` to
    /// CHANGES.md in its cwd (implement runs) and returns structured output
    /// from $FAKE_STRUCTURED when --json-schema is passed.
    pub fn fake_claude(dir: &Path) -> PathBuf {
        let p = dir.join("fake-claude");
        let script = r#"#!/bin/sh
schema=0; resume=""
while [ $# -gt 0 ]; do
  case "$1" in
    --json-schema) schema=1; shift ;;
    --resume) resume="$2"; shift ;;
  esac
  shift
done
sid="${resume:-sess-$$}"
echo '{"type":"system","subtype":"init","session_id":"'$sid'","model":"fake-model","tools":["Bash","Edit"],"capabilities":["interrupt_receipt_v1"]}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"working on it"},{"type":"tool_use","id":"tu1","name":"Edit","input":{"file_path":"CHANGES.md"}}]}}'
if [ "$schema" = 0 ]; then echo "fake-change" >> CHANGES.md; fi
echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"ok"}]}}'
if [ "${FAKE_SLEEP:-0}" != 0 ]; then sleep "$FAKE_SLEEP"; fi
so="${FAKE_STRUCTURED:-null}"
if [ "$schema" = 0 ]; then so=null; fi
echo '{"type":"result","subtype":"success","is_error":false,"result":"done","structured_output":'"$so"',"session_id":"'$sid'","num_turns":2,"total_cost_usd":0.0123,"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":5,"cache_creation_input_tokens":7},"modelUsage":{"fake-model":{"costUSD":0.0123,"costBasis":"list"}},"permission_denials":[]}'
"#;
        std::fs::write(&p, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn args_pin_setting_sources_and_options() {
        let spec = RunSpec {
            prompt: "hi".into(),
            resume: Some("s1".into()),
            tools: Some(vec![]),
            json_schema: Some(json!({"type": "object"})),
            strict_mcp: true,
            ..Default::default()
        };
        let a = Claude::args(&spec);
        let s = a.join(" ");
        assert!(s.contains("--setting-sources user"));
        assert!(s.contains("--resume s1"));
        assert!(s.contains("--strict-mcp-config"));
        assert!(s.contains("--no-session-persistence"));
        let i = a.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a[i + 1], "");
    }

    #[test]
    fn parses_stream_and_result() {
        let r = parse_result(&json!({
            "type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s",
            "total_cost_usd":0.5,"num_turns":3,"usage":{"input_tokens":10,"output_tokens":2},
            "modelUsage":{"m1":{"costUSD":0.1},"m2":{"costUSD":0.4,"costBasis":"list"}},
            "structured_output":{"a":1}
        }));
        assert_eq!(r.model.as_deref(), Some("m2"));
        assert_eq!(r.cost_basis.as_deref(), Some("list"));
        assert_eq!(r.structured, Some(json!({"a":1})));
        assert_eq!(r.input_tokens, Some(10));
        let items = parse_line(
            &json!({"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","id":"1","name":"Bash","input":{"command":"ls"}}]}}),
        );
        assert_eq!(items.len(), 2);
        assert_eq!(
            tool_summary("Bash", &json!({"command": "ls -la"})),
            "Bash: ls -la"
        );
    }

    #[test]
    fn fake_runtime_end_to_end() {
        let d = tempfile::tempdir().unwrap();
        let bin = testutil::fake_claude(d.path());
        let rt = Claude::new(&bin);
        let (tx, rx) = mpsc::channel();
        let spec = RunSpec {
            cwd: d.path().into(),
            prompt: "do it".into(),
            ..Default::default()
        };
        let res = run_blocking(
            &rt,
            &spec,
            Box::new(move |i| {
                let _ = tx.send(format!("{i:?}"));
            }),
        )
        .unwrap();
        assert_eq!(res.text, "done");
        assert_eq!(res.cost_usd, Some(0.0123));
        assert!(d.path().join("CHANGES.md").exists());
        let items: Vec<String> = rx.try_iter().collect();
        assert!(items.iter().any(|i| i.starts_with("Init")));
        assert!(items.iter().any(|i| i.starts_with("ToolUse")));
    }

    #[test]
    fn settings_file_has_hooks_and_denies() {
        let d = tempfile::tempdir().unwrap();
        let scope = RunScope {
            kind: tp_proto::RunKind::Implement,
            worktree: "/f/projects/a/worktrees/T1".into(),
            factory_root: "/f".into(),
            home: "/Users/me".into(),
            policy: crate::config::Policy::default(),
        };
        let p = write_settings(d.path(), &scope, Path::new("/bin/tp"), "R1").unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        let cmd = v
            .pointer("/hooks/PreToolUse/0/hooks/0/command")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(cmd, "/bin/tp hook pre --factory /f --run R1");
        assert_eq!(v.pointer("/sandbox/enabled"), Some(&json!(true)));
        assert!(
            v.pointer("/permissions/deny")
                .unwrap()
                .to_string()
                .contains("Read(//f/vault/**)")
        );
    }
}
