use async_trait::async_trait;

use crate::entry::FileEntry;
use crate::error::RavenResult;
use crate::path::RavenPath;

/// Async trait for virtual filesystem implementations.
#[async_trait]
pub trait VirtualFileSystem: Send + Sync {
    /// List entries in a directory.
    async fn list_dir(&self, path: &RavenPath) -> RavenResult<Vec<FileEntry>>;

    /// Get metadata for a single entry.
    async fn stat(&self, path: &RavenPath) -> RavenResult<FileEntry>;

    /// Read file contents as bytes.
    async fn read(&self, path: &RavenPath) -> RavenResult<Vec<u8>>;

    /// Write bytes to a file.
    async fn write(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()>;

    /// Copy a file or directory.
    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()>;

    /// Move/rename a file or directory.
    async fn rename(&self, source: &RavenPath, destination: &RavenPath) -> RavenResult<()>;

    /// Delete a file or directory.
    async fn delete(&self, path: &RavenPath) -> RavenResult<()>;

    /// Create a directory.
    async fn create_dir(&self, path: &RavenPath) -> RavenResult<()>;

    /// Check if a path exists.
    async fn exists(&self, path: &RavenPath) -> RavenResult<bool>;

    /// Check if the VFS supports the given path.
    fn supports(&self, path: &RavenPath) -> bool;
}
