#[cfg(feature = "sftp")]
use std::collections::HashMap;
#[cfg(feature = "sftp")]
use std::sync::Arc;

use async_trait::async_trait;
#[cfg(feature = "sftp")]
use tokio::sync::Mutex;

use raven_core::entry::FileEntry;
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

use crate::local::LocalFs;
use crate::smb::SmbFs;
#[cfg(feature = "sftp")]
use crate::sftp::SftpFs;
#[cfg(feature = "sftp")]
use raven_core::automation_types::SshAuth;

/// Routes `RavenPath` to the appropriate VFS implementation.
///
/// The router holds a local filesystem backend, an SMB stub, and a map
/// of active SFTP connections keyed by "user@host:port".
pub struct VfsRouter {
    local: LocalFs,
    smb: SmbFs,
    #[cfg(feature = "sftp")]
    sftp_connections: Arc<Mutex<HashMap<String, Arc<SftpFs>>>>,
}

impl VfsRouter {
    pub fn new() -> Self {
        Self {
            local: LocalFs::new(),
            smb: SmbFs::new(),
            #[cfg(feature = "sftp")]
            sftp_connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Generate a connection key for SFTP connections.
    pub fn sftp_key(host: &str, port: u16, user: &str) -> String {
        format!("{}@{}:{}", user, host, port)
    }

    /// Connect to an SFTP server and store the connection for later use.
    ///
    /// Returns the connection key that can be used with `disconnect_sftp`.
    #[cfg(feature = "sftp")]
    pub async fn connect_sftp(
        &self,
        host: String,
        port: u16,
        user: String,
        auth: &SshAuth,
    ) -> RavenResult<String> {
        let key = Self::sftp_key(&host, port, &user);
        let sftp = SftpFs::connect(host, port, user, auth).await?;
        let mut connections = self.sftp_connections.lock().await;
        connections.insert(key.clone(), Arc::new(sftp));
        Ok(key)
    }

    /// Disconnect an SFTP session by its connection key.
    #[cfg(feature = "sftp")]
    pub async fn disconnect_sftp(&self, key: &str) -> RavenResult<()> {
        let mut connections = self.sftp_connections.lock().await;
        if let Some(sftp) = connections.remove(key) {
            sftp.disconnect().await;
            Ok(())
        } else {
            Err(RavenError::Network {
                message: format!("No SFTP connection found for key: {}", key),
            })
        }
    }

    /// List all active SFTP connection keys.
    #[cfg(feature = "sftp")]
    pub async fn list_sftp_connections(&self) -> Vec<String> {
        let connections = self.sftp_connections.lock().await;
        connections.keys().cloned().collect()
    }

    /// Look up an SFTP connection for the given path.
    #[cfg(feature = "sftp")]
    async fn get_sftp(&self, path: &RavenPath) -> RavenResult<Arc<SftpFs>> {
        if let RavenPath::Sftp {
            host, port, user, ..
        } = path
        {
            let key = Self::sftp_key(host, *port, user);
            let connections = self.sftp_connections.lock().await;
            connections.get(&key).cloned().ok_or_else(|| RavenError::Network {
                message: format!(
                    "Not connected to SFTP server {}@{}:{}. Call connect_sftp() first.",
                    user, host, port
                ),
            })
        } else {
            Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            })
        }
    }
}

impl Default for VfsRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VirtualFileSystem for VfsRouter {
    async fn list_dir(&self, path: &RavenPath) -> RavenResult<Vec<FileEntry>> {
        match path {
            RavenPath::Local(_) => self.local.list_dir(path).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.list_dir(path).await
            }
            RavenPath::Smb { .. } => self.smb.list_dir(path).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    async fn stat(&self, path: &RavenPath) -> RavenResult<FileEntry> {
        match path {
            RavenPath::Local(_) => self.local.stat(path).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.stat(path).await
            }
            RavenPath::Smb { .. } => self.smb.stat(path).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    async fn read(&self, path: &RavenPath) -> RavenResult<Vec<u8>> {
        match path {
            RavenPath::Local(_) => self.local.read(path).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.read(path).await
            }
            RavenPath::Smb { .. } => self.smb.read(path).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    async fn write(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        match path {
            RavenPath::Local(_) => self.local.write(path, contents).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.write(path, contents).await
            }
            RavenPath::Smb { .. } => self.smb.write(path, contents).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        match source {
            RavenPath::Local(_) => self.local.copy(source, destination, progress).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(source).await?;
                sftp.copy(source, destination, progress).await
            }
            RavenPath::Smb { .. } => self.smb.copy(source, destination, progress).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", source),
            }),
        }
    }

    async fn rename(&self, source: &RavenPath, destination: &RavenPath) -> RavenResult<()> {
        match source {
            RavenPath::Local(_) => self.local.rename(source, destination).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(source).await?;
                sftp.rename(source, destination).await
            }
            RavenPath::Smb { .. } => self.smb.rename(source, destination).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", source),
            }),
        }
    }

    async fn delete(&self, path: &RavenPath) -> RavenResult<()> {
        match path {
            RavenPath::Local(_) => self.local.delete(path).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.delete(path).await
            }
            RavenPath::Smb { .. } => self.smb.delete(path).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    async fn create_dir(&self, path: &RavenPath) -> RavenResult<()> {
        match path {
            RavenPath::Local(_) => self.local.create_dir(path).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.create_dir(path).await
            }
            RavenPath::Smb { .. } => self.smb.create_dir(path).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    async fn exists(&self, path: &RavenPath) -> RavenResult<bool> {
        match path {
            RavenPath::Local(_) => self.local.exists(path).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.exists(path).await
            }
            RavenPath::Smb { .. } => self.smb.exists(path).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    fn supports(&self, path: &RavenPath) -> bool {
        match path {
            RavenPath::Local(_) => true,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => true,
            RavenPath::Smb { .. } => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_router_routes_local_paths() {
        let router = VfsRouter::new();
        let path = RavenPath::local(PathBuf::from("/tmp"));
        let result = router.list_dir(&path).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_router_stat_local() {
        let router = VfsRouter::new();
        let path = RavenPath::local(PathBuf::from("/tmp"));
        let result = router.stat(&path).await;
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert!(entry.is_dir());
    }

    #[tokio::test]
    async fn test_router_exists_local() {
        let router = VfsRouter::new();
        let exists = router
            .exists(&RavenPath::local(PathBuf::from("/tmp")))
            .await
            .unwrap();
        assert!(exists);
    }

    #[tokio::test]
    async fn test_router_smb_returns_error() {
        let router = VfsRouter::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/docs".to_string(),
        };
        let result = router.list_dir(&path).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RavenError::Network { .. }));
    }

    #[cfg(feature = "sftp")]
    #[tokio::test]
    async fn test_router_sftp_not_connected_returns_error() {
        let router = VfsRouter::new();
        let path = RavenPath::Sftp {
            host: "example.com".to_string(),
            port: 22,
            user: "alice".to_string(),
            path: "/home/alice".to_string(),
        };
        let result = router.list_dir(&path).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RavenError::Network { .. }));
    }

    #[tokio::test]
    async fn test_router_unsupported_protocol() {
        let router = VfsRouter::new();
        let path = RavenPath::Archive {
            archive: PathBuf::from("/tmp/test.tar.gz"),
            inner_path: "file.txt".to_string(),
        };
        let result = router.list_dir(&path).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RavenError::UnsupportedProtocol { .. }));
    }

    #[tokio::test]
    async fn test_router_supports_local() {
        let router = VfsRouter::new();
        assert!(router.supports(&RavenPath::local("/tmp")));
    }

    #[tokio::test]
    async fn test_router_supports_smb() {
        let router = VfsRouter::new();
        let smb_path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/".to_string(),
        };
        assert!(router.supports(&smb_path));
    }

    #[cfg(feature = "sftp")]
    #[tokio::test]
    async fn test_router_supports_sftp() {
        let router = VfsRouter::new();
        let sftp_path = RavenPath::Sftp {
            host: "server".to_string(),
            port: 22,
            user: "user".to_string(),
            path: "/".to_string(),
        };
        assert!(router.supports(&sftp_path));
    }

    #[test]
    fn test_sftp_key_format() {
        assert_eq!(VfsRouter::sftp_key("host.com", 22, "alice"), "alice@host.com:22");
        assert_eq!(
            VfsRouter::sftp_key("192.168.1.1", 2222, "deploy"),
            "deploy@192.168.1.1:2222"
        );
    }

    #[test]
    fn test_sftp_key_uniqueness() {
        let key1 = VfsRouter::sftp_key("host1.com", 22, "alice");
        let key2 = VfsRouter::sftp_key("host2.com", 22, "alice");
        let key3 = VfsRouter::sftp_key("host1.com", 2222, "alice");
        let key4 = VfsRouter::sftp_key("host1.com", 22, "bob");

        assert_ne!(key1, key2);
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
    }

    #[cfg(feature = "sftp")]
    #[tokio::test]
    async fn test_router_sftp_connections_initially_empty() {
        let router = VfsRouter::new();
        let connections = router.list_sftp_connections().await;
        assert!(connections.is_empty());
    }

    #[cfg(feature = "sftp")]
    #[tokio::test]
    async fn test_router_disconnect_nonexistent_sftp() {
        let router = VfsRouter::new();
        let result = router.disconnect_sftp("nobody@nowhere:22").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_router_default() {
        let _router = VfsRouter::default();
    }
}
