use async_trait::async_trait;
use raven_core::error::{RavenError, RavenResult};
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;
use tracing::debug;

use crate::{require_local, PreviewProvider};

/// Image file extensions this provider handles.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif",
];

/// Generates image previews by reading image dimensions and returning
/// metadata suitable for the UI to render a thumbnail.
pub struct ImagePreview;

impl ImagePreview {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ImagePreview {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PreviewProvider for ImagePreview {
    async fn generate(&self, path: &RavenPath) -> RavenResult<PreviewData> {
        let local_path = require_local(path)?.clone();
        let display_path = path.to_string();

        // Image decoding can be CPU-intensive; run in a blocking task.
        let preview = tokio::task::spawn_blocking(move || {
            read_image_dimensions(&local_path)
        })
        .await
        .map_err(|e| RavenError::Preview {
            message: format!("image preview task panicked for {}: {}", display_path, e),
        })??;

        if let PreviewData::Image { width, height, .. } = &preview {
            debug!(
                path = %path,
                width = width,
                height = height,
                "generated image preview"
            );
        }

        Ok(preview)
    }

    fn supports(&self, extension: &str) -> bool {
        let ext_lower = extension.to_ascii_lowercase();
        IMAGE_EXTENSIONS.contains(&ext_lower.as_str())
    }
}

/// Read image dimensions from a local file path.
fn read_image_dimensions(local_path: &std::path::Path) -> RavenResult<PreviewData> {
    // For SVG files, image crate may not support them; return a default size
    if let Some(ext) = local_path.extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("svg") {
            return Ok(PreviewData::Image {
                path: local_path.to_path_buf(),
                width: 0,
                height: 0,
            });
        }
    }

    let reader = image::ImageReader::open(local_path).map_err(|e| RavenError::Preview {
        message: format!("failed to open image {}: {}", local_path.display(), e),
    })?;

    let dimensions = reader.into_dimensions().map_err(|e| RavenError::Preview {
        message: format!(
            "failed to read image dimensions for {}: {}",
            local_path.display(),
            e
        ),
    })?;

    Ok(PreviewData::Image {
        path: local_path.to_path_buf(),
        width: dimensions.0,
        height: dimensions.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_common_image_formats() {
        let preview = ImagePreview::new();
        assert!(preview.supports("png"));
        assert!(preview.supports("jpg"));
        assert!(preview.supports("jpeg"));
        assert!(preview.supports("gif"));
        assert!(preview.supports("webp"));
        assert!(preview.supports("bmp"));
        assert!(preview.supports("svg"));
        assert!(!preview.supports("rs"));
        assert!(!preview.supports("txt"));
    }

    #[test]
    fn test_supports_case_insensitive() {
        let preview = ImagePreview::new();
        assert!(preview.supports("PNG"));
        assert!(preview.supports("Jpg"));
        assert!(preview.supports("WEBP"));
    }
}
