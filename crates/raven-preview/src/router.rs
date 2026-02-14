use raven_core::error::RavenResult;
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;
use tracing::{debug, warn};

use crate::cache::PreviewCache;
use crate::directory::DirectoryPreview;
use crate::image_preview::ImagePreview;
use crate::text::TextPreview;
use crate::{path_extension, require_local, PreviewProvider};

/// Routes preview requests to the appropriate `PreviewProvider` based
/// on file extension and file type (regular file vs directory).
///
/// For directories the `DirectoryPreview` is used unconditionally.  For
/// regular files the extension is matched against registered providers.
/// Unknown extensions result in `PreviewData::Unsupported`.
pub struct PreviewRouter {
    text: TextPreview,
    image: ImagePreview,
    directory: DirectoryPreview,
    cache: Option<PreviewCache>,
}

impl PreviewRouter {
    /// Create a new router with all built-in providers and no cache.
    pub fn new() -> Self {
        Self {
            text: TextPreview::new(),
            image: ImagePreview::new(),
            directory: DirectoryPreview::new(),
            cache: None,
        }
    }

    /// Create a new router with a custom `TextPreview` configuration.
    pub fn with_text_preview(mut self, text: TextPreview) -> Self {
        self.text = text;
        self
    }

    /// Attach a `PreviewCache` to the router.  When set, the router
    /// will check the cache before generating and store results after.
    pub fn with_cache(mut self, cache: PreviewCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Generate a preview for the given path, selecting the appropriate
    /// provider automatically.
    pub async fn preview(&self, path: &RavenPath) -> RavenResult<PreviewData> {
        // Check the cache first
        if let Some(ref cache) = self.cache {
            if let Some(cached) = cache.get(path).await {
                debug!(path = %path, "returning cached preview");
                return Ok(cached);
            }
        }

        let result = self.generate_uncached(path).await?;

        // Store in cache (best-effort; do not fail the whole operation)
        if let Some(ref cache) = self.cache {
            if let Err(e) = cache.put(path, &result).await {
                warn!(path = %path, error = %e, "failed to cache preview");
            }
        }

        Ok(result)
    }

    /// Generate a preview without consulting the cache.
    async fn generate_uncached(&self, path: &RavenPath) -> RavenResult<PreviewData> {
        // For local paths, check if it's a directory
        if let Ok(local_path) = require_local(path) {
            if let Ok(meta) = tokio::fs::metadata(local_path).await {
                if meta.is_dir() {
                    debug!(path = %path, "routing to directory provider");
                    return self.directory.generate(path).await;
                }
            }
        }

        // Select provider by extension
        if let Some(ext) = path_extension(path) {
            if self.text.supports(&ext) {
                debug!(path = %path, ext = %ext, "routing to text provider");
                return self.text.generate(path).await;
            }

            if self.image.supports(&ext) {
                debug!(path = %path, ext = %ext, "routing to image provider");
                return self.image.generate(path).await;
            }

            // No provider found for this extension
            debug!(path = %path, ext = %ext, "no provider for extension");
            return Ok(PreviewData::Unsupported {
                mime_type: format!("unknown/{}", ext),
            });
        }

        // No extension at all
        debug!(path = %path, "no file extension, returning unsupported");
        Ok(PreviewData::Unsupported {
            mime_type: "unknown/no-extension".to_string(),
        })
    }

    /// Invalidate any cached preview for the given path.
    pub async fn invalidate(&self, path: &RavenPath) {
        if let Some(ref cache) = self.cache {
            cache.invalidate(path).await;
        }
    }

    /// Clear the entire preview cache.
    pub async fn clear_cache(&self) -> RavenResult<()> {
        if let Some(ref cache) = self.cache {
            cache.clear().await?;
        }
        Ok(())
    }
}

impl Default for PreviewRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_default() {
        let router = PreviewRouter::new();
        // Smoke test: the router should be constructible
        assert!(router.cache.is_none());
    }

    #[tokio::test]
    async fn test_unsupported_extension() {
        let router = PreviewRouter::new();
        let path = RavenPath::local("/tmp/test.xyz_unknown_ext");
        let result = router.preview(&path).await;
        assert!(result.is_ok());
        match result.unwrap() {
            PreviewData::Unsupported { mime_type } => {
                assert!(mime_type.contains("xyz_unknown_ext"));
            }
            other => panic!("expected Unsupported, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_no_extension() {
        let router = PreviewRouter::new();
        let path = RavenPath::local("/tmp/Makefile_no_ext_test");
        // This path likely doesn't exist on disk, but the router should
        // still return Unsupported rather than panicking.
        let result = router.preview(&path).await;
        assert!(result.is_ok());
        match result.unwrap() {
            PreviewData::Unsupported { mime_type } => {
                assert_eq!(mime_type, "unknown/no-extension");
            }
            other => panic!("expected Unsupported, got {:?}", other),
        }
    }
}
