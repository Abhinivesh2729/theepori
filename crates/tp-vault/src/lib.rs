//! TP Vault: the Thee Pori secrets vault, a Rust port of Panuval's crypto.
//!
//! Key hierarchy (identical to Panuval):
//!
//! ```text
//! master password ──Argon2id──▶ KEK ──AES-256-GCM──▶ DEK ──AES-256-GCM(AAD)──▶ items
//! ```
//!
//! - [`Vault`] is the on-disk vault (`<dir>/vault.json`, 0600, atomic writes,
//!   directory 0700). [`Vault::unlock`] or [`Vault::unlock_with_wrap`] yields
//!   an [`Unlocked`] handle that stores and reads secrets per [`Scope`].
//! - [`KeyHolder`] keeps an [`Unlocked`] in memory with a TTL, an idle
//!   timeout and a scoped [`Lease`] (the key agent inside tpd).
//! - [`import_panuval`] migrates secrets from a Panuval local store.
//! - [`crypto`] holds the Panuval-compatible primitives; `tests/` proves Rust
//!   decrypts Go-produced ciphertext byte-for-byte.
//!
//! Secret values only exist as [`zeroize::Zeroizing`] buffers, are never
//! logged, and never appear in `Debug` output or errors.

pub mod crypto;
mod error;
mod fsutil;
mod holder;
mod panuval;
mod scope;
mod vault;

pub use error::{Error, Result};
pub use holder::{Clock, HolderConfig, HolderStatus, KeyHolder, Lease, LeaseScopes, ScopedAccess};
pub use panuval::{
    ENV_TYPES, ImportReport, ImportedItem, PanuvalItem, SkippedItem, import_panuval,
};
pub use scope::{Scope, validate_name};
pub use vault::{
    ItemMeta, ItemOptions, MAX_VALUE_BYTES, MIN_PASSWORD_CHARS, Unlocked, VAULT_FILE, Vault,
};
pub use zeroize::Zeroizing;
