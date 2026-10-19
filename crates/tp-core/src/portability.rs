//! Factory export / import (PRD §18). Archives are `tar.gz` with a manifest
//! (schema, versions, checksums). Secrets are excluded unless explicitly
//! requested with a confirmation phrase, and even then stay encrypted.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tp_proto::{ExportReport, ExportRequest, ImportReport, Project, ProjectAdd};

use crate::config::FactoryConfig;
use crate::paths::FactoryPaths;
use crate::{Error, Result, memory, registry, store};

pub const SCHEMA: u32 = 1;
pub const VAULT_CONFIRMATION: &str = "include encrypted vault";

const RECONNECT: &[&str] = &[
    "Claude Code login (`claude` on this machine)",
    "GitHub CLI login (`gh auth login`)",
    "Atlassian connector in Claude Code (claude.ai connectors)",
    "launchd agent (`tp daemon install`)",
    "vault unlock (master password) and any paired devices",
    "local repository checkouts listed as missing",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub tp_version: String,
    pub db_schema: u32,
    pub created_at: String,
    pub files: BTreeMap<String, String>,
    pub included: Vec<String>,
    pub excluded: Vec<String>,
    pub reconnect: Vec<String>,
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub fn export(paths: &FactoryPaths, c: &Connection, req: &ExportRequest) -> Result<ExportReport> {
    if req.include_vault && req.vault_confirmation.as_deref() != Some(VAULT_CONFIRMATION) {
        return Err(Error::Invalid(format!(
            "exporting the vault needs vault_confirmation = \"{VAULT_CONFIRMATION}\""
        )));
    }
    let out = PathBuf::from(&req.path);
    if out.starts_with(paths.root()) {
        return Err(Error::Invalid(
            "write the export outside the factory directory".into(),
        ));
    }
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut included = Vec::new();
    let mut excluded = vec![
        "state.db (rebuilt on import)".to_string(),
        "task worktrees, evidence and run logs".into(),
        "machine credentials (keychain, gh, Claude login, connectors)".into(),
    ];
    let read = |p: &Path| std::fs::read(p).map_err(|e| Error::io(p, e));

    files.insert("factory.yaml".into(), read(&paths.config())?);
    included.push("factory configuration and policy".into());

    let projects = registry::list(c)?;
    files.insert(
        "projects.json".into(),
        serde_json::to_vec_pretty(&projects)?,
    );
    for p in &projects {
        for f in ["project.yaml", "verify.yaml"] {
            let src = paths.project_dir(&p.slug).join(f);
            if src.exists() {
                files.insert(format!("projects/{}/{f}", p.slug), read(&src)?);
            }
        }
    }
    included.push(format!(
        "project registry and per-project config ({} projects)",
        projects.len()
    ));

    files.insert(
        "memory.json".into(),
        serde_json::to_vec_pretty(&memory::export(c)?)?,
    );
    included.push("institutional memory (global and project scopes, with provenance)".into());
    files.insert(
        "budgets.json".into(),
        serde_json::to_vec_pretty(&store::list_budgets(c)?)?,
    );
    included.push("budgets".into());

    if let Ok(rd) = walk(&paths.skills_dir()) {
        for f in rd {
            let rel = f
                .strip_prefix(paths.root())
                .unwrap_or(&f)
                .display()
                .to_string();
            files.insert(rel, read(&f)?);
        }
        included.push("skills".into());
    }

    if req.audit_days > 0 {
        let cutoff = (time::OffsetDateTime::now_utc()
            - time::Duration::days(i64::from(req.audit_days)))
        .date();
        let month = format!("{:04}-{:02}", cutoff.year(), u8::from(cutoff.month()));
        for f in walk(&paths.audit_dir()).unwrap_or_default() {
            let name = f
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if name.ends_with(".jsonl") && name[..7.min(name.len())] >= *month.as_str() {
                files.insert(format!("audit/{name}"), read(&f)?);
            }
        }
        included.push(format!(
            "audit history (last {} days, by month)",
            req.audit_days
        ));
    } else {
        excluded.push("audit history (audit_days = 0)".into());
    }

    if req.include_vault {
        let v = paths.vault_dir().join(tp_vault::VAULT_FILE);
        if v.exists() {
            files.insert(format!("vault/{}", tp_vault::VAULT_FILE), read(&v)?);
            included.push("TP Vault (encrypted; needs the master password to use)".into());
        }
    } else {
        excluded.push("TP Vault secrets (pass include_vault with confirmation to include them, still encrypted)".into());
    }

    let manifest = Manifest {
        schema: SCHEMA,
        tp_version: crate::VERSION.into(),
        db_schema: crate::db::latest_version(),
        created_at: crate::now_rfc3339(),
        files: files.iter().map(|(k, v)| (k.clone(), sha(v))).collect(),
        included: included.clone(),
        excluded: excluded.clone(),
        reconnect: RECONNECT.iter().map(|s| s.to_string()).collect(),
    };
    let tmp = out.with_extension("partial");
    {
        let f = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut tar = tar::Builder::new(GzEncoder::new(f, Compression::default()));
        let mut add = |name: &str, data: &[u8]| -> Result<()> {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o600);
            h.set_mtime(0);
            h.set_cksum();
            tar.append_data(&mut h, name, data)
                .map_err(|e| Error::io(&tmp, e))
        };
        add("manifest.json", &serde_json::to_vec_pretty(&manifest)?)?;
        for (k, v) in &files {
            add(k, v)?;
        }
        tar.into_inner()
            .and_then(|g| g.finish())
            .map_err(|e| Error::io(&tmp, e))?;
    }
    std::fs::rename(&tmp, &out).map_err(|e| Error::io(&out, e))?;
    let bytes = std::fs::read(&out).map_err(|e| Error::io(&out, e))?;
    Ok(ExportReport {
        path: out.display().to_string(),
        schema: SCHEMA,
        included,
        excluded,
        reconnect: manifest.reconnect,
        bytes: bytes.len() as u64,
        sha256: sha(&bytes),
    })
}

fn walk(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Reads and verifies an archive: manifest + files with matching checksums.
pub fn read_archive(path: &Path) -> Result<(Manifest, BTreeMap<String, Vec<u8>>)> {
    let f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut ar = tar::Archive::new(GzDecoder::new(f));
    let mut files = BTreeMap::new();
    for e in ar.entries().map_err(|e| Error::io(path, e))? {
        let mut e = e.map_err(|e| Error::io(path, e))?;
        let name = e
            .path()
            .map_err(|e| Error::io(path, e))?
            .display()
            .to_string();
        if name.contains("..") || name.starts_with('/') {
            return Err(Error::Invalid(format!(
                "archive entry {name:?} is not allowed"
            )));
        }
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).map_err(|e| Error::io(path, e))?;
        files.insert(name, buf);
    }
    let raw = files
        .remove("manifest.json")
        .ok_or_else(|| Error::Invalid("archive has no manifest.json".into()))?;
    let m: Manifest = serde_json::from_slice(&raw)?;
    if m.schema != SCHEMA {
        return Err(Error::Invalid(format!(
            "export schema {} is not supported by this tp",
            m.schema
        )));
    }
    for (name, want) in &m.files {
        let got = files.get(name).map(|b| sha(b));
        if got.as_deref() != Some(want.as_str()) {
            return Err(Error::Invalid(format!(
                "{name} is missing or corrupted (checksum mismatch)"
            )));
        }
    }
    Ok((m, files))
}

/// Previews (`apply = false`) or applies an import into `paths`.
pub fn import(
    paths: &FactoryPaths,
    c: &Connection,
    archive: &Path,
    apply: bool,
) -> Result<ImportReport> {
    let (m, files) = read_archive(archive)?;
    let mut rep = ImportReport {
        path: archive.display().to_string(),
        schema: m.schema,
        applied: apply,
        projects: vec![],
        memory_new: 0,
        memory_duplicates: 0,
        memory_conflicts: vec![],
        files: vec![],
        reconnect: m.reconnect.clone(),
        warnings: vec![],
    };
    if m.db_schema > crate::db::latest_version() {
        rep.warnings.push(format!(
            "archive was made by a newer tp (db schema {})",
            m.db_schema
        ));
    }
    // factory.yaml: only replace a pristine config.
    if let Some(cfg) = files.get("factory.yaml") {
        let current = FactoryConfig::load(&paths.config()).ok();
        let pristine = current
            .as_ref()
            .is_none_or(|c| *c == FactoryConfig::default());
        let target = if pristine {
            paths.config()
        } else {
            paths.root().join("factory.imported.yaml")
        };
        if !pristine {
            rep.warnings.push(
                "factory.yaml differs here; imported copy saved as factory.imported.yaml".into(),
            );
        }
        if apply {
            crate::project_config::write_atomic(&target, &String::from_utf8_lossy(cfg))?;
        }
        rep.files.push(target.display().to_string());
    }
    // Projects.
    let projects: Vec<Project> = files
        .get("projects.json")
        .map(|b| serde_json::from_slice(b))
        .transpose()?
        .unwrap_or_default();
    for p in projects {
        if registry::get(c, &p.slug)?.is_some() {
            rep.warnings.push(format!(
                "project {} already registered; kept the existing one",
                p.slug
            ));
            continue;
        }
        if !Path::new(&p.repo_path).join(".git").exists() {
            rep.warnings.push(format!(
                "project {}: {} is missing here; clone it and run `tp project add`",
                p.slug, p.repo_path
            ));
            continue;
        }
        if apply {
            registry::add(
                c,
                paths,
                &ProjectAdd {
                    repo_path: p.repo_path.clone(),
                    slug: Some(p.slug.clone()),
                    name: Some(p.name.clone()),
                    jira_key: p.jira_key.clone(),
                },
            )?;
        }
        rep.projects.push(p.slug.clone());
    }
    for (name, data) in &files {
        if let Some(rest) = name.strip_prefix("projects/") {
            let target = paths.projects_dir().join(rest);
            if target.exists() {
                continue;
            }
            if apply {
                crate::project_config::write_atomic(&target, &String::from_utf8_lossy(data))?;
            }
            rep.files.push(target.display().to_string());
        } else if name.starts_with("skills/") {
            let target = paths.root().join(name);
            if apply && !target.exists() {
                crate::project_config::write_atomic(&target, &String::from_utf8_lossy(data))?;
            }
        } else if let Some(a) = name.strip_prefix("audit/") {
            // Imported history is kept apart from this factory's own chain.
            let target = paths.audit_dir().join("imported").join(a);
            if apply {
                crate::project_config::write_atomic(&target, &String::from_utf8_lossy(data))?;
            }
            rep.files.push(target.display().to_string());
        } else if let Some(v) = name.strip_prefix("vault/") {
            let target = paths.vault_dir().join(v);
            if target.exists() {
                rep.warnings.push(
                    "a vault already exists here; the archived vault was not restored".into(),
                );
            } else {
                // Validate before restoring: it must parse as a TP Vault.
                let tmpdir = tempdir_in(paths.root())?;
                std::fs::write(tmpdir.join(v), data).map_err(|e| Error::io(&tmpdir, e))?;
                let ok = tp_vault::Vault::open(&tmpdir).is_ok();
                let _ = std::fs::remove_dir_all(&tmpdir);
                if !ok {
                    rep.warnings
                        .push("archived vault failed validation and was not restored".into());
                } else {
                    if apply {
                        std::fs::write(&target, data).map_err(|e| Error::io(&target, e))?;
                    }
                    rep.files.push(target.display().to_string());
                    rep.warnings.push("vault restored encrypted; secrets stay unusable until unlocked with the master password".into());
                }
            }
        }
    }
    if let Some(b) = files.get("memory.json") {
        let ex: memory::MemoryExport = serde_json::from_slice(b)?;
        let r = memory::import(c, &ex, apply)?;
        rep.memory_new = r.new;
        rep.memory_duplicates = r.duplicates;
        rep.memory_conflicts = r.conflicts;
    }
    if let Some(b) = files.get("budgets.json") {
        let budgets: Vec<tp_proto::Budget> = serde_json::from_slice(b)?;
        let have = store::list_budgets(c)?;
        for bud in budgets {
            if apply
                && !have
                    .iter()
                    .any(|h| h.scope == bud.scope && h.period == bud.period)
            {
                store::set_budget(c, &bud)?;
            }
        }
    }
    Ok(rep)
}

fn tempdir_in(root: &Path) -> Result<PathBuf> {
    let p = root.join(format!(".import-{}", std::process::id()));
    std::fs::create_dir_all(&p).map_err(|e| Error::io(&p, e))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn round_trip_into_clean_factory() {
        let d = tempfile::tempdir().unwrap();
        let a = FactoryPaths::new(d.path().join("a"));
        crate::setup::setup(&a, "t").unwrap();
        let ca = crate::db::open(&a.db()).unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        registry::add(
            &ca,
            &a,
            &ProjectAdd {
                repo_path: repo.display().to_string(),
                slug: None,
                name: None,
                jira_key: Some("SF".into()),
            },
        )
        .unwrap();
        std::fs::write(a.project_dir("repo").join("verify.yaml"), "checks: []\n").unwrap();
        memory::propose(
            &ca,
            "repo",
            "build",
            "Use the locked lockfile for every CI build",
            Value::Null,
            0.8,
        )
        .unwrap();
        store::set_budget(
            &ca,
            &tp_proto::Budget {
                scope: "global".into(),
                period: "day".into(),
                limit_usd: 5.0,
            },
        )
        .unwrap();

        let out = d.path().join("f.tar.gz");
        let req = ExportRequest {
            path: out.display().to_string(),
            include_vault: true,
            vault_confirmation: None,
            audit_days: 30,
        };
        assert!(
            export(&a, &ca, &req).is_err(),
            "vault needs the confirmation phrase"
        );
        let req = ExportRequest {
            include_vault: false,
            ..req
        };
        let rep = export(&a, &ca, &req).unwrap();
        assert!(rep.excluded.iter().any(|e| e.contains("Vault")));

        let b = FactoryPaths::new(d.path().join("b"));
        crate::setup::setup(&b, "t").unwrap();
        let cb = crate::db::open(&b.db()).unwrap();
        let preview = import(&b, &cb, &out, false).unwrap();
        assert_eq!(preview.projects, vec!["repo"]);
        assert!(
            registry::list(&cb).unwrap().is_empty(),
            "preview changes nothing"
        );
        let applied = import(&b, &cb, &out, true).unwrap();
        assert_eq!(applied.memory_new, 1);
        assert_eq!(
            registry::list(&cb).unwrap()[0].jira_key.as_deref(),
            Some("SF")
        );
        assert!(b.project_dir("repo").join("verify.yaml").exists());
        assert_eq!(store::list_budgets(&cb).unwrap().len(), 1);
        let again = import(&b, &cb, &out, true).unwrap();
        assert_eq!(
            (again.memory_new, again.projects.len()),
            (0, 0),
            "re-import creates nothing new"
        );
    }

    #[test]
    fn rejects_tampered_archive() {
        let d = tempfile::tempdir().unwrap();
        let a = FactoryPaths::new(d.path().join("a"));
        crate::setup::setup(&a, "t").unwrap();
        let ca = crate::db::open(&a.db()).unwrap();
        let out = d.path().join("f.tar.gz");
        export(
            &a,
            &ca,
            &ExportRequest {
                path: out.display().to_string(),
                include_vault: false,
                vault_confirmation: None,
                audit_days: 0,
            },
        )
        .unwrap();
        let (mut m, files) = read_archive(&out).unwrap();
        m.files.insert("factory.yaml".into(), "0".repeat(64));
        // Rebuild the archive with a wrong checksum.
        let f = std::fs::File::create(&out).unwrap();
        let mut tar = tar::Builder::new(GzEncoder::new(f, Compression::default()));
        let mut add = |n: &str, b: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_size(b.len() as u64);
            h.set_cksum();
            tar.append_data(&mut h, n, b).unwrap();
        };
        add("manifest.json", &serde_json::to_vec(&m).unwrap());
        for (k, v) in &files {
            add(k, v);
        }
        tar.into_inner().unwrap().finish().unwrap();
        assert!(
            read_archive(&out)
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
    }
}
