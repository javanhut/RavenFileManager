use async_trait::async_trait;

use raven_core::entry::FileEntry;
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

/// Stub SMB filesystem backend.
///
/// SMB support requires the smbclient C library, which may not be available
/// on all systems. This stub returns meaningful errors for all operations
/// until a proper implementation is added with confirmed library availability.
pub struct SmbFs;

impl SmbFs {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SmbFs {
    fn default() -> Self {
        Self::new()
    }
}

fn smb_unavailable() -> RavenError {
    RavenError::Network {
        message: "SMB support not yet available: requires smbclient libraries".to_string(),
    }
}

#[async_trait]
impl VirtualFileSystem for SmbFs {
    async fn list_dir(&self, _path: &RavenPath) -> RavenResult<Vec<FileEntry>> {
        Err(smb_unavailable())
    }

    async fn stat(&self, _path: &RavenPath) -> RavenResult<FileEntry> {
        Err(smb_unavailable())
    }

    async fn read(&self, _path: &RavenPath) -> RavenResult<Vec<u8>> {
        Err(smb_unavailable())
    }

    async fn write(&self, _path: &RavenPath, _contents: &[u8]) -> RavenResult<()> {
        Err(smb_unavailable())
    }

    async fn copy(
        &self,
        _source: &RavenPath,
        _destination: &RavenPath,
        _progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        Err(smb_unavailable())
    }

    async fn rename(&self, _source: &RavenPath, _destination: &RavenPath) -> RavenResult<()> {
        Err(smb_unavailable())
    }

    async fn delete(&self, _path: &RavenPath) -> RavenResult<()> {
        Err(smb_unavailable())
    }

    async fn create_dir(&self, _path: &RavenPath) -> RavenResult<()> {
        Err(smb_unavailable())
    }

    async fn exists(&self, _path: &RavenPath) -> RavenResult<bool> {
        Err(smb_unavailable())
    }

    fn supports(&self, path: &RavenPath) -> bool {
        matches!(path, RavenPath::Smb { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_smb_stub_returns_network_error() {
        let smb = SmbFs::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/docs".to_string(),
        };

        let err = smb.list_dir(&path).await.unwrap_err();
        assert!(
            matches!(err, RavenError::Network { .. }),
            "Expected Network error, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn test_smb_stub_stat_returns_error() {
        let smb = SmbFs::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/file.txt".to_string(),
        };

        assert!(smb.stat(&path).await.is_err());
    }

    #[tokio::test]
    async fn test_smb_stub_read_returns_error() {
        let smb = SmbFs::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/file.txt".to_string(),
        };

        assert!(smb.read(&path).await.is_err());
    }

    #[tokio::test]
    async fn test_smb_stub_write_returns_error() {
        let smb = SmbFs::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/file.txt".to_string(),
        };

        assert!(smb.write(&path, b"hello").await.is_err());
    }

    #[tokio::test]
    async fn test_smb_stub_exists_returns_error() {
        let smb = SmbFs::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/file.txt".to_string(),
        };

        assert!(smb.exists(&path).await.is_err());
    }

    #[tokio::test]
    async fn test_smb_supports_smb_paths() {
        let smb = SmbFs::new();

        let smb_path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/".to_string(),
        };
        assert!(smb.supports(&smb_path));

        let local_path = RavenPath::local("/tmp");
        assert!(!smb.supports(&local_path));

        let sftp_path = RavenPath::Sftp {
            host: "host".to_string(),
            port: 22,
            user: "user".to_string(),
            path: "/".to_string(),
        };
        assert!(!smb.supports(&sftp_path));
    }

    #[tokio::test]
    async fn test_smb_stub_delete_returns_error() {
        let smb = SmbFs::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/file.txt".to_string(),
        };

        assert!(smb.delete(&path).await.is_err());
    }

    #[tokio::test]
    async fn test_smb_stub_create_dir_returns_error() {
        let smb = SmbFs::new();
        let path = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/newdir".to_string(),
        };

        assert!(smb.create_dir(&path).await.is_err());
    }

    #[tokio::test]
    async fn test_smb_stub_rename_returns_error() {
        let smb = SmbFs::new();
        let src = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/old.txt".to_string(),
        };
        let dst = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/new.txt".to_string(),
        };

        assert!(smb.rename(&src, &dst).await.is_err());
    }

    #[tokio::test]
    async fn test_smb_stub_copy_returns_error() {
        let smb = SmbFs::new();
        let src = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/old.txt".to_string(),
        };
        let dst = RavenPath::Smb {
            host: "server".to_string(),
            share: "share".to_string(),
            path: "/copy.txt".to_string(),
        };

        assert!(smb.copy(&src, &dst, None).await.is_err());
    }
}
