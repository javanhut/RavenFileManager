use std::path::Path;
use std::time::SystemTime;

use regex::Regex;
use tracing::warn;

use raven_core::automation_types::{Condition, ConditionMode};

/// Evaluate a single condition against a file path. Returns true if the condition matches.
pub fn evaluate_condition(condition: &Condition, file_path: &Path) -> bool {
    match condition {
        Condition::NameMatches { pattern } => {
            let file_name = match file_path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => return false,
            };
            match Regex::new(pattern) {
                Ok(re) => re.is_match(file_name),
                Err(e) => {
                    warn!(pattern, error = %e, "invalid regex in NameMatches condition");
                    false
                }
            }
        }

        Condition::ExtensionIs { extensions } => {
            let ext = match file_path.extension().and_then(|e| e.to_str()) {
                Some(ext) => ext.to_lowercase(),
                None => return false,
            };
            extensions.iter().any(|e| e.to_lowercase() == ext)
        }

        Condition::SizeLargerThan { bytes } => match std::fs::metadata(file_path) {
            Ok(meta) => meta.len() > *bytes,
            Err(e) => {
                warn!(path = %file_path.display(), error = %e, "failed to read metadata for SizeLargerThan");
                false
            }
        },

        Condition::SizeSmallerThan { bytes } => match std::fs::metadata(file_path) {
            Ok(meta) => meta.len() < *bytes,
            Err(e) => {
                warn!(path = %file_path.display(), error = %e, "failed to read metadata for SizeSmallerThan");
                false
            }
        },

        Condition::OlderThan { duration } => match std::fs::metadata(file_path) {
            Ok(meta) => match meta.modified() {
                Ok(modified) => match SystemTime::now().duration_since(modified) {
                    Ok(age) => age > *duration,
                    Err(_) => false,
                },
                Err(e) => {
                    warn!(path = %file_path.display(), error = %e, "failed to read mtime for OlderThan");
                    false
                }
            },
            Err(e) => {
                warn!(path = %file_path.display(), error = %e, "failed to read metadata for OlderThan");
                false
            }
        },

        Condition::NewerThan { duration } => match std::fs::metadata(file_path) {
            Ok(meta) => match meta.modified() {
                Ok(modified) => match SystemTime::now().duration_since(modified) {
                    Ok(age) => age < *duration,
                    Err(_) => {
                        // File modification time is in the future, so it is definitely "newer"
                        true
                    }
                },
                Err(e) => {
                    warn!(path = %file_path.display(), error = %e, "failed to read mtime for NewerThan");
                    false
                }
            },
            Err(e) => {
                warn!(path = %file_path.display(), error = %e, "failed to read metadata for NewerThan");
                false
            }
        },

        Condition::MimeTypeIs { mime_type } => {
            let ext = match file_path.extension().and_then(|e| e.to_str()) {
                Some(ext) => ext.to_lowercase(),
                None => return false,
            };
            let guessed = mime_from_extension(&ext);
            guessed == *mime_type
        }

        Condition::All { conditions } => {
            conditions.iter().all(|c| evaluate_condition(c, file_path))
        }

        Condition::Any { conditions } => {
            conditions.iter().any(|c| evaluate_condition(c, file_path))
        }

        Condition::Not { condition } => !evaluate_condition(condition, file_path),
    }
}

/// Evaluate multiple conditions using the given mode (All/Any).
pub fn evaluate_conditions(
    conditions: &[Condition],
    mode: &ConditionMode,
    file_path: &Path,
) -> bool {
    if conditions.is_empty() {
        return true;
    }

    match mode {
        ConditionMode::All => conditions.iter().all(|c| evaluate_condition(c, file_path)),
        ConditionMode::Any => conditions.iter().any(|c| evaluate_condition(c, file_path)),
    }
}

/// Simple built-in MIME type lookup from file extension.
fn mime_from_extension(ext: &str) -> String {
    match ext {
        // Text
        "txt" | "text" => "text/plain".to_string(),
        "html" | "htm" => "text/html".to_string(),
        "css" => "text/css".to_string(),
        "csv" => "text/csv".to_string(),
        "xml" => "text/xml".to_string(),
        "js" => "text/javascript".to_string(),
        "json" => "application/json".to_string(),
        "md" | "markdown" => "text/markdown".to_string(),
        "rs" => "text/x-rust".to_string(),
        "py" => "text/x-python".to_string(),
        "sh" => "text/x-shellscript".to_string(),
        "toml" => "text/x-toml".to_string(),
        "yaml" | "yml" => "text/x-yaml".to_string(),

        // Images
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "webp" => "image/webp".to_string(),
        "bmp" => "image/bmp".to_string(),
        "ico" => "image/x-icon".to_string(),
        "tiff" | "tif" => "image/tiff".to_string(),

        // Audio
        "mp3" => "audio/mpeg".to_string(),
        "wav" => "audio/wav".to_string(),
        "ogg" => "audio/ogg".to_string(),
        "flac" => "audio/flac".to_string(),
        "aac" => "audio/aac".to_string(),

        // Video
        "mp4" => "video/mp4".to_string(),
        "avi" => "video/x-msvideo".to_string(),
        "mkv" => "video/x-matroska".to_string(),
        "webm" => "video/webm".to_string(),
        "mov" => "video/quicktime".to_string(),

        // Documents
        "pdf" => "application/pdf".to_string(),
        "doc" => "application/msword".to_string(),
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string()
        }
        "xls" => "application/vnd.ms-excel".to_string(),
        "xlsx" => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()
        }
        "ppt" => "application/vnd.ms-powerpoint".to_string(),
        "pptx" => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation".to_string()
        }
        "odt" => "application/vnd.oasis.opendocument.text".to_string(),
        "ods" => "application/vnd.oasis.opendocument.spreadsheet".to_string(),

        // Archives
        "zip" => "application/zip".to_string(),
        "gz" | "gzip" => "application/gzip".to_string(),
        "tar" => "application/x-tar".to_string(),
        "bz2" => "application/x-bzip2".to_string(),
        "xz" => "application/x-xz".to_string(),
        "7z" => "application/x-7z-compressed".to_string(),
        "rar" => "application/vnd.rar".to_string(),

        // Executables / misc
        "wasm" => "application/wasm".to_string(),

        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;

    fn make_test_file(dir: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_name_matches() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "report_2024.pdf", b"data");

        let cond = Condition::NameMatches {
            pattern: r"report_\d+\.pdf".to_string(),
        };
        assert!(evaluate_condition(&cond, &path));

        let cond_no_match = Condition::NameMatches {
            pattern: r"^invoice".to_string(),
        };
        assert!(!evaluate_condition(&cond_no_match, &path));
    }

    #[test]
    fn test_name_matches_invalid_regex() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "test.txt", b"data");

        let cond = Condition::NameMatches {
            pattern: r"[invalid".to_string(),
        };
        assert!(!evaluate_condition(&cond, &path));
    }

    #[test]
    fn test_extension_is() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "photo.JPG", b"image data");

        let cond = Condition::ExtensionIs {
            extensions: vec!["jpg".to_string(), "png".to_string()],
        };
        assert!(evaluate_condition(&cond, &path));

        let cond_no_match = Condition::ExtensionIs {
            extensions: vec!["pdf".to_string()],
        };
        assert!(!evaluate_condition(&cond_no_match, &path));
    }

    #[test]
    fn test_extension_is_no_extension() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "Makefile", b"data");

        let cond = Condition::ExtensionIs {
            extensions: vec!["txt".to_string()],
        };
        assert!(!evaluate_condition(&cond, &path));
    }

    #[test]
    fn test_size_larger_than() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "big.bin", &[0u8; 2048]);

        let cond = Condition::SizeLargerThan { bytes: 1024 };
        assert!(evaluate_condition(&cond, &path));

        let cond_no_match = Condition::SizeLargerThan { bytes: 4096 };
        assert!(!evaluate_condition(&cond_no_match, &path));
    }

    #[test]
    fn test_size_smaller_than() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "small.txt", b"hi");

        let cond = Condition::SizeSmallerThan { bytes: 1024 };
        assert!(evaluate_condition(&cond, &path));

        let cond_no_match = Condition::SizeSmallerThan { bytes: 1 };
        assert!(!evaluate_condition(&cond_no_match, &path));
    }

    #[test]
    fn test_newer_than() {
        let dir = TempDir::new().unwrap();
        // File just created, should be newer than 1 hour
        let path = make_test_file(&dir, "fresh.txt", b"new");

        let cond = Condition::NewerThan {
            duration: Duration::from_secs(3600),
        };
        assert!(evaluate_condition(&cond, &path));

        // File just created should not be newer than 0 seconds
        let cond_no_match = Condition::NewerThan {
            duration: Duration::from_secs(0),
        };
        assert!(!evaluate_condition(&cond_no_match, &path));
    }

    #[test]
    fn test_older_than() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "recent.txt", b"data");

        // File just created, should NOT be older than 1 hour
        let cond = Condition::OlderThan {
            duration: Duration::from_secs(3600),
        };
        assert!(!evaluate_condition(&cond, &path));

        // File just created, should be older than 0 seconds (technically)
        let cond_match = Condition::OlderThan {
            duration: Duration::from_secs(0),
        };
        assert!(evaluate_condition(&cond_match, &path));
    }

    #[test]
    fn test_mime_type_is() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "image.png", b"png data");

        let cond = Condition::MimeTypeIs {
            mime_type: "image/png".to_string(),
        };
        assert!(evaluate_condition(&cond, &path));

        let cond_no_match = Condition::MimeTypeIs {
            mime_type: "text/plain".to_string(),
        };
        assert!(!evaluate_condition(&cond_no_match, &path));
    }

    #[test]
    fn test_mime_type_unknown_extension() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "data.xyz", b"something");

        let cond = Condition::MimeTypeIs {
            mime_type: "application/octet-stream".to_string(),
        };
        assert!(evaluate_condition(&cond, &path));
    }

    #[test]
    fn test_all_combinator() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "report.pdf", &[0u8; 2048]);

        let cond = Condition::All {
            conditions: vec![
                Condition::ExtensionIs {
                    extensions: vec!["pdf".to_string()],
                },
                Condition::SizeLargerThan { bytes: 1024 },
            ],
        };
        assert!(evaluate_condition(&cond, &path));

        // Fails because size is not larger than 4096
        let cond_fail = Condition::All {
            conditions: vec![
                Condition::ExtensionIs {
                    extensions: vec!["pdf".to_string()],
                },
                Condition::SizeLargerThan { bytes: 4096 },
            ],
        };
        assert!(!evaluate_condition(&cond_fail, &path));
    }

    #[test]
    fn test_any_combinator() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "notes.txt", b"hello");

        let cond = Condition::Any {
            conditions: vec![
                Condition::ExtensionIs {
                    extensions: vec!["pdf".to_string()],
                },
                Condition::ExtensionIs {
                    extensions: vec!["txt".to_string()],
                },
            ],
        };
        assert!(evaluate_condition(&cond, &path));

        let cond_fail = Condition::Any {
            conditions: vec![
                Condition::ExtensionIs {
                    extensions: vec!["pdf".to_string()],
                },
                Condition::ExtensionIs {
                    extensions: vec!["doc".to_string()],
                },
            ],
        };
        assert!(!evaluate_condition(&cond_fail, &path));
    }

    #[test]
    fn test_not_combinator() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "readme.md", b"# Hello");

        let cond = Condition::Not {
            condition: Box::new(Condition::ExtensionIs {
                extensions: vec!["pdf".to_string()],
            }),
        };
        assert!(evaluate_condition(&cond, &path));

        let cond_fail = Condition::Not {
            condition: Box::new(Condition::ExtensionIs {
                extensions: vec!["md".to_string()],
            }),
        };
        assert!(!evaluate_condition(&cond_fail, &path));
    }

    #[test]
    fn test_evaluate_conditions_all_mode() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "doc.pdf", &[0u8; 500]);

        let conditions = vec![
            Condition::ExtensionIs {
                extensions: vec!["pdf".to_string()],
            },
            Condition::SizeSmallerThan { bytes: 1024 },
        ];
        assert!(evaluate_conditions(
            &conditions,
            &ConditionMode::All,
            &path
        ));
    }

    #[test]
    fn test_evaluate_conditions_any_mode() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "doc.pdf", &[0u8; 500]);

        let conditions = vec![
            Condition::ExtensionIs {
                extensions: vec!["txt".to_string()],
            },
            Condition::SizeSmallerThan { bytes: 1024 },
        ];
        assert!(evaluate_conditions(
            &conditions,
            &ConditionMode::Any,
            &path
        ));
    }

    #[test]
    fn test_evaluate_conditions_empty() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "anything.bin", b"data");

        // Empty conditions should always match
        assert!(evaluate_conditions(&[], &ConditionMode::All, &path));
        assert!(evaluate_conditions(&[], &ConditionMode::Any, &path));
    }

    #[test]
    fn test_nonexistent_file_size() {
        let path = std::path::PathBuf::from("/tmp/nonexistent_raven_test_file_12345");
        let cond = Condition::SizeLargerThan { bytes: 0 };
        assert!(!evaluate_condition(&cond, &path));
    }

    #[test]
    fn test_nested_combinators() {
        let dir = TempDir::new().unwrap();
        let path = make_test_file(&dir, "photo.jpg", &[0u8; 5000]);

        // (extension is jpg OR png) AND (size > 1024) AND NOT (name matches "temp")
        let cond = Condition::All {
            conditions: vec![
                Condition::Any {
                    conditions: vec![
                        Condition::ExtensionIs {
                            extensions: vec!["jpg".to_string()],
                        },
                        Condition::ExtensionIs {
                            extensions: vec!["png".to_string()],
                        },
                    ],
                },
                Condition::SizeLargerThan { bytes: 1024 },
                Condition::Not {
                    condition: Box::new(Condition::NameMatches {
                        pattern: "temp".to_string(),
                    }),
                },
            ],
        };
        assert!(evaluate_condition(&cond, &path));
    }
}
