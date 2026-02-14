use async_trait::async_trait;

use raven_core::entry::FileEntry;
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

use crate::local::LocalFs;

/// Routes RavenPath to the appropriate VFS implementation.
pub struct VfsRouter {
    local: LocalFs,
}

impl VfsRouter {
    pub fn new() -> Self {
        Self {
            local: LocalFs::new(),
        }
    }

    fn resolve(&self, path: &RavenPath) -> RavenResult<&dyn VirtualFileSystem> {
        if self.local.supports(path) {
            return Ok(&self.local);
        }
        Err(RavenError::UnsupportedProtocol {
            protocol: format!("{}", path),
        })
    }
}

#[async_trait]
impl VirtualFileSystem for VfsRouter {
    async fn list_dir(&self, path: &RavenPath) -> RavenResult<Vec<FileEntry>> {
        self.resolve(path)?.list_dir(path).await
    }

    async fn stat(&self, path: &RavenPath) -> RavenResult<FileEntry> {
        self.resolve(path)?.stat(path).await
    }

    async fn read(&self, path: &RavenPath) -> RavenResult<Vec<u8>> {
        self.resolve(path)?.read(path).await
    }

    async fn write(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        self.resolve(path)?.write(path, contents).await
    }

    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        self.resolve(source)?.copy(source, destination, progress).await
    }

    async fn rename(&self, source: &RavenPath, destination: &RavenPath) -> RavenResult<()> {
        self.resolve(source)?.rename(source, destination).await
    }

    async fn delete(&self, path: &RavenPath) -> RavenResult<()> {
        self.resolve(path)?.delete(path).await
    }

    async fn create_dir(&self, path: &RavenPath) -> RavenResult<()> {
        self.resolve(path)?.create_dir(path).await
    }

    async fn exists(&self, path: &RavenPath) -> RavenResult<bool> {
        self.resolve(path)?.exists(path).await
    }

    fn supports(&self, path: &RavenPath) -> bool {
        self.local.supports(path)
    }
}
