use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors carry enough context to tell the user how to fix them.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{path}: invalid config: {message}")]
    Config { path: PathBuf, message: String },
    #[error("{0}")]
    Invalid(String),
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0} already exists")]
    Conflict(String),
    #[error("audit chain broken: {0}")]
    AuditChain(String),
    #[error("daemon: {0}")]
    Daemon(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}
