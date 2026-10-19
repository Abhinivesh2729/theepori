#![allow(clippy::unwrap_used)]
//! Runs the real `tp` binary against a scratch factory (no daemon).

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn tp(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tp"))
        .arg("--factory")
        .arg(root)
        .args(args)
        .env("USER", "tester")
        .output()
        .unwrap()
}

fn json(out: &Output) -> Value {
    assert!(
        out.status.success() || !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn setup_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("f");
    let first = json(&tp(&root, &["setup", "--json"]));
    assert!(!first["created"].as_array().unwrap().is_empty());
    let second = json(&tp(&root, &["setup", "--json"]));
    assert_eq!(second["created"], serde_json::json!([]));
    assert_eq!(second["migrations_applied"], 0);
    let tail = json(&tp(&root, &["audit", "tail", "--json"]));
    assert_eq!(
        tail.as_array().unwrap().len(),
        1,
        "only the first setup is audited"
    );
    assert_eq!(tail[0]["actor"]["id"], "cli:tester");
}

#[test]
fn project_add_without_daemon_is_audited() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("f");
    let repo = dir.path().join("Repo One");
    std::fs::create_dir_all(repo.join(".git")).unwrap();

    let out = tp(&root, &["project", "list"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("run `tp setup` first"));

    tp(&root, &["setup"]);
    let p = json(&tp(
        &root,
        &[
            "project",
            "add",
            repo.to_str().unwrap(),
            "--jira",
            "sf",
            "--json",
        ],
    ));
    assert_eq!(p["slug"], "repo-one");
    assert_eq!(p["jira_key"], "SF");
    let list = json(&tp(&root, &["project", "list", "--json"]));
    assert_eq!(list.as_array().unwrap().len(), 1);

    let dup = tp(&root, &["project", "add", repo.to_str().unwrap()]);
    assert!(!dup.status.success());
    assert!(String::from_utf8_lossy(&dup.stderr).contains("already exists"));

    let v = json(&tp(&root, &["audit", "verify", "--json"]));
    assert_eq!(v["ok"], true);
    assert_eq!(v["records"], 2);
}

#[test]
fn doctor_reports_factory_and_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("f");
    let before = tp(&root, &["doctor", "--json"]);
    assert!(!before.status.success());
    assert_eq!(json(&before)["status"], "fail");

    tp(&root, &["setup"]);
    let after = json(&tp(&root, &["doctor", "--json"]));
    let checks = after["checks"].as_array().unwrap();
    let by_name = |n: &str| checks.iter().find(|c| c["name"] == n).unwrap().clone();
    assert_eq!(by_name("config")["status"], "ok");
    assert_eq!(by_name("audit chain")["status"], "ok");
    assert_ne!(by_name("daemon")["status"], "ok");
}

#[test]
fn daemon_status_when_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let out = tp(&dir.path().join("f"), &["daemon", "status", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["running"], false);
}
