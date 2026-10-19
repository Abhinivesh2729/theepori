//! Runs tpd as a launchd user agent so it outlives the GUI and restarts on crash.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::paths::FactoryPaths;
use crate::{Error, Result};

pub const DEFAULT_LABEL: &str = "space.0xabhi.tpd";

pub fn plist_path(home: &Path, label: &str) -> PathBuf {
    home.join("Library/LaunchAgents").join(format!("{label}.plist"))
}

pub fn plist(label: &str, tpd: &Path, paths: &FactoryPaths) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let tpd = esc(&tpd.display().to_string());
    let root = esc(&paths.root().display().to_string());
    let log = esc(&paths.logs().join("tpd.log").display().to_string());
    // launchd starts agents with a minimal PATH; give tpd the usual tool dirs.
    let path = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array><string>{tpd}</string><string>--factory</string><string>{root}</string></array>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>{path}</string><key>TP_FACTORY</key><string>{root}</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#
    )
}

fn domain() -> Result<String> {
    let out = Command::new("id").arg("-u").output().map_err(|e| Error::Invalid(format!("id -u: {e}")))?;
    Ok(format!("gui/{}", String::from_utf8_lossy(&out.stdout).trim()))
}

fn launchctl(args: &[&str]) -> Result<std::process::Output> {
    Command::new("launchctl").args(args).output().map_err(|e| Error::Invalid(format!("launchctl: {e}")))
}

/// Writes the plist and (re)loads the agent.
pub fn install(home: &Path, label: &str, tpd: &Path, paths: &FactoryPaths) -> Result<PathBuf> {
    if !tpd.is_file() {
        return Err(Error::Invalid(format!("{} not found; build it with `make build`", tpd.display())));
    }
    let path = plist_path(home, label);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(&path, plist(label, tpd, paths)).map_err(|e| Error::io(&path, e))?;
    let domain = domain()?;
    let _ = launchctl(&["bootout", &format!("{domain}/{label}")]);
    let out = launchctl(&["bootstrap", &domain, &path.display().to_string()])?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(path)
}

pub fn uninstall(home: &Path, label: &str) -> Result<bool> {
    let path = plist_path(home, label);
    let _ = launchctl(&["bootout", &format!("{}/{label}", domain()?)]);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
        return Ok(true);
    }
    Ok(false)
}

pub fn is_loaded(label: &str) -> Result<bool> {
    Ok(launchctl(&["print", &format!("{}/{label}", domain()?)])?.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_has_program_and_keepalive() {
        let p = plist("x.test", Path::new("/opt/tp/tpd"), &FactoryPaths::new("/f a&b"));
        assert!(p.contains("<string>/opt/tp/tpd</string><string>--factory</string><string>/f a&amp;b</string>"));
        assert!(p.contains("<key>SuccessfulExit</key><false/>"));
        assert!(p.contains("/f a&amp;b/logs/tpd.log"));
    }
}
