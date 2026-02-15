use serde::{Deserialize, Serialize};

/// A tag rule that automatically categorizes files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRule {
    pub name: String,
    pub icon: String,
    pub color: String,
    pub rules: Vec<TagMatchRule>,
    pub is_smart: bool,
}

/// Criteria for matching files to a tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TagMatchRule {
    Extension { exts: Vec<String> },
    NamePattern { pattern: String },
    MimePrefix { prefix: String },
    SizeRange { min: Option<u64>, max: Option<u64> },
    ModifiedRange { after_days: Option<i64>, before_days: Option<i64> },
}

/// A manual tag assignment for a specific file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualTag {
    pub path: String,
    pub tags: Vec<String>,
}

/// The complete tag database stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TagDatabase {
    #[serde(default)]
    pub tag_rules: Vec<TagRule>,
    #[serde(default)]
    pub manual_tags: Vec<ManualTag>,
}

impl TagRule {
    /// Create default smart tag rules.
    pub fn defaults() -> Vec<TagRule> {
        vec![
            TagRule {
                name: "Large Files".to_string(),
                icon: "drive-harddisk-symbolic".to_string(),
                color: "#e74c3c".to_string(),
                rules: vec![TagMatchRule::SizeRange {
                    min: Some(100 * 1024 * 1024),
                    max: None,
                }],
                is_smart: true,
            },
            TagRule {
                name: "Images".to_string(),
                icon: "image-x-generic-symbolic".to_string(),
                color: "#3498db".to_string(),
                rules: vec![TagMatchRule::Extension {
                    exts: vec![
                        "jpg", "jpeg", "png", "gif", "bmp", "svg", "webp", "ico", "tiff", "raw",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                }],
                is_smart: true,
            },
            TagRule {
                name: "Documents".to_string(),
                icon: "x-office-document-symbolic".to_string(),
                color: "#2ecc71".to_string(),
                rules: vec![TagMatchRule::Extension {
                    exts: vec![
                        "pdf", "doc", "docx", "odt", "txt", "rtf", "md", "tex", "epub",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                }],
                is_smart: true,
            },
            TagRule {
                name: "Code".to_string(),
                icon: "text-x-script-symbolic".to_string(),
                color: "#9b59b6".to_string(),
                rules: vec![TagMatchRule::Extension {
                    exts: vec![
                        "rs", "py", "js", "ts", "java", "go", "c", "cpp", "h", "hpp", "rb",
                        "swift", "kt", "cs", "sh", "bash", "html", "css", "scss", "json",
                        "yaml", "yml", "toml", "xml",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                }],
                is_smart: true,
            },
            TagRule {
                name: "Archives".to_string(),
                icon: "package-x-generic-symbolic".to_string(),
                color: "#f39c12".to_string(),
                rules: vec![TagMatchRule::Extension {
                    exts: vec![
                        "zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst", "lz4",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                }],
                is_smart: true,
            },
            TagRule {
                name: "Videos".to_string(),
                icon: "video-x-generic-symbolic".to_string(),
                color: "#e67e22".to_string(),
                rules: vec![TagMatchRule::Extension {
                    exts: vec![
                        "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ogv",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                }],
                is_smart: true,
            },
            TagRule {
                name: "Audio".to_string(),
                icon: "audio-x-generic-symbolic".to_string(),
                color: "#1abc9c".to_string(),
                rules: vec![TagMatchRule::Extension {
                    exts: vec![
                        "mp3", "flac", "ogg", "wav", "aac", "wma", "m4a", "opus",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                }],
                is_smart: true,
            },
            TagRule {
                name: "Old Files".to_string(),
                icon: "appointment-missed-symbolic".to_string(),
                color: "#95a5a6".to_string(),
                rules: vec![TagMatchRule::ModifiedRange {
                    after_days: None,
                    before_days: Some(365),
                }],
                is_smart: true,
            },
            TagRule {
                name: "Recent".to_string(),
                icon: "document-open-recent-symbolic".to_string(),
                color: "#27ae60".to_string(),
                rules: vec![TagMatchRule::ModifiedRange {
                    after_days: Some(7),
                    before_days: None,
                }],
                is_smart: true,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_count() {
        let defaults = TagRule::defaults();
        assert_eq!(defaults.len(), 9);
    }

    #[test]
    fn test_defaults_all_smart() {
        for rule in TagRule::defaults() {
            assert!(rule.is_smart);
        }
    }

    #[test]
    fn test_defaults_have_names() {
        let defaults = TagRule::defaults();
        let names: Vec<&str> = defaults.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"Large Files"));
        assert!(names.contains(&"Images"));
        assert!(names.contains(&"Documents"));
        assert!(names.contains(&"Code"));
        assert!(names.contains(&"Archives"));
        assert!(names.contains(&"Videos"));
        assert!(names.contains(&"Audio"));
        assert!(names.contains(&"Old Files"));
        assert!(names.contains(&"Recent"));
    }

    #[test]
    fn test_tag_database_default() {
        let db = TagDatabase::default();
        assert!(db.tag_rules.is_empty());
        assert!(db.manual_tags.is_empty());
    }

    #[test]
    fn test_tag_database_serde() {
        let mut db = TagDatabase::default();
        db.tag_rules = TagRule::defaults();
        db.manual_tags.push(ManualTag {
            path: "/home/test/file.txt".to_string(),
            tags: vec!["important".to_string()],
        });
        let serialized = toml::to_string(&db).unwrap();
        let deserialized: TagDatabase = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.tag_rules.len(), 9);
        assert_eq!(deserialized.manual_tags.len(), 1);
    }
}
