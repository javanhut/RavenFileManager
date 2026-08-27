use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::entry::{EntryKind, FileEntry};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilterSpec {
    pub query: String,
    pub show_hidden: bool,
    pub file_types: Vec<FileTypeFilter>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    #[serde(default)]
    pub modified_after: Option<DateTime<Utc>>,
    #[serde(default)]
    pub modified_before: Option<DateTime<Utc>>,
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTypeFilter {
    Files,
    Directories,
    Symlinks,
    Images,
    Documents,
    Videos,
    Audio,
    Archives,
    Custom(String),
}

impl FilterSpec {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_query(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
            && self.file_types.is_empty()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.modified_after.is_none()
            && self.modified_before.is_none()
            && self.extensions.is_empty()
    }

    /// Whether `entry` satisfies every clause of this spec. An empty spec matches
    /// everything, so callers can apply it unconditionally.
    pub fn matches(&self, entry: &FileEntry) -> bool {
        // Query substring match on name
        if !self.query.is_empty()
            && !entry
                .name
                .to_lowercase()
                .contains(&self.query.to_lowercase())
        {
            return false;
        }

        // File type filter
        if !self.file_types.is_empty() {
            let matches_type = self.file_types.iter().any(|ft| match ft {
                FileTypeFilter::Files => entry.is_file(),
                FileTypeFilter::Directories => entry.is_dir(),
                FileTypeFilter::Symlinks => entry.kind == EntryKind::Symlink,
                FileTypeFilter::Images => matches!(
                    entry.extension(),
                    Some(
                        "jpg"
                            | "jpeg"
                            | "png"
                            | "gif"
                            | "bmp"
                            | "svg"
                            | "webp"
                            | "tiff"
                            | "raw"
                            | "ico"
                    )
                ),
                FileTypeFilter::Videos => matches!(
                    entry.extension(),
                    Some("mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm")
                ),
                FileTypeFilter::Audio => matches!(
                    entry.extension(),
                    Some("mp3" | "flac" | "ogg" | "wav" | "aac" | "wma" | "m4a" | "opus")
                ),
                FileTypeFilter::Documents => matches!(
                    entry.extension(),
                    Some("pdf" | "doc" | "docx" | "odt" | "txt" | "rtf" | "md" | "tex" | "epub")
                ),
                FileTypeFilter::Archives => matches!(
                    entry.extension(),
                    Some("zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst")
                ),
                FileTypeFilter::Custom(_) => true,
            });
            if !matches_type {
                return false;
            }
        }

        // Size filters
        if let Some(min) = self.min_size {
            if entry.metadata.size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if entry.metadata.size > max {
                return false;
            }
        }

        // Date filters
        if let Some(after) = self.modified_after {
            match entry.metadata.modified {
                Some(m) if m >= after => {}
                _ => return false,
            }
        }
        if let Some(before) = self.modified_before {
            match entry.metadata.modified {
                Some(m) if m <= before => {}
                _ => return false,
            }
        }

        // Extension filter
        if !self.extensions.is_empty() {
            match entry.extension() {
                Some(ext) => {
                    if !self.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{EntryKind, EntryMetadata};
    use crate::path::RavenPath;
    use chrono::Duration;
    use std::path::PathBuf;

    fn file(name: &str, size: u64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: RavenPath::Local(PathBuf::from(format!("/test/{}", name))),
            kind: EntryKind::File,
            metadata: EntryMetadata {
                size,
                modified: Some(Utc::now()),
                ..EntryMetadata::default()
            },
        }
    }

    #[test]
    fn empty_spec_matches_everything() {
        let spec = FilterSpec::empty();
        assert!(spec.matches(&file("anything.xyz", 1)));
    }

    #[test]
    fn query_is_case_insensitive_substring() {
        let spec = FilterSpec::with_query("REP");
        assert!(spec.matches(&file("report.pdf", 1)));
        assert!(!spec.matches(&file("notes.txt", 1)));
    }

    #[test]
    fn clauses_are_conjunctive() {
        let spec = FilterSpec {
            query: "photo".to_string(),
            extensions: vec!["png".to_string()],
            ..FilterSpec::default()
        };
        assert!(spec.matches(&file("photo.png", 1)));
        // Right name, wrong extension.
        assert!(!spec.matches(&file("photo.jpg", 1)));
        // Right extension, wrong name.
        assert!(!spec.matches(&file("diagram.png", 1)));
    }

    #[test]
    fn size_range_is_inclusive() {
        let spec = FilterSpec {
            min_size: Some(100),
            max_size: Some(200),
            ..FilterSpec::default()
        };
        assert!(spec.matches(&file("a.bin", 100)));
        assert!(spec.matches(&file("b.bin", 200)));
        assert!(!spec.matches(&file("c.bin", 99)));
        assert!(!spec.matches(&file("d.bin", 201)));
    }

    #[test]
    fn extension_filter_rejects_extensionless_files() {
        let spec = FilterSpec {
            extensions: vec!["rs".to_string()],
            ..FilterSpec::default()
        };
        assert!(spec.matches(&file("main.rs", 1)));
        assert!(!spec.matches(&file("Makefile", 1)));
    }

    #[test]
    fn file_types_are_disjunctive() {
        let spec = FilterSpec {
            file_types: vec![FileTypeFilter::Images, FileTypeFilter::Audio],
            ..FilterSpec::default()
        };
        assert!(spec.matches(&file("a.png", 1)));
        assert!(spec.matches(&file("b.mp3", 1)));
        assert!(!spec.matches(&file("c.rs", 1)));
    }

    #[test]
    fn modified_bounds_reject_missing_timestamps() {
        let mut entry = file("undated.txt", 1);
        entry.metadata.modified = None;
        let spec = FilterSpec {
            modified_after: Some(Utc::now() - Duration::days(1)),
            ..FilterSpec::default()
        };
        assert!(!spec.matches(&entry));
    }
}
