#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use tp_deps::{Ecosystem, Manifest, discover};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn rel(root: &Path, m: &Manifest) -> (String, Ecosystem, Option<String>) {
    let r = |p: &Path| p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
    (r(&m.path), m.ecosystem, m.lockfile.as_deref().map(r))
}

#[test]
fn discovers_every_fixture_format() {
    let root = fixtures();
    let found: Vec<_> = discover(&root).iter().map(|m| rel(&root, m)).collect();
    let s = |x: &str| x.to_string();
    let want = vec![
        (
            s("cargo/Cargo.toml"),
            Ecosystem::Cargo,
            Some(s("cargo/Cargo.lock")),
        ),
        (
            s("cargo-ws/Cargo.toml"),
            Ecosystem::Cargo,
            Some(s("cargo-ws/Cargo.lock")),
        ),
        (s("go/go.mod"), Ecosystem::Go, Some(s("go/go.sum"))),
        (
            s("npm/package.json"),
            Ecosystem::Npm,
            Some(s("npm/package-lock.json")),
        ),
        (
            s("npm-v1/package.json"),
            Ecosystem::Npm,
            Some(s("npm-v1/package-lock.json")),
        ),
        (
            s("npm-v2/package.json"),
            Ecosystem::Npm,
            Some(s("npm-v2/package-lock.json")),
        ),
        (
            s("poetry/pyproject.toml"),
            Ecosystem::PyPI,
            Some(s("poetry/poetry.lock")),
        ),
        (
            s("pub/pubspec.yaml"),
            Ecosystem::Pub,
            Some(s("pub/pubspec.lock")),
        ),
        (s("python/requirements.txt"), Ecosystem::PyPI, None),
    ];
    assert_eq!(found, want);
}

#[test]
fn skips_vendor_and_build_dirs_and_handles_missing_locks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let write = |p: &str, body: &str| {
        let path = root.join(p);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    };
    for skipped in [
        ".git/package.json",
        "node_modules/left-pad/package.json",
        "target/debug/Cargo.toml",
        "build/go.mod",
        "dist/requirements.txt",
        ".dart_tool/pubspec.yaml",
        "vendor/github.com/x/go.mod",
        "web/node_modules/a/package.json",
    ] {
        write(skipped, "{}");
    }
    write("web/package.json", "{}");
    write("svc/requirements.txt", "flask==2.0.0\n");
    write("svc/poetry.lock", "");
    write("orphan/package-lock.json", "{}");

    let found: Vec<_> = discover(root).iter().map(|m| rel(root, m)).collect();
    assert_eq!(
        found,
        vec![
            (
                "svc/requirements.txt".to_string(),
                Ecosystem::PyPI,
                Some("svc/poetry.lock".to_string())
            ),
            ("web/package.json".to_string(), Ecosystem::Npm, None),
        ]
    );
}

#[test]
fn repo_root_named_like_a_skipped_dir_is_still_walked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("build");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("go.mod"), "module x\n").unwrap();
    let found = discover(&root);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].ecosystem, Ecosystem::Go);
    assert_eq!(found[0].lockfile, None);
}
