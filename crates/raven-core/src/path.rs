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
            Self::Sftp { path, .. } | Self::Archive { inner_path: path, .. } => {
                path.rsplit('/').next().filter(|s| !s.is_empty())
            }
            // A share's root is named after the share.
            Self::Smb { share, path, .. } => path
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .or_else(|| Some(share.as_str()).filter(|s| !s.is_empty())),
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
            Self::Smb { host, share, path } => {
                let trimmed = path.trim_end_matches('/');
                if trimmed.is_empty() {
                    // Above a share's root is the server's list of shares.
                    (!share.is_empty()).then(|| Self::Smb {
                        host: host.clone(),
                        share: String::new(),
                        path: "/".to_string(),
                    })
                } else {
                    let parent = trimmed.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                    Some(Self::Smb {
                        host: host.clone(),
                        share: share.clone(),
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

    /// Whether this path lies strictly inside `outer` (a descendant, not
    /// `outer` itself). SMB names are compared without case, the way
    /// Windows and default Samba shares resolve them.
    pub fn is_inside(&self, outer: &RavenPath) -> bool {
        fn key(p: &RavenPath) -> String {
            match p {
                RavenPath::Smb { .. } => p.to_string().trim_end_matches('/').to_lowercase(),
                _ => p.to_string().trim_end_matches('/').to_string(),
            }
        }
        if std::mem::discriminant(self) != std::mem::discriminant(outer) {
            return false;
        }
        let outer_key = key(outer);
        let mut current = self.parent();
        while let Some(p) = current {
            if key(&p) == outer_key {
                return true;
            }
            current = p.parent();
        }
        false
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
            // With no share this is the server itself: smb://host/.
            Self::Smb { host, share, path } if share.is_empty() => {
                write!(f, "smb://{}{}", host, path)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn smb(share: &str, path: &str) -> RavenPath {
        RavenPath::Smb {
            host: "nas".to_string(),
            share: share.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn smb_parents_climb_to_the_share_list() {
        assert_eq!(smb("docs", "/a/b").parent(), Some(smb("docs", "/a")));
        assert_eq!(smb("docs", "/a/").parent(), Some(smb("docs", "/")));
        assert_eq!(smb("docs", "/").parent(), Some(smb("", "/")));
        assert_eq!(smb("", "/").parent(), None);
    }

    #[test]
    fn smb_names_and_display() {
        assert_eq!(smb("docs", "/a.txt").file_name(), Some("a.txt"));
        assert_eq!(smb("docs", "/").file_name(), Some("docs"));
        assert_eq!(smb("", "/").file_name(), None);
        assert_eq!(smb("docs", "/a").to_string(), "smb://nas/docs/a");
        assert_eq!(smb("", "/").to_string(), "smb://nas/");
        assert_eq!(smb("", "/").join("docs"), smb("", "/docs"));
    }
}
