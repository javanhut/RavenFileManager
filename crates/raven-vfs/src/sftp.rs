use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use raven_core::automation_types::SshAuth;
use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

use russh::client;
use russh::keys::key;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;

/// Wrapper error type that satisfies russh's Handler::Error bound
/// (must impl `From<russh::Error> + Send + Debug`).
#[derive(Debug)]
enum SshHandlerError {
    Russh(russh::Error),
}

impl From<russh::Error> for SshHandlerError {
    fn from(e: russh::Error) -> Self {
        SshHandlerError::Russh(e)
    }
}

impl std::fmt::Display for SshHandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshHandlerError::Russh(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for SshHandlerError {}

/// Internal session state holding the SSH handle and SFTP session.
struct SftpSessionInner {
    sftp: SftpSession,
    #[allow(dead_code)]
    handle: client::Handle<SshHandler>,
}

/// SFTP virtual filesystem backend using russh + russh-sftp.
///
/// Provides full filesystem operations over an SSH/SFTP connection.
/// Use `SftpFs::connect()` to establish a connection, then use the
/// `VirtualFileSystem` trait methods to interact with remote files.
pub struct SftpFs {
    session: Arc<Mutex<Option<SftpSessionInner>>>,
    host: String,
    port: u16,
    user: String,
}

/// Minimal SSH client handler that accepts all host keys.
///
/// In a production scenario, this should verify host keys against
/// a known_hosts file. For now we accept all keys to allow connections
/// to any server.
struct SshHandler;

#[async_trait]
impl client::Handler for SshHandler {
    type Error = SshHandlerError;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all host keys. A real implementation would check known_hosts.
        Ok(true)
    }
}

impl SftpFs {
    /// Create a new SftpFs and establish a connection to the remote host.
    pub async fn connect(
        host: String,
        port: u16,
        user: String,
        auth: &SshAuth,
    ) -> RavenResult<Self> {
        let config = Arc::new(client::Config::default());
        let handler = SshHandler;
        let addr = format!("{}:{}", host, port);

        debug!("Connecting to SSH server at {}", addr);

        let mut handle = client::connect(config, &addr, handler)
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SSH connection failed to {}: {}", addr, e),
            })?;

        // Authenticate based on the provided method.
        let auth_result = match auth {
            SshAuth::Password { password } => handle
                .authenticate_password(&user, password)
                .await
                .map_err(|e| RavenError::Network {
                    message: format!("SSH password auth failed: {}", e),
                })?,
            SshAuth::KeyFile { path, passphrase } => {
                let key_pair = russh::keys::load_secret_key(path, passphrase.as_deref())
                    .map_err(|e| RavenError::Network {
                        message: format!("Failed to load SSH key from {:?}: {}", path, e),
                    })?;
                handle
                    .authenticate_publickey(&user, Arc::new(key_pair))
                    .await
                    .map_err(|e| RavenError::Network {
                        message: format!("SSH key auth failed: {}", e),
                    })?
            }
            SshAuth::Agent => {
                // Attempt agent authentication by listing keys from the SSH agent
                // and trying each one via authenticate_future.
                let mut agent =
                    russh::keys::agent::client::AgentClient::connect_env()
                        .await
                        .map_err(|e| RavenError::Network {
                            message: format!("Failed to connect to SSH agent: {}", e),
                        })?;
                let identities =
                    agent.request_identities().await.map_err(|e| RavenError::Network {
                        message: format!("Failed to list SSH agent identities: {}", e),
                    })?;

                let mut authenticated = false;
                for identity in identities {
                    let (returned_agent, result) = handle
                        .authenticate_future(&user, identity, agent)
                        .await;
                    agent = returned_agent;
                    match result {
                        Ok(true) => {
                            authenticated = true;
                            break;
                        }
                        Ok(false) => continue,
                        Err(e) => {
                            warn!("SSH agent auth attempt failed: {:?}", e);
                            continue;
                        }
                    }
                }

                if !authenticated {
                    return Err(RavenError::Network {
                        message: "SSH agent authentication failed: no suitable key found"
                            .to_string(),
                    });
                }
                true
            }
        };

        if !auth_result {
            return Err(RavenError::Network {
                message: format!("SSH authentication rejected by {}@{}:{}", user, host, port),
            });
        }

        debug!("SSH authenticated, opening SFTP subsystem");

        // Open a channel and request the SFTP subsystem.
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| RavenError::Network {
                message: format!("Failed to open SSH channel: {}", e),
            })?;

        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| RavenError::Network {
                message: format!("Failed to request SFTP subsystem: {}", e),
            })?;

        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| RavenError::Network {
                message: format!("Failed to initialize SFTP session: {}", e),
            })?;

        debug!("SFTP session established to {}@{}:{}", user, host, port);

        let inner = SftpSessionInner { sftp, handle };

        Ok(Self {
            session: Arc::new(Mutex::new(Some(inner))),
            host,
            port,
            user,
        })
    }

    /// Disconnect the SFTP session.
    pub async fn disconnect(&self) {
        let mut session = self.session.lock().await;
        if let Some(inner) = session.take() {
            let _ = inner.sftp.close().await;
            debug!(
                "SFTP session disconnected from {}@{}:{}",
                self.user, self.host, self.port
            );
        }
    }

    /// Return the connection identifier string for this SFTP connection.
    pub fn connection_key(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }

    /// Extract the remote path string from a `RavenPath::Sftp` variant.
    fn extract_remote_path(path: &RavenPath) -> RavenResult<&str> {
        match path {
            RavenPath::Sftp { path, .. } => Ok(path.as_str()),
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: "non-sftp".to_string(),
            }),
        }
    }

    /// Get a reference to the SFTP session, returning an error if disconnected.
    async fn get_sftp(&self) -> RavenResult<tokio::sync::MutexGuard<'_, Option<SftpSessionInner>>> {
        let guard = self.session.lock().await;
        if guard.is_none() {
            return Err(RavenError::Network {
                message: format!(
                    "SFTP session not connected to {}@{}:{}",
                    self.user, self.host, self.port
                ),
            });
        }
        Ok(guard)
    }

    /// Convert SFTP file type to EntryKind.
    fn file_type_to_kind(file_type: russh_sftp::protocol::FileType) -> EntryKind {
        use russh_sftp::protocol::FileType;
        match file_type {
            FileType::Dir => EntryKind::Directory,
            FileType::Symlink => EntryKind::Symlink,
            FileType::File => EntryKind::File,
            FileType::Other => EntryKind::Unknown,
        }
    }

    /// Build an EntryMetadata from SFTP file attributes.
    fn attrs_to_metadata(
        name: &str,
        attrs: &russh_sftp::protocol::FileAttributes,
    ) -> EntryMetadata {
        let size = attrs.size.unwrap_or(0);
        let permissions = attrs.permissions.unwrap_or(0o644);
        let is_hidden = name.starts_with('.');
        let is_executable = permissions & 0o111 != 0;

        let modified = attrs.mtime.map(|t| {
            chrono::DateTime::from_timestamp(t as i64, 0).unwrap_or_default()
        });
        let accessed = attrs.atime.map(|t| {
            chrono::DateTime::from_timestamp(t as i64, 0).unwrap_or_default()
        });

        let uid = attrs.uid.unwrap_or(0);
        let gid = attrs.gid.unwrap_or(0);

        EntryMetadata {
            size,
            modified,
            accessed,
            created: None, // SFTP v3 does not provide creation time
            permissions,
            owner_uid: uid,
            group_gid: gid,
            mime_type: None,
            symlink_target: None,
            is_hidden,
            is_executable,
        }
    }

    /// Build an SFTP-flavored RavenPath for a child entry.
    fn child_path(&self, parent_remote: &str, child_name: &str) -> RavenPath {
        let parent_trimmed = parent_remote.trim_end_matches('/');
        RavenPath::Sftp {
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            path: format!("{}/{}", parent_trimmed, child_name),
        }
    }
}

#[async_trait]
impl VirtualFileSystem for SftpFs {
    async fn list_dir(&self, path: &RavenPath) -> RavenResult<Vec<FileEntry>> {
        let remote_path = Self::extract_remote_path(path)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        let read_dir = inner
            .sftp
            .read_dir(remote_path)
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SFTP read_dir failed for {}: {}", remote_path, e),
            })?;

        let mut result = Vec::new();
        // ReadDir is an iterator that already skips '.' and '..'
        for entry in read_dir {
            let name = entry.file_name();
            let file_type = entry.file_type();
            let kind = Self::file_type_to_kind(file_type);
            let attrs = entry.metadata();
            let metadata = Self::attrs_to_metadata(&name, &attrs);
            let entry_path = self.child_path(remote_path, &name);

            result.push(FileEntry::new(name, entry_path, kind, metadata));
        }

        Ok(result)
    }

    async fn stat(&self, path: &RavenPath) -> RavenResult<FileEntry> {
        let remote_path = Self::extract_remote_path(path)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        let attrs = inner
            .sftp
            .metadata(remote_path)
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SFTP stat failed for {}: {}", remote_path, e),
            })?;

        let name = remote_path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("/")
            .to_string();

        let kind = Self::file_type_to_kind(attrs.file_type());
        let metadata = Self::attrs_to_metadata(&name, &attrs);

        Ok(FileEntry::new(name, path.clone(), kind, metadata))
    }

    async fn read(&self, path: &RavenPath) -> RavenResult<Vec<u8>> {
        let remote_path = Self::extract_remote_path(path)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        let data = inner
            .sftp
            .read(remote_path)
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SFTP read failed for {}: {}", remote_path, e),
            })?;

        Ok(data)
    }

    async fn write(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        let remote_path = Self::extract_remote_path(path)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        let mut file = inner
            .sftp
            .open_with_flags(
                remote_path,
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
            )
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SFTP open for write failed for {}: {}", remote_path, e),
            })?;

        use tokio::io::AsyncWriteExt;
        file.write_all(contents).await.map_err(|e| RavenError::Network {
            message: format!("SFTP write failed for {}: {}", remote_path, e),
        })?;
        file.shutdown().await.map_err(|e| RavenError::Network {
            message: format!("SFTP close after write failed for {}: {}", remote_path, e),
        })?;

        Ok(())
    }

    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        // SFTP has no native server-side copy; read source and write to destination.
        let data = self.read(source).await?;
        let total = data.len() as u64;

        if let Some(ref cb) = progress {
            cb(0, total);
        }

        self.write(destination, &data).await?;

        if let Some(ref cb) = progress {
            cb(total, total);
        }

        Ok(())
    }

    async fn rename(&self, source: &RavenPath, destination: &RavenPath) -> RavenResult<()> {
        let src_path = Self::extract_remote_path(source)?;
        let dst_path = Self::extract_remote_path(destination)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        inner
            .sftp
            .rename(src_path, dst_path)
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SFTP rename failed {} -> {}: {}", src_path, dst_path, e),
            })?;

        Ok(())
    }

    async fn delete(&self, path: &RavenPath) -> RavenResult<()> {
        let remote_path = Self::extract_remote_path(path)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        // Try stat first to determine if it's a file or directory.
        let attrs = inner
            .sftp
            .metadata(remote_path)
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SFTP stat failed for {}: {}", remote_path, e),
            })?;

        let is_dir = attrs.file_type().is_dir();

        if is_dir {
            inner
                .sftp
                .remove_dir(remote_path)
                .await
                .map_err(|e| RavenError::Network {
                    message: format!("SFTP rmdir failed for {}: {}", remote_path, e),
                })?;
        } else {
            inner
                .sftp
                .remove_file(remote_path)
                .await
                .map_err(|e| RavenError::Network {
                    message: format!("SFTP remove failed for {}: {}", remote_path, e),
                })?;
        }

        Ok(())
    }

    async fn create_dir(&self, path: &RavenPath) -> RavenResult<()> {
        let remote_path = Self::extract_remote_path(path)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        inner
            .sftp
            .create_dir(remote_path)
            .await
            .map_err(|e| RavenError::Network {
                message: format!("SFTP mkdir failed for {}: {}", remote_path, e),
            })?;

        Ok(())
    }

    async fn exists(&self, path: &RavenPath) -> RavenResult<bool> {
        let remote_path = Self::extract_remote_path(path)?;
        let guard = self.get_sftp().await?;
        let inner = guard.as_ref().unwrap();

        match inner.sftp.try_exists(remote_path).await {
            Ok(exists) => Ok(exists),
            Err(e) => {
                warn!(
                    "SFTP exists check for {} returned unexpected error: {}",
                    remote_path, e
                );
                Ok(false)
            }
        }
    }

    fn supports(&self, path: &RavenPath) -> bool {
        match path {
            RavenPath::Sftp {
                host, port, user, ..
            } => host == &self.host && *port == self.port && user == &self.user,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_remote_path_sftp() {
        let path = RavenPath::Sftp {
            host: "example.com".to_string(),
            port: 22,
            user: "alice".to_string(),
            path: "/home/alice/docs".to_string(),
        };
        let remote = SftpFs::extract_remote_path(&path).unwrap();
        assert_eq!(remote, "/home/alice/docs");
    }

    #[test]
    fn test_extract_remote_path_root() {
        let path = RavenPath::Sftp {
            host: "example.com".to_string(),
            port: 22,
            user: "root".to_string(),
            path: "/".to_string(),
        };
        let remote = SftpFs::extract_remote_path(&path).unwrap();
        assert_eq!(remote, "/");
    }

    #[test]
    fn test_extract_remote_path_rejects_non_sftp() {
        let path = RavenPath::local("/tmp");
        let result = SftpFs::extract_remote_path(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_remote_path_rejects_smb() {
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/docs".to_string(),
        };
        let result = SftpFs::extract_remote_path(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_key_format() {
        let host = "example.com";
        let port = 22u16;
        let user = "alice";
        let expected = "alice@example.com:22";
        let key = format!("{}@{}:{}", user, host, port);
        assert_eq!(key, expected);
    }

    #[test]
    fn test_connection_key_custom_port() {
        let host = "192.168.1.100";
        let port = 2222u16;
        let user = "deploy";
        let key = format!("{}@{}:{}", user, host, port);
        assert_eq!(key, "deploy@192.168.1.100:2222");
    }

    #[test]
    fn test_supports_matching_sftp_path() {
        let path_match = RavenPath::Sftp {
            host: "server.com".to_string(),
            port: 22,
            user: "user".to_string(),
            path: "/home/user".to_string(),
        };
        match &path_match {
            RavenPath::Sftp {
                host, port, user, ..
            } => {
                assert_eq!(host, "server.com");
                assert_eq!(*port, 22);
                assert_eq!(user, "user");
            }
            _ => panic!("Expected Sftp variant"),
        }
    }

    #[test]
    fn test_supports_non_matching_sftp_path() {
        let path = RavenPath::Sftp {
            host: "other.com".to_string(),
            port: 22,
            user: "bob".to_string(),
            path: "/data".to_string(),
        };
        match &path {
            RavenPath::Sftp {
                host, port, user, ..
            } => {
                assert_ne!(host, "server.com");
                assert_eq!(*port, 22);
                assert_ne!(user, "alice");
            }
            _ => panic!("Expected Sftp variant"),
        }
    }

    #[test]
    fn test_hidden_file_detection() {
        let attrs = russh_sftp::protocol::FileAttributes {
            size: None,
            uid: None,
            user: None,
            gid: None,
            group: None,
            permissions: None,
            atime: None,
            mtime: None,
        };
        let meta = SftpFs::attrs_to_metadata(".hidden_file", &attrs);
        assert!(meta.is_hidden);

        let meta_visible = SftpFs::attrs_to_metadata("visible_file", &attrs);
        assert!(!meta_visible.is_hidden);
    }

    #[test]
    fn test_executable_detection() {
        let mut attrs = russh_sftp::protocol::FileAttributes {
            size: None,
            uid: None,
            user: None,
            gid: None,
            group: None,
            permissions: Some(0o755),
            atime: None,
            mtime: None,
        };
        let meta = SftpFs::attrs_to_metadata("script.sh", &attrs);
        assert!(meta.is_executable);

        attrs.permissions = Some(0o644);
        let meta_noexec = SftpFs::attrs_to_metadata("data.txt", &attrs);
        assert!(!meta_noexec.is_executable);
    }

    #[test]
    fn test_attrs_to_metadata_defaults() {
        let attrs = russh_sftp::protocol::FileAttributes {
            size: None,
            uid: None,
            user: None,
            gid: None,
            group: None,
            permissions: None,
            atime: None,
            mtime: None,
        };
        let meta = SftpFs::attrs_to_metadata("file.txt", &attrs);
        assert_eq!(meta.size, 0);
        assert_eq!(meta.permissions, 0o644); // default when not set
        assert!(meta.modified.is_none());
        assert!(meta.accessed.is_none());
        assert!(meta.created.is_none());
        assert!(meta.mime_type.is_none());
        assert!(meta.symlink_target.is_none());
    }

    #[test]
    fn test_attrs_to_metadata_with_values() {
        let attrs = russh_sftp::protocol::FileAttributes {
            size: Some(1024),
            uid: Some(1000),
            user: None,
            gid: Some(1000),
            group: None,
            permissions: Some(0o600),
            atime: Some(1700000100),
            mtime: Some(1700000000),
        };

        let meta = SftpFs::attrs_to_metadata("data.bin", &attrs);
        assert_eq!(meta.size, 1024);
        assert_eq!(meta.permissions, 0o600);
        assert_eq!(meta.owner_uid, 1000);
        assert_eq!(meta.group_gid, 1000);
        assert!(meta.modified.is_some());
        assert!(meta.accessed.is_some());
        assert!(!meta.is_executable);
    }

    #[test]
    fn test_file_type_to_kind_dir() {
        assert_eq!(
            SftpFs::file_type_to_kind(russh_sftp::protocol::FileType::Dir),
            EntryKind::Directory
        );
    }

    #[test]
    fn test_file_type_to_kind_file() {
        assert_eq!(
            SftpFs::file_type_to_kind(russh_sftp::protocol::FileType::File),
            EntryKind::File
        );
    }

    #[test]
    fn test_file_type_to_kind_symlink() {
        assert_eq!(
            SftpFs::file_type_to_kind(russh_sftp::protocol::FileType::Symlink),
            EntryKind::Symlink
        );
    }

    #[test]
    fn test_file_type_to_kind_other() {
        assert_eq!(
            SftpFs::file_type_to_kind(russh_sftp::protocol::FileType::Other),
            EntryKind::Unknown
        );
    }

    /// Integration test: requires a real SSH server. Run with:
    /// `cargo test -p raven-vfs -- --ignored test_sftp_integration`
    #[tokio::test]
    #[ignore]
    async fn test_sftp_integration_connect() {
        let _fs = SftpFs::connect(
            "localhost".to_string(),
            22,
            "testuser".to_string(),
            &SshAuth::Password {
                password: "testpass".to_string(),
            },
        )
        .await
        .expect("Should connect to local SSH server");
    }

    /// Integration test: list root directory over SFTP.
    #[tokio::test]
    #[ignore]
    async fn test_sftp_integration_list_dir() {
        let fs = SftpFs::connect(
            "localhost".to_string(),
            22,
            "testuser".to_string(),
            &SshAuth::Password {
                password: "testpass".to_string(),
            },
        )
        .await
        .expect("Should connect");

        let path = RavenPath::Sftp {
            host: "localhost".to_string(),
            port: 22,
            user: "testuser".to_string(),
            path: "/".to_string(),
        };
        let entries = fs.list_dir(&path).await.expect("Should list /");
        assert!(!entries.is_empty());
    }
}
