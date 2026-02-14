use std::path::Path;

use tracing::{debug, info};

use raven_core::error::RavenResult;
use raven_core::operations::{ConflictInfo, ConflictStrategy, OperationId};
use raven_core::path::RavenPath;

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
    pub async fn check_conflict(
        &self,
        operation_id: OperationId,
        source: &RavenPath,
        destination: &RavenPath,
    ) -> RavenResult<Option<ConflictInfo>> {
        let dst_path = match destination.as_local_path() {
            Some(p) => p.clone(),
            None => return Ok(None),
        };

        if !tokio::fs::try_exists(&dst_path).await? {
            return Ok(None);
        }

        let src_path = match source.as_local_path() {
            Some(p) => p.clone(),
            None => return Ok(None),
        };

        let src_meta = tokio::fs::metadata(&src_path).await?;
        let dst_meta = tokio::fs::metadata(&dst_path).await?;

        debug!(
            src = %source,
            dst = %destination,
            "conflict detected"
        );

        Ok(Some(ConflictInfo {
            operation_id,
            source: source.clone(),
            destination: destination.clone(),
            source_size: src_meta.len(),
            dest_size: dst_meta.len(),
        }))
    }

    /// Resolve a conflict by applying the given strategy.
    ///
    /// Returns the resolved destination path (which may be renamed) and
    /// whether the operation should proceed (`true`) or be skipped (`false`).
    pub async fn resolve(
        &self,
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
                let src_path = conflict.source.as_local_path().cloned().unwrap_or_default();
                let dst_path = conflict.destination.as_local_path().cloned().unwrap_or_default();

                let src_modified = tokio::fs::metadata(&src_path).await?.modified()?;
                let dst_modified = tokio::fs::metadata(&dst_path).await?.modified()?;

                if src_modified > dst_modified {
                    info!(dst = %conflict.destination, "overwriting older file");
                    Ok(ConflictResolution::Proceed {
                        destination: conflict.destination.clone(),
                    })
                } else {
                    info!(dst = %conflict.destination, "skipping: destination is newer");
                    Ok(ConflictResolution::Skip)
                }
            }

            ConflictStrategy::Skip | ConflictStrategy::SkipAll => {
                info!(dst = %conflict.destination, "skipping conflicting file");
                Ok(ConflictResolution::Skip)
            }

            ConflictStrategy::Rename | ConflictStrategy::RenameAll => {
                let new_dest = self.generate_unique_name(&conflict.destination).await?;
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
        conflict: &ConflictInfo,
    ) -> RavenResult<ConflictResolution> {
        self.resolve(conflict, self.default_strategy).await
    }

    /// Generate a unique file name by appending a numeric suffix.
    ///
    /// Example: `file.txt` -> `file (1).txt` -> `file (2).txt`
    pub async fn generate_unique_name(&self, path: &RavenPath) -> RavenResult<RavenPath> {
        let local_path = match path.as_local_path() {
            Some(p) => p.clone(),
            None => {
                // For non-local paths, just append a suffix to the file name
                let name = path.file_name().unwrap_or("file");
                let new_name = format!("{} (1)", name);
                if let Some(parent) = path.parent() {
                    return Ok(parent.join(&new_name));
                }
                return Ok(path.clone());
            }
        };

        let stem = local_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let extension = local_path.extension().and_then(|e| e.to_str());
        let parent = local_path.parent().unwrap_or(Path::new("."));

        let mut counter = 1u32;
        loop {
            let new_name = match extension {
                Some(ext) => format!("{} ({}).{}", stem, counter, ext),
                None => format!("{} ({})", stem, counter),
            };
            let candidate = parent.join(&new_name);

            if !tokio::fs::try_exists(&candidate).await? {
                return Ok(RavenPath::local(candidate));
            }

            counter += 1;
            if counter > 10000 {
                return Err(raven_core::error::RavenError::Other {
                    message: format!(
                        "unable to generate unique name after {} attempts",
                        counter
                    ),
                });
            }
        }
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
        };

        let result = resolver.resolve(&conflict, ConflictStrategy::Skip).await.unwrap();
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
            .generate_unique_name(&RavenPath::local(&existing))
            .await
            .unwrap();

        let expected = RavenPath::local(dir.join("file (1).txt"));
        assert_eq!(unique, expected);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
