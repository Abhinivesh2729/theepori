//! Deployment adapters (PRD §13): run the project's deploy command, then
//! verify health and the deployed version; roll back only when a rollback
//! command is configured.

use std::path::Path;
use std::time::Duration;

use crate::project_config::DeployEnv;
use crate::redact::Redactor;
use crate::verify::run_shell;

#[derive(Debug, Clone, PartialEq)]
pub struct DeployResult {
    /// healthy | failed | rolled_back
    pub state: String,
    pub health: Option<String>,
    pub version_ok: Option<bool>,
    pub rolled_back: bool,
    pub log: String,
}

/// GET `url`, retrying for up to `wait`; returns (2xx?, short status).
pub fn http_ok(url: &str, wait: Duration) -> (bool, String) {
    let deadline = std::time::Instant::now() + wait;
    loop {
        let r = ureq::get(url)
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call();
        let (ok, s) = match r {
            Ok(resp) => (
                resp.status().is_success(),
                format!("HTTP {}", resp.status().as_u16()),
            ),
            Err(e) => (false, e.to_string()),
        };
        if ok || std::time::Instant::now() >= deadline {
            return (ok, s);
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

pub fn body_contains(url: &str, needle: &str) -> bool {
    ureq::get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .is_some_and(|b| b.contains(needle))
}

pub fn deploy(
    checkout: &Path,
    env: &DeployEnv,
    commit: &str,
    secrets: &[(String, String)],
    health_wait: Duration,
) -> DeployResult {
    let mut red = Redactor::new();
    red.add(secrets.iter().map(|(_, v)| v.clone()));
    let mut vars = secrets.to_vec();
    vars.push(("TP_COMMIT".into(), commit.into()));
    vars.push(("TP_ENVIRONMENT".into(), env.name.clone()));
    let mut log = String::new();
    let (code, out) = match run_shell(
        checkout,
        &env.command,
        &vars,
        Duration::from_secs(env.timeout_secs),
    ) {
        Ok(r) => r,
        Err(e) => (Some(-1), e.to_string()),
    };
    log.push_str(&format!(
        "$ {}\n{}\n[exit {code:?}]\n",
        env.command,
        red.redact(&out)
    ));
    let mut failed = code != Some(0);
    let mut health = None;
    let mut version_ok = None;
    if !failed {
        if let Some(u) = &env.health_url {
            let (ok, s) = http_ok(u, health_wait);
            health = Some(s);
            failed |= !ok;
        }
        if let Some(u) = &env.version_url {
            let short = &commit[..commit.len().min(7)];
            let ok = body_contains(u, short);
            version_ok = Some(ok);
            failed |= !ok;
        }
    }
    if !failed {
        return DeployResult {
            state: "healthy".into(),
            health,
            version_ok,
            rolled_back: false,
            log,
        };
    }
    if let Some(rb) = &env.rollback_command {
        let (rc, rout) = run_shell(checkout, rb, &vars, Duration::from_secs(env.timeout_secs))
            .unwrap_or((Some(-1), String::new()));
        log.push_str(&format!("$ {rb}\n{}\n[exit {rc:?}]\n", red.redact(&rout)));
        return DeployResult {
            state: "rolled_back".into(),
            health,
            version_ok,
            rolled_back: rc == Some(0),
            log,
        };
    }
    DeployResult {
        state: "failed".into(),
        health,
        version_ok,
        rolled_back: false,
        log,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(cmd: &str, health: Option<&str>, rollback: Option<&str>) -> DeployEnv {
        DeployEnv {
            name: "staging".into(),
            command: cmd.into(),
            production: false,
            health_url: health.map(Into::into),
            version_url: None,
            rollback_command: rollback.map(Into::into),
            secrets: None,
            timeout_secs: 30,
        }
    }

    #[test]
    fn success_and_rollback_on_failed_health() {
        let d = tempfile::tempdir().unwrap();
        let ok = deploy(
            d.path(),
            &env("echo deploying $TP_COMMIT", None, None),
            "abc1234567",
            &[],
            Duration::ZERO,
        );
        assert_eq!(ok.state, "healthy");
        assert!(ok.log.contains("abc1234567"));
        // Nothing listens on port 9: health fails -> rollback runs.
        let bad = deploy(
            d.path(),
            &env(
                "true",
                Some("http://127.0.0.1:9/health"),
                Some("echo rolled > rb.txt"),
            ),
            "abc",
            &[("TOKEN".into(), "verysecretvalue1".into())],
            Duration::ZERO,
        );
        assert_eq!(bad.state, "rolled_back");
        assert!(bad.rolled_back);
        assert!(d.path().join("rb.txt").exists());
        let no_rb = deploy(
            d.path(),
            &env("exit 1", None, None),
            "abc",
            &[],
            Duration::ZERO,
        );
        assert_eq!(no_rb.state, "failed");
        assert!(!no_rb.rolled_back);
    }
}
