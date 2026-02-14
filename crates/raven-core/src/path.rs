use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Protocol-aware path abstraction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RavenPath {
    Local(PathBuf),
    Sftp {
        host: String,
        port: u16,
        user: String,
        path: String,
    },
    Smb {
        host: String,
        share: String,
        path: String,
    },
    Archive {
        archive: PathBuf,
        inner_path: String,
    },
    Trash {
        original_path: PathBuf,
        trash_id: String,
    },
}

impl RavenPath {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }

    pub fn file_name(&self) -> Option<&str> {
        match self {
            Self::Local(p) => p.file_name().and_then(|n| n.to_str()),
            Self::Sftp { path, .. } | Self::Smb { path, .. } | Self::Archive { inner_path: path, .. } => {
                path.rsplit('/').next().filter(|s| !s.is_empty())
            }
            Self::Trash { original_path, .. } => {
                original_path.file_name().and_then(|n| n.to_str())
            }
        }
    }

    pub fn parent(&self) -> Option<Self> {
        match self {
            Self::Local(p) => p.parent().map(|pp| Self::Local(pp.to_path_buf())),
            Self::Sftp { host, port, user, path } => {
                let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                if parent.is_empty() && path == "/" {
                    None
                } else {
                    Some(Self::Sftp {
                        host: host.clone(),
                        port: *port,
                        user: user.clone(),
                        path: if parent.is_empty() { "/".to_string() } else { parent.to_string() },
                    })
                }
            }
            _ => None,
        }
    }

    pub fn join(&self, name: &str) -> Self {
        match self {
            Self::Local(p) => Self::Local(p.join(name)),
            Self::Sftp { host, port, user, path } => Self::Sftp {
                host: host.clone(),
                port: *port,
                user: user.clone(),
                path: format!("{}/{}", path.trim_end_matches('/'), name),
            },
            Self::Smb { host, share, path } => Self::Smb {
                host: host.clone(),
                share: share.clone(),
                path: format!("{}/{}", path.trim_end_matches('/'), name),
            },
            Self::Archive { archive, inner_path } => Self::Archive {
                archive: archive.clone(),
                inner_path: format!("{}/{}", inner_path.trim_end_matches('/'), name),
            },
            Self::Trash { .. } => self.clone(),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    pub fn as_local_path(&self) -> Option<&PathBuf> {
        match self {
            Self::Local(p) => Some(p),
            _ => None,
        }
    }
}

impl fmt::Display for RavenPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(p) => write!(f, "{}", p.display()),
            Self::Sftp { host, port, user, path } => {
                write!(f, "sftp://{}@{}:{}{}", user, host, port, path)
            }
            Self::Smb { host, share, path } => {
                write!(f, "smb://{}/{}{}", host, share, path)
            }
            Self::Archive { archive, inner_path } => {
                write!(f, "archive://{}!/{}", archive.display(), inner_path)
            }
            Self::Trash { original_path, trash_id } => {
                write!(f, "trash://{}#{}", original_path.display(), trash_id)
            }
        }
    }
}

impl From<PathBuf> for RavenPath {
    fn from(path: PathBuf) -> Self {
        Self::Local(path)
    }
}

impl From<&std::path::Path> for RavenPath {
    fn from(path: &std::path::Path) -> Self {
        Self::Local(path.to_path_buf())
    }
}
