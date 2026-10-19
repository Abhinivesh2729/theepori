//! Import from a Panuval store written by the real Go vault code
//! (`fixtures/genfixtures.go`). The store's master password is
//! "panuval fixture password"; its values are fixture-only.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tp_vault::crypto::KdfParams;
use tp_vault::{Error, PanuvalItem, Scope, Unlocked, Vault, import_panuval};

const PANUVAL_PW: &str = "panuval fixture password";
const TP_PW: &str = "correct horse battery staple";

fn store() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/panuval_store/panuval-atmega-uid-fixture.json")
}

fn target() -> (tempfile::TempDir, Unlocked) {
    let tmp = tempfile::tempdir().expect("tempdir");
    Vault::init_with_kdf(
        tmp.path(),
        TP_PW,
        KdfParams::with_cost(1024, 1, 1).expect("kdf"),
    )
    .expect("init");
    let u = Vault::open(tmp.path())
        .expect("open")
        .unlock(TP_PW)
        .expect("unlock");
    (tmp, u)
}

/// `projects/<p>` → `<p>/default`, everything else → `global/default`.
fn by_folder(it: &PanuvalItem) -> Scope {
    match it.folder.strip_prefix("projects/") {
        Some(p) => Scope::project(p),
        None => Scope::global(),
    }
}

#[test]
fn imports_env_items_from_go_store() {
    let (_t, mut u) = target();
    let report = import_panuval(&store(), PANUVAL_PW, &mut u, by_folder).expect("import");
    assert_eq!(report.uid, "uid-fixture");
    assert_eq!(report.tombstones, 1, "OLD_KEY was deleted in Panuval");

    let imported: Vec<(String, String)> = report
        .imported
        .iter()
        .map(|i| (i.panuval_path.clone(), i.scope.to_string()))
        .collect();
    assert_eq!(
        imported,
        vec![
            ("EMPTY_VALUE".into(), "global/default".into()),
            ("GITHUB_TOKEN".into(), "global/default".into()),
            ("projects/kavi/OPENAI_API_KEY".into(), "kavi/default".into()),
            (
                "projects/thinai/DATABASE_URL".into(),
                "thinai/default".into()
            ),
            (
                "projects/thinai/OPENAI_API_KEY".into(),
                "thinai/default".into()
            ),
        ]
    );
    let skipped: Vec<&str> = report
        .skipped
        .iter()
        .map(|s| s.panuval_path.as_str())
        .collect();
    assert_eq!(skipped, vec!["notes/recovery", "personal/github"]);
    assert!(
        report
            .skipped
            .iter()
            .all(|s| s.reason.contains("not an environment secret"))
    );

    let thinai = Scope::project("thinai");
    assert_eq!(
        *u.get(&thinai, "OPENAI_API_KEY").expect("get"),
        "sk-thinai-0002",
        "latest version wins"
    );
    assert_eq!(
        *u.get(&thinai, "DATABASE_URL").expect("get"),
        "postgres://u:p@localhost/db?x=தமிழ்"
    );
    assert_eq!(
        *u.get(&Scope::project("kavi"), "OPENAI_API_KEY")
            .expect("get"),
        "sk-kavi-0001"
    );
    assert_eq!(
        *u.get(&Scope::global(), "GITHUB_TOKEN").expect("get"),
        "ghp_fixture_token"
    );
    assert_eq!(*u.get(&Scope::global(), "EMPTY_VALUE").expect("get"), "");
    let env = u.env_for(&thinai).expect("env");
    let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "DATABASE_URL",
            "EMPTY_VALUE",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY"
        ]
    );

    // The report carries no values.
    let dbg = format!("{report:?}");
    for secret in ["sk-thinai", "ghp_fixture", "hunter2", "postgres://"] {
        assert!(!dbg.contains(secret), "{dbg}");
    }

    // Re-running never overwrites.
    let again = import_panuval(&store(), PANUVAL_PW, &mut u, by_folder).expect("reimport");
    assert!(again.imported.is_empty());
    assert_eq!(again.skipped.len(), 7);
    assert!(
        again
            .skipped
            .iter()
            .any(|s| s.reason.contains("already exists"))
    );
}

#[test]
fn mapper_sees_metadata_and_collisions_are_reported() {
    let (_t, mut u) = target();
    let report = import_panuval(&store(), PANUVAL_PW, &mut u, |it| {
        assert!(!it.id.is_empty() && it.version >= 1);
        assert!(it.created_at.is_some() && it.updated_at.is_some());
        if it.name == "OPENAI_API_KEY" && it.folder == "projects/thinai" {
            assert!(it.agent_access);
            assert_eq!(it.version, 2);
        }
        Scope::global()
    })
    .expect("import");
    assert_eq!(report.imported.len(), 4);
    let dup = report
        .skipped
        .iter()
        .find(|s| s.panuval_path == "projects/thinai/OPENAI_API_KEY")
        .expect("collision reported");
    assert!(dup.reason.contains("already imported"), "{}", dup.reason);
    assert_eq!(
        *u.get(&Scope::global(), "OPENAI_API_KEY").expect("get"),
        "sk-kavi-0001",
        "first by path wins"
    );
}

#[test]
fn invalid_mapped_scope_is_skipped() {
    let (_t, mut u) = target();
    let report = import_panuval(&store(), PANUVAL_PW, &mut u, |_| {
        Scope::new("bad/scope", "x")
    })
    .expect("import");
    assert!(report.imported.is_empty());
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.reason.contains("invalid scope"))
    );
}

#[test]
fn wrong_panuval_password() {
    let (_t, mut u) = target();
    assert!(matches!(
        import_panuval(&store(), "not the password", &mut u, by_folder),
        Err(Error::WrongPassword)
    ));
    assert!(u.list(None).is_empty());
}

#[test]
fn accepts_a_config_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join("cache")).expect("mkdir");
    std::fs::copy(
        store(),
        tmp.path().join("cache/panuval-atmega-uid-fixture.json"),
    )
    .expect("cp");
    std::fs::write(tmp.path().join("session.json"), b"{}").expect("session");
    let (_t, mut u) = target();
    let report = import_panuval(tmp.path(), PANUVAL_PW, &mut u, by_folder).expect("import");
    assert_eq!(report.imported.len(), 5);

    let empty = tempfile::tempdir().expect("tempdir");
    assert!(matches!(
        import_panuval(empty.path(), PANUVAL_PW, &mut u, by_folder),
        Err(Error::Panuval(_))
    ));
}

#[test]
fn tampered_store_items_are_skipped_not_imported() {
    let mut v: Value =
        serde_json::from_slice(&std::fs::read(store()).expect("read")).expect("json");
    // Bump one live item's version (a rollback/swap attempt): AAD no longer matches.
    let id = v["items"]
        .as_object()
        .expect("items")
        .iter()
        .find(|(_, it)| it["deleted"] == false)
        .map(|(id, _)| id.clone())
        .expect("live item");
    v["items"][&id]["version"] = Value::from(99);
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("cache.json");
    std::fs::write(&path, serde_json::to_vec(&v).expect("json")).expect("write");

    let (_t, mut u) = target();
    let report = import_panuval(&path, PANUVAL_PW, &mut u, by_folder).expect("import");
    let bad = report
        .skipped
        .iter()
        .find(|s| s.panuval_path == id)
        .expect("tampered item reported by id");
    assert!(bad.reason.contains("failed authentication"));
    assert_eq!(report.imported.len() + report.skipped.len(), 7);
}

#[test]
fn store_without_header_is_explained() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("c.json");
    std::fs::write(&path, br#"{"formatVersion":1,"uid":"u","items":{}}"#).expect("write");
    let (_t, mut u) = target();
    let err = import_panuval(&path, PANUVAL_PW, &mut u, by_folder).expect_err("no header");
    assert!(err.to_string().contains("panuval sync"), "{err}");
}
