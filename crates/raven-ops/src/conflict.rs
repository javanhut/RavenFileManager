use std::path::Path;

use chrono::{DateTime, Utc};
use tracing::{debug, info};

use raven_core::error::{RavenError, RavenResult};
use raven_core::operations::{ConflictInfo, ConflictStrategy, OperationId};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

/// Whether `path` exists.
///
/// Local paths are checked on the filesystem directly; every other backend
/// (SMB, SFTP, ...) is asked through the VFS, so remote destinations get the
/// same conflict handling as local ones.
pub async fn path_exists(vfs: &dyn VirtualFileSystem, path: &RavenPath) -> RavenResult<bool> {
    match path.as_local_path() {
        Some(p) => Ok(tokio::fs::try_exists(p).await?),
        None => vfs.exists(path).await,
    }
}

/// What conflict handling needs to know about an item.
#[derive(Debug, Clone, Copy)]
struct ItemInfo {
    size: u64,
    modified: Option<DateTime<Utc>>,
    is_dir: bool,
}

/// Size, modification time and kind of `path`, from whichever backend holds
/// it, or `None` when nothing is there. One request in the common case, so a
/// remote conflict check does not ask the server the same thing twice.
async fn item_info(vfs: &dyn VirtualFileSystem, path: &RavenPath) -> RavenResult<Option<ItemInfo>> {
    match path.as_local_path() {
        Some(p) => match tokio::fs::metadata(p).await {
            Ok(meta) => Ok(Some(ItemInfo {
                size: meta.len(),
                modified: meta.modified().ok().map(DateTime::<Utc>::from),
                is_dir: meta.is_dir(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A dangling link is still a name that is taken.
                match tokio::fs::symlink_metadata(p).await {
                    Ok(_) => Ok(Some(ItemInfo {
                        size: 0,
                        modified: None,
                        is_dir: false,
                    })),
                    Err(_) => Ok(None),
                }
            }
            Err(e) => Err(e.into()),
        },
        None => match vfs.stat(path).await {
            Ok(entry) => Ok(Some(ItemInfo {
                size: entry.metadata.size,
                modified: entry.metadata.modified,
                is_dir: entry.is_dir(),
            })),
            Err(RavenError::NotFound { .. }) => Ok(None),
            // Not every backend reports a missing path as NotFound, so ask
            // directly before treating the error as real.
            Err(e) => match vfs.exists(path).await? {
                false => Ok(None),
                true => Err(e),
            },
        },
    }
}

/// Whether `a` and `b` name the same file or folder.
///
/// Local paths compare device and inode, so a folder reached through a
/// symlink or a different letter case on a case-insensitive filesystem is
/// recognised. For remote paths only what the names show is used: equal
/// paths, or SMB paths that differ only in case (SMB shares resolve names
/// without case). Aliases a name cannot show (a second host name for one
/// server, a symlinked folder on SFTP) are not detected here.
pub async fn same_object(
    _vfs: &dyn VirtualFileSystem,
    a: &RavenPath,
    b: &RavenPath,
) -> RavenResult<bool> {
    if a == b {
        return Ok(true);
    }
    match (a, b) {
        (RavenPath::Local(x), RavenPath::Local(y)) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                match (tokio::fs::metadata(x).await, tokio::fs::metadata(y).await) {
                    (Ok(m1), Ok(m2)) => Ok(m1.dev() == m2.dev() && m1.ino() == m2.ino()),
                    _ => Ok(false),
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (x, y);
                Ok(false)
            }
        }
        (RavenPath::Smb { .. }, RavenPath::Smb { .. }) => {
            Ok(a.to_string().to_lowercase() == b.to_string().to_lowercase())
        }
        _ => Ok(false),
    }
}

/// Handles file name conflict detection and resolution.
pub struct ConflictResolver {
    /// Default strategy when no user interaction is available.
    default_strategy: ConflictStrategy,
}

impl ConflictResolver {
    /// Create a ConflictResolver with the given default strategy.
    pub fn new(default_strategy: ConflictStrategy) -> Self {
        Self { default_strategy }
    }

    /// Check if a destination path already exists and return conflict info if so.
    ///
    /// Works for every backend: non-local paths are looked up through `vfs`.
    /// Folders conflict the same way files do; overwriting one merges into it.
    pub async fn check_conflict(
        &self,
        vfs: &dyn VirtualFileSystem,
        operation_id: OperationId,
        source: &RavenPath,
        destination: &RavenPath,
    ) -> RavenResult<Option<ConflictInfo>> {
        let Some(dest) = item_info(vfs, destination).await? else {
            return Ok(None);
        };
        let src = item_info(vfs, source).await?.unwrap_or(ItemInfo {
            size: 0,
            modified: None,
            is_dir: false,
        });

        debug!(
            src = %source,
            dst = %destination,
            "conflict detected"
        );

        Ok(Some(ConflictInfo {
            operation_id,
            source: source.clone(),
            destination: destination.clone(),
            source_size: src.size,
            dest_size: dest.size,
            source_is_dir: src.is_dir,
            dest_is_dir: dest.is_dir,
        }))
    }

    /// Resolve a conflict by applying the given strategy.
    ///
    /// Returns the resolved destination path (which may be renamed) and
    /// whether the operation should proceed (`true`) or be skipped (`false`).
    pub async fn resolve(
        &self,
        vfs: &dyn VirtualFileSystem,
        conflict: &ConflictInfo,
        strategy: ConflictStrategy,
    ) -> RavenResult<ConflictResolution> {
        match strategy {
            ConflictStrategy::Overwrite | ConflictStrategy::OverwriteAll => {
                info!(dst = %conflict.destination, "overwriting existing file");
                Ok(ConflictResolution::Proceed {
                    destination: conflict.destination.clone(),
                })
            }

            ConflictStrategy::OverwriteOlder => {
                // A folder's own time says nothing about the files in it:
                // overwriting would merge and replace every nested file,
                // newer ones included. Leave that decision to the user.
                if conflict.source_is_dir || conflict.dest_is_dir {
                    info!(dst = %conflict.destination, "folder conflict: overwrite-older cannot decide");
                    return Ok(ConflictResolution::NeedsInput {
                        conflict: conflict.clone(),
                    });
                }
                let src_modified = item_info(vfs, &conflict.source).await?.and_then(|i| i.modified);
                let dst_modified = item_info(vfs, &conflict.destination).await?.and_then(|i| i.modified);

                match (src_modified, dst_modified) {
                    (Some(src), Some(dst)) if src > dst => {
                        info!(dst = %conflict.destination, "overwriting older file");
                        Ok(ConflictResolution::Proceed {
                            destination: conflict.destination.clone(),
                        })
                    }
                    // Unknown times cannot prove the destination is older.
                    _ => {
                        info!(dst = %conflict.destination, "skipping: destination is not older");
                        Ok(ConflictResolution::Skip)
                    }
                }
            }

            ConflictStrategy::Skip | ConflictStrategy::SkipAll => {
                info!(dst = %conflict.destination, "skipping conflicting file");
                Ok(ConflictResolution::Skip)
            }

            ConflictStrategy::Rename | ConflictStrategy::RenameAll => {
                let new_dest = self.generate_unique_name(vfs, &conflict.destination).await?;
                info!(
                    original = %conflict.destination,
                    renamed = %new_dest,
                    "renamed to avoid conflict"
                );
                Ok(ConflictResolution::Proceed {
                    destination: new_dest,
                })
            }

            ConflictStrategy::Ask => {
                // When strategy is Ask, we return NeedsInput so the caller
                // can prompt the user via the UI.
                Ok(ConflictResolution::NeedsInput {
                    conflict: conflict.clone(),
                })
            }
        }
    }

    /// Apply the default strategy to a conflict.
    pub async fn resolve_with_default(
        &self,
        vfs: &dyn VirtualFileSystem,
        conflict: &ConflictInfo,
    ) -> RavenResult<ConflictResolution> {
        self.resolve(vfs, conflict, self.default_strategy).await
    }

    /// Generate a free name next to `path` by appending a numeric suffix,
    /// checking each candidate on the backend that holds it.
    ///
    /// Example: `file.txt` -> `file (1).txt` -> `file (2).txt`
    pub async fn generate_unique_name(
        &self,
        vfs: &dyn VirtualFileSystem,
        path: &RavenPath,
    ) -> RavenResult<RavenPath> {
        let name = path.file_name().ok_or_else(|| RavenError::Other {
            message: format!("unable to pick a new name for {}", path),
        })?;
        let parent = path.parent().ok_or_else(|| RavenError::Other {
            message: format!("unable to pick a new name for {}", path),
        })?;

        let as_path = Path::new(name);
        let stem = as_path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
        let extension = as_path.extension().and_then(|e| e.to_str());

        // On a remote folder, one listing rules out every taken name at
        // once, so only the pick itself costs a round trip. SMB names are
        // matched without case, as the server matches them.
        let fold = |n: &str| match path {
            RavenPath::Smb { .. } => n.to_lowercase(),
            _ => n.to_string(),
        };
        let taken: std::collections::HashSet<String> = match path {
            RavenPath::Local(_) => Default::default(),
            _ => vfs
                .list_dir(&parent)
                .await
                .map(|entries| entries.iter().map(|e| fold(&e.name)).collect())
                .unwrap_or_default(),
        };

        const MAX_ATTEMPTS: u32 = 10000;
        for counter in 1..=MAX_ATTEMPTS {
            let new_name = match extension {
                Some(ext) => format!("{} ({}).{}", stem, counter, ext),
                None => format!("{} ({})", stem, counter),
            };
            if taken.contains(&fold(&new_name)) {
                continue;
            }
            let candidate = parent.join(&new_name);

            if !path_exists(vfs, &candidate).await? {
                return Ok(candidate);
            }
        }

        Err(RavenError::Other {
            message: format!(
                "unable to generate unique name after {} attempts",
                MAX_ATTEMPTS
            ),
        })
    }

    /// Get the current default strategy.
    pub fn default_strategy(&self) -> ConflictStrategy {
        self.default_strategy
    }

    /// Set a new default strategy.
    pub fn set_default_strategy(&mut self, strategy: ConflictStrategy) {
        self.default_strategy = strategy;
    }
}

impl Default for ConflictResolver {
    fn default() -> Self {
        Self::new(ConflictStrategy::Ask)
    }
}

/// Result of conflict resolution.
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// Proceed with the operation to the given destination.
    Proceed { destination: RavenPath },
    /// Skip this file entirely.
    Skip,
    /// User input is needed to decide.
    NeedsInput { conflict: ConflictInfo },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{remote, MemFs};
    use raven_vfs::local::LocalFs;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_no_conflict_when_dest_missing() {
        let resolver = ConflictResolver::new(ConflictStrategy::Skip);

        // We test the case where dst doesn't exist but src does.
        let dir = PathBuf::from("/tmp/raven_conflict_test_dir");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let src_file = dir.join("src.txt");
        tokio::fs::write(&src_file, b"hello").await.unwrap();

        let dst_file = dir.join("nonexistent_dst.txt");
        let result = resolver
            .check_conflict(
                &LocalFs::new(),
                OperationId(1),
                &RavenPath::local(&src_file),
                &RavenPath::local(&dst_file),
            )
            .await
            .unwrap();

        assert!(result.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_conflict_detected() {
        let resolver = ConflictResolver::new(ConflictStrategy::Skip);
        let dir = PathBuf::from("/tmp/raven_conflict_test_detect");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let src_file = dir.join("src.txt");
        let dst_file = dir.join("dst.txt");
        tokio::fs::write(&src_file, b"source").await.unwrap();
        tokio::fs::write(&dst_file, b"dest").await.unwrap();

        let result = resolver
            .check_conflict(
                &LocalFs::new(),
                OperationId(1),
                &RavenPath::local(&src_file),
                &RavenPath::local(&dst_file),
            )
            .await
            .unwrap();

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.source_size, 6);
        assert_eq!(info.dest_size, 4);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_resolve_skip() {
        let resolver = ConflictResolver::new(ConflictStrategy::Skip);
        let conflict = ConflictInfo {
            operation_id: OperationId(1),
            source: RavenPath::local("/tmp/src.txt"),
            destination: RavenPath::local("/tmp/dst.txt"),
            source_size: 100,
            dest_size: 200,
            source_is_dir: false,
            dest_is_dir: false,
        };

        let result = resolver
            .resolve(&LocalFs::new(), &conflict, ConflictStrategy::Skip)
            .await
            .unwrap();
        assert!(matches!(result, ConflictResolution::Skip));
    }

    #[tokio::test]
    async fn test_generate_unique_name() {
        let dir = PathBuf::from("/tmp/raven_conflict_unique");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let existing = dir.join("file.txt");
        tokio::fs::write(&existing, b"x").await.unwrap();

        let resolver = ConflictResolver::default();
        let unique = resolver
            .generate_unique_name(&LocalFs::new(), &RavenPath::local(&existing))
            .await
            .unwrap();

        let expected = RavenPath::local(dir.join("file (1).txt"));
        assert_eq!(unique, expected);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_remote_conflict_detected_through_vfs() {
        let fs = MemFs::new();
        let src = fs.file("/src/f.txt", b"newer");
        fs.file("/dst/f.txt", b"old");
        let resolver = ConflictResolver::default();

        let info = resolver
            .check_conflict(&fs, OperationId(7), &src, &remote("/dst/f.txt"))
            .await
            .unwrap()
            .expect("an existing remote file is a conflict");
        assert_eq!(info.operation_id, OperationId(7));
        assert_eq!((info.source_size, info.dest_size), (5, 3));

        let free = resolver
            .check_conflict(&fs, OperationId(7), &src, &remote("/dst/g.txt"))
            .await
            .unwrap();
        assert!(free.is_none());

        // Folders conflict too.
        fs.dir("/src/d");
        fs.dir("/dst/d");
        let folder = resolver
            .check_conflict(&fs, OperationId(7), &remote("/src/d"), &remote("/dst/d"))
            .await
            .unwrap();
        assert!(folder.is_some());
    }

    #[tokio::test]
    async fn test_remote_unique_name_skips_taken_names() {
        let fs = MemFs::new();
        fs.file("/dst/f.txt", b"a");
        fs.file("/dst/f (1).txt", b"b");
        fs.dir("/dst/folder");
        let resolver = ConflictResolver::default();

        let unique = resolver
            .generate_unique_name(&fs, &remote("/dst/f.txt"))
            .await
            .unwrap();
        assert_eq!(unique, remote("/dst/f (2).txt"));

        let unique = resolver
            .generate_unique_name(&fs, &remote("/dst/folder"))
            .await
            .unwrap();
        assert_eq!(unique, remote("/dst/folder (1)"));
    }

    #[tokio::test]
    async fn test_overwrite_older_on_folders_asks() {
        let fs = MemFs::new();
        let src = fs.dir("/src/d");
        let dst = fs.dir("/dst/d");
        let resolver = ConflictResolver::new(ConflictStrategy::OverwriteOlder);
        let conflict = resolver
            .check_conflict(&fs, OperationId(1), &src, &dst)
            .await
            .unwrap()
            .unwrap();
        assert!(conflict.source_is_dir && conflict.dest_is_dir);
        let result = resolver.resolve_with_default(&fs, &conflict).await.unwrap();
        assert!(matches!(result, ConflictResolution::NeedsInput { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_same_object_sees_through_local_symlinked_folder() {
        let dir = std::env::temp_dir().join(format!("raven_same_object_{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(dir.join("real")).await.unwrap();
        tokio::fs::write(dir.join("real/f.txt"), b"x").await.unwrap();
        tokio::fs::write(dir.join("real/g.txt"), b"x").await.unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();

        let fs = LocalFs::new();
        let real = RavenPath::local(dir.join("real/f.txt"));
        let aliased = RavenPath::local(dir.join("link/f.txt"));
        let other = RavenPath::local(dir.join("real/g.txt"));
        assert!(same_object(&fs, &real, &aliased).await.unwrap());
        assert!(!same_object(&fs, &real, &other).await.unwrap());

        let smb = MemFs::new();
        assert!(same_object(&smb, &remote("/d/Report.txt"), &remote("/d/report.txt")).await.unwrap());
        assert!(!same_object(&smb, &remote("/d/a.txt"), &remote("/d/b.txt")).await.unwrap());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_remote_overwrite_older_without_times_skips() {
        let fs = MemFs::new();
        let src = fs.file("/src/f.txt", b"new");
        let dst = fs.file("/dst/f.txt", b"old");
        let conflict = ConflictInfo {
            operation_id: OperationId(1),
            source: src,
            destination: dst,
            source_size: 3,
            dest_size: 3,
            source_is_dir: false,
            dest_is_dir: false,
        };
        let result = ConflictResolver::default()
            .resolve(&fs, &conflict, ConflictStrategy::OverwriteOlder)
            .await
            .unwrap();
        assert!(matches!(result, ConflictResolution::Skip));
    }
}
