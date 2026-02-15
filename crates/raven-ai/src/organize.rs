use std::collections::HashMap;
use std::path::PathBuf;

use raven_core::ai_types::OrganizeSuggestion;
use raven_core::entry::{EntryKind, FileEntry};

/// Configuration for the organization analyzer.
#[derive(Debug, Clone)]
pub struct OrganizeConfig {
    pub min_group_size: usize,
    pub strategies: Vec<OrganizeStrategy>,
}

impl Default for OrganizeConfig {
    fn default() -> Self {
        Self {
            min_group_size: 3,
            strategies: vec![
                OrganizeStrategy::ByExtensionType,
                OrganizeStrategy::ByDate,
                OrganizeStrategy::ByProject,
            ],
        }
    }
}

/// Strategy for organizing files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrganizeStrategy {
    ByExtensionType,
    ByDate,
    ByProject,
}

/// Analyzes directory contents and suggests organization.
pub struct OrganizationAnalyzer;

impl OrganizationAnalyzer {
    /// Analyze entries and return organization suggestions.
    pub fn analyze(entries: &[FileEntry], config: &OrganizeConfig) -> Vec<OrganizeSuggestion> {
        // Check for project markers first - don't suggest reorganizing project dirs
        if Self::is_project_directory(entries) {
            return Vec::new();
        }

        let mut suggestions = Vec::new();

        for strategy in &config.strategies {
            match strategy {
                OrganizeStrategy::ByExtensionType => {
                    suggestions.extend(Self::suggest_by_extension(entries, config.min_group_size));
                }
                OrganizeStrategy::ByDate => {
                    suggestions.extend(Self::suggest_by_date(entries, config.min_group_size));
                }
                OrganizeStrategy::ByProject => {
                    // Project detection is handled above
                }
            }
        }

        suggestions
    }

    fn is_project_directory(entries: &[FileEntry]) -> bool {
        let project_markers = [
            "Cargo.toml",
            "package.json",
            "Makefile",
            "CMakeLists.txt",
            "go.mod",
            "pom.xml",
            "build.gradle",
            "pyproject.toml",
            "setup.py",
            ".git",
        ];

        entries
            .iter()
            .any(|e| project_markers.contains(&e.name.as_str()))
    }

    fn suggest_by_extension(
        entries: &[FileEntry],
        min_group_size: usize,
    ) -> Vec<OrganizeSuggestion> {
        let mut type_groups: HashMap<&str, Vec<&FileEntry>> = HashMap::new();

        for entry in entries {
            if !entry.kind.is_file() {
                continue;
            }
            if let Some(category) = extension_category(entry.extension()) {
                type_groups.entry(category).or_default().push(entry);
            }
        }

        type_groups
            .into_iter()
            .filter(|(_, files)| files.len() >= min_group_size)
            .map(|(category, files)| {
                let folder = format!("{}/", category);
                let count = files.len();
                let ext_summary: Vec<String> = {
                    let mut exts: HashMap<String, usize> = HashMap::new();
                    for f in &files {
                        if let Some(ext) = f.extension() {
                            *exts.entry(format!(".{}", ext)).or_insert(0) += 1;
                        }
                    }
                    let mut sorted: Vec<(String, usize)> = exts.into_iter().collect();
                    sorted.sort_by(|a, b| b.1.cmp(&a.1));
                    sorted.into_iter().map(|(ext, _)| ext).take(3).collect()
                };

                OrganizeSuggestion {
                    description: format!(
                        "Move {} {} files ({}) to '{}'",
                        count,
                        category.to_lowercase(),
                        ext_summary.join(", "),
                        folder
                    ),
                    source_files: files.into_iter().cloned().collect(),
                    destination: folder,
                    category: category.to_string(),
                }
            })
            .collect()
    }

    fn suggest_by_date(
        entries: &[FileEntry],
        min_group_size: usize,
    ) -> Vec<OrganizeSuggestion> {
        // Only suggest by date if there are 10+ files spanning 2+ years
        let files: Vec<&FileEntry> = entries
            .iter()
            .filter(|e| e.kind.is_file() && e.metadata.modified.is_some())
            .collect();

        if files.len() < 10 {
            return Vec::new();
        }

        let mut year_groups: HashMap<i32, Vec<&FileEntry>> = HashMap::new();
        for entry in &files {
            if let Some(modified) = entry.metadata.modified {
                let year = modified.format("%Y").to_string().parse::<i32>().unwrap_or(0);
                year_groups.entry(year).or_default().push(entry);
            }
        }

        if year_groups.len() < 2 {
            return Vec::new();
        }

        year_groups
            .into_iter()
            .filter(|(_, files)| files.len() >= min_group_size)
            .map(|(year, files)| {
                let folder = format!("{}/", year);
                let count = files.len();
                OrganizeSuggestion {
                    description: format!("Move {} files from {} to '{}'", count, year, folder),
                    source_files: files.into_iter().cloned().collect(),
                    destination: folder,
                    category: format!("Year {}", year),
                }
            })
            .collect()
    }
}

/// Map file extensions to human-readable category names.
fn extension_category(ext: Option<&str>) -> Option<&'static str> {
    let ext = ext?.to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tiff" | "raw" => {
            Some("Images")
        }
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" => Some("Videos"),
        "mp3" | "flac" | "ogg" | "wav" | "aac" | "wma" | "m4a" | "opus" => Some("Audio"),
        "pdf" | "doc" | "docx" | "odt" | "txt" | "rtf" | "md" | "tex" | "epub" => {
            Some("Documents")
        }
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" => Some("Archives"),
        "rs" | "py" | "js" | "ts" | "java" | "go" | "c" | "cpp" | "h" | "hpp" | "rb"
        | "swift" | "kt" | "cs" | "sh" | "bash" | "html" | "css" | "scss" | "json" | "yaml"
        | "yml" | "toml" | "xml" | "jsx" | "tsx" => Some("Code"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use raven_core::entry::EntryMetadata;
    use raven_core::path::RavenPath;

    fn make_file(name: &str, modified: Option<DateTime<Utc>>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: RavenPath::Local(PathBuf::from(format!("/test/{}", name))),
            kind: EntryKind::File,
            metadata: EntryMetadata {
                size: 1024,
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
    fn test_empty_directory() {
        let suggestions = OrganizationAnalyzer::analyze(&[], &OrganizeConfig::default());
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_project_directory_skipped() {
        let entries = vec![
            make_file("Cargo.toml", None),
            make_file("main.rs", None),
            make_file("lib.rs", None),
            make_file("utils.rs", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_suggest_images_folder() {
        let entries = vec![
            make_file("a.jpg", None),
            make_file("b.png", None),
            make_file("c.gif", None),
            make_file("readme.txt", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        let image_suggestion = suggestions.iter().find(|s| s.category == "Images");
        assert!(image_suggestion.is_some());
        assert_eq!(image_suggestion.unwrap().source_files.len(), 3);
    }

    #[test]
    fn test_min_group_size() {
        let entries = vec![
            make_file("a.jpg", None),
            make_file("b.png", None),
            // Only 2 images, default min is 3
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        let image_suggestion = suggestions.iter().find(|s| s.category == "Images");
        assert!(image_suggestion.is_none());
    }

    #[test]
    fn test_suggest_multiple_categories() {
        let entries = vec![
            make_file("a.jpg", None),
            make_file("b.png", None),
            make_file("c.gif", None),
            make_file("d.mp4", None),
            make_file("e.mkv", None),
            make_file("f.avi", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        assert!(suggestions.len() >= 2);
    }

    #[test]
    fn test_directories_excluded_from_grouping() {
        let entries = vec![
            make_dir("subdir1"),
            make_dir("subdir2"),
            make_dir("subdir3"),
            make_file("a.jpg", None),
            make_file("b.png", None),
            make_file("c.gif", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        // Directories should not be suggested for moving
        for s in &suggestions {
            for f in &s.source_files {
                assert!(f.kind.is_file());
            }
        }
    }

    #[test]
    fn test_date_grouping_requires_10_files() {
        use chrono::NaiveDate;
        let date_2020 = NaiveDate::from_ymd_opt(2020, 6, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let date_2024 = NaiveDate::from_ymd_opt(2024, 6, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();

        // Only 6 files, not enough for date grouping
        let entries: Vec<FileEntry> = (0..6)
            .map(|i| {
                let date = if i < 3 { date_2020 } else { date_2024 };
                make_file(&format!("file{}.dat", i), Some(date))
            })
            .collect();

        let config = OrganizeConfig {
            strategies: vec![OrganizeStrategy::ByDate],
            ..OrganizeConfig::default()
        };
        let suggestions = OrganizationAnalyzer::analyze(&entries, &config);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_date_grouping_with_enough_files() {
        use chrono::NaiveDate;
        let date_2020 = NaiveDate::from_ymd_opt(2020, 6, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let date_2024 = NaiveDate::from_ymd_opt(2024, 6, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();

        let mut entries: Vec<FileEntry> = Vec::new();
        for i in 0..6 {
            entries.push(make_file(&format!("old{}.dat", i), Some(date_2020)));
        }
        for i in 0..6 {
            entries.push(make_file(&format!("new{}.dat", i), Some(date_2024)));
        }

        let config = OrganizeConfig {
            strategies: vec![OrganizeStrategy::ByDate],
            ..OrganizeConfig::default()
        };
        let suggestions = OrganizationAnalyzer::analyze(&entries, &config);
        assert!(suggestions.len() >= 2);
    }

    #[test]
    fn test_suggestion_description() {
        let entries = vec![
            make_file("a.jpg", None),
            make_file("b.png", None),
            make_file("c.gif", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        let s = suggestions.iter().find(|s| s.category == "Images").unwrap();
        assert!(s.description.contains("3"));
        assert!(s.description.contains("Images/"));
    }

    #[test]
    fn test_suggestion_destination() {
        let entries = vec![
            make_file("a.jpg", None),
            make_file("b.png", None),
            make_file("c.gif", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        let s = suggestions.iter().find(|s| s.category == "Images").unwrap();
        assert_eq!(s.destination, "Images/");
    }

    #[test]
    fn test_extension_category() {
        assert_eq!(extension_category(Some("jpg")), Some("Images"));
        assert_eq!(extension_category(Some("mp4")), Some("Videos"));
        assert_eq!(extension_category(Some("mp3")), Some("Audio"));
        assert_eq!(extension_category(Some("pdf")), Some("Documents"));
        assert_eq!(extension_category(Some("zip")), Some("Archives"));
        assert_eq!(extension_category(Some("rs")), Some("Code"));
        assert_eq!(extension_category(Some("xyz")), None);
        assert_eq!(extension_category(None), None);
    }

    #[test]
    fn test_custom_min_group_size() {
        let entries = vec![
            make_file("a.jpg", None),
            make_file("b.png", None),
        ];
        let config = OrganizeConfig {
            min_group_size: 2,
            ..OrganizeConfig::default()
        };
        let suggestions = OrganizationAnalyzer::analyze(&entries, &config);
        assert!(!suggestions.is_empty());
    }

    #[test]
    fn test_well_organized_empty_result() {
        // A directory with only unique file types
        let entries = vec![
            make_file("readme.txt", None),
            make_file("photo.jpg", None),
            make_file("video.mp4", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        // Each type has only 1 file, below min_group_size
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_package_json_project_detected() {
        let entries = vec![
            make_file("package.json", None),
            make_file("index.js", None),
            make_file("app.js", None),
            make_file("utils.js", None),
        ];
        let suggestions = OrganizationAnalyzer::analyze(&entries, &OrganizeConfig::default());
        assert!(suggestions.is_empty());
    }
}
