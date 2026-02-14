use async_trait::async_trait;
use raven_core::error::{RavenError, RavenResult};
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;
use syntect::parsing::SyntaxSet;
use tracing::{debug, warn};

use crate::{path_extension, require_local, PreviewProvider};

/// Default maximum file size to read for text previews (1 MiB).
const DEFAULT_MAX_SIZE: u64 = 1024 * 1024;

/// Text file extensions that this provider handles.
const TEXT_EXTENSIONS: &[&str] = &[
    // Programming languages
    "rs", "py", "js", "ts", "jsx", "tsx", "go", "c", "cpp", "cc", "cxx", "h", "hpp",
    "java", "kt", "kts", "scala", "rb", "php", "swift", "m", "mm", "cs", "fs", "fsx",
    "zig", "nim", "d", "r", "jl", "lua", "pl", "pm", "ex", "exs", "erl", "hrl",
    "hs", "lhs", "ml", "mli", "clj", "cljs", "cljc", "lisp", "el", "scm", "rkt",
    "v", "sv", "vhd", "vhdl",
    // Shell / scripting
    "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
    // Web
    "html", "htm", "css", "scss", "sass", "less", "vue", "svelte",
    // Data / config
    "json", "yaml", "yml", "toml", "xml", "csv", "tsv", "ini", "cfg", "conf",
    "properties",
    // Markup / docs
    "md", "markdown", "rst", "adoc", "tex", "latex", "org", "txt", "log",
    // Build / CI
    "cmake", "makefile", "dockerfile", "gradle", "sbt",
    // SQL
    "sql",
    // Misc
    "diff", "patch", "graphql", "gql", "proto", "thrift", "nix",
];

/// Generates syntax-highlighted text previews using `syntect`.
pub struct TextPreview {
    syntax_set: SyntaxSet,
    max_size: u64,
}

impl TextPreview {
    /// Create a new `TextPreview` with the default maximum file size (1 MiB).
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            max_size: DEFAULT_MAX_SIZE,
        }
    }

    /// Create a new `TextPreview` with a custom maximum file size.
    pub fn with_max_size(max_size: u64) -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            max_size,
        }
    }

    /// Detect the language name for the given file extension using syntect.
    fn detect_language(&self, extension: &str) -> Option<String> {
        // Syntect's find_syntax_by_extension uses extension without the dot
        self.syntax_set
            .find_syntax_by_extension(extension)
            .map(|syntax| syntax.name.clone())
    }
}

impl Default for TextPreview {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PreviewProvider for TextPreview {
    async fn generate(&self, path: &RavenPath) -> RavenResult<PreviewData> {
        let local_path = require_local(path)?;

        let metadata = tokio::fs::metadata(local_path).await.map_err(|e| {
            RavenError::Preview {
                message: format!("failed to read metadata for {}: {}", path, e),
            }
        })?;

        let file_size = metadata.len();
        if file_size > self.max_size {
            debug!(
                path = %path,
                size = file_size,
                max = self.max_size,
                "file exceeds max preview size, truncating"
            );
        }

        // Read the file, truncating at max_size bytes
        let bytes = tokio::fs::read(local_path).await.map_err(|e| {
            RavenError::Preview {
                message: format!("failed to read {}: {}", path, e),
            }
        })?;

        let content = if bytes.len() as u64 > self.max_size {
            // Truncate at a valid UTF-8 boundary
            let truncated = &bytes[..self.max_size as usize];
            match std::str::from_utf8(truncated) {
                Ok(s) => s.to_string(),
                Err(e) => {
                    // Use the valid portion up to the error point
                    let valid_up_to = e.valid_up_to();
                    String::from_utf8_lossy(&truncated[..valid_up_to]).into_owned()
                }
            }
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };

        // Detect language from extension
        let extension = path_extension(path);
        let language = extension
            .as_deref()
            .and_then(|ext| self.detect_language(ext));

        if language.is_none() {
            warn!(path = %path, "could not detect language for text preview");
        }

        debug!(
            path = %path,
            language = ?language,
            content_len = content.len(),
            "generated text preview"
        );

        Ok(PreviewData::Text { content, language })
    }

    fn supports(&self, extension: &str) -> bool {
        let ext_lower = extension.to_ascii_lowercase();
        TEXT_EXTENSIONS.contains(&ext_lower.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_common_extensions() {
        let preview = TextPreview::new();
        assert!(preview.supports("rs"));
        assert!(preview.supports("py"));
        assert!(preview.supports("js"));
        assert!(preview.supports("toml"));
        assert!(preview.supports("md"));
        assert!(!preview.supports("png"));
        assert!(!preview.supports("exe"));
    }

    #[test]
    fn test_supports_case_insensitive() {
        let preview = TextPreview::new();
        assert!(preview.supports("RS"));
        assert!(preview.supports("Py"));
        assert!(preview.supports("JSON"));
    }

    #[test]
    fn test_detect_language() {
        let preview = TextPreview::new();
        let lang = preview.detect_language("rs");
        assert!(lang.is_some());
        assert_eq!(lang.unwrap(), "Rust");

        let lang = preview.detect_language("py");
        assert!(lang.is_some());
        assert_eq!(lang.unwrap(), "Python");
    }
}
