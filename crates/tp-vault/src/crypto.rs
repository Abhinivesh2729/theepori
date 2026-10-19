//! Panuval-compatible primitives.
//!
//! ```text
//! master password ──Argon2id(NFC(pw), salt, t, m KiB, p, 32)──▶ KEK
//! KEK ──AES-256-GCM wrap──▶ DEK (random 32 bytes)
//! DEK ──AES-256-GCM(aad = item binding)──▶ item ciphertext
//! ```
//!
//! Blob format (every ciphertext): `0x01 || nonce(12) || ciphertext || tag(16)`.
//! This is byte-for-byte the format of Panuval's Go `internal/crypto`; the
//! fixtures in `tests/fixtures` prove it.

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

pub const KEY_SIZE: usize = 32;
pub const SALT_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 12;
pub const TAG_SIZE: usize = 16;
pub const BLOB_VERSION: u8 = 1;

/// A 32-byte AES-256 key.
pub type Key = [u8; KEY_SIZE];

/// Argon2id parameters, stored in plaintext next to the wrapped DEK.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub alg: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u8,
    #[serde(with = "hex_bytes")]
    pub salt: Vec<u8>,
}

impl KdfParams {
    /// Panuval's production parameters (64 MiB, t=3, p=1) with a fresh salt.
    pub fn production() -> Result<Self> {
        Self::with_cost(64 * 1024, 3, 1)
    }

    /// Custom cost with a fresh random salt (tests use cheap parameters).
    pub fn with_cost(memory_kib: u32, iterations: u32, parallelism: u8) -> Result<Self> {
        let mut salt = vec![0u8; SALT_SIZE];
        fill_random(&mut salt)?;
        Ok(Self {
            alg: "argon2id".into(),
            memory_kib,
            iterations,
            parallelism,
            salt,
        })
    }

    /// Same rules as Panuval's `KDFParams.validate`, plus upper bounds so a
    /// hostile vault file cannot make unlock exhaust memory or CPU.
    pub fn validate(&self) -> Result<()> {
        let p = u32::from(self.parallelism);
        if self.alg != "argon2id"
            || !(1..=64).contains(&self.iterations)
            || !(1..=64).contains(&p)
            || self.memory_kib < 8 * p
            || self.memory_kib > 4 * 1024 * 1024
            || self.salt.len() < SALT_SIZE
        {
            return Err(Error::BadKdf);
        }
        Ok(())
    }
}

/// Derives the key-encryption key. The password is NFC-normalised so the same
/// characters typed on different platforms give the same key.
pub fn derive_kek(password: &str, p: &KdfParams) -> Result<Zeroizing<Key>> {
    p.validate()?;
    let pw: Zeroizing<String> = Zeroizing::new(password.nfc().collect());
    let params = Params::new(
        p.memory_kib,
        p.iterations,
        u32::from(p.parallelism),
        Some(KEY_SIZE),
    )
    .map_err(|_| Error::BadKdf)?;
    let mut out = Zeroizing::new([0u8; KEY_SIZE]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pw.as_bytes(), &p.salt, out.as_mut())
        .map_err(|_| Error::BadKdf)?;
    Ok(out)
}

/// Encrypts with AES-256-GCM under a fresh random nonce.
pub fn seal(key: &Key, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_SIZE];
    fill_random(&mut nonce)?;
    seal_with_nonce(key, &nonce, plaintext, aad)
}

/// Deterministic sealing for test vectors only. Reusing a nonce under the same
/// key breaks GCM, so this is not public.
pub(crate) fn seal_with_nonce(
    key: &Key,
    nonce: &[u8; NONCE_SIZE],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::BadKeySize)?;
    let ct = cipher
        .encrypt(
            &(*nonce).into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Decrypt)?;
    let mut out = Vec::with_capacity(1 + NONCE_SIZE + ct.len());
    out.push(BLOB_VERSION);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypts a blob produced by [`seal`] (or by Panuval's Go `crypto.Seal`).
pub fn open(key: &Key, blob: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < 1 + NONCE_SIZE + TAG_SIZE {
        return Err(Error::BadBlob);
    }
    if blob[0] != BLOB_VERSION {
        return Err(Error::BadBlobVersion(blob[0]));
    }
    let mut nonce = [0u8; NONCE_SIZE];
    nonce.copy_from_slice(&blob[1..1 + NONCE_SIZE]);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::BadKeySize)?;
    cipher
        .decrypt(
            &nonce.into(),
            Payload {
                msg: &blob[1 + NONCE_SIZE..],
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| Error::Decrypt)
}

/// Wraps a DEK under a KEK.
pub fn wrap_key(kek: &Key, dek: &Key, aad: &[u8]) -> Result<Vec<u8>> {
    seal(kek, dek, aad)
}

/// Unwraps a DEK; [`Error::Decrypt`] means the wrapping key is wrong.
pub fn unwrap_key(kek: &Key, wrapped: &[u8], aad: &[u8]) -> Result<Zeroizing<Key>> {
    let pt = open(kek, wrapped, aad)?;
    let mut key = Zeroizing::new([0u8; KEY_SIZE]);
    if pt.len() != KEY_SIZE {
        return Err(Error::BadKeySize);
    }
    key.copy_from_slice(&pt);
    Ok(key)
}

/// A fresh random 32-byte key.
pub fn random_key() -> Result<Zeroizing<Key>> {
    let mut k = Zeroizing::new([0u8; KEY_SIZE]);
    fill_random(k.as_mut())?;
    Ok(k)
}

pub(crate) fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|e| Error::Random(e.to_string()))
}

/// Panuval's associated-data constants, for reading Panuval data.
pub mod panuval {
    /// AAD for the DEK wrapped under the master-password KEK.
    pub const DEK_AAD: &[u8] = b"panuval:dek:v1";

    /// `panuval:item:v1|<uid>|<itemId>|<version>`, as Go `crypto.ItemAAD`.
    pub fn item_aad(uid: &str, item_id: &str, version: i64) -> Vec<u8> {
        format!("panuval:item:v1|{uid}|{item_id}|{version}").into_bytes()
    }
}

pub(crate) mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(b))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panuval's own `testvectors/vectors.json`, copied verbatim.
    const VECTORS: &str = include_str!("../tests/fixtures/panuval_vectors.json");

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s).expect("hex")
    }

    fn key(s: &str) -> Key {
        h(s).try_into().expect("32-byte key")
    }

    #[test]
    fn go_seal_vectors_are_reproduced_byte_for_byte() {
        let v: serde_json::Value = serde_json::from_str(VECTORS).expect("json");
        let seals = v["seal"].as_array().expect("seal");
        assert_eq!(seals.len(), 3);
        for s in seals {
            let name = s["name"].as_str().expect("name");
            let nonce: [u8; NONCE_SIZE] = h(s["nonceHex"].as_str().expect("nonce"))
                .try_into()
                .expect("nonce len");
            let blob = seal_with_nonce(
                &key(s["keyHex"].as_str().expect("key")),
                &nonce,
                &h(s["plaintextHex"].as_str().expect("pt")),
                s["aad"].as_str().expect("aad").as_bytes(),
            )
            .expect("seal");
            assert_eq!(
                hex::encode(blob),
                s["blobHex"].as_str().expect("blob"),
                "seal {name}"
            );
        }
    }
}
