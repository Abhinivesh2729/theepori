use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A manifest or lockfile could not be parsed. `message` says how to fix it.
    #[error("{path}: {message}")]
    Parse { path: PathBuf, message: String },
    /// A network call failed. `message` includes the remedy where one is known.
    #[error("{url}: {message}")]
    Http { url: String, message: String },
}

impl Error {
    pub(crate) fn parse(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Error::Parse {
            path: path.into(),
            message: message.into(),
        }
    }
}
