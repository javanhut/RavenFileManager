#[cfg(feature = "sftp")]
use std::collections::HashMap;
#[cfg(feature = "sftp")]
use std::sync::Arc;

use async_trait::async_trait;
#[cfg(feature = "sftp")]
use tokio::sync::Mutex;

use raven_core::entry::{EntryKind, FileEntry};
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

use crate::local::LocalFs;
use crate::smb::{SmbCredentials, SmbFs};
#[cfg(feature = "sftp")]
use crate::sftp::SftpFs;
#[cfg(feature = "sftp")]
use raven_core::automation_types::SshAuth;

/// A boxed future for the recursive cross-backend copy.
type BoxedCopy<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = RavenResult<()>> + Send + 'a>>;

/// What a cross-backend copy does with one item.
#[derive(Debug, PartialEq, Eq)]
enum CopyWalk {
    Dir,
    File,
    Skip,
}

/// Routes `RavenPath` to the appropriate VFS implementation.
///
/// The router holds a local filesystem backend, the SMB backend (which keeps
/// one connection per server itself), and a map of active SFTP connections
/// keyed by "user@host:port".
pub struct VfsRouter {
    local: LocalFs,
    smb: SmbFs,
    /// SMB locations opened from Connect to Server, keyed like `smb_key`.
    /// A server stays logged in while any location on it is open.
    smb_locations: std::sync::Mutex<std::collections::HashSet<String>>,
    #[cfg(feature = "sftp")]
    sftp_connections: Arc<Mutex<HashMap<String, Arc<SftpFs>>>>,
}

impl VfsRouter {
    pub fn new() -> Self {
        Self {
            local: LocalFs::new(),
            smb: SmbFs::new(),
            smb_locations: std::sync::Mutex::new(std::collections::HashSet::new()),
            #[cfg(feature = "sftp")]
            sftp_connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The connection id of an SMB location: `smb://host/share`, or
    /// `smb://host/` for a server's share list. Host case is folded so one
    /// server is one id however it was typed.
    pub fn smb_key(host: &str, share: &str) -> String {
        format!("smb://{}/{}", host.to_ascii_lowercase(), share)
    }

    /// Log in to an SMB server and check that `share` (and `path` inside it)
    /// can be browsed. An empty share checks that the share list can be read.
    ///
    /// Returns the connection id and the folder to open.
    pub async fn connect_smb(
        &self,
        host: &str,
        share: &str,
        path: Option<&str>,
        credentials: SmbCredentials,
    ) -> RavenResult<(String, RavenPath)> {
        // One session serves every open location on a server. Logging in
        // again as someone else would switch those locations to the new
        // identity (or break them, if the new login is refused a share), so
        // an in-use server is only reused, never replaced.
        if self.smb_server_in_use(host) {
            let same = self.smb.credentials_for(host).as_ref() == Some(&credentials);
            if !same {
                return Err(RavenError::Network {
                    message: format!(
                        "Already connected to SMB server {} with other credentials: disconnect its open locations first to log in as someone else",
                        host
                    ),
                });
            }
        } else {
            self.smb.connect(host, credentials).await?;
        }

        let folder = path
            .map(|p| p.trim().trim_matches('/'))
            .filter(|p| !p.is_empty())
            .map(|p| format!("/{}", p))
            .unwrap_or_else(|| "/".to_string());
        let initial = RavenPath::Smb {
            host: host.to_string(),
            share: share.to_string(),
            path: if share.is_empty() { "/".to_string() } else { folder },
        };

        let checked = if share.is_empty() {
            self.smb.list_shares(host).await.map(|_| ())
        } else {
            match self.smb.stat(&initial).await {
                Ok(entry) if entry.is_dir() => Ok(()),
                Ok(_) => Err(RavenError::NotADirectory {
                    path: std::path::PathBuf::from(initial.to_string()),
                }),
                Err(e) => Err(e),
            }
        };
        let key = Self::smb_key(host, share);
        if let Err(e) = checked {
            // A login nothing else is using should not linger after a failure.
            if !self.smb_server_in_use(host) {
                self.smb.forget_server(host).await;
            }
            return Err(e);
        }

        if let Ok(mut locations) = self.smb_locations.lock() {
            locations.insert(key.clone());
        }
        Ok((key, initial))
    }

    /// Close an SMB location by its id. The server is logged out, and its
    /// credentials forgotten, once no other location on it remains open.
    pub async fn disconnect_smb(&self, key: &str) -> RavenResult<()> {
        let removed = self
            .smb_locations
            .lock()
            .map(|mut l| l.remove(key))
            .unwrap_or(false);
        if !removed {
            return Err(RavenError::Network {
                message: format!("No SMB connection found for {}", key),
            });
        }
        let host = key
            .trim_start_matches("smb://")
            .split('/')
            .next()
            .unwrap_or_default();
        if !self.smb_server_in_use(host) {
            self.smb.forget_server(host).await;
        }
        Ok(())
    }

    /// List the open SMB location ids.
    pub fn list_smb_connections(&self) -> Vec<String> {
        self.smb_locations
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn smb_server_in_use(&self, host: &str) -> bool {
        let prefix = format!("smb://{}/", host.to_ascii_lowercase());
        self.smb_locations
            .lock()
            .map(|l| l.iter().any(|k| k.starts_with(&prefix)))
            .unwrap_or(false)
    }

    /// Whether one backend can handle a copy between the two paths itself.
    /// SMB counts as one backend for every server; SFTP connections are
    /// separate sessions, so only the same connection qualifies.
    fn same_backend(a: &RavenPath, b: &RavenPath) -> bool {
        match (a, b) {
            (RavenPath::Local(_), RavenPath::Local(_)) => true,
            (RavenPath::Smb { .. }, RavenPath::Smb { .. }) => true,
            (
                RavenPath::Sftp {
                    host, port, user, ..
                },
                RavenPath::Sftp {
                    host: h2,
                    port: p2,
                    user: u2,
                    ..
                },
            ) => host == h2 && port == p2 && user == u2,
            _ => false,
        }
    }

    /// How `copy_across` should treat `path`. `listed_as_symlink` is what a
    /// parent listing said, since `stat` follows links on every backend.
    ///
    /// None of the other backends can create a symlink, so a link is copied
    /// as the file it points to. Links to folders and dangling links are
    /// skipped: following them could walk a cycle forever or pull in a whole
    /// unrelated tree, and a dangling one would abort the entire copy.
    ///
    /// The exception is a link to a folder the user picked by name
    /// (`selected`): it is followed, because that folder is what was asked
    /// for. Links found inside it are still skipped, so no cycle can form.
    async fn copy_walk(
        &self,
        path: &RavenPath,
        listed_as_symlink: bool,
        selected: bool,
    ) -> RavenResult<CopyWalk> {
        let is_link = match path.as_local_path() {
            Some(local) => tokio::fs::symlink_metadata(local).await?.is_symlink(),
            None => listed_as_symlink,
        };
        let target = match self.stat(path).await {
            Ok(entry) => entry,
            Err(e) if !is_link => return Err(e),
            Err(_) => {
                tracing::warn!("Skipping dangling symlink {} while copying", path);
                return Ok(CopyWalk::Skip);
            }
        };
        Ok(match (target.is_dir(), is_link) {
            (true, false) => CopyWalk::Dir,
            (true, true) if selected => CopyWalk::Dir,
            (true, true) => {
                tracing::warn!("Skipping symlink to a folder {} while copying", path);
                CopyWalk::Skip
            }
            (false, _) => CopyWalk::File,
        })
    }

    /// Refuse a copy whose destination lies inside its source: walking the
    /// source would keep finding the copies it just made, without end.
    fn refuse_into_itself(source: &RavenPath, destination: &RavenPath) -> RavenResult<()> {
        if destination.is_inside(source) {
            return Err(RavenError::Vfs {
                message: format!("Cannot copy {} into itself ({})", source, destination),
            });
        }
        Ok(())
    }

    /// Copy between two backends (local to SMB, SMB to local, ...) through
    /// the generic operations: folders are recreated and walked, files are
    /// read whole and written out. With `exclusive` every file is created
    /// only if its name is free, so nothing existing is ever replaced.
    fn copy_across<'a>(
        &'a self,
        source: &'a RavenPath,
        destination: &'a RavenPath,
        listed_as_symlink: bool,
        selected: bool,
        exclusive: bool,
        progress: Option<&'a (dyn Fn(u64, u64) + Send + Sync)>,
    ) -> BoxedCopy<'a> {
        Box::pin(async move {
            match self.copy_walk(source, listed_as_symlink, selected).await? {
                CopyWalk::Skip => return Ok(()),
                CopyWalk::Dir => {
                    let children = self.list_dir(source).await?;
                    self.create_dir(destination).await?;
                    for child in children {
                        let target = destination.join(&child.name);
                        let link = child.kind == EntryKind::Symlink;
                        self.copy_across(&child.path, &target, link, false, exclusive, progress)
                            .await?;
                    }
                    return Ok(());
                }
                CopyWalk::File => {}
            }
            let data = self.read(source).await?;
            let total = data.len() as u64;
            if let Some(cb) = progress {
                cb(0, total);
            }
            if exclusive {
                self.write_new(destination, &data).await?;
            } else {
                self.write(destination, &data).await?;
            }
            if let Some(cb) = progress {
                cb(total, total);
            }
            Ok(())
        })
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

    /// Resolve `remote_path` on the connection `key` names.
    #[cfg(feature = "sftp")]
    pub async fn sftp_canonicalize(&self, key: &str, remote_path: &str) -> RavenResult<String> {
        let sftp = {
            let connections = self.sftp_connections.lock().await;
            connections.get(key).cloned()
        };
        match sftp {
            Some(sftp) => sftp.canonicalize(remote_path).await,
            None => Err(RavenError::Network {
                message: format!("No SFTP connection found for key: {}", key),
            }),
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

    async fn write_new(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        match path {
            RavenPath::Local(_) => self.local.write_new(path, contents).await,
            #[cfg(feature = "sftp")]
            RavenPath::Sftp { .. } => {
                let sftp = self.get_sftp(path).await?;
                sftp.write_new(path, contents).await
            }
            RavenPath::Smb { .. } => self.smb.write_new(path, contents).await,
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", path),
            }),
        }
    }

    /// Always the generic walk, with every file created exclusively.
    async fn copy_new(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        if !self.supports(destination) {
            return Err(RavenError::UnsupportedProtocol {
                protocol: format!("{}", destination),
            });
        }
        Self::refuse_into_itself(source, destination)?;
        self.copy_across(source, destination, false, true, true, progress.as_deref())
            .await
    }

    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        Self::refuse_into_itself(source, destination)?;
        // SFTP has no server-side copy, and its own copy only handles single
        // files, so copies within one connection take the generic walk too:
        // that way folders copy, and merge into existing folders.
        let generic = !Self::same_backend(source, destination)
            || matches!(source, RavenPath::Sftp { .. });
        if generic && self.supports(destination) {
            return self
                .copy_across(source, destination, false, true, false, progress.as_deref())
                .await;
        }
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

    #[test]
    fn test_smb_key_format() {
        assert_eq!(VfsRouter::smb_key("NAS.local", "Media"), "smb://nas.local/Media");
        assert_eq!(VfsRouter::smb_key("nas", ""), "smb://nas/");
    }

    #[tokio::test]
    async fn test_router_disconnect_unknown_smb() {
        let router = VfsRouter::new();
        assert!(router.disconnect_smb("smb://nas/share").await.is_err());
        assert!(router.list_smb_connections().is_empty());
    }

    #[tokio::test]
    async fn test_router_smb_server_in_use_matches_host_only() {
        let router = VfsRouter::new();
        router
            .smb_locations
            .lock()
            .unwrap()
            .insert(VfsRouter::smb_key("nas", "a"));
        assert!(router.smb_server_in_use("NAS"));
        assert!(!router.smb_server_in_use("nas2"));
        router.disconnect_smb("smb://nas/a").await.unwrap();
        assert!(!router.smb_server_in_use("nas"));
    }

    #[test]
    fn test_same_backend() {
        let smb_a = RavenPath::Smb {
            host: "a".into(),
            share: "s".into(),
            path: "/".into(),
        };
        let smb_b = RavenPath::Smb {
            host: "b".into(),
            share: "t".into(),
            path: "/x".into(),
        };
        let local = RavenPath::local("/tmp");
        assert!(VfsRouter::same_backend(&smb_a, &smb_b));
        assert!(VfsRouter::same_backend(&local, &local));
        assert!(!VfsRouter::same_backend(&local, &smb_a));
        assert!(!VfsRouter::same_backend(&smb_a, &local));
    }

    #[tokio::test]
    async fn test_router_copy_local_to_smb_reaches_smb_backend() {
        let dir = std::env::temp_dir().join(format!("raven-smb-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"hello").unwrap();

        let router = VfsRouter::new();
        let dest = RavenPath::Smb {
            host: "nas".into(),
            share: "docs".into(),
            path: "/a.txt".into(),
        };
        // No server is connected, so the write fails in the SMB backend
        // rather than in the local one complaining about a non-local path.
        let err = router
            .copy(&RavenPath::local(&file), &dest, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Connect to Server"), "{}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_router_connect_smb_refuses_other_credentials_on_a_busy_server() {
        let router = VfsRouter::new();
        let alice = SmbCredentials {
            user: "alice".into(),
            password: "pw".into(),
            domain: String::new(),
        };
        router.smb.connect_for_test("nas", alice.clone());
        router
            .smb_locations
            .lock()
            .unwrap()
            .insert(VfsRouter::smb_key("nas", "Media"));

        let err = router
            .connect_smb("NAS", "Private", None, SmbCredentials::guest())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("other credentials"), "{}", err);
        // The open location keeps its session and identity.
        assert_eq!(router.smb.credentials_for("nas"), Some(alice));
        assert!(router.smb_server_in_use("nas"));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("raven-router-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn test_router_copy_into_itself_is_refused() {
        let dir = scratch("into-itself");
        std::fs::create_dir_all(dir.join("a/sub")).unwrap();
        let router = VfsRouter::new();
        let a = RavenPath::local(dir.join("a"));
        let inside = RavenPath::local(dir.join("a/sub/a"));
        assert!(router.copy(&a, &inside, None).await.is_err());
        assert!(router.copy_new(&a, &inside, None).await.is_err());
        assert!(!dir.join("a/sub/a").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_router_generic_copy_merges_and_exclusive_copy_never_overwrites() {
        let dir = scratch("generic-copy");
        std::fs::create_dir_all(dir.join("src/d/nested")).unwrap();
        std::fs::write(dir.join("src/d/a.txt"), b"new a").unwrap();
        std::fs::write(dir.join("src/d/nested/b.txt"), b"new b").unwrap();
        std::fs::create_dir_all(dir.join("dst/d")).unwrap();
        std::fs::write(dir.join("dst/d/a.txt"), b"old a").unwrap();
        std::fs::write(dir.join("dst/d/keep.txt"), b"keep").unwrap();
        let router = VfsRouter::new();
        let src = RavenPath::local(dir.join("src/d"));
        let dst = RavenPath::local(dir.join("dst/d"));

        // Exclusive: the taken name fails the copy and is left alone.
        let err = router
            .copy_across(&src, &dst, false, true, true, None)
            .await
            .unwrap_err();
        assert!(matches!(err, RavenError::Io { .. } | RavenError::AlreadyExists { .. }), "{err:?}");
        assert_eq!(std::fs::read(dir.join("dst/d/a.txt")).unwrap(), b"old a");

        // A brand-new destination copies fine exclusively.
        let fresh = RavenPath::local(dir.join("dst/fresh"));
        router.copy_new(&src, &fresh, None).await.unwrap();
        assert_eq!(std::fs::read(dir.join("dst/fresh/nested/b.txt")).unwrap(), b"new b");

        // Agreed overwrite: merges, replacing same names, keeping the rest.
        router
            .copy_across(&src, &dst, false, true, false, None)
            .await
            .unwrap();
        assert_eq!(std::fs::read(dir.join("dst/d/a.txt")).unwrap(), b"new a");
        assert_eq!(std::fs::read(dir.join("dst/d/keep.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read(dir.join("dst/d/nested/b.txt")).unwrap(), b"new b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_router_copy_walk_skips_folder_and_dangling_links() {
        let dir = std::env::temp_dir().join(format!("raven-copy-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("file.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("..", dir.join("sub/loop")).unwrap();
        std::os::unix::fs::symlink("missing", dir.join("dangling")).unwrap();
        std::os::unix::fs::symlink("file.txt", dir.join("to_file")).unwrap();

        let router = VfsRouter::new();
        let walk = |name: &str| RavenPath::local(dir.join(name));
        assert_eq!(router.copy_walk(&walk("sub"), false, false).await.unwrap(), CopyWalk::Dir);
        assert_eq!(router.copy_walk(&walk("file.txt"), false, false).await.unwrap(), CopyWalk::File);
        // Local links are detected even when the caller did not say so.
        assert_eq!(router.copy_walk(&walk("sub/loop"), false, false).await.unwrap(), CopyWalk::Skip);
        assert_eq!(router.copy_walk(&walk("dangling"), false, false).await.unwrap(), CopyWalk::Skip);
        assert_eq!(router.copy_walk(&walk("to_file"), false, false).await.unwrap(), CopyWalk::File);
        assert!(router.copy_walk(&walk("nope"), false, false).await.is_err());
        // A link to a folder the user picked is followed; a dangling one is not.
        assert_eq!(router.copy_walk(&walk("sub/loop"), false, true).await.unwrap(), CopyWalk::Dir);
        assert_eq!(router.copy_walk(&walk("dangling"), false, true).await.unwrap(), CopyWalk::Skip);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
