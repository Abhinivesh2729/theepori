use std::path::PathBuf;

use crate::Scope;

/// Everything that can go wrong in the vault. Messages never contain secret
/// values and say how to fix the problem where there is a fix.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("decryption failed: wrong key, or the data was tampered with")]
    Decrypt,
    #[error("malformed ciphertext blob (too short)")]
    BadBlob,
    #[error("unsupported ciphertext blob version {0} (this build understands version 1)")]
    BadBlobVersion(u8),
    #[error(
        "unsupported or unsafe KDF parameters (need argon2id, 1-64 iterations, 1-64 lanes, 8 KiB per lane to 4 GiB memory, salt >= 16 bytes)"
    )]
    BadKdf,
    #[error("key must be 32 bytes")]
    BadKeySize,
    #[error("the operating system random number generator failed: {0}")]
    Random(String),

    #[error("no TP Vault in {0}: create one with `Vault::init` (tp vault init)")]
    NotInitialized(PathBuf),
    #[error("a TP Vault already exists in {0}: unlock it instead, or choose another directory")]
    AlreadyInitialized(PathBuf),
    #[error("wrong master password")]
    WrongPassword,
    #[error("master password must be at least {min} characters")]
    PasswordTooShort { min: usize },
    #[error("unlock secret for wrap {0:?} is wrong")]
    WrongWrapSecret(String),
    #[error("no unlock wrap named {0:?}: list wraps with `Unlocked::wraps`")]
    UnknownWrap(String),
    #[error("an unlock wrap named {0:?} already exists: remove it first or pick another label")]
    WrapExists(String),
    #[error(
        "invalid wrap label {0:?}: use 1-64 characters from A-Z a-z 0-9 . _ : - (cannot be \"master\")"
    )]
    InvalidWrapLabel(String),

    #[error(
        "invalid scope component {0:?}: use 1-64 characters from A-Z a-z 0-9 . _ -, starting with a letter or digit"
    )]
    InvalidScope(String),
    #[error(
        "invalid secret name {0:?}: names must be environment variable names (A-Z a-z 0-9 _, not starting with a digit, max 256)"
    )]
    InvalidName(String),
    #[error("secret value is too large ({len} bytes, limit {limit})")]
    ValueTooLarge { len: usize, limit: usize },
    #[error("no secret {name} in {scope}: list secrets with `Unlocked::list`")]
    NotFound { scope: Scope, name: String },
    #[error("secret {name} in {scope} expired; set a new value to rotate it")]
    Expired { scope: Scope, name: String },

    #[error("vault file {path} is corrupt: {reason} (restore it from a backup)")]
    Corrupt { path: PathBuf, reason: String },
    #[error("item {name} in {scope} failed authentication: the vault file was tampered with")]
    Tampered { scope: Scope, name: String },
    #[error("the vault on disk was replaced since it was unlocked: lock and unlock again")]
    VaultReplaced,

    #[error("Panuval import: {0}")]
    Panuval(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> Error {
    let context = context.into();
    move |source| Error::Io { context, source }
}
