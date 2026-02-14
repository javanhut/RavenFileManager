use std::path::{Path, PathBuf};

use chrono::Local;
use tracing::{info, warn};

use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;

/// FreeDesktop Trash Specification implementation.
///
/// Moves files to `$HOME/.local/share/Trash/files/` with metadata written
/// to `$HOME/.local/share/Trash/info/<name>.trashinfo`.
pub struct TrashFs {
    trash_dir: PathBuf,
    files_dir: PathBuf,
    info_dir: PathBuf,
}

impl TrashFs {
    /// Create a new TrashFs using the default XDG trash location.
    pub fn new() -> RavenResult<Self> {
        let home = std::env::var("HOME").map_err(|_| RavenError::Other {
            message: "HOME environment variable not set".to_string(),
        })?;
        let trash_dir = PathBuf::from(home).join(".local/share/Trash");
        Self::with_trash_dir(trash_dir)
    }

    /// Create a new TrashFs with a custom trash directory.
    pub fn with_trash_dir(trash_dir: PathBuf) -> RavenResult<Self> {
        let files_dir = trash_dir.join("files");
        let info_dir = trash_dir.join("info");
        Ok(Self {
            trash_dir,
            files_dir,
            info_dir,
        })
    }

    /// Ensure the trash directories exist.
    pub async fn ensure_dirs(&self) -> RavenResult<()> {
        tokio::fs::create_dir_all(&self.files_dir).await?;
        tokio::fs::create_dir_all(&self.info_dir).await?;
        Ok(())
    }

    /// Move a file or directory to the trash.
    ///
    /// Returns a `TrashEntry` describing where the file was placed and its
    /// original location.
    pub async fn trash(&self, path: &Path) -> RavenResult<TrashEntry> {
        self.ensure_dirs().await?;

        // Canonicalize the source path for the .trashinfo Path field
        let canonical = tokio::fs::canonicalize(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RavenError::NotFound {
                    path: path.to_path_buf(),
                }
            } else {
                RavenError::Io { source: e }
            }
        })?;

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RavenError::Other {
                message: format!("unable to determine file name for {}", path.display()),
            })?;

        // Generate a unique name in the trash directory
        let (trash_name, trash_path) = self.unique_trash_name(file_name).await?;

        let deletion_date = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        // Write the .trashinfo file
        let trashinfo_content = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            canonical.display(),
            deletion_date
        );
        let trashinfo_path = self.info_dir.join(format!("{}.trashinfo", trash_name));
        tokio::fs::write(&trashinfo_path, trashinfo_content.as_bytes()).await?;

        // Move the file to the trash files directory
        if let Err(e) = tokio::fs::rename(path, &trash_path).await {
            // rename() fails across filesystems; fall back to copy + delete
            if e.kind() == std::io::ErrorKind::Other
                || e.kind() == std::io::ErrorKind::Unsupported
                || e.raw_os_error() == Some(18)
            // EXDEV
            {
                warn!(
                    "rename across filesystems failed, falling back to copy+delete: {}",
                    e
                );
                self.cross_device_move(path, &trash_path).await?;
            } else {
                // Clean up the trashinfo file we already wrote
                let _ = tokio::fs::remove_file(&trashinfo_path).await;
                return Err(RavenError::Io { source: e });
            }
        }

        let entry = TrashEntry {
            trash_name: trash_name.clone(),
            original_path: canonical,
            trash_file_path: trash_path,
            trashinfo_path,
            deletion_date,
        };

        info!(
            name = %entry.trash_name,
            original = %entry.original_path.display(),
            "moved to trash"
        );

        Ok(entry)
    }

    /// Trash a file given as a RavenPath.
    pub async fn trash_raven_path(&self, path: &RavenPath) -> RavenResult<TrashEntry> {
        let local = path.as_local_path().ok_or_else(|| RavenError::UnsupportedProtocol {
            protocol: format!("trash only supports local paths, got {}", path),
        })?;
        self.trash(local).await
    }

    /// Restore a file from the trash to its original location.
    pub async fn restore(&self, entry: &TrashEntry) -> RavenResult<()> {
        // Ensure the parent directory of the original path exists
        if let Some(parent) = entry.original_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Move from trash back to original location
        if let Err(e) = tokio::fs::rename(&entry.trash_file_path, &entry.original_path).await {
            if e.raw_os_error() == Some(18) {
                // EXDEV: cross device
                self.cross_device_move(&entry.trash_file_path, &entry.original_path)
                    .await?;
            } else {
                return Err(RavenError::Io { source: e });
            }
        }

        // Remove the .trashinfo file
        tokio::fs::remove_file(&entry.trashinfo_path).await?;

        info!(
            name = %entry.trash_name,
            restored_to = %entry.original_path.display(),
            "restored from trash"
        );

        Ok(())
    }

    /// Restore a trashed item by its trash name (e.g., "file.txt" or "file.2.txt").
    pub async fn restore_by_name(&self, trash_name: &str) -> RavenResult<()> {
        let entry = self.read_trash_entry(trash_name).await?;
        self.restore(&entry).await
    }

    /// List all entries in the trash.
    pub async fn list(&self) -> RavenResult<Vec<TrashEntry>> {
        self.ensure_dirs().await?;
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&self.info_dir).await?;

        while let Some(dir_entry) = read_dir.next_entry().await? {
            let file_name = dir_entry.file_name().to_string_lossy().to_string();
            if !file_name.ends_with(".trashinfo") {
                continue;
            }

            let trash_name = file_name.trim_end_matches(".trashinfo").to_string();
            match self.read_trash_entry(&trash_name).await {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    warn!(name = %trash_name, error = %e, "skipping corrupt trash entry");
                }
            }
        }

        Ok(entries)
    }

    /// Permanently delete a trashed item.
    pub async fn delete_permanently(&self, entry: &TrashEntry) -> RavenResult<()> {
        let metadata = tokio::fs::symlink_metadata(&entry.trash_file_path).await?;

        if metadata.is_dir() {
            tokio::fs::remove_dir_all(&entry.trash_file_path).await?;
        } else {
            tokio::fs::remove_file(&entry.trash_file_path).await?;
        }

        tokio::fs::remove_file(&entry.trashinfo_path).await?;

        info!(name = %entry.trash_name, "permanently deleted from trash");
        Ok(())
    }

    /// Empty the entire trash.
    pub async fn empty(&self) -> RavenResult<u64> {
        let entries = self.list().await?;
        let count = entries.len() as u64;

        for entry in &entries {
            if let Err(e) = self.delete_permanently(entry).await {
                warn!(
                    name = %entry.trash_name,
                    error = %e,
                    "failed to delete trash entry"
                );
            }
        }

        info!(count, "trash emptied");
        Ok(count)
    }

    /// Read a .trashinfo file and construct a TrashEntry.
    async fn read_trash_entry(&self, trash_name: &str) -> RavenResult<TrashEntry> {
        let trashinfo_path = self.info_dir.join(format!("{}.trashinfo", trash_name));
        let content = tokio::fs::read_to_string(&trashinfo_path).await?;

        let mut original_path = None;
        let mut deletion_date = None;

        for line in content.lines() {
            if let Some(path_str) = line.strip_prefix("Path=") {
                original_path = Some(PathBuf::from(path_str));
            } else if let Some(date_str) = line.strip_prefix("DeletionDate=") {
                deletion_date = Some(date_str.to_string());
            }
        }

        let original_path = original_path.ok_or_else(|| RavenError::Other {
            message: format!(
                "missing Path in trashinfo: {}",
                trashinfo_path.display()
            ),
        })?;

        let deletion_date = deletion_date.unwrap_or_default();
        let trash_file_path = self.files_dir.join(trash_name);

        Ok(TrashEntry {
            trash_name: trash_name.to_string(),
            original_path,
            trash_file_path,
            trashinfo_path,
            deletion_date,
        })
    }

    /// Generate a unique name inside the trash files directory.
    ///
    /// If "foo.txt" already exists, try "foo.2.txt", "foo.3.txt", etc.
    async fn unique_trash_name(&self, name: &str) -> RavenResult<(String, PathBuf)> {
        let candidate = self.files_dir.join(name);
        if !tokio::fs::try_exists(&candidate).await? {
            return Ok((name.to_string(), candidate));
        }

        let (stem, ext) = match name.rfind('.') {
            Some(pos) => (&name[..pos], Some(&name[pos..])),
            None => (name, None),
        };

        for i in 2u32..100_000 {
            let new_name = match ext {
                Some(ext) => format!("{}.{}{}", stem, i, ext),
                None => format!("{}.{}", stem, i),
            };
            let candidate = self.files_dir.join(&new_name);
            if !tokio::fs::try_exists(&candidate).await? {
                return Ok((new_name, candidate));
            }
        }

        Err(RavenError::Other {
            message: format!(
                "unable to generate unique trash name for {}",
                name
            ),
        })
    }

    /// Copy a file/directory across devices, then remove the source.
    async fn cross_device_move(&self, src: &Path, dst: &Path) -> RavenResult<()> {
        let metadata = tokio::fs::symlink_metadata(src).await?;

        if metadata.is_dir() {
            tokio::fs::create_dir_all(dst).await?;
            let mut read_dir = tokio::fs::read_dir(src).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                let child_src = src.join(entry.file_name());
                let child_dst = dst.join(entry.file_name());
                Box::pin(self.cross_device_move(&child_src, &child_dst)).await?;
            }
            tokio::fs::remove_dir_all(src).await?;
        } else {
            tokio::fs::copy(src, dst).await?;
            tokio::fs::remove_file(src).await?;
        }

        Ok(())
    }

    /// Get the path to the trash directory.
    pub fn trash_dir(&self) -> &Path {
        &self.trash_dir
    }

    /// Get the path to the trash files directory.
    pub fn files_dir(&self) -> &Path {
        &self.files_dir
    }

    /// Get the path to the trash info directory.
    pub fn info_dir(&self) -> &Path {
        &self.info_dir
    }
}

/// Represents a file that has been moved to the trash.
#[derive(Debug, Clone)]
pub struct TrashEntry {
    /// The name of the file inside the trash files directory.
    pub trash_name: String,
    /// The original absolute path of the file.
    pub original_path: PathBuf,
    /// Full path to the file in the trash files directory.
    pub trash_file_path: PathBuf,
    /// Full path to the .trashinfo metadata file.
    pub trashinfo_path: PathBuf,
    /// Deletion date as a string (ISO 8601 local time).
    pub deletion_date: String,
}

impl TrashEntry {
    /// Convert this entry to a RavenPath::Trash.
    pub fn to_raven_path(&self) -> RavenPath {
        RavenPath::Trash {
            original_path: self.original_path.clone(),
            trash_id: self.trash_name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    async fn test_trash_dir() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "raven_trash_test_{}_{}", std::process::id(), id
        ));
        let _ = tokio::fs::remove_dir_all(&path).await;
        tokio::fs::create_dir_all(&path).await.unwrap();
        path
    }

    #[tokio::test]
    async fn test_trash_and_restore() {
        let trash_root = test_trash_dir().await;
        let work_dir = trash_root.join("work");
        let trash_dir = trash_root.join("trash");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let file_path = work_dir.join("test.txt");
        tokio::fs::write(&file_path, b"hello trash").await.unwrap();

        let fs = TrashFs::with_trash_dir(trash_dir.clone()).unwrap();

        // Trash the file
        let entry = fs.trash(&file_path).await.unwrap();
        assert!(!tokio::fs::try_exists(&file_path).await.unwrap());
        assert!(tokio::fs::try_exists(&entry.trash_file_path).await.unwrap());
        assert!(tokio::fs::try_exists(&entry.trashinfo_path).await.unwrap());

        // Read back the trashinfo to verify format
        let info_content = tokio::fs::read_to_string(&entry.trashinfo_path)
            .await
            .unwrap();
        assert!(info_content.starts_with("[Trash Info]\n"));
        assert!(info_content.contains("Path="));
        assert!(info_content.contains("DeletionDate="));

        // Restore the file
        fs.restore(&entry).await.unwrap();
        assert!(tokio::fs::try_exists(&entry.original_path).await.unwrap());
        let content = tokio::fs::read_to_string(&entry.original_path)
            .await
            .unwrap();
        assert_eq!(content, "hello trash");

        let _ = tokio::fs::remove_dir_all(&trash_root).await;
    }

    #[tokio::test]
    async fn test_trash_directory() {
        let trash_root = test_trash_dir().await;
        let work_dir = trash_root.join("work");
        let trash_dir = trash_root.join("trash");
        tokio::fs::create_dir_all(work_dir.join("mydir/sub")).await.unwrap();
        tokio::fs::write(work_dir.join("mydir/a.txt"), b"aaa").await.unwrap();
        tokio::fs::write(work_dir.join("mydir/sub/b.txt"), b"bbb")
            .await
            .unwrap();

        let fs = TrashFs::with_trash_dir(trash_dir).unwrap();
        let entry = fs.trash(&work_dir.join("mydir")).await.unwrap();

        assert!(!tokio::fs::try_exists(work_dir.join("mydir")).await.unwrap());
        assert!(tokio::fs::try_exists(&entry.trash_file_path).await.unwrap());

        fs.restore(&entry).await.unwrap();
        let content = tokio::fs::read_to_string(work_dir.join("mydir/sub/b.txt"))
            .await
            .unwrap();
        assert_eq!(content, "bbb");

        let _ = tokio::fs::remove_dir_all(&trash_root).await;
    }

    #[tokio::test]
    async fn test_list_trash() {
        let trash_root = test_trash_dir().await;
        let work_dir = trash_root.join("work");
        let trash_dir = trash_root.join("trash");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let fs = TrashFs::with_trash_dir(trash_dir).unwrap();

        let f1 = work_dir.join("file1.txt");
        let f2 = work_dir.join("file2.txt");
        tokio::fs::write(&f1, b"one").await.unwrap();
        tokio::fs::write(&f2, b"two").await.unwrap();

        fs.trash(&f1).await.unwrap();
        fs.trash(&f2).await.unwrap();

        let entries = fs.list().await.unwrap();
        assert_eq!(entries.len(), 2);

        let _ = tokio::fs::remove_dir_all(&trash_root).await;
    }

    #[tokio::test]
    async fn test_empty_trash() {
        let trash_root = test_trash_dir().await;
        let work_dir = trash_root.join("work");
        let trash_dir = trash_root.join("trash");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let fs = TrashFs::with_trash_dir(trash_dir).unwrap();

        let f1 = work_dir.join("file1.txt");
        tokio::fs::write(&f1, b"data").await.unwrap();
        fs.trash(&f1).await.unwrap();

        let count = fs.empty().await.unwrap();
        assert_eq!(count, 1);

        let entries = fs.list().await.unwrap();
        assert_eq!(entries.len(), 0);

        let _ = tokio::fs::remove_dir_all(&trash_root).await;
    }

    #[tokio::test]
    async fn test_unique_name_dedup() {
        let trash_root = test_trash_dir().await;
        let work_dir = trash_root.join("work");
        let trash_dir = trash_root.join("trash");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let fs = TrashFs::with_trash_dir(trash_dir).unwrap();
        fs.ensure_dirs().await.unwrap();

        // Create a file with the same name twice
        let f = work_dir.join("dup.txt");
        tokio::fs::write(&f, b"first").await.unwrap();
        let e1 = fs.trash(&f).await.unwrap();
        assert_eq!(e1.trash_name, "dup.txt");

        tokio::fs::write(&f, b"second").await.unwrap();
        let e2 = fs.trash(&f).await.unwrap();
        assert_eq!(e2.trash_name, "dup.2.txt");

        let _ = tokio::fs::remove_dir_all(&trash_root).await;
    }
}
