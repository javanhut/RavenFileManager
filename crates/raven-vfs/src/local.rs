use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;

use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

pub struct LocalFs;

impl LocalFs {
    pub fn new() -> Self {
        Self
    }

    fn to_local_path(path: &RavenPath) -> RavenResult<PathBuf> {
        match path {
            RavenPath::Local(p) => Ok(p.clone()),
            _ => Err(RavenError::UnsupportedProtocol {
                protocol: "non-local".to_string(),
            }),
        }
    }

    fn metadata_to_entry(
        name: String,
        path: RavenPath,
        file_type: std::fs::FileType,
        metadata: std::fs::Metadata,
    ) -> FileEntry {
        let kind = if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_block_device() {
            EntryKind::BlockDevice
        } else if file_type.is_char_device() {
            EntryKind::CharDevice
        } else if file_type.is_fifo() {
            EntryKind::Fifo
        } else if file_type.is_socket() {
            EntryKind::Socket
        } else {
            EntryKind::File
        };

        let modified = metadata.modified().ok().map(chrono::DateTime::from);
        let accessed = metadata.accessed().ok().map(chrono::DateTime::from);
        let created = metadata.created().ok().map(chrono::DateTime::from);

        let is_hidden = name.starts_with('.');
        let is_executable = metadata.mode() & 0o111 != 0;

        let entry_meta = EntryMetadata {
            size: metadata.len(),
            modified,
            accessed,
            created,
            permissions: metadata.mode(),
            owner_uid: metadata.uid(),
            group_gid: metadata.gid(),
            mime_type: None,
            symlink_target: None,
            is_hidden,
            is_executable,
        };

        FileEntry::new(name, path, kind, entry_meta)
    }
}

use std::os::unix::fs::FileTypeExt;

#[async_trait]
impl VirtualFileSystem for LocalFs {
    async fn list_dir(&self, path: &RavenPath) -> RavenResult<Vec<FileEntry>> {
        let local_path = Self::to_local_path(path)?;
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&local_path).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().await?;
            let metadata = entry.metadata().await?;
            let entry_path = RavenPath::local(local_path.join(&name));

            entries.push(Self::metadata_to_entry(name, entry_path, file_type, metadata));
        }

        Ok(entries)
    }

    async fn stat(&self, path: &RavenPath) -> RavenResult<FileEntry> {
        let local_path = Self::to_local_path(path)?;
        let metadata = tokio::fs::metadata(&local_path).await?;
        let file_type = metadata.file_type();
        let name = local_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        Ok(Self::metadata_to_entry(name, path.clone(), file_type, metadata))
    }

    async fn read(&self, path: &RavenPath) -> RavenResult<Vec<u8>> {
        let local_path = Self::to_local_path(path)?;
        let contents = tokio::fs::read(&local_path).await?;
        Ok(contents)
    }

    async fn write(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        let local_path = Self::to_local_path(path)?;
        tokio::fs::write(&local_path, contents).await?;
        Ok(())
    }

    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        let src = Self::to_local_path(source)?;
        let dst = Self::to_local_path(destination)?;

        let metadata = tokio::fs::metadata(&src).await?;
        let total = metadata.len();

        let mut src_file = tokio::fs::File::open(&src).await?;
        let mut dst_file = tokio::fs::File::create(&dst).await?;

        let mut buf = vec![0u8; 64 * 1024];
        let mut copied = 0u64;

        loop {
            let n = src_file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            tokio::io::AsyncWriteExt::write_all(&mut dst_file, &buf[..n]).await?;
            copied += n as u64;
            if let Some(ref cb) = progress {
                cb(copied, total);
            }
        }

        Ok(())
    }

    async fn rename(&self, source: &RavenPath, destination: &RavenPath) -> RavenResult<()> {
        let src = Self::to_local_path(source)?;
        let dst = Self::to_local_path(destination)?;
        tokio::fs::rename(&src, &dst).await?;
        Ok(())
    }

    async fn delete(&self, path: &RavenPath) -> RavenResult<()> {
        let local_path = Self::to_local_path(path)?;
        let metadata = tokio::fs::metadata(&local_path).await?;
        if metadata.is_dir() {
            tokio::fs::remove_dir_all(&local_path).await?;
        } else {
            tokio::fs::remove_file(&local_path).await?;
        }
        Ok(())
    }

    async fn create_dir(&self, path: &RavenPath) -> RavenResult<()> {
        let local_path = Self::to_local_path(path)?;
        tokio::fs::create_dir_all(&local_path).await?;
        Ok(())
    }

    async fn exists(&self, path: &RavenPath) -> RavenResult<bool> {
        let local_path = Self::to_local_path(path)?;
        Ok(tokio::fs::try_exists(&local_path).await?)
    }

    fn supports(&self, path: &RavenPath) -> bool {
        path.is_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_list_dir() {
        let fs = LocalFs::new();
        let path = RavenPath::local(PathBuf::from("/tmp"));
        let result = fs.list_dir(&path).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stat() {
        let fs = LocalFs::new();
        let path = RavenPath::local(PathBuf::from("/tmp"));
        let result = fs.stat(&path).await;
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert!(entry.is_dir());
    }

    #[tokio::test]
    async fn test_exists() {
        let fs = LocalFs::new();
        let exists = fs.exists(&RavenPath::local(PathBuf::from("/tmp"))).await.unwrap();
        assert!(exists);
        let exists = fs.exists(&RavenPath::local(PathBuf::from("/nonexistent_path_12345"))).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_create_and_delete() {
        let fs = LocalFs::new();
        let dir = RavenPath::local(PathBuf::from("/tmp/raven_test_dir"));
        let _ = fs.delete(&dir).await;

        fs.create_dir(&dir).await.unwrap();
        assert!(fs.exists(&dir).await.unwrap());

        let file_path = dir.join("test.txt");
        fs.write(&file_path, b"hello").await.unwrap();
        let content = fs.read(&file_path).await.unwrap();
        assert_eq!(content, b"hello");

        fs.delete(&dir).await.unwrap();
        assert!(!fs.exists(&dir).await.unwrap());
    }
}
