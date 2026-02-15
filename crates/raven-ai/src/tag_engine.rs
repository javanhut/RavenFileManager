use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use raven_core::entry::FileEntry;

use crate::tags::{ManualTag, TagDatabase, TagMatchRule, TagRule};

/// Engine for computing and managing file tags.
pub struct TagEngine {
    db: TagDatabase,
    db_path: PathBuf,
}

impl TagEngine {
    /// Create a new TagEngine, loading from the given path or using defaults.
    pub fn new(db_path: PathBuf) -> Self {
        let db = Self::load_or_default(&db_path);
        Self { db, db_path }
    }

    /// Create a TagEngine with default tags (for testing).
    pub fn with_defaults() -> Self {
        Self {
            db: TagDatabase {
                tag_rules: TagRule::defaults(),
                manual_tags: Vec::new(),
            },
            db_path: PathBuf::from("/tmp/raven-tags-test.toml"),
        }
    }

    fn load_or_default(path: &Path) -> TagDatabase {
        if path.exists() {
            match std::fs::read_to_string(path) {
                Ok(content) => match toml::from_str(&content) {
                    Ok(db) => return db,
                    Err(e) => {
                        tracing::warn!("Failed to parse tag database: {}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read tag database: {}", e);
                }
            }
        }
        TagDatabase {
            tag_rules: TagRule::defaults(),
            manual_tags: Vec::new(),
        }
    }

    /// Get all tags that apply to a given file entry.
    pub fn tags_for_entry(&self, entry: &FileEntry) -> Vec<String> {
        let mut tags = Vec::new();

        // Check smart/rule-based tags
        for rule in &self.db.tag_rules {
            if self.entry_matches_rule(entry, rule) {
                tags.push(rule.name.clone());
            }
        }

        // Check manual tags
        let path_str = entry.path.to_string();
        for manual in &self.db.manual_tags {
            if manual.path == path_str {
                for tag in &manual.tags {
                    if !tags.contains(tag) {
                        tags.push(tag.clone());
                    }
                }
            }
        }

        tags
    }

    /// Count how many entries match each tag.
    pub fn tag_counts(&self, entries: &[FileEntry]) -> Vec<(String, usize)> {
        let mut counts: HashMap<String, usize> = HashMap::new();

        for entry in entries {
            let tags = self.tags_for_entry(entry);
            for tag in tags {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }

        let mut result: Vec<(String, usize)> = counts.into_iter().collect();
        result.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        result
    }

    /// Add a manual tag to a file path.
    pub fn add_manual_tag(&mut self, path: &str, tag: &str) {
        if let Some(manual) = self.db.manual_tags.iter_mut().find(|m| m.path == path) {
            if !manual.tags.contains(&tag.to_string()) {
                manual.tags.push(tag.to_string());
            }
        } else {
            self.db.manual_tags.push(ManualTag {
                path: path.to_string(),
                tags: vec![tag.to_string()],
            });
        }
    }

    /// Remove a manual tag from a file path.
    pub fn remove_manual_tag(&mut self, path: &str, tag: &str) {
        if let Some(manual) = self.db.manual_tags.iter_mut().find(|m| m.path == path) {
            manual.tags.retain(|t| t != tag);
        }
        self.db.manual_tags.retain(|m| !m.tags.is_empty());
    }

    /// Add a new tag rule.
    pub fn add_tag_rule(&mut self, rule: TagRule) {
        self.db.tag_rules.push(rule);
    }

    /// Remove a tag rule by name.
    pub fn remove_tag_rule(&mut self, name: &str) {
        self.db.tag_rules.retain(|r| r.name != name);
    }

    /// Get all tag rules.
    pub fn tag_rules(&self) -> &[TagRule] {
        &self.db.tag_rules
    }

    /// Get all tag rule names.
    pub fn tag_names(&self) -> Vec<String> {
        self.db.tag_rules.iter().map(|r| r.name.clone()).collect()
    }

    /// Save the tag database to disk.
    pub fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&self.db).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })?;
        std::fs::write(&self.db_path, content)
    }

    fn entry_matches_rule(&self, entry: &FileEntry, rule: &TagRule) -> bool {
        rule.rules.iter().any(|r| self.entry_matches_match_rule(entry, r))
    }

    fn entry_matches_match_rule(&self, entry: &FileEntry, rule: &TagMatchRule) -> bool {
        match rule {
            TagMatchRule::Extension { exts } => {
                if let Some(ext) = entry.extension() {
                    exts.iter().any(|e| e.eq_ignore_ascii_case(ext))
                } else {
                    false
                }
            }
            TagMatchRule::NamePattern { pattern } => {
                match regex::Regex::new(pattern) {
                    Ok(re) => re.is_match(&entry.name),
                    Err(_) => entry.name.contains(pattern),
                }
            }
            TagMatchRule::MimePrefix { prefix } => {
                if let Some(ref mime) = entry.metadata.mime_type {
                    mime.starts_with(prefix)
                } else {
                    false
                }
            }
            TagMatchRule::SizeRange { min, max } => {
                if !entry.kind.is_file() {
                    return false;
                }
                let size = entry.metadata.size;
                if let Some(min) = min {
                    if size < *min {
                        return false;
                    }
                }
                if let Some(max) = max {
                    if size > *max {
                        return false;
                    }
                }
                true
            }
            TagMatchRule::ModifiedRange { after_days, before_days } => {
                if let Some(modified) = entry.metadata.modified {
                    let now = Utc::now();
                    let age_days = (now - modified).num_days();
                    if let Some(after) = after_days {
                        if age_days > *after {
                            return false;
                        }
                    }
                    if let Some(before) = before_days {
                        if age_days < *before {
                            return false;
                        }
                    }
                    true
                } else {
                    false
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use raven_core::entry::{EntryKind, EntryMetadata};
    use raven_core::path::RavenPath;

    fn make_file(name: &str, size: u64, modified: Option<chrono::DateTime<Utc>>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: RavenPath::Local(PathBuf::from(format!("/test/{}", name))),
            kind: EntryKind::File,
            metadata: EntryMetadata {
                size,
                modified,
                ..EntryMetadata::default()
            },
        }
    }

    fn make_dir(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: RavenPath::Local(PathBuf::from(format!("/test/{}", name))),
            kind: EntryKind::Directory,
            metadata: EntryMetadata::default(),
        }
    }

    #[test]
    fn test_tags_for_image() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("photo.jpg", 1024, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Images".to_string()));
    }

    #[test]
    fn test_tags_for_large_file() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("big.bin", 200 * 1024 * 1024, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Large Files".to_string()));
    }

    #[test]
    fn test_tags_for_code() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("main.rs", 500, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Code".to_string()));
    }

    #[test]
    fn test_tags_for_document() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("report.pdf", 1024, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Documents".to_string()));
    }

    #[test]
    fn test_tags_for_video() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("movie.mp4", 1024, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Videos".to_string()));
    }

    #[test]
    fn test_tags_for_audio() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("song.mp3", 1024, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Audio".to_string()));
    }

    #[test]
    fn test_tags_for_archive() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("backup.tar.gz", 1024, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Archives".to_string()));
    }

    #[test]
    fn test_tags_for_old_file() {
        let engine = TagEngine::with_defaults();
        let old_date = Utc::now() - Duration::days(400);
        let entry = make_file("old.txt", 100, Some(old_date));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Old Files".to_string()));
    }

    #[test]
    fn test_tags_for_recent_file() {
        let engine = TagEngine::with_defaults();
        let recent_date = Utc::now() - Duration::days(2);
        let entry = make_file("new.txt", 100, Some(recent_date));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Recent".to_string()));
    }

    #[test]
    fn test_no_large_tag_for_directories() {
        let engine = TagEngine::with_defaults();
        let entry = make_dir("bigdir");
        let tags = engine.tags_for_entry(&entry);
        assert!(!tags.contains(&"Large Files".to_string()));
    }

    #[test]
    fn test_tag_counts() {
        let engine = TagEngine::with_defaults();
        let entries = vec![
            make_file("a.jpg", 100, Some(Utc::now())),
            make_file("b.png", 200, Some(Utc::now())),
            make_file("c.rs", 300, Some(Utc::now())),
        ];
        let counts = engine.tag_counts(&entries);
        let images_count = counts.iter().find(|(n, _)| n == "Images").map(|(_, c)| *c);
        assert_eq!(images_count, Some(2));
        let code_count = counts.iter().find(|(n, _)| n == "Code").map(|(_, c)| *c);
        assert_eq!(code_count, Some(1));
    }

    #[test]
    fn test_manual_tag_add_remove() {
        let mut engine = TagEngine::with_defaults();
        engine.add_manual_tag("/test/file.txt", "important");
        let entry = make_file("file.txt", 100, None);
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"important".to_string()));

        engine.remove_manual_tag("/test/file.txt", "important");
        let tags = engine.tags_for_entry(&entry);
        assert!(!tags.contains(&"important".to_string()));
    }

    #[test]
    fn test_add_remove_tag_rule() {
        let mut engine = TagEngine::with_defaults();
        let initial_count = engine.tag_rules().len();
        engine.add_tag_rule(TagRule {
            name: "Custom".to_string(),
            icon: "custom-symbolic".to_string(),
            color: "#ff0000".to_string(),
            rules: vec![TagMatchRule::Extension {
                exts: vec!["custom".to_string()],
            }],
            is_smart: false,
        });
        assert_eq!(engine.tag_rules().len(), initial_count + 1);

        engine.remove_tag_rule("Custom");
        assert_eq!(engine.tag_rules().len(), initial_count);
    }

    #[test]
    fn test_tag_names() {
        let engine = TagEngine::with_defaults();
        let names = engine.tag_names();
        assert_eq!(names.len(), 9);
        assert!(names.contains(&"Images".to_string()));
    }

    #[test]
    fn test_duplicate_manual_tag() {
        let mut engine = TagEngine::with_defaults();
        engine.add_manual_tag("/test/file.txt", "important");
        engine.add_manual_tag("/test/file.txt", "important");
        let entry = make_file("file.txt", 100, None);
        let tags = engine.tags_for_entry(&entry);
        let count = tags.iter().filter(|t| *t == "important").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_case_insensitive_extensions() {
        let engine = TagEngine::with_defaults();
        let entry = make_file("photo.JPG", 1024, Some(Utc::now()));
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Images".to_string()));
    }

    #[test]
    fn test_multiple_tags() {
        let engine = TagEngine::with_defaults();
        // Large recent image
        let entry = make_file(
            "big_photo.jpg",
            200 * 1024 * 1024,
            Some(Utc::now() - Duration::days(1)),
        );
        let tags = engine.tags_for_entry(&entry);
        assert!(tags.contains(&"Images".to_string()));
        assert!(tags.contains(&"Large Files".to_string()));
        assert!(tags.contains(&"Recent".to_string()));
    }
}
