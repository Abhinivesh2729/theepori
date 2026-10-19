use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use time::OffsetDateTime;
use tp_vault::crypto::KdfParams;
use tp_vault::{Error, ItemOptions, Scope, Unlocked, VAULT_FILE, Vault};

const PW: &str = "correct horse battery staple";

fn fast() -> KdfParams {
    KdfParams::with_cost(1024, 1, 1).expect("params")
}

fn new_vault() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("vault");
    Vault::init_with_kdf(&dir, PW, fast()).expect("init");
    (tmp, dir)
}

fn unlock(dir: &Path) -> Unlocked {
    Vault::open(dir).expect("open").unlock(PW).expect("unlock")
}

fn read_file(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join(VAULT_FILE)).expect("read")).expect("json")
}

fn write_file(dir: &Path, v: &Value) {
    std::fs::write(dir.join(VAULT_FILE), serde_json::to_vec(v).expect("json")).expect("write");
}

fn item_index(v: &Value, project: &str, name: &str) -> usize {
    v["items"]
        .as_array()
        .expect("items")
        .iter()
        .position(|i| i["project"] == project && i["name"] == name)
        .expect("item present")
}

#[test]
fn init_open_and_permissions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("nested").join("vault");
    assert!(!Vault::is_initialized(&dir));
    assert!(matches!(Vault::open(&dir), Err(Error::NotInitialized(_))));
    assert!(matches!(
        Vault::init_with_kdf(&dir, "short", fast()),
        Err(Error::PasswordTooShort { min: 12 })
    ));
    Vault::init_with_kdf(&dir, PW, fast()).expect("init");
    assert!(Vault::is_initialized(&dir));
    assert!(matches!(
        Vault::init_with_kdf(&dir, PW, fast()),
        Err(Error::AlreadyInitialized(_))
    ));

    let mode = |p: &Path| std::fs::metadata(p).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(VAULT_FILE)), 0o600);
    assert_eq!(mode(&dir.join("vault.lock")), 0o600);

    let mut u = unlock(&dir);
    u.set(&Scope::global(), "TOKEN", "v").expect("set");
    assert_eq!(mode(&dir.join(VAULT_FILE)), 0o600, "rewrite keeps 0600");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .expect("ls")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "atomic write leaves no temp files");
}

#[test]
fn production_kdf_init_and_unlock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    Vault::init(tmp.path(), PW).expect("init");
    let v = read_file(tmp.path());
    assert_eq!(v["kdf"]["memory_kib"], 65536);
    assert_eq!(v["kdf"]["iterations"], 3);
    Vault::open(tmp.path())
        .expect("open")
        .unlock(PW)
        .expect("unlock");
}

#[test]
fn wrong_password_is_rejected() {
    let (_t, dir) = new_vault();
    let v = Vault::open(&dir).expect("open");
    assert!(matches!(
        v.unlock("wrong password!!"),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(v.unlock(""), Err(Error::WrongPassword)));
    v.unlock(PW).expect("right password");
}

#[test]
fn round_trip_versions_list_delete() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    let thinai = Scope::new("thinai", "prod");
    u.set(&thinai, "OPENAI_API_KEY", "sk-1").expect("set");
    u.set(&Scope::global(), "GITHUB_TOKEN", "ghp-தமிழ்")
        .expect("set");
    u.set(&thinai, "EMPTY", "").expect("set empty");
    assert_eq!(*u.get(&thinai, "OPENAI_API_KEY").expect("get"), "sk-1");
    assert_eq!(*u.get(&thinai, "EMPTY").expect("get"), "");

    u.set(&thinai, "OPENAI_API_KEY", "sk-2").expect("update");
    let metas = u.list(Some(&thinai));
    assert_eq!(metas.len(), 2);
    let m = metas
        .iter()
        .find(|m| m.name == "OPENAI_API_KEY")
        .expect("meta");
    assert_eq!(m.version, 2);
    assert_eq!(m.scope, thinai);
    assert!(m.updated_at >= m.created_at);
    assert_eq!(m.expires_at, None);
    assert!(!m.rotation_due);
    assert_eq!(u.list(None).len(), 3);
    assert_eq!(u.list(None)[0].scope, Scope::global(), "sorted by scope");

    // Persisted: a fresh unlock sees the new value.
    let u2 = unlock(&dir);
    assert_eq!(*u2.get(&thinai, "OPENAI_API_KEY").expect("get"), "sk-2");
    assert_eq!(
        *u2.get(&Scope::global(), "GITHUB_TOKEN").expect("get"),
        "ghp-தமிழ்"
    );

    // `get` is exact-scope; the global token is not in thinai/prod.
    assert!(matches!(
        u.get(&thinai, "GITHUB_TOKEN"),
        Err(Error::NotFound { .. })
    ));
    u.delete(&thinai, "OPENAI_API_KEY").expect("delete");
    assert!(matches!(
        u.get(&thinai, "OPENAI_API_KEY"),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        u.delete(&thinai, "OPENAI_API_KEY"),
        Err(Error::NotFound { .. })
    ));

    // No plaintext on disk.
    let raw = std::fs::read_to_string(dir.join(VAULT_FILE)).expect("read");
    assert!(!raw.contains("ghp-"));
    assert!(!raw.contains(&hex::encode("ghp-")));
}

#[test]
fn invalid_names_and_scopes() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    assert!(matches!(
        u.set(&Scope::global(), "NOT-AN-ENV", "x"),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(
        u.set(&Scope::new("a|b", "prod"), "X", "x"),
        Err(Error::InvalidScope(_))
    ));
    assert!(matches!(
        u.set(
            &Scope::global(),
            "BIG",
            &"x".repeat(tp_vault::MAX_VALUE_BYTES + 1)
        ),
        Err(Error::ValueTooLarge { .. })
    ));
}

#[test]
fn env_for_layers_scopes() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    u.set(&Scope::global(), "A", "global-default").expect("set");
    u.set(&Scope::global(), "B", "global-default").expect("set");
    u.set(&Scope::global(), "C", "global-default").expect("set");
    u.set(&Scope::global(), "D", "global-default").expect("set");
    u.set(&Scope::new("global", "prod"), "B", "global-prod")
        .expect("set");
    u.set(&Scope::project("thinai"), "C", "thinai-default")
        .expect("set");
    u.set(&Scope::new("thinai", "prod"), "D", "thinai-prod")
        .expect("set");
    u.set(&Scope::new("kavi", "prod"), "E", "kavi-prod")
        .expect("set");

    let env = u.env_for(&Scope::new("thinai", "prod")).expect("env");
    let got: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        got,
        vec![
            ("A", "global-default"),
            ("B", "global-prod"),
            ("C", "thinai-default"),
            ("D", "thinai-prod"),
        ]
    );
    assert_eq!(
        *u.resolve(&Scope::new("thinai", "prod"), "C")
            .expect("resolve"),
        "thinai-default"
    );
    assert!(matches!(
        u.resolve(&Scope::new("thinai", "prod"), "E"),
        Err(Error::NotFound { .. })
    ));

    let dev = u.env_for(&Scope::new("thinai", "dev")).expect("env");
    let names: Vec<&str> = dev.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, vec!["A", "B", "C", "D"]);
    assert_eq!(
        *dev[1].1, "global-default",
        "global/prod does not apply to dev"
    );
}

#[test]
fn expiry_and_rotation() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    let s = Scope::project("thinai");
    let past = OffsetDateTime::now_utc() - time::Duration::hours(1);
    u.set(&Scope::global(), "KEY", "global").expect("set");
    u.set_with(
        &s,
        "KEY",
        "expired",
        ItemOptions {
            expires_at: Some(past),
            rotate_after: None,
        },
    )
    .expect("set");
    assert!(matches!(u.get(&s, "KEY"), Err(Error::Expired { .. })));
    assert!(matches!(u.resolve(&s, "KEY"), Err(Error::Expired { .. })));
    // An expired project key hides the global one rather than falling back.
    assert!(u.env_for(&s).expect("env").is_empty());
    let m = &u.list(Some(&s))[0];
    assert!(m.rotation_due);
    assert_eq!(m.expires_at, Some(past));

    // Rotation policy: due immediately with rotate_after = 0, kept across set().
    u.set_with(
        &s,
        "ROTATE",
        "v1",
        ItemOptions {
            expires_at: None,
            rotate_after: Some(Duration::ZERO),
        },
    )
    .expect("set");
    let due = |u: &Unlocked, n: &str| {
        u.list(Some(&s))
            .into_iter()
            .find(|m| m.name == n)
            .expect("meta")
            .rotation_due
    };
    assert!(due(&u, "ROTATE"));
    u.set(&s, "ROTATE", "v2").expect("rotate");
    assert!(due(&u, "ROTATE"), "rotate_after kept");
    u.set_with(
        &s,
        "LONG",
        "v",
        ItemOptions {
            expires_at: None,
            rotate_after: Some(Duration::from_secs(90 * 86400)),
        },
    )
    .expect("set");
    assert!(!due(&u, "LONG"));

    // set() clears expiry: the new value is served again.
    u.set(&s, "KEY", "fresh").expect("set");
    assert_eq!(*u.get(&s, "KEY").expect("get"), "fresh");
    assert!(!due(&u, "KEY"));
}

#[test]
fn tampered_ciphertext_is_detected() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    u.set(&Scope::global(), "A", "secret-a").expect("set");
    let mut v = read_file(&dir);
    let i = item_index(&v, "global", "A");
    let blob = v["items"][i]["blob"].as_str().expect("blob").to_string();
    let mut bytes = hex::decode(&blob).expect("hex");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    v["items"][i]["blob"] = Value::String(hex::encode(bytes));
    write_file(&dir, &v);
    let u = unlock(&dir);
    assert!(matches!(
        u.get(&Scope::global(), "A"),
        Err(Error::Tampered { .. })
    ));
    assert!(matches!(
        u.env_for(&Scope::global()),
        Err(Error::Tampered { .. })
    ));
}

#[test]
fn aad_binds_item_scope_name_and_version() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    let prod = Scope::new("thinai", "prod");
    u.set(&prod, "A", "secret-a").expect("set");
    u.set(&prod, "B", "secret-b").expect("set");
    let original = read_file(&dir);
    let ia = item_index(&original, "thinai", "A");
    let ib = item_index(&original, "thinai", "B");

    // 1. Swap ciphertexts between two items.
    let mut v = original.clone();
    let (ba, bb) = (
        v["items"][ia]["blob"].clone(),
        v["items"][ib]["blob"].clone(),
    );
    v["items"][ia]["blob"] = bb;
    v["items"][ib]["blob"] = ba;
    write_file(&dir, &v);
    let u2 = unlock(&dir);
    assert!(matches!(u2.get(&prod, "A"), Err(Error::Tampered { .. })));
    assert!(matches!(u2.get(&prod, "B"), Err(Error::Tampered { .. })));

    // 2. Move an item to another scope (e.g. to leak it into a leased one).
    let mut v = original.clone();
    v["items"][ia]["project"] = Value::String("kavi".into());
    write_file(&dir, &v);
    let u2 = unlock(&dir);
    assert!(matches!(
        u2.get(&Scope::new("kavi", "prod"), "A"),
        Err(Error::Tampered { .. })
    ));
    let mut v = original.clone();
    v["items"][ia]["environment"] = Value::String("dev".into());
    write_file(&dir, &v);
    assert!(matches!(
        unlock(&dir).get(&Scope::new("thinai", "dev"), "A"),
        Err(Error::Tampered { .. })
    ));

    // 3. Rename an item.
    let mut v = original.clone();
    v["items"][ia]["name"] = Value::String("C".into());
    write_file(&dir, &v);
    assert!(matches!(
        unlock(&dir).get(&prod, "C"),
        Err(Error::Tampered { .. })
    ));

    // 4. Roll a blob back to an older version under the current version number.
    write_file(&dir, &original);
    let mut u3 = unlock(&dir);
    u3.set(&prod, "A", "secret-a2").expect("update");
    let mut v = read_file(&dir);
    let ia2 = item_index(&v, "thinai", "A");
    v["items"][ia2]["blob"] = original["items"][ia]["blob"].clone();
    write_file(&dir, &v);
    assert!(matches!(
        unlock(&dir).get(&prod, "A"),
        Err(Error::Tampered { .. })
    ));

    // Untouched file still works.
    write_file(&dir, &original);
    assert_eq!(*unlock(&dir).get(&prod, "A").expect("get"), "secret-a");
}

#[test]
fn corrupt_vault_file_is_reported() {
    let (_t, dir) = new_vault();
    std::fs::write(dir.join(VAULT_FILE), b"{nope").expect("write");
    assert!(matches!(Vault::open(&dir), Err(Error::Corrupt { .. })));
}

#[test]
fn wrap_unlock_add_remove() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    u.set(&Scope::project("thinai"), "KEY", "via-phone")
        .expect("set");
    let secret = *tp_vault::crypto::random_key().expect("key");
    u.add_wrap("phone:pixel", &secret).expect("add wrap");
    assert!(matches!(
        u.add_wrap("phone:pixel", &secret),
        Err(Error::WrapExists(_))
    ));
    assert!(matches!(
        u.add_wrap("master", &secret),
        Err(Error::InvalidWrapLabel(_))
    ));
    assert!(matches!(
        u.add_wrap("bad label", &secret),
        Err(Error::InvalidWrapLabel(_))
    ));
    assert_eq!(u.wraps(), vec!["phone:pixel".to_string()]);

    let v = Vault::open(&dir).expect("open");
    assert_eq!(v.wraps(), vec!["phone:pixel".to_string()]);
    let pu = v
        .unlock_with_wrap("phone:pixel", &secret)
        .expect("wrap unlock");
    assert_eq!(
        *pu.get(&Scope::project("thinai"), "KEY").expect("get"),
        "via-phone"
    );
    assert!(matches!(
        v.unlock_with_wrap("phone:pixel", &[0u8; 32]),
        Err(Error::WrongWrapSecret(_))
    ));
    assert!(matches!(
        v.unlock_with_wrap("phone:other", &secret),
        Err(Error::UnknownWrap(_))
    ));

    // A wrap moved to another label does not open (label is in the AAD).
    let mut file = read_file(&dir);
    let entry = file["wraps"]["phone:pixel"].clone();
    file["wraps"]["phone:moved"] = entry;
    write_file(&dir, &file);
    assert!(matches!(
        Vault::open(&dir)
            .expect("open")
            .unlock_with_wrap("phone:moved", &secret),
        Err(Error::WrongWrapSecret(_))
    ));

    let mut u = unlock(&dir);
    u.remove_wrap("phone:moved").expect("remove");
    u.remove_wrap("phone:pixel").expect("remove");
    assert!(matches!(
        u.remove_wrap("phone:pixel"),
        Err(Error::UnknownWrap(_))
    ));
    assert!(matches!(
        Vault::open(&dir)
            .expect("open")
            .unlock_with_wrap("phone:pixel", &secret),
        Err(Error::UnknownWrap(_))
    ));
    // Master password still works after wrap changes.
    assert_eq!(
        *unlock(&dir)
            .get(&Scope::project("thinai"), "KEY")
            .expect("get"),
        "via-phone"
    );
}

#[test]
fn concurrent_handles_do_not_lose_updates() {
    let (_t, dir) = new_vault();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut u = unlock(&dir);
                for j in 0..5 {
                    u.set(&Scope::global(), &format!("K_{i}_{j}"), "v")
                        .expect("set");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread");
    }
    let mut u = unlock(&dir);
    assert_eq!(u.list(None).len(), 40);
    u.refresh().expect("refresh");
}

#[test]
fn replaced_vault_is_detected() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    std::fs::remove_file(dir.join(VAULT_FILE)).expect("rm");
    Vault::init_with_kdf(&dir, PW, fast()).expect("re-init");
    assert!(matches!(
        u.set(&Scope::global(), "X", "y"),
        Err(Error::VaultReplaced)
    ));
    assert!(matches!(u.refresh(), Err(Error::VaultReplaced)));
}

#[test]
fn debug_output_has_no_secrets() {
    let (_t, dir) = new_vault();
    let mut u = unlock(&dir);
    u.set(&Scope::global(), "SECRET", "super-secret-value")
        .expect("set");
    let value = u.get(&Scope::global(), "SECRET").expect("get");
    let dbg = format!(
        "{u:?} {:?} {:?}",
        Vault::open(&dir).expect("open"),
        u.list(None)
    );
    assert!(!dbg.contains("super-secret-value"), "{dbg}");
    assert!(dbg.contains("<redacted>"));
    let err = format!(
        "{}",
        u.get(&Scope::global(), "MISSING").expect_err("missing")
    );
    assert!(
        err.contains("MISSING") && err.contains("global/default"),
        "{err}"
    );
    drop(value);
}
