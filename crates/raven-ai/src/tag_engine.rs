use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use raven_core::entry::FileEntry;

use regex::Regex;

use crate::tags::{ManualTag, TagDatabase, TagMatchRule, TagRule};

/// Engine for computing and managing file tags.
pub struct TagEngine {
    db: TagDatabase,
    db_path: PathBuf,
    /// Compiled `NamePattern` rules, keyed by pattern. `None` marks a pattern that
    /// failed to compile. Counting walks every entry in a directory, so patterns are
    /// compiled once per rule set rather than once per entry.
    regex_cache: HashMap<String, Option<Regex>>,
}

impl TagEngine {
    /// Create a new TagEngine, loading from the given path or using defaults.
    pub fn new(db_path: PathBuf) -> Self {
        let db = Self::load_or_default(&db_path);
        let regex_cache = Self::build_regex_cache(&db.tag_rules);
        Self {
            db,
            db_path,
            regex_cache,
        }
    }

    /// Create a TagEngine with default tags (for testing).
    pub fn with_defaults() -> Self {
        let db = TagDatabase {
            tag_rules: TagRule::defaults(),
            manual_tags: Vec::new(),
        };
        let regex_cache = Self::build_regex_cache(&db.tag_rules);
        Self {
            db,
            db_path: PathBuf::from("/tmp/raven-tags-test.toml"),
            regex_cache,
        }
    }

    fn build_regex_cache(rules: &[TagRule]) -> HashMap<String, Option<Regex>> {
        let mut cache = HashMap::new();
        for rule in rules {
            for match_rule in &rule.rules {
                if let TagMatchRule::NamePattern { pattern } = match_rule {
                    cache.entry(pattern.clone()).or_insert_with(|| {
                        Regex::new(pattern)
                            .map_err(|e| {
                                tracing::warn!(
                                    "Tag rule '{}' has an invalid pattern '{}': {}",
                                    rule.name,
                                    pattern,
                                    e
                                );
                            })
                            .ok()
                    });
                }
            }
        }
        cache
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

    /// Whether `entry` carries `tag`, by rule or by manual assignment.
    ///
    /// Filtering a directory checks one known tag per entry, so this avoids the
    /// per-entry `Vec<String>` that `tags_for_entry` builds.
    pub fn entry_has_tag(&self, entry: &FileEntry, tag: &str) -> bool {
        let matches_rule = self
            .db
            .tag_rules
            .iter()
            .any(|rule| rule.name == tag && self.entry_matches_rule(entry, rule));
        if matches_rule {
            return true;
        }

        let path_str = entry.path.to_string();
        self.db
            .manual_tags
            .iter()
            .any(|m| m.path == path_str && m.tags.iter().any(|t| t == tag))
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
        self.regex_cache = Self::build_regex_cache(&self.db.tag_rules);
    }

    /// Remove a tag rule by name.
    pub fn remove_tag_rule(&mut self, name: &str) {
        self.db.tag_rules.retain(|r| r.name != name);
        self.regex_cache = Self::build_regex_cache(&self.db.tag_rules);
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
                match self.regex_cache.get(pattern) {
                    Some(Some(re)) => re.is_match(&entry.name),
                    // Pattern is known-invalid: fall back to a literal substring match.
                    Some(None) => entry.name.contains(pattern),
                    // Not cached (rule set mutated without a rebuild); compile on demand.
                    None => match Regex::new(pattern) {
                        Ok(re) => re.is_match(&entry.name),
                        Err(_) => entry.name.contains(pattern),
                    },
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
    fn test_tag_counts_include_manual_tags() {
        let mut engine = TagEngine::with_defaults();
        engine.add_manual_tag("/test/a.jpg", "important");
        engine.add_manual_tag("/test/b.png", "important");

        let entries = vec![
            make_file("a.jpg", 100, Some(Utc::now())),
            make_file("b.png", 200, Some(Utc::now())),
            make_file("c.rs", 300, Some(Utc::now())),
        ];
        let counts = engine.tag_counts(&entries);
        let important = counts
            .iter()
            .find(|(n, _)| n == "important")
            .map(|(_, c)| *c);
        assert_eq!(important, Some(2));
    }

    #[test]
    fn test_name_pattern_rule_matches_after_add() {
        let mut engine = TagEngine::with_defaults();
        engine.add_tag_rule(TagRule {
            name: "Drafts".to_string(),
            icon: "document-edit-symbolic".to_string(),
            color: "#95a5a6".to_string(),
            rules: vec![TagMatchRule::NamePattern {
                pattern: r"^draft[-_]".to_string(),
            }],
            is_smart: false,
        });

        let entries = vec![
            make_file("draft-report.md", 100, None),
            make_file("draft_notes.md", 100, None),
            make_file("final.md", 100, None),
        ];
        let counts = engine.tag_counts(&entries);
        let drafts = counts.iter().find(|(n, _)| n == "Drafts").map(|(_, c)| *c);
        assert_eq!(drafts, Some(2));
    }

    #[test]
    fn test_invalid_name_pattern_falls_back_to_substring() {
        let mut engine = TagEngine::with_defaults();
        engine.add_tag_rule(TagRule {
            name: "Broken".to_string(),
            icon: "dialog-warning-symbolic".to_string(),
            color: "#c0392b".to_string(),
            // Unclosed group: never compiles.
            rules: vec![TagMatchRule::NamePattern {
                pattern: "log(".to_string(),
            }],
            is_smart: false,
        });

        let entries = vec![
            make_file("log(1).txt", 100, None),
            make_file("plain.txt", 100, None),
        ];
        let counts = engine.tag_counts(&entries);
        let broken = counts.iter().find(|(n, _)| n == "Broken").map(|(_, c)| *c);
        assert_eq!(broken, Some(1));
    }

    #[test]
    fn test_entry_has_tag_matches_rule_and_manual() {
        let mut engine = TagEngine::with_defaults();
        engine.add_manual_tag("/test/notes.txt", "important");

        let image = make_file("photo.jpg", 100, Some(Utc::now()));
        assert!(engine.entry_has_tag(&image, "Images"));
        assert!(!engine.entry_has_tag(&image, "Code"));
        assert!(!engine.entry_has_tag(&image, "important"));

        let notes = make_file("notes.txt", 100, None);
        assert!(engine.entry_has_tag(&notes, "important"));
    }

    #[test]
    fn test_entry_has_tag_agrees_with_tags_for_entry() {
        let mut engine = TagEngine::with_defaults();
        engine.add_manual_tag("/test/big_photo.jpg", "keep");

        let entry = make_file(
            "big_photo.jpg",
            200 * 1024 * 1024,
            Some(Utc::now() - Duration::days(1)),
        );
        let listed = engine.tags_for_entry(&entry);
        for tag in &listed {
            assert!(
                engine.entry_has_tag(&entry, tag),
                "entry_has_tag disagreed on '{}'",
                tag
            );
        }
        assert!(!engine.entry_has_tag(&entry, "Videos"));
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
