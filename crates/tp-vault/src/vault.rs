//! The on-disk vault and its unlocked handle.
//!
//! `<dir>/vault.json` (0600, replaced atomically) holds only ciphertext and
//! metadata:
//!
//! ```text
//! kdf          Argon2id params + salt          (plaintext)
//! wrapped_dek  seal(KEK, DEK, "tp-vault:dek:v1")
//! wraps        label -> seal(U, DEK, "tp-vault:wrap:v1|<label>")   (phone unlock)
//! items[]      id, project, environment, name, version, timestamps,
//!              blob = seal(DEK, value, "tp-vault:item:v1|<id>|<project>|<env>|<name>|<version>")
//! ```
//!
//! Scope and name are in plaintext so listing needs no decryption, but they
//! are bound into every item's AAD: moving a blob to another item, scope,
//! name or older version makes decryption fail.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zeroize::{Zeroize, Zeroizing};

use crate::crypto::{self, KdfParams, Key, hex_bytes};
use crate::error::{Error, Result, io};
use crate::fsutil::{DirLock, ensure_private_dir, write_atomic};
use crate::scope::{Scope, validate_name};

/// File name of the vault inside its directory.
pub const VAULT_FILE: &str = "vault.json";
/// Minimum master password length in characters (same as Panuval).
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Largest secret value accepted.
pub const MAX_VALUE_BYTES: usize = 1 << 20;

const FORMAT: &str = "tp-vault";
const FORMAT_VERSION: u32 = 1;
const DEK_AAD: &[u8] = b"tp-vault:dek:v1";

fn wrap_aad(label: &str) -> Vec<u8> {
    format!("tp-vault:wrap:v1|{label}").into_bytes()
}

fn item_aad(id: &str, scope: &Scope, name: &str, version: u64) -> Vec<u8> {
    format!(
        "tp-vault:item:v1|{id}|{}|{}|{name}|{version}",
        scope.project, scope.environment
    )
    .into_bytes()
}

#[derive(Clone, Serialize, Deserialize)]
struct VaultFile {
    format: String,
    version: u32,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    kdf: KdfParams,
    #[serde(with = "hex_bytes")]
    wrapped_dek: Vec<u8>,
    #[serde(default)]
    wraps: BTreeMap<String, WrapEntry>,
    #[serde(default)]
    items: Vec<StoredItem>,
}

#[derive(Clone, Serialize, Deserialize)]
struct WrapEntry {
    #[serde(with = "hex_bytes")]
    wrapped_dek: Vec<u8>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredItem {
    id: String,
    project: String,
    environment: String,
    name: String,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
    #[serde(default)]
    rotate_after_secs: Option<u64>,
    #[serde(with = "hex_bytes")]
    blob: Vec<u8>,
}

impl StoredItem {
    fn scope(&self) -> Scope {
        Scope::new(self.project.clone(), self.environment.clone())
    }

    fn is(&self, scope: &Scope, name: &str) -> bool {
        self.project == scope.project && self.environment == scope.environment && self.name == name
    }

    fn expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|e| e <= now)
    }

    fn rotation_due(&self, now: OffsetDateTime) -> bool {
        self.expired(now)
            || self.rotate_after_secs.is_some_and(|secs| {
                let due = i64::try_from(secs)
                    .ok()
                    .and_then(|s| self.updated_at.checked_add(time::Duration::seconds(s)));
                due.is_some_and(|d| d <= now)
            })
    }

    fn meta(&self, now: OffsetDateTime) -> ItemMeta {
        ItemMeta {
            scope: self.scope(),
            name: self.name.clone(),
            version: self.version,
            created_at: self.created_at,
            updated_at: self.updated_at,
            expires_at: self.expires_at,
            rotation_due: self.rotation_due(now),
        }
    }
}

/// Metadata about one secret. Never contains the value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMeta {
    pub scope: Scope,
    pub name: String,
    pub version: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// True when `expires_at` has passed or `rotate_after` has elapsed since
    /// the value was last set.
    pub rotation_due: bool,
}

/// Lifecycle options for [`Unlocked::set_with`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemOptions {
    /// After this instant the secret is no longer served.
    pub expires_at: Option<OffsetDateTime>,
    /// Flag the secret as due for rotation this long after it was set.
    pub rotate_after: Option<Duration>,
}

fn now() -> OffsetDateTime {
    let t = OffsetDateTime::now_utc();
    t.replace_nanosecond(0).unwrap_or(t)
}

fn new_id() -> Result<String> {
    let mut b = [0u8; 16];
    crypto::fill_random(&mut b)?;
    Ok(hex::encode(b))
}

fn vault_path(dir: &Path) -> PathBuf {
    dir.join(VAULT_FILE)
}

fn validate_wrap_label(label: &str) -> Result<()> {
    let ok = !label.is_empty()
        && label.len() <= 64
        && label != "master"
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidWrapLabel(label.to_string()))
    }
}

fn load(dir: &Path) -> Result<VaultFile> {
    let path = vault_path(dir);
    let raw = match std::fs::read(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::NotInitialized(dir.to_path_buf()));
        }
        Err(e) => return Err(io(format!("read {}", path.display()))(e)),
    };
    let corrupt = |reason: String| Error::Corrupt {
        path: path.clone(),
        reason,
    };
    let f: VaultFile = serde_json::from_slice(&raw).map_err(|e| corrupt(e.to_string()))?;
    if f.format != FORMAT || f.version != FORMAT_VERSION {
        return Err(corrupt(format!(
            "unsupported format {} v{} (expected {FORMAT} v{FORMAT_VERSION})",
            f.format, f.version
        )));
    }
    f.kdf.validate()?;
    let mut seen = std::collections::HashSet::new();
    for it in &f.items {
        it.scope()
            .validate()
            .and_then(|()| validate_name(&it.name))
            .map_err(|e| corrupt(e.to_string()))?;
        if it.version == 0 || !seen.insert((it.scope(), it.name.clone())) {
            return Err(corrupt(format!(
                "duplicate or invalid entry for {} in {}",
                it.name,
                it.scope()
            )));
        }
    }
    for label in f.wraps.keys() {
        validate_wrap_label(label).map_err(|e| corrupt(e.to_string()))?;
    }
    Ok(f)
}

fn save(dir: &Path, f: &VaultFile) -> Result<()> {
    let raw = serde_json::to_vec_pretty(f).map_err(|e| Error::Corrupt {
        path: vault_path(dir),
        reason: e.to_string(),
    })?;
    write_atomic(&vault_path(dir), &raw)
}

/// A vault on disk, not yet unlocked.
pub struct Vault {
    dir: PathBuf,
    file: VaultFile,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("dir", &self.dir)
            .field("items", &self.file.items.len())
            .field("wraps", &self.file.wraps.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Vault {
    /// Creates a vault in `dir` (created 0700) with Panuval's production KDF
    /// parameters (Argon2id, 64 MiB, t=3, p=1).
    pub fn init(dir: &Path, master_password: &str) -> Result<()> {
        Self::init_with_kdf(dir, master_password, KdfParams::production()?)
    }

    /// [`Vault::init`] with explicit KDF parameters (tests use cheap ones).
    pub fn init_with_kdf(dir: &Path, master_password: &str, kdf: KdfParams) -> Result<()> {
        if master_password.chars().count() < MIN_PASSWORD_CHARS {
            return Err(Error::PasswordTooShort {
                min: MIN_PASSWORD_CHARS,
            });
        }
        kdf.validate()?;
        if Self::is_initialized(dir) {
            return Err(Error::AlreadyInitialized(dir.to_path_buf()));
        }
        ensure_private_dir(dir)?;
        let _lock = DirLock::acquire(dir)?;
        if Self::is_initialized(dir) {
            return Err(Error::AlreadyInitialized(dir.to_path_buf()));
        }
        let kek = crypto::derive_kek(master_password, &kdf)?;
        let dek = crypto::random_key()?;
        let wrapped_dek = crypto::wrap_key(&kek, &dek, DEK_AAD)?;
        let file = VaultFile {
            format: FORMAT.into(),
            version: FORMAT_VERSION,
            created_at: now(),
            kdf,
            wrapped_dek,
            wraps: BTreeMap::new(),
            items: Vec::new(),
        };
        save(dir, &file)
    }

    /// Reads the vault header and items (all ciphertext).
    pub fn open(dir: &Path) -> Result<Vault> {
        Ok(Vault {
            dir: dir.to_path_buf(),
            file: load(dir)?,
        })
    }

    pub fn is_initialized(dir: &Path) -> bool {
        vault_path(dir).is_file()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Labels of the extra DEK wraps (e.g. paired phones).
    pub fn wraps(&self) -> Vec<String> {
        self.file.wraps.keys().cloned().collect()
    }

    /// Unlocks with the master password.
    pub fn unlock(&self, password: &str) -> Result<Unlocked> {
        let kek = crypto::derive_kek(password, &self.file.kdf)?;
        let dek =
            crypto::unwrap_key(&kek, &self.file.wrapped_dek, DEK_AAD).map_err(|e| match e {
                Error::Decrypt => Error::WrongPassword,
                e => e,
            })?;
        Ok(Unlocked {
            dir: self.dir.clone(),
            dek,
            file: self.file.clone(),
        })
    }

    /// Unlocks with a 32-byte secret previously registered by
    /// [`Unlocked::add_wrap`] (the phone's unlock secret U).
    pub fn unlock_with_wrap(&self, label: &str, secret: &[u8; 32]) -> Result<Unlocked> {
        let entry = self
            .file
            .wraps
            .get(label)
            .ok_or_else(|| Error::UnknownWrap(label.to_string()))?;
        let dek = crypto::unwrap_key(secret, &entry.wrapped_dek, &wrap_aad(label)).map_err(
            |e| match e {
                Error::Decrypt => Error::WrongWrapSecret(label.to_string()),
                e => e,
            },
        )?;
        Ok(Unlocked {
            dir: self.dir.clone(),
            dek,
            file: self.file.clone(),
        })
    }
}

/// An unlocked vault: holds the DEK (wiped on drop) and a cached copy of the
/// vault file. Writes take the directory lock, re-read the file, apply the
/// change and replace the file atomically; reads use the cache (call
/// [`Unlocked::refresh`] to pick up writes from another process).
pub struct Unlocked {
    dir: PathBuf,
    dek: Zeroizing<Key>,
    file: VaultFile,
}

impl fmt::Debug for Unlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unlocked")
            .field("dir", &self.dir)
            .field("dek", &"<redacted>")
            .field("items", &self.file.items.len())
            .finish()
    }
}

impl Unlocked {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Re-reads the vault file.
    pub fn refresh(&mut self) -> Result<()> {
        let f = load(&self.dir)?;
        if f.wrapped_dek != self.file.wrapped_dek {
            return Err(Error::VaultReplaced);
        }
        self.file = f;
        Ok(())
    }

    fn mutate<R>(&mut self, op: impl FnOnce(&mut VaultFile, &Key) -> Result<R>) -> Result<R> {
        let _lock = DirLock::acquire(&self.dir)?;
        let mut f = load(&self.dir)?;
        if f.wrapped_dek != self.file.wrapped_dek {
            return Err(Error::VaultReplaced);
        }
        let r = op(&mut f, &self.dek)?;
        save(&self.dir, &f)?;
        self.file = f;
        Ok(r)
    }

    /// Stores `value` as `name` in `scope`, creating it or bumping its
    /// version. An existing item keeps its `rotate_after` policy; its
    /// `expires_at` is cleared because it described the old value.
    pub fn set(&mut self, scope: &Scope, name: &str, value: &str) -> Result<()> {
        let rotate_after = self
            .find(scope, name)
            .and_then(|it| it.rotate_after_secs)
            .map(Duration::from_secs);
        self.set_with(
            scope,
            name,
            value,
            ItemOptions {
                expires_at: None,
                rotate_after,
            },
        )
    }

    /// [`Unlocked::set`] with explicit lifecycle options (replacing any
    /// existing ones).
    pub fn set_with(
        &mut self,
        scope: &Scope,
        name: &str,
        value: &str,
        opts: ItemOptions,
    ) -> Result<()> {
        scope.validate()?;
        validate_name(name)?;
        if value.len() > MAX_VALUE_BYTES {
            return Err(Error::ValueTooLarge {
                len: value.len(),
                limit: MAX_VALUE_BYTES,
            });
        }
        let ts = now();
        let rotate_after_secs = opts.rotate_after.map(|d| d.as_secs());
        self.mutate(|f, dek| {
            if let Some(it) = f.items.iter_mut().find(|it| it.is(scope, name)) {
                let version = it.version + 1;
                it.blob = crypto::seal(
                    dek,
                    value.as_bytes(),
                    &item_aad(&it.id, scope, name, version),
                )?;
                it.version = version;
                it.updated_at = ts;
                it.expires_at = opts.expires_at;
                it.rotate_after_secs = rotate_after_secs;
            } else {
                let id = new_id()?;
                let blob = crypto::seal(dek, value.as_bytes(), &item_aad(&id, scope, name, 1))?;
                f.items.push(StoredItem {
                    id,
                    project: scope.project.clone(),
                    environment: scope.environment.clone(),
                    name: name.to_string(),
                    version: 1,
                    created_at: ts,
                    updated_at: ts,
                    expires_at: opts.expires_at,
                    rotate_after_secs,
                    blob,
                });
            }
            Ok(())
        })
    }

    fn find(&self, scope: &Scope, name: &str) -> Option<&StoredItem> {
        self.file.items.iter().find(|it| it.is(scope, name))
    }

    fn decrypt(&self, it: &StoredItem) -> Result<Zeroizing<String>> {
        let scope = it.scope();
        let mut pt = crypto::open(
            &self.dek,
            &it.blob,
            &item_aad(&it.id, &scope, &it.name, it.version),
        )
        .map_err(|_| Error::Tampered {
            scope: scope.clone(),
            name: it.name.clone(),
        })?;
        match String::from_utf8(std::mem::take(&mut *pt)) {
            Ok(s) => Ok(Zeroizing::new(s)),
            Err(e) => {
                e.into_bytes().zeroize();
                Err(Error::Tampered {
                    scope,
                    name: it.name.clone(),
                })
            }
        }
    }

    /// The value of `name` in exactly `scope` (no layering; see
    /// [`Unlocked::env_for`]). Expired secrets are not served.
    pub fn get(&self, scope: &Scope, name: &str) -> Result<Zeroizing<String>> {
        let it = self.find(scope, name).ok_or_else(|| Error::NotFound {
            scope: scope.clone(),
            name: name.to_string(),
        })?;
        if it.expired(OffsetDateTime::now_utc()) {
            return Err(Error::Expired {
                scope: scope.clone(),
                name: name.to_string(),
            });
        }
        self.decrypt(it)
    }

    /// The effective value of `name` for a process in `scope`: the most
    /// specific layer that has it (same rules as [`Unlocked::env_for`]).
    pub fn resolve(&self, scope: &Scope, name: &str) -> Result<Zeroizing<String>> {
        scope.validate()?;
        let layers = scope.layers();
        let it = layers
            .iter()
            .rev()
            .find_map(|layer| self.find(layer, name))
            .ok_or_else(|| Error::NotFound {
                scope: scope.clone(),
                name: name.to_string(),
            })?;
        if it.expired(OffsetDateTime::now_utc()) {
            return Err(Error::Expired {
                scope: it.scope(),
                name: name.to_string(),
            });
        }
        self.decrypt(it)
    }

    /// Metadata of every secret, or of one scope, sorted by scope then name.
    pub fn list(&self, scope: Option<&Scope>) -> Vec<ItemMeta> {
        let now = OffsetDateTime::now_utc();
        let mut out: Vec<ItemMeta> = self
            .file
            .items
            .iter()
            .filter(|it| {
                scope.is_none_or(|s| it.project == s.project && it.environment == s.environment)
            })
            .map(|it| it.meta(now))
            .collect();
        out.sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));
        out
    }

    /// Deletes `name` from `scope`.
    pub fn delete(&mut self, scope: &Scope, name: &str) -> Result<()> {
        self.mutate(|f, _| {
            let before = f.items.len();
            f.items.retain(|it| !it.is(scope, name));
            if f.items.len() == before {
                return Err(Error::NotFound {
                    scope: scope.clone(),
                    name: name.to_string(),
                });
            }
            Ok(())
        })
    }

    /// The environment for a process running in `scope`, sorted by name.
    ///
    /// Layers (see [`Scope::layers`]) are applied least specific first, so a
    /// project secret overrides a `global` one of the same name. An expired
    /// secret is left out and also hides less specific values of that name,
    /// so an expired project key never silently falls back to a global one.
    pub fn env_for(&self, scope: &Scope) -> Result<Vec<(String, Zeroizing<String>)>> {
        scope.validate()?;
        let now = OffsetDateTime::now_utc();
        let mut merged: BTreeMap<String, Option<Zeroizing<String>>> = BTreeMap::new();
        for layer in scope.layers() {
            for it in self
                .file
                .items
                .iter()
                .filter(|it| it.project == layer.project && it.environment == layer.environment)
            {
                let v = if it.expired(now) {
                    None
                } else {
                    Some(self.decrypt(it)?)
                };
                merged.insert(it.name.clone(), v);
            }
        }
        Ok(merged
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect())
    }

    /// Labels of the extra DEK wraps.
    pub fn wraps(&self) -> Vec<String> {
        self.file.wraps.keys().cloned().collect()
    }

    /// Wraps the DEK under a random 32-byte `secret` (e.g. the phone's unlock
    /// secret U), so [`Vault::unlock_with_wrap`] can unlock without the master
    /// password. The caller hands `secret` to the device and forgets it.
    pub fn add_wrap(&mut self, label: &str, secret: &[u8; 32]) -> Result<()> {
        validate_wrap_label(label)?;
        self.mutate(|f, dek| {
            if f.wraps.contains_key(label) {
                return Err(Error::WrapExists(label.to_string()));
            }
            let wrapped_dek = crypto::wrap_key(secret, dek, &wrap_aad(label))?;
            f.wraps.insert(
                label.to_string(),
                WrapEntry {
                    wrapped_dek,
                    created_at: now(),
                },
            );
            Ok(())
        })
    }

    /// Removes a wrap (revokes a device).
    pub fn remove_wrap(&mut self, label: &str) -> Result<()> {
        self.mutate(|f, _| {
            f.wraps
                .remove(label)
                .map(|_| ())
                .ok_or_else(|| Error::UnknownWrap(label.to_string()))
        })
    }

    /// Whether `scope` has `name` (exact scope, expired or not).
    pub fn contains(&self, scope: &Scope, name: &str) -> bool {
        self.find(scope, name).is_some()
    }
}
