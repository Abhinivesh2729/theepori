//! One-way import from a Panuval local store into a TP Vault.
//!
//! Reads the Panuval CLI's cache file
//! (`~/.config/panuval/cache/<projectId>-<uid>.json`), which holds the vault
//! header (KDF params + wrapped DEK) and every item's ciphertext. Nothing is
//! fetched from Firebase. Only `env`, `apiKey` and `token` items are imported:
//! those are the items Panuval injects with `run --env`, and their names are
//! already valid environment variable names.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::crypto::{self, KdfParams, panuval};
use crate::error::{Error, Result, io};
use crate::scope::{Scope, validate_name};
use crate::vault::Unlocked;

/// Panuval item types that are injected as environment variables.
pub const ENV_TYPES: [&str; 3] = ["env", "apiKey", "token"];

#[derive(Deserialize)]
struct Cache {
    uid: String,
    meta: Option<Meta>,
    #[serde(default)]
    items: HashMap<String, StoreItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    kdf: Kdf,
    #[serde(default)]
    wrapped_dek: Option<String>,
}

#[derive(Deserialize)]
struct Kdf {
    alg: String,
    #[serde(rename = "memoryKiB")]
    memory_kib: i64,
    iterations: i64,
    parallelism: i64,
    #[serde(default)]
    salt: Option<String>,
}

#[derive(Deserialize)]
struct StoreItem {
    id: String,
    #[serde(default)]
    blob: Option<String>,
    version: i64,
    #[serde(default)]
    deleted: bool,
}

/// Decrypted item plaintext (`docs/PROTOCOL.md` in Panuval). `notes` and
/// unknown fields are deliberately not read.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plain {
    v: i64,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    item_type: String,
    #[serde(default)]
    folder: String,
    #[serde(default)]
    value: Zeroizing<String>,
    #[serde(default)]
    username: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    agent_access: bool,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

/// A Panuval item's metadata, passed to the scope mapper. Has no value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanuvalItem {
    pub id: String,
    pub version: i64,
    pub name: String,
    /// `password | apiKey | token | ssh | env | note`.
    pub item_type: String,
    /// Slash-separated folder, e.g. `projects/thinai` (empty at the root).
    pub folder: String,
    pub username: String,
    pub url: String,
    pub agent_access: bool,
    pub created_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
}

impl PanuvalItem {
    /// `folder/name`, Panuval's human reference.
    pub fn path(&self) -> String {
        if self.folder.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.folder, self.name)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedItem {
    pub panuval_path: String,
    pub scope: Scope,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedItem {
    /// `folder/name`, or the item id when it could not be decrypted.
    pub panuval_path: String,
    pub reason: String,
}

/// What [`import_panuval`] did. Contains no secret values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub uid: String,
    pub imported: Vec<ImportedItem>,
    pub skipped: Vec<SkippedItem>,
    /// Deleted items (tombstones) that were ignored.
    pub tombstones: usize,
}

fn perr(msg: impl Into<String>) -> Error {
    Error::Panuval(msg.into())
}

fn b64(field: &str, s: Option<&str>) -> Result<Vec<u8>> {
    B64.decode(s.unwrap_or_default())
        .map_err(|e| perr(format!("{field} is not base64: {e}")))
}

fn parse_time(s: Option<&str>) -> Option<OffsetDateTime> {
    s.and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
}

/// Accepts the cache file itself, or a directory holding exactly one cache
/// file (directly or under `cache/`, as in `~/.config/panuval`).
fn resolve_store(path: &Path) -> Result<PathBuf> {
    if !path.is_dir() {
        return Ok(path.to_path_buf());
    }
    let mut found = Vec::new();
    for dir in [path.to_path_buf(), path.join("cache")] {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "json") {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
                if name.is_some_and(|n| !n.starts_with('.') && n != "session.json") {
                    found.push(p);
                }
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(perr(format!(
            "no Panuval cache file in {}: pass the path to cache/<project>-<uid>.json",
            path.display()
        ))),
        n => Err(perr(format!(
            "{n} Panuval cache files in {}: pass the path of the one to import",
            path.display()
        ))),
    }
}

/// Imports Panuval secrets into `target`.
///
/// `map` chooses the TP scope for each importable item (e.g. folder
/// `projects/thinai` → `Scope::project("thinai")`). Items are never
/// overwritten: an item already in the target, or a second Panuval item that
/// maps to the same scope and name, is skipped and reported.
pub fn import_panuval(
    store_path: &Path,
    panuval_password: &str,
    target: &mut Unlocked,
    map: impl Fn(&PanuvalItem) -> Scope,
) -> Result<ImportReport> {
    let path = resolve_store(store_path)?;
    let raw = std::fs::read(&path).map_err(io(format!("read {}", path.display())))?;
    let cache: Cache = serde_json::from_slice(&raw).map_err(|e| {
        perr(format!(
            "{} is not a Panuval cache file: {e}",
            path.display()
        ))
    })?;
    let meta = cache.meta.ok_or_else(|| {
        perr("the store has no vault header: run `panuval sync` online once, then retry")
    })?;

    let kdf = KdfParams {
        alg: meta.kdf.alg,
        memory_kib: u32::try_from(meta.kdf.memory_kib).map_err(|_| Error::BadKdf)?,
        iterations: u32::try_from(meta.kdf.iterations).map_err(|_| Error::BadKdf)?,
        parallelism: u8::try_from(meta.kdf.parallelism).map_err(|_| Error::BadKdf)?,
        salt: b64("meta.kdf.salt", meta.kdf.salt.as_deref())?,
    };
    let wrapped = b64("meta.wrappedDek", meta.wrapped_dek.as_deref())?;
    let kek = crypto::derive_kek(panuval_password, &kdf)?;
    let dek = crypto::unwrap_key(&kek, &wrapped, panuval::DEK_AAD).map_err(|e| match e {
        Error::Decrypt => Error::WrongPassword,
        e => e,
    })?;

    let mut report = ImportReport {
        uid: cache.uid.clone(),
        ..ImportReport::default()
    };
    let mut items: Vec<(PanuvalItem, Zeroizing<String>)> = Vec::new();
    let mut stored: Vec<&StoreItem> = cache.items.values().collect();
    stored.sort_by(|a, b| a.id.cmp(&b.id));
    for si in stored {
        if si.deleted {
            report.tombstones += 1;
            continue;
        }
        let skip = |reason: &str| SkippedItem {
            panuval_path: si.id.clone(),
            reason: reason.to_string(),
        };
        let Ok(blob) = b64("item blob", si.blob.as_deref()) else {
            report.skipped.push(skip("ciphertext is not base64"));
            continue;
        };
        let aad = panuval::item_aad(&cache.uid, &si.id, si.version);
        let Ok(pt) = crypto::open(&dek, &blob, &aad) else {
            report.skipped.push(skip(
                "failed authentication (tampered, or written by another account)",
            ));
            continue;
        };
        let plain: Plain = match serde_json::from_slice(&pt) {
            Ok(p) => p,
            Err(_) => {
                report
                    .skipped
                    .push(skip("item plaintext is not valid JSON"));
                continue;
            }
        };
        if plain.v != 1 {
            report
                .skipped
                .push(skip("unsupported Panuval item format version"));
            continue;
        }
        if plain.deleted {
            report.tombstones += 1;
            continue;
        }
        let item = PanuvalItem {
            id: si.id.clone(),
            version: si.version,
            name: plain.name,
            item_type: plain.item_type,
            folder: plain.folder,
            username: plain.username,
            url: plain.url,
            agent_access: plain.agent_access,
            created_at: parse_time(plain.created_at.as_deref()),
            updated_at: parse_time(plain.updated_at.as_deref()),
        };
        items.push((item, plain.value));
    }
    items.sort_by_key(|(it, _)| it.path());

    let mut taken: BTreeSet<(Scope, String)> = BTreeSet::new();
    for (item, value) in items {
        let path = item.path();
        let mut skip = |reason: String| {
            report.skipped.push(SkippedItem {
                panuval_path: path.clone(),
                reason,
            })
        };
        if !ENV_TYPES.contains(&item.item_type.as_str()) {
            skip(format!(
                "type {:?} is not an environment secret (only env, apiKey and token are imported)",
                item.item_type
            ));
            continue;
        }
        if validate_name(&item.name).is_err() {
            skip("name is not a valid environment variable name".into());
            continue;
        }
        let scope = map(&item);
        if let Err(e) = scope.validate() {
            skip(format!("mapped to an invalid scope: {e}"));
            continue;
        }
        let key = (scope.clone(), item.name.clone());
        if taken.contains(&key) {
            skip(format!(
                "another Panuval item was already imported as {} in {scope}",
                item.name
            ));
            continue;
        }
        if target.contains(&scope, &item.name) {
            skip(format!(
                "{} already exists in {scope} in the TP Vault (delete it there to re-import)",
                item.name
            ));
            continue;
        }
        target.set(&scope, &item.name, &value)?;
        taken.insert(key);
        report.imported.push(ImportedItem {
            panuval_path: path,
            scope,
            name: item.name,
        });
    }
    Ok(report)
}
