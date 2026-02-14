use async_trait::async_trait;
use raven_core::error::{RavenError, RavenResult};
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;
use tracing::debug;

use crate::{require_local, PreviewProvider};

/// Generates directory previews by counting items and calculating total size.
pub struct DirectoryPreview;

impl DirectoryPreview {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DirectoryPreview {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PreviewProvider for DirectoryPreview {
    async fn generate(&self, path: &RavenPath) -> RavenResult<PreviewData> {
        let local_path = require_local(path)?;

        let metadata = tokio::fs::metadata(local_path).await.map_err(|e| {
            RavenError::Preview {
                message: format!("failed to read metadata for {}: {}", path, e),
            }
        })?;

        if !metadata.is_dir() {
            return Err(RavenError::NotADirectory {
                path: local_path.clone(),
            });
        }

        let mut item_count: u64 = 0;
        let mut total_size: u64 = 0;

        let mut read_dir = tokio::fs::read_dir(local_path).await.map_err(|e| {
            RavenError::Preview {
                message: format!("failed to read directory {}: {}", path, e),
            }
        })?;

        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            RavenError::Preview {
                message: format!("failed to read directory entry in {}: {}", path, e),
            }
        })? {
            item_count += 1;

            // Attempt to get file size; skip errors (e.g. broken symlinks)
            match entry.metadata().await {
                Ok(meta) => {
                    if meta.is_file() {
                        total_size += meta.len();
                    }
                }
                Err(_) => {
                    // Skip entries whose metadata cannot be read
                }
            }
        }

        debug!(
            path = %path,
            item_count = item_count,
            total_size = total_size,
            "generated directory preview"
        );

        Ok(PreviewData::Directory {
            item_count,
            total_size,
        })
    }

    fn supports(&self, _extension: &str) -> bool {
        // Directory preview is not selected by file extension; it is
        // chosen by the router when the path is a directory.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_returns_false() {
        let preview = DirectoryPreview::new();
        // DirectoryPreview is never selected by extension
        assert!(!preview.supports(""));
        assert!(!preview.supports("dir"));
        assert!(!preview.supports("folder"));
    }
}
