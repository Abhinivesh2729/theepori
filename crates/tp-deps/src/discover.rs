//! Manifest and lockfile discovery.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::{Ecosystem, Manifest};

/// Directories never descended into: VCS metadata, installed dependencies and
/// build output.
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "build",
    "dist",
    ".dart_tool",
    "vendor",
];

/// Walk `repo` and return every supported manifest with its lockfile.
///
/// Rules:
/// - npm `package.json` ↔ `package-lock.json`, Cargo `Cargo.toml` ↔
///   `Cargo.lock`, pub `pubspec.yaml` ↔ `pubspec.lock`: a manifest without a
///   sibling lockfile whose ancestor (inside `repo`) has one is a workspace
///   member and is skipped — the root manifest already covers its packages.
/// - Go `go.mod` ↔ `go.sum` (go.mod carries the versions itself).
/// - Python: `pyproject.toml` ↔ `poetry.lock`, and `requirements.txt`
///   (paired with `poetry.lock` only when there is no `pyproject.toml`).
///
/// Symlinks are not followed. Results are sorted by path.
pub fn discover(repo: &Path) -> Vec<Manifest> {
    let mut dirs: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let walker = WalkDir::new(repo)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
        });
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_interesting(&name) {
            continue;
        }
        if let Some(parent) = entry.path().parent() {
            dirs.entry(parent.to_path_buf()).or_default().insert(name);
        }
    }

    let has = |dir: &Path, file: &str| dirs.get(dir).is_some_and(|s| s.contains(file));
    let ancestor_has = |dir: &Path, file: &str| {
        dir.ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(repo))
            .any(|a| has(a, file))
    };

    let mut out = Vec::new();
    for (dir, files) in &dirs {
        let pairs: [(&str, &str, Ecosystem); 3] = [
            ("package.json", "package-lock.json", Ecosystem::Npm),
            ("Cargo.toml", "Cargo.lock", Ecosystem::Cargo),
            ("pubspec.yaml", "pubspec.lock", Ecosystem::Pub),
        ];
        for (manifest, lock, eco) in pairs {
            if !files.contains(manifest) {
                continue;
            }
            if files.contains(lock) {
                out.push(Manifest {
                    path: dir.join(manifest),
                    ecosystem: eco,
                    lockfile: Some(dir.join(lock)),
                });
            } else if !ancestor_has(dir, lock) {
                out.push(Manifest {
                    path: dir.join(manifest),
                    ecosystem: eco,
                    lockfile: None,
                });
            }
        }
        if files.contains("go.mod") {
            out.push(Manifest {
                path: dir.join("go.mod"),
                ecosystem: Ecosystem::Go,
                lockfile: files.contains("go.sum").then(|| dir.join("go.sum")),
            });
        }
        let poetry = files
            .contains("poetry.lock")
            .then(|| dir.join("poetry.lock"));
        let pyproject = files.contains("pyproject.toml");
        if pyproject {
            out.push(Manifest {
                path: dir.join("pyproject.toml"),
                ecosystem: Ecosystem::PyPI,
                lockfile: poetry.clone(),
            });
        }
        if files.contains("requirements.txt") {
            out.push(Manifest {
                path: dir.join("requirements.txt"),
                ecosystem: Ecosystem::PyPI,
                lockfile: if pyproject { None } else { poetry },
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn is_interesting(name: &str) -> bool {
    matches!(
        name,
        "package.json"
            | "package-lock.json"
            | "Cargo.toml"
            | "Cargo.lock"
            | "go.mod"
            | "go.sum"
            | "pubspec.yaml"
            | "pubspec.lock"
            | "requirements.txt"
            | "poetry.lock"
            | "pyproject.toml"
    )
}
