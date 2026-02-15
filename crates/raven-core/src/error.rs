use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RavenError {
    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("path not found: {path}")]
    NotFound { path: PathBuf },

    #[error("permission denied: {path}")]
    PermissionDenied { path: PathBuf },

    #[error("path already exists: {path}")]
    AlreadyExists { path: PathBuf },

    #[error("not a directory: {path}")]
    NotADirectory { path: PathBuf },

    #[error("operation cancelled")]
    Cancelled,

    #[error("operation conflict: {message}")]
    Conflict { message: String },

    #[error("unsupported protocol: {protocol}")]
    UnsupportedProtocol { protocol: String },

    #[error("VFS error: {message}")]
    Vfs { message: String },

    #[error("config error: {message}")]
    Config { message: String },

    #[error("git error: {message}")]
    Git { message: String },

    #[error("search error: {message}")]
    Search { message: String },

    #[error("preview error: {message}")]
    Preview { message: String },

    #[error("automation error: {message}")]
    Automation { message: String },

    #[error("plugin error: {message}")]
    Plugin { message: String },

    #[error("dbus error: {message}")]
    Dbus { message: String },

    #[error("system error: {message}")]
    System { message: String },

    #[error("network error: {message}")]
    Network { message: String },

    #[error("{message}")]
    Other { message: String },
}

pub type RavenResult<T> = Result<T, RavenError>;
