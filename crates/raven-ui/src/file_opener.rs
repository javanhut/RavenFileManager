use std::path::Path;

use raven_core::config::AppConfig;
use raven_core::path::RavenPath;

/// Open a file using configured file associations, falling back to xdg-open.
pub fn open_file(path: &RavenPath, config: &AppConfig) {
    let local = match path.as_local_path() {
        Some(p) => p,
        None => return,
    };

    let extension = local
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());

    // Check file associations for a matching extension
    for assoc in &config.file_associations {
        let ext_match = extension
            .as_deref()
            .map(|ext| assoc.extensions.iter().any(|a| a.eq_ignore_ascii_case(ext)))
            .unwrap_or(false);

        if ext_match {
            spawn_application(&assoc.application, local);
            return;
        }
    }

    // Check file associations for a matching MIME pattern
    if let Some(ref ext) = extension {
        let mime = mime_from_extension(ext);
        for assoc in &config.file_associations {
            if !assoc.mime_pattern.is_empty() && mime_matches(&assoc.mime_pattern, &mime) {
                spawn_application(&assoc.application, local);
                return;
            }
        }
    }

    // Fallback: use system default
    let uri = format!("file://{}", local.display());
    if let Err(e) = open::that(&uri) {
        tracing::warn!("Failed to open {}: {}", uri, e);
    }
}

/// Spawn an application command, replacing `%f` with the file path.
fn spawn_application(command: &str, file_path: &Path) {
    let path_str = file_path.to_string_lossy();
    let full_cmd = if command.contains("%f") {
        command.replace("%f", &path_str)
    } else {
        format!("{} {}", command, shell_escape(&path_str))
    };

    tracing::info!("Opening with: {}", full_cmd);
    if let Err(e) = std::process::Command::new("sh")
        .arg("-c")
        .arg(&full_cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        tracing::warn!("Failed to spawn '{}': {}", full_cmd, e);
        // Fallback to open::that
        let uri = format!("file://{}", file_path.display());
        let _ = open::that(&uri);
    }
}

/// Simple shell escaping: wrap in single quotes, escaping existing single quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Check if a MIME pattern matches a MIME type (supports `*` wildcards like `text/*`).
fn mime_matches(pattern: &str, mime: &str) -> bool {
    if pattern == "*" || pattern == "*/*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        // e.g., "text/*" matches "text/plain"
        if let Some(mime_prefix) = mime.split('/').next() {
            return prefix == mime_prefix;
        }
    }
    pattern == mime
}

/// Map common file extensions to MIME types.
fn mime_from_extension(ext: &str) -> String {
    match ext {
        // Text
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "xml" => "text/xml",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/x-yaml",
        // Code
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "go" | "java" | "rb" | "sh" => {
            "text/x-script"
        }
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        // Audio
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        // Video
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        // Documents
        "pdf" => "application/pdf",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        // Archives
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "xz" => "application/x-xz",
        "bz2" => "application/x-bzip2",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Get matching file associations for a given file entry (used for "Open With" submenu).
pub fn get_matching_associations(
    config: &AppConfig,
    extension: Option<&str>,
) -> Vec<(usize, String)> {
    let mut matches = Vec::new();
    let mime = extension.map(|e| mime_from_extension(&e.to_lowercase()));

    for (i, assoc) in config.file_associations.iter().enumerate() {
        let ext_match = extension
            .map(|ext| assoc.extensions.iter().any(|a| a.eq_ignore_ascii_case(ext)))
            .unwrap_or(false);

        let mime_match = mime
            .as_deref()
            .map(|m| !assoc.mime_pattern.is_empty() && mime_matches(&assoc.mime_pattern, m))
            .unwrap_or(false);

        if ext_match || mime_match {
            matches.push((i, assoc.label.clone()));
        }
    }
    matches
}

/// Open a file with a specific association index from config.
pub fn open_with_association(path: &RavenPath, config: &AppConfig, index: usize) {
    let local = match path.as_local_path() {
        Some(p) => p,
        None => return,
    };

    if let Some(assoc) = config.file_associations.get(index) {
        spawn_application(&assoc.application, local);
    }
}
