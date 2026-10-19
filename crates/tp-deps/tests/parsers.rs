#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use tp_deps::{Ecosystem, Error, Manifest, Package, packages};

fn fixture(p: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(p)
}

fn manifest(eco: Ecosystem, path: &str, lock: Option<&str>) -> Manifest {
    Manifest {
        path: fixture(path),
        ecosystem: eco,
        lockfile: lock.map(fixture),
    }
}

/// (name, version, direct) sorted, for compact assertions.
fn triples(pkgs: &[Package]) -> Vec<(&str, &str, bool)> {
    let mut v: Vec<_> = pkgs
        .iter()
        .map(|p| (p.name.as_str(), p.version.as_str(), p.direct))
        .collect();
    v.sort();
    v
}

#[test]
fn npm_lock_v3() {
    let m = manifest(
        Ecosystem::Npm,
        "npm/package.json",
        Some("npm/package-lock.json"),
    );
    let pkgs = packages(&m).unwrap();
    assert!(pkgs.iter().all(|p| p.ecosystem == Ecosystem::Npm));
    assert_eq!(
        triples(&pkgs),
        vec![
            ("@scope/util", "2.1.3", true),
            ("jest", "29.7.0", true),
            ("lodash", "4.17.20", true), // top-level copy direct; nested copy merged
            ("minimist", "1.2.5", false),
            ("minimist", "1.2.8", false),
            ("nested-lodash-user", "1.0.0", false),
            ("node-fetch", "2.6.1", true), // alias "fetchy" resolved to real name
        ]
    );
}

#[test]
fn npm_lock_v2() {
    let m = manifest(
        Ecosystem::Npm,
        "npm-v2/package.json",
        Some("npm-v2/package-lock.json"),
    );
    assert_eq!(
        triples(&packages(&m).unwrap()),
        vec![("express", "4.17.1", true), ("qs", "6.7.0", false)]
    );
}

#[test]
fn npm_lock_v1_is_rejected_with_a_fix() {
    let m = manifest(
        Ecosystem::Npm,
        "npm-v1/package.json",
        Some("npm-v1/package-lock.json"),
    );
    let err = packages(&m).unwrap_err();
    assert!(matches!(err, Error::Parse { .. }));
    let msg = err.to_string();
    assert!(msg.contains("lockfileVersion 1"), "{msg}");
    assert!(msg.contains("npm install --package-lock-only"), "{msg}");
}

#[test]
fn cargo_lock_single_crate() {
    let m = manifest(
        Ecosystem::Cargo,
        "cargo/Cargo.toml",
        Some("cargo/Cargo.lock"),
    );
    let pkgs = packages(&m).unwrap();
    // git dependency and local crates (demo, mylocal) are excluded.
    assert_eq!(
        triples(&pkgs),
        vec![
            ("libc", "0.2.150", true),
            ("rand", "0.7.3", false), // only mylocal uses 0.7
            ("rand", "0.8.5", true),
            ("serde", "1.0.190", true),
            ("serde_derive", "1.0.190", false),
            ("smallvec", "1.6.0", true),
            ("time", "0.3.20", true), // renamed `time-crate`, sparse registry
        ]
    );
}

#[test]
fn cargo_lock_workspace_root() {
    let m = manifest(
        Ecosystem::Cargo,
        "cargo-ws/Cargo.toml",
        Some("cargo-ws/Cargo.lock"),
    );
    assert_eq!(
        triples(&packages(&m).unwrap()),
        vec![
            ("bytes", "1.4.0", true),
            ("pin-project-lite", "0.2.9", false),
            ("tokio", "1.28.0", true),
        ]
    );
}

#[test]
fn go_mod_require_blocks() {
    let m = manifest(Ecosystem::Go, "go/go.mod", Some("go/go.sum"));
    assert_eq!(
        triples(&packages(&m).unwrap()),
        vec![
            ("github.com/BurntSushi/toml", "v1.3.2", true),
            ("github.com/gin-gonic/gin", "v1.9.0", true),
            (
                "golang.org/x/net",
                "v0.0.0-20220722155237-a158d28d115b",
                false
            ),
            ("golang.org/x/text", "v0.3.7", false),
            ("gopkg.in/yaml.v3", "v3.0.1", true),
        ]
    );
}

#[test]
fn pubspec_lock() {
    let m = manifest(Ecosystem::Pub, "pub/pubspec.yaml", Some("pub/pubspec.lock"));
    // sdk (flutter, flutter_test) and path (local_pkg) packages are excluded.
    assert_eq!(
        triples(&packages(&m).unwrap()),
        vec![
            ("collection", "1.17.2", false),
            ("http", "0.13.5", true),
            ("lints", "2.1.1", true),
            ("provider", "6.0.5+1", true),
        ]
    );
}

#[test]
fn requirements_txt_pins() {
    let m = manifest(Ecosystem::PyPI, "python/requirements.txt", None);
    assert_eq!(
        triples(&packages(&m).unwrap()),
        vec![
            ("django", "3.2.0", true),
            ("jinja2", "2.11.2", true),
            ("pyyaml", "5.3.1", true),
            ("requests", "2.25.1", true),
        ]
    );
}

#[test]
fn poetry_lock_with_pyproject() {
    let m = manifest(
        Ecosystem::PyPI,
        "poetry/pyproject.toml",
        Some("poetry/poetry.lock"),
    );
    // directory source (mylib) excluded; names PEP 503-normalized.
    assert_eq!(
        triples(&packages(&m).unwrap()),
        vec![
            ("certifi", "2022.12.7", false),
            ("pytest", "7.4.0", true),
            ("requests", "2.28.1", true),
            ("urllib3", "1.26.5", true),
        ]
    );
}

#[test]
fn pyproject_without_lock_yields_exact_pins_only() {
    let m = manifest(Ecosystem::PyPI, "poetry/pyproject.toml", None);
    assert_eq!(
        triples(&packages(&m).unwrap()),
        vec![("urllib3", "1.26.5", true)]
    );
}

#[test]
fn lockless_npm_and_cargo_yield_nothing() {
    let npm = manifest(Ecosystem::Npm, "npm/packages/ws-lib/package.json", None);
    assert!(packages(&npm).unwrap().is_empty());
    let cargo = manifest(Ecosystem::Cargo, "cargo/mylocal/Cargo.toml", None);
    assert!(packages(&cargo).unwrap().is_empty());
}

#[test]
fn missing_and_malformed_files_are_errors() {
    let missing = manifest(Ecosystem::Go, "nope/go.mod", None);
    assert!(matches!(packages(&missing), Err(Error::Io { .. })));

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
    std::fs::write(tmp.path().join("package-lock.json"), "{not json").unwrap();
    let bad = Manifest {
        path: tmp.path().join("package.json"),
        ecosystem: Ecosystem::Npm,
        lockfile: Some(tmp.path().join("package-lock.json")),
    };
    let err = packages(&bad).unwrap_err();
    assert!(err.to_string().contains("invalid JSON"), "{err}");
}

#[test]
fn types_round_trip_through_serde() {
    let m = manifest(Ecosystem::Go, "go/go.mod", Some("go/go.sum"));
    let json = serde_json::to_string(&m).unwrap();
    assert!(json.contains("\"ecosystem\":\"go\""));
    let back: Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m);
    let pkgs = packages(&m).unwrap();
    let back: Vec<Package> = serde_json::from_str(&serde_json::to_string(&pkgs).unwrap()).unwrap();
    assert_eq!(back, pkgs);
}
