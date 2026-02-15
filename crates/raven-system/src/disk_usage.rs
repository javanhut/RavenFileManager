use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use raven_core::error::{RavenError, RavenResult};
use raven_core::system_types::DiskUsageEntry;

/// Async recursive directory size calculator with streaming progress.
pub struct DiskUsageCalculator;

impl DiskUsageCalculator {
    /// Calculate disk usage of a path, streaming entries through the provided sender.
    ///
    /// Returns `(total_size, total_items)` on completion.
    /// Set `cancelled` to true to abort the calculation early.
    pub async fn calculate(
        path: &Path,
        tx: tokio::sync::mpsc::UnboundedSender<DiskUsageEntry>,
        cancelled: Arc<AtomicBool>,
    ) -> RavenResult<(u64, u64)> {
        let metadata = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|e| RavenError::System {
                message: format!("failed to stat {}: {}", path.display(), e),
            })?;

        if metadata.is_file() {
            let size = metadata.len();
            let _ = tx.send(DiskUsageEntry {
                path: path.to_path_buf(),
                size,
                is_dir: false,
                depth: 0,
            });
            return Ok((size, 1));
        }

        let mut total_size = 0u64;
        let mut total_items = 0u64;

        Self::walk_dir(path, &tx, &cancelled, 0, &mut total_size, &mut total_items).await?;

        Ok((total_size, total_items))
    }

    /// Calculate size of a single file or directory (no streaming).
    pub async fn calculate_single(path: &Path) -> RavenResult<u64> {
        let metadata = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|e| RavenError::System {
                message: format!("failed to stat {}: {}", path.display(), e),
            })?;

        if metadata.is_file() {
            return Ok(metadata.len());
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        let mut total_size = 0u64;
        let mut total_items = 0u64;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        Self::walk_dir(path, &tx, &cancelled, 0, &mut total_size, &mut total_items).await?;

        Ok(total_size)
    }

    fn walk_dir<'a>(
        path: &'a Path,
        tx: &'a tokio::sync::mpsc::UnboundedSender<DiskUsageEntry>,
        cancelled: &'a Arc<AtomicBool>,
        depth: u32,
        total_size: &'a mut u64,
        total_items: &'a mut u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RavenResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if cancelled.load(Ordering::Relaxed) {
                return Err(RavenError::Cancelled);
            }

            let mut dir_size = 0u64;
            let mut entries = match tokio::fs::read_dir(path).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!("Cannot read directory {}: {}", path.display(), e);
                    return Ok(()); // Skip inaccessible directories
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(RavenError::Cancelled);
                }

                let entry_path = entry.path();
                let metadata = match tokio::fs::symlink_metadata(&entry_path).await {
                    Ok(m) => m,
                    Err(_) => continue, // Skip entries we can't stat
                };

                if metadata.is_symlink() {
                    continue; // Don't follow symlinks
                }

                if metadata.is_dir() {
                    Self::walk_dir(&entry_path, tx, cancelled, depth + 1, total_size, total_items)
                        .await?;
                } else {
                    let size = metadata.len();
                    dir_size += size;
                    *total_size += size;
                    *total_items += 1;

                    let _ = tx.send(DiskUsageEntry {
                        path: entry_path,
                        size,
                        is_dir: false,
                        depth: depth + 1,
                    });
                }
            }

            // Send directory entry itself
            *total_items += 1;
            let _ = tx.send(DiskUsageEntry {
                path: path.to_path_buf(),
                size: dir_size,
                is_dir: true,
                depth,
            });

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_calculate_single_file() {
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        write!(tmpfile, "hello world").unwrap();
        let size = DiskUsageCalculator::calculate_single(tmpfile.path())
            .await
            .unwrap();
        assert_eq!(size, 11);
    }

    #[tokio::test]
    async fn test_calculate_directory() {
        let dir = tempfile::TempDir::new().unwrap();

        // Create some files with known sizes
        std::fs::write(dir.path().join("a.txt"), "aaaa").unwrap(); // 4 bytes
        std::fs::write(dir.path().join("b.txt"), "bbbbbbb").unwrap(); // 7 bytes
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("c.txt"), "cc").unwrap(); // 2 bytes

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(false));

        let (total_size, total_items) = DiskUsageCalculator::calculate(dir.path(), tx, cancelled)
            .await
            .unwrap();

        assert_eq!(total_size, 13); // 4 + 7 + 2
        assert_eq!(total_items, 5); // 3 files + 2 dirs (root + sub)

        // Collect all streamed entries
        let mut entries = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            entries.push(entry);
        }

        // Should have entries for: a.txt, b.txt, c.txt, sub/, root/
        assert_eq!(entries.len(), 5);

        let file_entries: Vec<_> = entries.iter().filter(|e| !e.is_dir).collect();
        assert_eq!(file_entries.len(), 3);

        let total_file_size: u64 = file_entries.iter().map(|e| e.size).sum();
        assert_eq!(total_file_size, 13);
    }

    #[tokio::test]
    async fn test_calculate_empty_directory() {
        let dir = tempfile::TempDir::new().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(false));

        let (total_size, total_items) = DiskUsageCalculator::calculate(dir.path(), tx, cancelled)
            .await
            .unwrap();

        assert_eq!(total_size, 0);
        assert_eq!(total_items, 1); // Just the directory itself

        let mut entries = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            entries.push(entry);
        }
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_dir);
    }

    #[tokio::test]
    async fn test_cancellation() {
        let dir = tempfile::TempDir::new().unwrap();
        // Create enough files that we might actually cancel mid-walk
        for i in 0..100 {
            std::fs::write(dir.path().join(format!("file_{}.txt", i)), "data").unwrap();
        }

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(true)); // Cancel immediately

        let result = DiskUsageCalculator::calculate(dir.path(), tx, cancelled).await;
        assert!(result.is_err());
        match result {
            Err(RavenError::Cancelled) => {} // Expected
            other => panic!("Expected Cancelled error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_calculate_single_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let size = DiskUsageCalculator::calculate_single(dir.path())
            .await
            .unwrap();
        assert_eq!(size, 5);
    }

    #[tokio::test]
    async fn test_streaming_entries_match_total() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("x.txt"), "xxxxx").unwrap(); // 5 bytes
        std::fs::write(dir.path().join("y.txt"), "yy").unwrap(); // 2 bytes

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(false));

        let (total_size, _) = DiskUsageCalculator::calculate(dir.path(), tx, cancelled)
            .await
            .unwrap();

        let mut entries = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            entries.push(entry);
        }

        let streamed_file_size: u64 = entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
        assert_eq!(streamed_file_size, total_size);
    }

    #[tokio::test]
    async fn test_nonexistent_path() {
        let result = DiskUsageCalculator::calculate_single(Path::new("/nonexistent/path/12345")).await;
        assert!(result.is_err());
    }
}
