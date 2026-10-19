//! Project registry: which repositories the factory manages.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use tp_proto::{Project, ProjectAdd};

use crate::paths::FactoryPaths;
use crate::{Error, Result};

/// Validates and normalizes a registration request, without touching state.
pub fn prepare(req: &ProjectAdd) -> Result<Project> {
    let repo = canonical_repo(Path::new(&req.repo_path))?;
    let dir_name = repo.file_name().and_then(|n| n.to_str()).unwrap_or("project");
    let slug = match &req.slug {
        Some(s) => s.clone(),
        None => slugify(dir_name),
    };
    validate_slug(&slug)?;
    let jira_key = req.jira_key.as_ref().map(|k| k.trim().to_ascii_uppercase());
    if let Some(k) = &jira_key {
        validate_jira_key(k)?;
    }
    let name = req.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| dir_name.to_owned());
    Ok(Project {
        slug,
        name,
        repo_path: repo.to_string_lossy().into_owned(),
        jira_key,
        created_at: crate::now_rfc3339(),
    })
}

/// Inserts the project and creates its factory directory.
pub fn add(conn: &Connection, paths: &FactoryPaths, req: &ProjectAdd) -> Result<Project> {
    let p = prepare(req)?;
    if get(conn, &p.slug)?.is_some() {
        return Err(Error::Conflict(format!("project {:?}", p.slug)));
    }
    if let Some(other) = by_repo(conn, &p.repo_path)? {
        return Err(Error::Conflict(format!(
            "repository {} (registered as {:?})",
            p.repo_path, other.slug
        )));
    }
    let dir = paths.project_dir(&p.slug);
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    conn.execute(
        "INSERT INTO projects (slug, name, repo_path, jira_key, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![p.slug, p.name, p.repo_path, p.jira_key, p.created_at],
    )?;
    Ok(p)
}

pub fn list(conn: &Connection) -> Result<Vec<Project>> {
    let mut stmt = conn.prepare(
        "SELECT slug, name, repo_path, jira_key, created_at FROM projects ORDER BY slug",
    )?;
    let rows = stmt.query_map([], row_to_project)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn get(conn: &Connection, slug: &str) -> Result<Option<Project>> {
    Ok(conn
        .query_row(
            "SELECT slug, name, repo_path, jira_key, created_at FROM projects WHERE slug = ?1",
            [slug],
            row_to_project,
        )
        .optional()?)
}

pub fn count(conn: &Connection) -> Result<u64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get::<_, i64>(0))? as u64)
}

fn by_repo(conn: &Connection, repo: &str) -> Result<Option<Project>> {
    Ok(conn
        .query_row(
            "SELECT slug, name, repo_path, jira_key, created_at FROM projects WHERE repo_path = ?1",
            [repo],
            row_to_project,
        )
        .optional()?)
}

fn row_to_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        slug: r.get(0)?,
        name: r.get(1)?,
        repo_path: r.get(2)?,
        jira_key: r.get(3)?,
        created_at: r.get(4)?,
    })
}

fn canonical_repo(path: &Path) -> Result<PathBuf> {
    let repo = path.canonicalize().map_err(|e| {
        Error::Invalid(format!("{}: {e}; pass the path to a local git checkout", path.display()))
    })?;
    if !repo.join(".git").exists() {
        return Err(Error::Invalid(format!(
            "{} is not a git repository (no .git); clone it first",
            repo.display()
        )));
    }
    Ok(repo)
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

pub fn validate_slug(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 40
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "slug {s:?} must be 1-40 chars of a-z, 0-9 and '-', not starting or ending with '-'"
        )))
    }
}

fn validate_jira_key(k: &str) -> Result<()> {
    let mut chars = k.chars();
    let ok = k.len() >= 2
        && k.len() <= 10
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("Jira key {k:?} must look like SF or PAN2 (2-10 chars, letter first)")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn fixture() -> (tempfile::TempDir, FactoryPaths, Connection, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let paths = FactoryPaths::new(dir.path().join("factory"));
        std::fs::create_dir_all(paths.root()).unwrap();
        let mut conn = db::open(&paths.db()).unwrap();
        db::migrate(&mut conn).unwrap();
        let repo = dir.path().join("My App");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        (dir, paths, conn, repo)
    }

    fn req(repo: &Path) -> ProjectAdd {
        ProjectAdd { repo_path: repo.to_string_lossy().into(), slug: None, name: None, jira_key: Some("sf".into()) }
    }

    #[test]
    fn add_list_get() {
        let (_d, paths, conn, repo) = fixture();
        let p = add(&conn, &paths, &req(&repo)).unwrap();
        assert_eq!(p.slug, "my-app");
        assert_eq!(p.name, "My App");
        assert_eq!(p.jira_key.as_deref(), Some("SF"));
        assert!(paths.project_dir("my-app").is_dir());
        assert_eq!(list(&conn).unwrap(), vec![p.clone()]);
        assert_eq!(get(&conn, "my-app").unwrap(), Some(p));
        assert_eq!(count(&conn).unwrap(), 1);
    }

    #[test]
    fn rejects_duplicates() {
        let (_d, paths, conn, repo) = fixture();
        add(&conn, &paths, &req(&repo)).unwrap();
        assert!(matches!(add(&conn, &paths, &req(&repo)), Err(Error::Conflict(_))));
        let mut other = req(&repo);
        other.slug = Some("other".into());
        let err = add(&conn, &paths, &other).unwrap_err().to_string();
        assert!(err.contains("registered as \"my-app\""), "{err}");
    }

    #[test]
    fn rejects_non_git_and_bad_input() {
        let (d, paths, conn, repo) = fixture();
        let plain = d.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(add(&conn, &paths, &req(&plain)).unwrap_err().to_string().contains("not a git repository"));
        let mut bad = req(&repo);
        bad.slug = Some("Bad Slug".into());
        assert!(add(&conn, &paths, &bad).is_err());
        let mut bad = req(&repo);
        bad.jira_key = Some("1X".into());
        assert!(add(&conn, &paths, &bad).is_err());
    }

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("Thee Pori!"), "thee-pori");
        assert_eq!(slugify("--a__b--"), "a-b");
    }
}
