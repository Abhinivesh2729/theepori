//! `tp doctor`: checks the factory and its environment, each with a fix.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::audit::AuditLog;
use crate::client::Client;
use crate::config::FactoryConfig;
use crate::paths::FactoryPaths;
use crate::{db, launchd};

/// Oldest Claude Code the spikes were run against.
pub const CLAUDE_MIN: (u32, u32, u32) = (2, 1, 294);
/// First version with `onFailure: block` for HTTP hooks.
pub const CLAUDE_HTTP_HOOK_BLOCK: (u32, u32, u32) = (2, 1, 295);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Ok,
            detail: detail.into(),
            fix: None,
        }
    }
    fn warn(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
    fn fail(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

pub fn run_all(paths: &FactoryPaths, home: &Path) -> Vec<Check> {
    let mut checks = factory(paths);
    checks.push(claude());
    checks.push(tool(
        "gh",
        &["auth", "status"],
        "install with `brew install gh`, then `gh auth login`",
    ));
    checks.push(tool(
        "git",
        &["--version"],
        "install the Xcode command line tools: `xcode-select --install`",
    ));
    checks.push(claude_user_settings(&home.join(".claude/settings.json")));
    checks.push(cloudflared_config(&home.join(".cloudflared/config.yml")));
    checks.push(project_access(paths));
    checks.push(daemon(paths));
    checks
}

pub const FDA_FIX: &str = "System Settings → Privacy & Security → Full Disk Access → + → add ~/Library/Application Support/theepori/bin/tpd (press ⌘⇧G to type the path), then restart tpd (`tp daemon install`)";

/// Tries to list `path` within `timeout`. macOS privacy (TCC) can make a
/// launchd agent block on ~/Documents, ~/Desktop or ~/Downloads instead of
/// failing; this turns that into a clear error.
pub fn can_access(path: &Path, timeout: Duration) -> std::result::Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let p = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(std::fs::read_dir(&p).map(|_| ()).map_err(|e| e.to_string()));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) if e.contains("Operation not permitted") => Err(format!(
            "macOS privacy settings block access to {} ({e}); {FDA_FIX}",
            path.display()
        )),
        Ok(Err(e)) => Err(format!("{}: {e}", path.display())),
        Err(_) => Err(format!(
            "macOS is holding access to {} (privacy prompt or Full Disk Access needed); {FDA_FIX}",
            path.display()
        )),
    }
}

/// Every registered repository must be readable by this process.
pub fn project_access(paths: &FactoryPaths) -> Check {
    let list = db::open(&paths.db())
        .ok()
        .and_then(|c| crate::registry::list(&c).ok())
        .unwrap_or_default();
    let blocked: Vec<String> = list
        .iter()
        .filter_map(|p| can_access(Path::new(&p.repo_path), Duration::from_secs(3)).err())
        .collect();
    if blocked.is_empty() {
        Check::ok(
            "repository access",
            format!("{} project(s) readable", list.len()),
        )
    } else {
        Check::fail("repository access", blocked.join("; "), FDA_FIX)
    }
}

pub fn worst(checks: &[Check]) -> Status {
    checks.iter().map(|c| c.status).max().unwrap_or(Status::Ok)
}

pub fn factory(paths: &FactoryPaths) -> Vec<Check> {
    let setup_fix = "run `tp setup`";
    if !paths.root().is_dir() {
        return vec![Check::fail(
            "factory",
            format!("{} does not exist", paths.root().display()),
            setup_fix,
        )];
    }
    let mut out = vec![match FactoryConfig::load(&paths.config()) {
        Ok(c) => Check::ok(
            "config",
            format!("{} (schema {})", paths.config().display(), c.schema),
        ),
        Err(e) => Check::fail(
            "config",
            e.to_string(),
            "fix factory.yaml or move it aside and run `tp setup`",
        ),
    }];
    let conn = match db::open(&paths.db()) {
        Ok(c) => c,
        Err(e) => {
            out.push(Check::fail("database", e.to_string(), setup_fix));
            return out;
        }
    };
    match db::version(&conn) {
        Ok(v) if v == db::latest_version() => {
            out.push(Check::ok("database", format!("schema v{v}")))
        }
        Ok(v) if v > db::latest_version() => out.push(Check::fail(
            "database",
            format!(
                "schema v{v} is newer than this tp (v{})",
                db::latest_version()
            ),
            "upgrade tp",
        )),
        Ok(v) => out.push(Check::warn(
            "database",
            format!("schema v{v}, migrations pending"),
            setup_fix,
        )),
        Err(e) => out.push(Check::fail("database", e.to_string(), setup_fix)),
    }
    match AuditLog::new(paths).verify(&conn) {
        Ok(v) if v.ok => out.push(Check::ok(
            "audit chain",
            format!("{} records verified", v.records),
        )),
        Ok(v) => out.push(Check::fail(
            "audit chain",
            v.problem.unwrap_or_default(),
            "do not edit audit files; restore audit/ from a backup",
        )),
        Err(e) => out.push(Check::fail("audit chain", e.to_string(), setup_fix)),
    }
    out
}

pub fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| Path::new(d).join(bin))
        .find(|p| p.is_file())
}

fn tool(bin: &str, args: &[&str], fix: &str) -> Check {
    let Some(path) = which(bin) else {
        return Check::fail(bin, "not found on PATH", fix);
    };
    match Command::new(&path).args(args).output() {
        Ok(o) if o.status.success() => Check::ok(bin, path.display().to_string()),
        Ok(o) => Check::fail(
            bin,
            format!(
                "`{bin} {}` failed: {}",
                args.join(" "),
                first_line(&o.stderr, &o.stdout)
            ),
            fix,
        ),
        Err(e) => Check::fail(bin, e.to_string(), fix),
    }
}

fn claude() -> Check {
    let fix = "install Claude Code: https://code.claude.com/docs";
    let Some(path) = which("claude") else {
        return Check::fail("claude", "not found on PATH", fix);
    };
    let out = match Command::new(&path).arg("--version").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(e) => return Check::fail("claude", e.to_string(), fix),
    };
    claude_version_check(&path, &out)
}

pub fn claude_version_check(path: &Path, version_output: &str) -> Check {
    let Some(v) = parse_version(version_output) else {
        return Check::warn(
            "claude",
            format!("unrecognized version {version_output:?}"),
            "run `claude update`",
        );
    };
    let shown = format!("{}.{}.{} at {}", v.0, v.1, v.2, path.display());
    if v < CLAUDE_MIN {
        Check::fail("claude", shown, "run `claude update`")
    } else if v < CLAUDE_HTTP_HOOK_BLOCK {
        Check::warn(
            "claude",
            format!("{shown}; policy hooks use the command fallback until 2.1.295"),
            "run `claude update` when 2.1.295+ is available",
        )
    } else {
        Check::ok("claude", shown)
    }
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let word = s.split_whitespace().next()?;
    let mut it = word.split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// Spike 2: `disableAllHooks` in user settings would switch off tp's policy hooks.
pub fn claude_user_settings(path: &Path) -> Check {
    let name = "claude user settings";
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Check::ok(name, "none"),
        Err(e) => {
            return Check::warn(
                name,
                format!("{}: {e}", path.display()),
                "check file permissions",
            );
        }
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) if v.get("disableAllHooks").and_then(|b| b.as_bool()) == Some(true) => Check::fail(
            name,
            format!(
                "{} sets disableAllHooks; tp's policy hooks would not run",
                path.display()
            ),
            "remove disableAllHooks from that file",
        ),
        Ok(_) => Check::ok(name, path.display().to_string()),
        Err(e) => Check::warn(name, format!("{}: {e}", path.display()), "fix the JSON"),
    }
}

/// Spike 5: a stray ~/.cloudflared/config.yml is picked up silently by cloudflared.
pub fn cloudflared_config(path: &Path) -> Check {
    if path.exists() {
        Check::warn(
            "cloudflared",
            format!(
                "{} exists (another tunnel); tp always passes its own --config",
                path.display()
            ),
            "no action needed; listed so tunnels never collide",
        )
    } else {
        Check::ok("cloudflared", "no default config")
    }
}

pub fn daemon(paths: &FactoryPaths) -> Check {
    let loaded = launchd::is_loaded(launchd::DEFAULT_LABEL).unwrap_or(false);
    match Client::connect(&paths.socket(), Duration::from_secs(2)).and_then(|mut c| {
        c.call::<tp_proto::DaemonStatus>(tp_proto::method::STATUS, serde_json::Value::Null)
    }) {
        Ok(s) => Check::ok(
            "daemon",
            format!(
                "tpd {} pid {} up {}s{}",
                s.version,
                s.pid,
                s.uptime_ms / 1000,
                if loaded { " (launchd)" } else { "" }
            ),
        ),
        Err(_) if loaded => Check::fail(
            "daemon",
            "launchd agent loaded but tpd not answering",
            "see logs/tpd.log",
        ),
        Err(_) => Check::warn("daemon", "not running", "run `tp daemon install`"),
    }
}

fn first_line(a: &[u8], b: &[u8]) -> String {
    let s = if a.is_empty() { b } else { a };
    String::from_utf8_lossy(s)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("2.1.294 (Claude Code)"), Some((2, 1, 294)));
        assert_eq!(parse_version("garbage"), None);
        let p = Path::new("/bin/claude");
        assert_eq!(
            claude_version_check(p, "2.1.200 (Claude Code)").status,
            Status::Fail
        );
        assert_eq!(
            claude_version_check(p, "2.1.294 (Claude Code)").status,
            Status::Warn
        );
        assert_eq!(
            claude_version_check(p, "2.2.0 (Claude Code)").status,
            Status::Ok
        );
    }

    #[test]
    fn disable_all_hooks_fails() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        assert_eq!(claude_user_settings(&p).status, Status::Ok);
        std::fs::write(&p, r#"{"disableAllHooks": true}"#).unwrap();
        assert_eq!(claude_user_settings(&p).status, Status::Fail);
        std::fs::write(&p, r#"{"model": "x"}"#).unwrap();
        assert_eq!(claude_user_settings(&p).status, Status::Ok);
    }

    #[test]
    fn factory_checks_before_and_after_setup() {
        let d = tempfile::tempdir().unwrap();
        let paths = FactoryPaths::new(d.path().join("f"));
        assert_eq!(worst(&factory(&paths)), Status::Fail);
        crate::setup::setup(&paths, "t").unwrap();
        let checks = factory(&paths);
        assert_eq!(worst(&checks), Status::Ok, "{checks:?}");
    }

    #[test]
    fn worst_of_empty_is_ok() {
        assert_eq!(worst(&[]), Status::Ok);
    }
}
