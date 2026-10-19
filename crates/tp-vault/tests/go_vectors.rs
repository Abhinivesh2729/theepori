//! Byte-for-byte compatibility with Panuval's Go `internal/crypto`.
//!
//! - `fixtures/panuval_vectors.json` is Panuval's own `testvectors/vectors.json`.
//! - `fixtures/go_vectors.json` was produced by `fixtures/genfixtures.go`
//!   (random nonces, i.e. exactly what Panuval stores, plus a full
//!   password → KEK → DEK → item chain).

use serde_json::Value;
use tp_vault::Error;
use tp_vault::crypto::{self, KdfParams, Key, panuval};

const PANUVAL: &str = include_str!("fixtures/panuval_vectors.json");
const GO: &str = include_str!("fixtures/go_vectors.json");

fn h(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().expect("hex string")).expect("hex")
}

fn key(v: &Value) -> Key {
    h(v).try_into().expect("32-byte key")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn params(v: &Value) -> KdfParams {
    KdfParams {
        alg: "argon2id".into(),
        memory_kib: v["memoryKiB"].as_u64().expect("m") as u32,
        iterations: v["iterations"].as_u64().expect("t") as u32,
        parallelism: v["parallelism"].as_u64().expect("p") as u8,
        salt: h(&v["saltHex"]),
    }
}

#[test]
fn kdf_vectors_match_go() {
    let v: Value = serde_json::from_str(PANUVAL).expect("json");
    let kdf = v["kdf"].as_array().expect("kdf");
    assert_eq!(kdf.len(), 3);
    for k in kdf {
        // passwordUtf8Hex is the raw (possibly NFD) input; NFC happens inside.
        let pw = String::from_utf8(h(&k["passwordUtf8Hex"])).expect("utf8");
        let kek = crypto::derive_kek(&pw, &params(k)).expect("derive");
        assert_eq!(hex::encode(*kek), s(&k["kekHex"]), "kdf {}", s(&k["name"]));
    }
}

#[test]
fn nfc_and_nfd_derive_the_same_key() {
    let p = KdfParams {
        alg: "argon2id".into(),
        memory_kib: 1024,
        iterations: 1,
        parallelism: 1,
        salt: vec![7; 16],
    };
    let composed = crypto::derive_kek("caf\u{e9}", &p).expect("nfc");
    let decomposed = crypto::derive_kek("cafe\u{301}", &p).expect("nfd");
    assert_eq!(*composed, *decomposed);
}

#[test]
fn go_seal_vectors_decrypt() {
    let v: Value = serde_json::from_str(PANUVAL).expect("json");
    for sv in v["seal"].as_array().expect("seal") {
        let pt = crypto::open(
            &key(&sv["keyHex"]),
            &h(&sv["blobHex"]),
            s(&sv["aad"]).as_bytes(),
        )
        .expect("open");
        assert_eq!(*pt, h(&sv["plaintextHex"]), "open {}", s(&sv["name"]));
    }
    for a in v["itemAad"].as_array().expect("itemAad") {
        let aad = panuval::item_aad(
            s(&a["uid"]),
            s(&a["itemId"]),
            a["version"].as_i64().expect("version"),
        );
        assert_eq!(aad, s(&a["aad"]).as_bytes());
    }
}

#[test]
fn go_random_nonce_blobs_decrypt_and_detect_tampering() {
    let v: Value = serde_json::from_str(GO).expect("json");
    let open = v["open"].as_array().expect("open");
    assert_eq!(open.len(), 5);
    for ov in open {
        let name = s(&ov["name"]);
        let k = key(&ov["keyHex"]);
        let aad = h(&ov["aadHex"]);
        let blob = h(&ov["blobHex"]);
        let pt = crypto::open(&k, &blob, &aad).expect(name);
        assert_eq!(*pt, h(&ov["plaintextHex"]), "{name}");

        let mut flipped = blob.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert!(
            matches!(crypto::open(&k, &flipped, &aad), Err(Error::Decrypt)),
            "{name}: bit flip"
        );
        let mut other_aad = aad.clone();
        other_aad.push(b'x');
        assert!(
            matches!(crypto::open(&k, &blob, &other_aad), Err(Error::Decrypt)),
            "{name}: aad"
        );
    }
}

#[test]
fn go_key_chain_unwraps_and_decrypts_items() {
    let v: Value = serde_json::from_str(GO).expect("json");
    let c = &v["chain"];
    let kek = crypto::derive_kek(s(&c["password"]), &params(c)).expect("derive");
    assert_eq!(hex::encode(*kek), s(&c["kekHex"]));
    let dek = crypto::unwrap_key(&kek, &h(&c["wrappedDekHex"]), panuval::DEK_AAD).expect("unwrap");
    assert_eq!(hex::encode(*dek), s(&c["dekHex"]));

    let wrong = crypto::derive_kek("not the password", &params(c)).expect("derive");
    assert!(matches!(
        crypto::unwrap_key(&wrong, &h(&c["wrappedDekHex"]), panuval::DEK_AAD),
        Err(Error::Decrypt)
    ));

    let uid = s(&c["uid"]);
    let items = c["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    for it in items {
        let id = s(&it["itemId"]);
        let ver = it["version"].as_i64().expect("version");
        let blob = h(&it["blobHex"]);
        let pt = crypto::open(&dek, &blob, &panuval::item_aad(uid, id, ver)).expect("item");
        assert_eq!(*pt, h(&it["plaintextHex"]));
        // Rollback / swap / other-user bindings all fail.
        for bad in [
            panuval::item_aad(uid, id, ver + 1),
            panuval::item_aad(uid, "other-item", ver),
            panuval::item_aad("other-uid", id, ver),
        ] {
            assert!(matches!(
                crypto::open(&dek, &blob, &bad),
                Err(Error::Decrypt)
            ));
        }
    }
}

#[test]
fn rust_blobs_have_panuval_layout() {
    let k = crypto::random_key().expect("key");
    let aad = panuval::item_aad("u", "i", 1);
    let a = crypto::seal(&k, b"secret", &aad).expect("seal");
    let b = crypto::seal(&k, b"secret", &aad).expect("seal");
    assert_eq!(a[0], crypto::BLOB_VERSION);
    assert_eq!(a.len(), 1 + crypto::NONCE_SIZE + 6 + crypto::TAG_SIZE);
    assert_ne!(a, b, "nonces must be fresh");
    assert_eq!(*crypto::open(&k, &a, &aad).expect("open"), b"secret");
}

#[test]
fn rejects_malformed_input() {
    let k = crypto::random_key().expect("key");
    assert!(matches!(
        crypto::open(&k, &[1, 2, 3], b""),
        Err(Error::BadBlob)
    ));
    let mut blob = crypto::seal(&k, b"x", b"").expect("seal");
    blob[0] = 9;
    assert!(matches!(
        crypto::open(&k, &blob, b""),
        Err(Error::BadBlobVersion(9))
    ));
    let mut weak = KdfParams::with_cost(1024, 1, 1).expect("params");
    weak.salt.truncate(5);
    assert!(matches!(
        crypto::derive_kek("pw", &weak),
        Err(Error::BadKdf)
    ));
    let mut weak = KdfParams::with_cost(1024, 1, 1).expect("params");
    weak.alg = "argon2i".into();
    assert!(matches!(
        crypto::derive_kek("pw", &weak),
        Err(Error::BadKdf)
    ));
    let prod = KdfParams::production().expect("params");
    assert_eq!(
        (prod.memory_kib, prod.iterations, prod.parallelism),
        (65536, 3, 1)
    );
    assert_eq!(prod.salt.len(), crypto::SALT_SIZE);
}
