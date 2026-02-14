use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, instrument};

use raven_core::error::RavenResult;

/// Default buffer size for chunked copy: 64 KB.
const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;

/// Progress callback: (bytes_done, bytes_total).
pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// Async chunked copy engine with progress reporting.
pub struct CopyEngine {
    buffer_size: usize,
}

impl CopyEngine {
    /// Create a new CopyEngine with the default buffer size.
    pub fn new() -> Self {
        Self {
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }

    /// Create a new CopyEngine with a custom buffer size.
    pub fn with_buffer_size(buffer_size: usize) -> Self {
        Self { buffer_size }
    }

    /// Copy a single file from `src` to `dst` with optional progress reporting.
    ///
    /// `bytes_offset` is added to `bytes_done` in progress reports, useful when
    /// copying multiple files and tracking cumulative progress.
    #[instrument(skip(self, progress), fields(src = %src.display(), dst = %dst.display()))]
    pub async fn copy_file(
        &self,
        src: &Path,
        dst: &Path,
        bytes_offset: u64,
        bytes_total: u64,
        progress: Option<&ProgressCallback>,
    ) -> RavenResult<u64> {
        let metadata = tokio::fs::metadata(src).await?;
        let file_size = metadata.len();

        // Ensure parent directory exists
        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut src_file = tokio::fs::File::open(src).await?;
        let mut dst_file = tokio::fs::File::create(dst).await?;

        let mut buf = vec![0u8; self.buffer_size];
        let mut copied = 0u64;

        loop {
            let n = src_file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            dst_file.write_all(&buf[..n]).await?;
            copied += n as u64;

            if let Some(cb) = progress {
                cb(bytes_offset + copied, bytes_total);
            }
        }

        dst_file.flush().await?;

        // Preserve permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(metadata.permissions().mode());
            tokio::fs::set_permissions(dst, perms).await?;
        }

        debug!(bytes = copied, "file copy complete");
        Ok(file_size)
    }

    /// Recursively copy a file or directory from `src` to `dst`.
    ///
    /// Returns total bytes copied.
    #[instrument(skip(self, progress), fields(src = %src.display(), dst = %dst.display()))]
    pub async fn copy_recursive(
        &self,
        src: &Path,
        dst: &Path,
        progress: Option<&ProgressCallback>,
    ) -> RavenResult<u64> {
        let total_bytes = self.calculate_size(src).await?;
        let bytes_done = Arc::new(AtomicU64::new(0));
        self.copy_recursive_inner(src, dst, &bytes_done, total_bytes, progress)
            .await?;
        Ok(total_bytes)
    }

    /// Inner recursive copy that tracks cumulative bytes.
    fn copy_recursive_inner<'a>(
        &'a self,
        src: &'a Path,
        dst: &'a Path,
        bytes_done: &'a Arc<AtomicU64>,
        bytes_total: u64,
        progress: Option<&'a ProgressCallback>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RavenResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let metadata = tokio::fs::symlink_metadata(src).await?;

            if metadata.is_dir() {
                tokio::fs::create_dir_all(dst).await?;

                let mut read_dir = tokio::fs::read_dir(src).await?;
                while let Some(entry) = read_dir.next_entry().await? {
                    let entry_name = entry.file_name();
                    let child_src = src.join(&entry_name);
                    let child_dst = dst.join(&entry_name);
                    self.copy_recursive_inner(
                        &child_src,
                        &child_dst,
                        bytes_done,
                        bytes_total,
                        progress,
                    )
                    .await?;
                }

                // Preserve directory permissions
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(metadata.permissions().mode());
                    tokio::fs::set_permissions(dst, perms).await?;
                }
            } else if metadata.is_symlink() {
                let target = tokio::fs::read_link(src).await?;
                // Remove destination if it already exists to avoid errors
                let _ = tokio::fs::remove_file(dst).await;
                tokio::fs::symlink(&target, dst).await?;
            } else {
                let current_done = bytes_done.load(Ordering::SeqCst);
                let file_size = self
                    .copy_file(src, dst, current_done, bytes_total, progress)
                    .await?;
                bytes_done.fetch_add(file_size, Ordering::SeqCst);
            }

            Ok(())
        })
    }

    /// Calculate total size of a file or directory tree.
    pub async fn calculate_size(&self, path: &Path) -> RavenResult<u64> {
        let metadata = tokio::fs::symlink_metadata(path).await?;
        if metadata.is_dir() {
            let mut total = 0u64;
            let mut read_dir = tokio::fs::read_dir(path).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                let child = path.join(entry.file_name());
                total += Box::pin(self.calculate_size(&child)).await?;
            }
            Ok(total)
        } else if metadata.is_file() {
            Ok(metadata.len())
        } else {
            Ok(0)
        }
    }

    /// Calculate total size for a list of paths.
    pub async fn calculate_total_size(&self, paths: &[PathBuf]) -> RavenResult<u64> {
        let mut total = 0u64;
        for path in paths {
            total += self.calculate_size(path).await?;
        }
        Ok(total)
    }
}

impl Default for CopyEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn test_copy_single_file() {
        let dir = tempdir().await;
        let src = dir.join("source.txt");
        let dst = dir.join("dest.txt");

        tokio::fs::write(&src, b"hello world").await.unwrap();

        let engine = CopyEngine::new();
        let last_done = Arc::new(AtomicU64::new(0));
        let last_done_clone = last_done.clone();
        let cb: ProgressCallback = Arc::new(move |done, _total| {
            last_done_clone.store(done, Ordering::SeqCst);
        });

        engine
            .copy_file(&src, &dst, 0, 11, Some(&cb))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&dst).await.unwrap();
        assert_eq!(content, "hello world");
        assert_eq!(last_done.load(Ordering::SeqCst), 11);
    }

    #[tokio::test]
    async fn test_copy_recursive_directory() {
        let dir = tempdir().await;
        let src_dir = dir.join("src_dir");
        let dst_dir = dir.join("dst_dir");

        tokio::fs::create_dir_all(src_dir.join("sub")).await.unwrap();
        tokio::fs::write(src_dir.join("a.txt"), b"aaa").await.unwrap();
        tokio::fs::write(src_dir.join("sub/b.txt"), b"bbbbb")
            .await
            .unwrap();

        let engine = CopyEngine::new();
        let total = engine.copy_recursive(&src_dir, &dst_dir, None).await.unwrap();
        assert_eq!(total, 8); // 3 + 5

        let a = tokio::fs::read_to_string(dst_dir.join("a.txt")).await.unwrap();
        assert_eq!(a, "aaa");
        let b = tokio::fs::read_to_string(dst_dir.join("sub/b.txt"))
            .await
            .unwrap();
        assert_eq!(b, "bbbbb");
    }

    #[tokio::test]
    async fn test_calculate_size() {
        let dir = tempdir().await;
        let sub = dir.join("sub");
        tokio::fs::create_dir_all(&sub).await.unwrap();
        tokio::fs::write(dir.join("a.txt"), b"1234").await.unwrap();
        tokio::fs::write(sub.join("b.txt"), b"56789").await.unwrap();

        let engine = CopyEngine::new();
        let size = engine.calculate_size(&dir).await.unwrap();
        assert_eq!(size, 9);
    }

    static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    async fn tempdir() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "raven_ops_test_{}_{}",
            std::process::id(),
            id
        ));
        let _ = tokio::fs::remove_dir_all(&path).await;
        tokio::fs::create_dir_all(&path).await.unwrap();
        path
    }
}
