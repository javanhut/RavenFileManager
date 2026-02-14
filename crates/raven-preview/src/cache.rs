use std::path::{Path, PathBuf};

use raven_core::error::{RavenError, RavenResult};
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// On-disk cache for generated preview data.
///
/// Previews are stored under `$XDG_CACHE_HOME/raven/previews/` (or
/// `~/.cache/raven/previews/` if `XDG_CACHE_HOME` is unset).  Each
/// entry is keyed by a BLAKE3 hash of the file's absolute path.
///
/// Cache entries are invalidated when the source file's modification
/// time is newer than the cached entry.
pub struct PreviewCache {
    cache_dir: PathBuf,
}

/// Serializable representation of a cached preview entry.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    /// Seconds since UNIX epoch of the source file's last modification.
    source_mtime_secs: i64,
    /// The preview data itself.
    preview: CachedPreviewData,
}

/// Mirror of `PreviewData` that derives Serialize/Deserialize.
#[derive(Debug, Serialize, Deserialize)]
enum CachedPreviewData {
    Text {
        content: String,
        language: Option<String>,
    },
    Image {
        path: PathBuf,
        width: u32,
        height: u32,
    },
    Directory {
        item_count: u64,
        total_size: u64,
    },
    Unsupported {
        mime_type: String,
    },
}

impl From<&PreviewData> for CachedPreviewData {
    fn from(data: &PreviewData) -> Self {
        match data {
            PreviewData::Text { content, language } => CachedPreviewData::Text {
                content: content.clone(),
                language: language.clone(),
            },
            PreviewData::Image {
                path,
                width,
                height,
            } => CachedPreviewData::Image {
                path: path.clone(),
                width: *width,
                height: *height,
            },
            PreviewData::Directory {
                item_count,
                total_size,
            } => CachedPreviewData::Directory {
                item_count: *item_count,
                total_size: *total_size,
            },
            PreviewData::Unsupported { mime_type } => CachedPreviewData::Unsupported {
                mime_type: mime_type.clone(),
            },
        }
    }
}

impl From<CachedPreviewData> for PreviewData {
    fn from(cached: CachedPreviewData) -> Self {
        match cached {
            CachedPreviewData::Text { content, language } => {
                PreviewData::Text { content, language }
            }
            CachedPreviewData::Image {
                path,
                width,
                height,
            } => PreviewData::Image {
                path,
                width,
                height,
            },
            CachedPreviewData::Directory {
                item_count,
                total_size,
            } => PreviewData::Directory {
                item_count,
                total_size,
            },
            CachedPreviewData::Unsupported { mime_type } => {
                PreviewData::Unsupported { mime_type }
            }
        }
    }
}

impl PreviewCache {
    /// Create a new `PreviewCache`, using the standard cache directory.
    ///
    /// The directory is created on disk if it does not exist.
    pub async fn new() -> RavenResult<Self> {
        let cache_dir = default_cache_dir();
        Self::with_dir(cache_dir).await
    }

    /// Create a new `PreviewCache` using an explicit directory path.
    pub async fn with_dir(cache_dir: PathBuf) -> RavenResult<Self> {
        tokio::fs::create_dir_all(&cache_dir).await.map_err(|e| {
            RavenError::Preview {
                message: format!(
                    "failed to create preview cache directory {}: {}",
                    cache_dir.display(),
                    e
                ),
            }
        })?;

        debug!(dir = %cache_dir.display(), "preview cache initialised");
        Ok(Self { cache_dir })
    }

    /// Look up a cached preview for the given `RavenPath`.
    ///
    /// Returns `None` if there is no cached entry or if the cache is
    /// stale (source file has been modified since the entry was written).
    pub async fn get(&self, path: &RavenPath) -> Option<PreviewData> {
        let cache_file = self.cache_path(path);
        let contents = tokio::fs::read_to_string(&cache_file).await.ok()?;
        let entry: CacheEntry = serde_json::from_str(&contents).ok()?;

        // Validate modification time against the source file
        if let Some(local_path) = path.as_local_path() {
            if let Ok(meta) = tokio::fs::metadata(local_path).await {
                if let Ok(mtime) = meta.modified() {
                    let source_secs = mtime
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    if source_secs > entry.source_mtime_secs {
                        debug!(
                            path = %path,
                            "cache entry stale (source mtime {} > cached {})",
                            source_secs,
                            entry.source_mtime_secs
                        );
                        return None;
                    }
                }
            }
        }

        debug!(path = %path, "cache hit");
        Some(entry.preview.into())
    }

    /// Store a preview in the cache for the given `RavenPath`.
    pub async fn put(&self, path: &RavenPath, preview: &PreviewData) -> RavenResult<()> {
        let source_mtime_secs = source_mtime(path).await;

        let entry = CacheEntry {
            source_mtime_secs,
            preview: CachedPreviewData::from(preview),
        };

        let json = serde_json::to_string(&entry).map_err(|e| RavenError::Preview {
            message: format!("failed to serialise cache entry: {}", e),
        })?;

        let cache_file = self.cache_path(path);
        tokio::fs::write(&cache_file, json).await.map_err(|e| {
            RavenError::Preview {
                message: format!(
                    "failed to write cache file {}: {}",
                    cache_file.display(),
                    e
                ),
            }
        })?;

        debug!(path = %path, cache_file = %cache_file.display(), "cached preview");
        Ok(())
    }

    /// Remove a specific cache entry.
    pub async fn invalidate(&self, path: &RavenPath) {
        let cache_file = self.cache_path(path);
        if let Err(e) = tokio::fs::remove_file(&cache_file).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    path = %path,
                    error = %e,
                    "failed to remove cache file"
                );
            }
        }
    }

    /// Remove all cached previews.
    pub async fn clear(&self) -> RavenResult<()> {
        let mut entries = tokio::fs::read_dir(&self.cache_dir).await.map_err(|e| {
            RavenError::Preview {
                message: format!(
                    "failed to read cache directory {}: {}",
                    self.cache_dir.display(),
                    e
                ),
            }
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            RavenError::Preview {
                message: format!("failed to iterate cache directory: {}", e),
            }
        })? {
            let entry_path = entry.path();
            if entry_path.extension().and_then(|e| e.to_str()) == Some("json") {
                let _ = tokio::fs::remove_file(&entry_path).await;
            }
        }

        debug!("cleared preview cache");
        Ok(())
    }

    /// Compute the cache file path for a given `RavenPath`.
    fn cache_path(&self, path: &RavenPath) -> PathBuf {
        let key = cache_key(path);
        self.cache_dir.join(format!("{}.json", key))
    }
}

/// Compute a BLAKE3-based cache key from the `RavenPath` display string.
fn cache_key(path: &RavenPath) -> String {
    let hash = blake3::hash(path.to_string().as_bytes());
    hash.to_hex().to_string()
}

/// Resolve the default cache directory (`$XDG_CACHE_HOME/raven/previews/`).
fn default_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            Path::new(&home).join(".cache")
        });
    base.join("raven").join("previews")
}

/// Get the modification time of a local file as seconds since UNIX epoch.
/// Returns 0 if the path is not local or metadata cannot be read.
async fn source_mtime(path: &RavenPath) -> i64 {
    if let Some(local_path) = path.as_local_path() {
        if let Ok(meta) = tokio::fs::metadata(local_path).await {
            if let Ok(mtime) = meta.modified() {
                return mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_deterministic() {
        let path = RavenPath::local("/home/user/test.rs");
        let key1 = cache_key(&path);
        let key2 = cache_key(&path);
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_cache_key_different_paths() {
        let path1 = RavenPath::local("/home/user/a.rs");
        let path2 = RavenPath::local("/home/user/b.rs");
        assert_ne!(cache_key(&path1), cache_key(&path2));
    }

    #[test]
    fn test_default_cache_dir() {
        let dir = default_cache_dir();
        assert!(dir.to_string_lossy().contains("raven"));
        assert!(dir.to_string_lossy().contains("previews"));
    }

    #[test]
    fn test_cached_preview_roundtrip() {
        let preview = PreviewData::Text {
            content: "hello world".to_string(),
            language: Some("Rust".to_string()),
        };
        let cached: CachedPreviewData = (&preview).into();
        let restored: PreviewData = cached.into();

        match restored {
            PreviewData::Text { content, language } => {
                assert_eq!(content, "hello world");
                assert_eq!(language, Some("Rust".to_string()));
            }
            _ => panic!("expected Text variant"),
        }
    }
}
