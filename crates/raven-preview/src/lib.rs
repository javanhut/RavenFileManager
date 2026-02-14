pub mod cache;
pub mod directory;
pub mod image_preview;
pub mod router;
pub mod text;

use async_trait::async_trait;
use raven_core::error::RavenResult;
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;

/// Trait for generating file previews.
///
/// Implementors produce a `PreviewData` from a given path and declare
/// which file extensions they can handle.
#[async_trait]
pub trait PreviewProvider: Send + Sync {
    /// Generate a preview for the file at `path`.
    async fn generate(&self, path: &RavenPath) -> RavenResult<PreviewData>;

    /// Return `true` if this provider handles the given file extension
    /// (without the leading dot, e.g. `"rs"`, `"png"`).
    fn supports(&self, extension: &str) -> bool;
}

/// Extract the file extension (lowercase, no dot) from a `RavenPath`.
pub(crate) fn path_extension(path: &RavenPath) -> Option<String> {
    let name = path.file_name()?;
    let dot_pos = name.rfind('.')?;
    if dot_pos == 0 || dot_pos + 1 >= name.len() {
        return None;
    }
    Some(name[dot_pos + 1..].to_ascii_lowercase())
}

/// Extract a local `std::path::PathBuf` from a `RavenPath`, returning an error
/// for non-local paths.
pub(crate) fn require_local(path: &RavenPath) -> RavenResult<&std::path::PathBuf> {
    path.as_local_path().ok_or_else(|| {
        raven_core::error::RavenError::UnsupportedProtocol {
            protocol: format!("preview only supports local paths, got: {}", path),
        }
    })
}
