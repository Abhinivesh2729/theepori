//! Local git operations run by tpd (never by agents): worktrees, diffs,
//! commits and pushes of task branches.

use std::path::Path;
use std::process::{Command, Output};

use tp_proto::{DiffFile, TaskDiff};

use crate::{Error, Result};

fn git(repo: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| Error::Invalid(format!("git: {e}")))
}

fn git_ok(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git(repo, args)?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

pub fn default_branch(repo: &Path) -> Option<String> {
    if let Ok(r) = git_ok(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return r.strip_prefix("origin/").map(str::to_owned);
    }
    for b in ["main", "master"] {
        if git(repo, &["rev-parse", "--verify", "--quiet", b]).is_ok_and(|o| o.status.success()) {
            return Some(b.into());
        }
    }
    git_ok(repo, &["branch", "--show-current"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// `owner/name` from the origin remote if it points at GitHub.
pub fn github_repo(repo: &Path) -> Option<String> {
    let url = git_ok(repo, &["remote", "get-url", "origin"]).ok()?;
    parse_github_url(&url)
}

pub fn parse_github_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let (o, n) = (parts.next()?, parts.next()?);
    (!o.is_empty() && !n.is_empty()).then(|| format!("{o}/{n}"))
}

pub fn head_sha(repo: &Path) -> Result<String> {
    git_ok(repo, &["rev-parse", "HEAD"])
}

/// Creates (or reuses) a worktree on `branch` started from `base`.
pub fn ensure_worktree(repo: &Path, worktree: &Path, branch: &str, base: &str) -> Result<()> {
    if worktree.join(".git").exists() {
        return Ok(());
    }
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let _ = git(repo, &["fetch", "--quiet", "origin", base]);
    let start = if git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("origin/{base}"),
        ],
    )?
    .status
    .success()
    {
        format!("origin/{base}")
    } else {
        base.to_string()
    };
    let wt = worktree.to_string_lossy();
    let branch_exists = git(repo, &["rev-parse", "--verify", "--quiet", branch])?
        .status
        .success();
    if branch_exists {
        git_ok(repo, &["worktree", "add", &wt, branch])?;
    } else {
        git_ok(repo, &["worktree", "add", "-b", branch, &wt, &start])?;
    }
    Ok(())
}

pub fn remove_worktree(repo: &Path, worktree: &Path) -> Result<()> {
    if worktree.exists() {
        git_ok(
            repo,
            &["worktree", "remove", "--force", &worktree.to_string_lossy()],
        )?;
    }
    let _ = git(repo, &["worktree", "prune"]);
    Ok(())
}

/// Merge base of HEAD with `base` (falls back to `base`).
fn diff_base(worktree: &Path, base: &str) -> String {
    for b in [format!("origin/{base}"), base.to_string()] {
        if let Ok(mb) = git_ok(worktree, &["merge-base", "HEAD", &b]) {
            return mb;
        }
    }
    base.to_string()
}

/// Diff of the worktree (committed + uncommitted, incl. untracked) against base.
pub fn diff(worktree: &Path, base: &str, max_bytes: usize) -> Result<TaskDiff> {
    let mb = diff_base(worktree, base);
    // Stage intent-to-add for untracked files so they show up in the diff.
    let _ = git(worktree, &["add", "-N", "."]);
    let numstat = git_ok(worktree, &["diff", "--numstat", &mb])?;
    let files = numstat
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let a = it.next()?.parse().unwrap_or(0);
            let d = it.next()?.parse().unwrap_or(0);
            Some(DiffFile {
                path: it.next()?.to_string(),
                additions: a,
                deletions: d,
            })
        })
        .collect();
    let mut text = git_ok(worktree, &["diff", &mb])?;
    let truncated = text.len() > max_bytes;
    if truncated {
        let mut cut = max_bytes;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Ok(TaskDiff {
        base: mb,
        files,
        diff: text,
        truncated,
    })
}

pub fn has_changes(worktree: &Path, base: &str) -> Result<bool> {
    let d = diff(worktree, base, 0)?;
    Ok(!d.files.is_empty())
}

/// Commits everything in the worktree as tp (if anything changed).
pub fn commit_all(worktree: &Path, message: &str) -> Result<bool> {
    git_ok(worktree, &["add", "-A"])?;
    if git(worktree, &["diff", "--cached", "--quiet"])?
        .status
        .success()
    {
        return Ok(false);
    }
    git_ok(
        worktree,
        &[
            "-c",
            "user.name=Thee Pori",
            "-c",
            "user.email=tp@localhost",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    )?;
    Ok(true)
}

pub fn push_branch(worktree: &Path, branch: &str) -> Result<()> {
    git_ok(worktree, &["push", "--quiet", "-u", "origin", branch])?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// A repo with one commit on `main`.
    pub fn repo(dir: &Path) -> std::path::PathBuf {
        let r = dir.join("repo");
        std::fs::create_dir_all(&r).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            git_ok(&r, &args).unwrap();
        }
        std::fs::write(r.join("README.md"), "hello\n").unwrap();
        git_ok(&r, &["add", "."]).unwrap();
        git_ok(&r, &["commit", "-qm", "init"]).unwrap();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls() {
        assert_eq!(
            parse_github_url("git@github.com:a/b.git").as_deref(),
            Some("a/b")
        );
        assert_eq!(
            parse_github_url("https://github.com/a/b").as_deref(),
            Some("a/b")
        );
        assert_eq!(parse_github_url("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn worktree_diff_commit() {
        let d = tempfile::tempdir().unwrap();
        let r = testutil::repo(d.path());
        assert_eq!(default_branch(&r).as_deref(), Some("main"));
        let wt = d.path().join("wt");
        ensure_worktree(&r, &wt, "tp/T1", "main").unwrap();
        ensure_worktree(&r, &wt, "tp/T1", "main").unwrap();
        assert!(!has_changes(&wt, "main").unwrap());
        std::fs::write(wt.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(wt.join("new.txt"), "x\n").unwrap();
        let df = diff(&wt, "main", 10_000).unwrap();
        assert_eq!(df.files.len(), 2);
        assert!(df.diff.contains("+world"));
        assert!(commit_all(&wt, "T1: change").unwrap());
        assert!(!commit_all(&wt, "again").unwrap());
        assert_eq!(diff(&wt, "main", 10).unwrap().files.len(), 2);
        remove_worktree(&r, &wt).unwrap();
        assert!(!wt.exists());
    }
}
