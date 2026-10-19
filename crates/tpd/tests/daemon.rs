#![allow(clippy::unwrap_used)]
//! Starts the real tpd binary against a scratch factory.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tp_core::client::Client;
use tp_proto::{AuditRecord, AuditVerify, DaemonStatus, Project, method};

struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Daemon {
    fn start(root: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_tpd"))
            .args(["--factory", root.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let d = Self {
            child,
            root: root.into(),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while d.client_opt().is_none() {
            assert!(Instant::now() < deadline, "tpd did not come up");
            std::thread::sleep(Duration::from_millis(20));
        }
        d
    }
    fn client_opt(&self) -> Option<Client> {
        let mut c = Client::connect(&self.root.join("tpd.sock"), Duration::from_secs(5)).ok()?;
        c.call_raw(method::STATUS, Value::Null).ok()?;
        Some(c)
    }
    fn client(&self) -> Client {
        self.client_opt().unwrap()
    }
    fn stop(mut self) -> std::process::ExitStatus {
        self.client()
            .call_raw(method::SHUTDOWN, Value::Null)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(Instant::now() < deadline, "tpd did not stop");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn scratch() -> (tempfile::TempDir, PathBuf) {
    // Short path: macOS limits Unix socket paths to 104 bytes.
    let dir = tempfile::Builder::new()
        .prefix("tpd")
        .tempdir_in("/tmp")
        .unwrap();
    let root = dir.path().join("f");
    (dir, root)
}

fn git_repo(parent: &Path, name: &str) -> PathBuf {
    let p = parent.join(name);
    std::fs::create_dir_all(p.join(".git")).unwrap();
    p
}

#[test]
fn status_projects_audit_and_shutdown() {
    let (dir, root) = scratch();
    let d = Daemon::start(&root);
    let mut c = d.client();

    let s: DaemonStatus = c.call(method::STATUS, Value::Null).unwrap();
    assert_eq!(s.pid, d.child.id());
    assert_eq!(s.projects, 0);

    let repo = git_repo(dir.path(), "demo");
    let p: Project = c
        .call(
            method::PROJECT_ADD,
            json!({"repo_path": repo, "jira_key": "SF"}),
        )
        .unwrap();
    assert_eq!(p.slug, "demo");
    let err = c
        .call_raw(method::PROJECT_ADD, json!({"repo_path": repo}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Conflict"), "{err}");
    let list: Vec<Project> = c.call(method::PROJECT_LIST, Value::Null).unwrap();
    assert_eq!(list, vec![p]);

    let err = c.call_raw("nope", Value::Null).unwrap_err().to_string();
    assert!(err.contains("UnknownMethod"), "{err}");

    let tail: Vec<AuditRecord> = c.call(method::AUDIT_TAIL, json!({"limit": 10})).unwrap();
    // The scheduler may add its own records (plan, intake); check ours in order.
    let actions: Vec<_> = tail
        .iter()
        .map(|r| r.action.as_str())
        .filter(|a| !a.starts_with("plan.") && !a.starts_with("intake."))
        .collect();
    assert_eq!(
        actions,
        ["factory.setup", "daemon.started", "project.added"]
    );

    assert!(d.stop().success());
    assert!(!root.join("tpd.sock").exists(), "socket removed on exit");

    // Restart: chain continues and verifies.
    let d = Daemon::start(&root);
    let v: AuditVerify = d.client().call(method::AUDIT_VERIFY, Value::Null).unwrap();
    assert!(v.ok, "{v:?}");
    let tail: Vec<AuditRecord> = d
        .client()
        .call(method::AUDIT_TAIL, json!({"limit": 3}))
        .unwrap();
    let actions: Vec<_> = tail.iter().map(|r| r.action.as_str()).collect();
    assert_eq!(
        actions,
        ["daemon.stop_requested", "daemon.stopped", "daemon.started"]
    );
    d.stop();
}

#[test]
fn second_instance_refuses_to_start() {
    let (_dir, root) = scratch();
    let d = Daemon::start(&root);
    let out = Command::new(env!("CARGO_BIN_EXE_tpd"))
        .args(["--factory", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(75));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already running"));
    // The first daemon is unaffected.
    let _: DaemonStatus = d.client().call(method::STATUS, Value::Null).unwrap();
    d.stop();
}

#[test]
fn subscribers_receive_audit_events() {
    let (dir, root) = scratch();
    let d = Daemon::start(&root);
    let sub = d.client();
    sub.set_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut events = sub.subscribe().unwrap();

    let repo = git_repo(dir.path(), "evt");
    let _: Project = d
        .client()
        .call(method::PROJECT_ADD, json!({"repo_path": repo}))
        .unwrap();

    let first = events
        .find(|e| {
            e.as_ref()
                .is_ok_and(|e| e.event != "audit.appended" || e.data["action"] == "project.added")
                && e.as_ref()
                    .is_ok_and(|e| e.event == "audit.appended" || e.event == "project.added")
        })
        .unwrap()
        .unwrap();
    assert_eq!(first.event, "audit.appended");
    assert_eq!(first.data["action"], "project.added");
    let second = events
        .find(|e| e.as_ref().is_ok_and(|e| e.event == "project.added"))
        .unwrap()
        .unwrap();
    assert_eq!(second.event, "project.added");
    assert_eq!(second.data["slug"], "evt");
    drop(events);
    d.stop();
}

#[test]
fn rejects_garbage_lines() {
    use std::io::{BufRead, BufReader, Write};
    let (_dir, root) = scratch();
    let d = Daemon::start(&root);
    let mut s = std::os::unix::net::UnixStream::connect(root.join("tpd.sock")).unwrap();
    s.write_all(b"not json\n").unwrap();
    let mut line = String::new();
    BufReader::new(&s).read_line(&mut line).unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["error"]["code"], "bad_request");
    d.stop();
}
