use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc};
use raven_core::filter::{FileTypeFilter, FilterSpec};

/// A parsed natural language query broken down into searchable components.
#[derive(Debug, Clone, Default)]
pub struct ParsedQuery {
    pub filename_pattern: String,
    pub file_types: Vec<FileTypeFilter>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<DateTime<Utc>>,
    pub modified_before: Option<DateTime<Utc>>,
    pub extensions: Vec<String>,
    pub needs_recursive: bool,
}

/// Parse a natural language query into structured search parameters.
pub fn parse_nl_query(input: &str) -> ParsedQuery {
    let mut parsed = ParsedQuery::default();
    let lower = input.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    let mut consumed = vec![false; tokens.len()];

    // Parse file type keywords
    for (i, token) in tokens.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        if let Some(ft) = match_file_type(token) {
            parsed.file_types.push(ft);
            consumed[i] = true;
            // Also consume preceding "all" or articles
            if i > 0 && matches!(tokens[i - 1], "all" | "the" | "my" | "any") {
                consumed[i - 1] = true;
            }
        }
    }

    // Parse size keywords
    parse_size_keywords(&tokens, &mut consumed, &mut parsed);

    // Parse time keywords
    parse_time_keywords(&tokens, &mut consumed, &mut parsed);

    // Parse language/extension keywords
    parse_extension_keywords(&tokens, &mut consumed, &mut parsed);

    // Parse modifiers
    for (i, token) in tokens.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        match *token {
            "recursive" | "recursively" | "everywhere" | "all" => {
                parsed.needs_recursive = true;
                consumed[i] = true;
            }
            "in" | "from" | "find" | "show" | "search" | "for" | "me" | "files" | "file"
            | "that" | "are" | "is" | "the" | "a" | "an" | "with" | "named" | "called" => {
                consumed[i] = true;
            }
            _ => {}
        }
    }

    // Remaining unconsumed tokens become the filename pattern
    let remaining: Vec<&str> = tokens
        .iter()
        .enumerate()
        .filter(|(i, _)| !consumed[*i])
        .map(|(_, t)| *t)
        .collect();
    parsed.filename_pattern = remaining.join(" ");

    // If we have file types or extensions, default to recursive
    if !parsed.file_types.is_empty() || !parsed.extensions.is_empty() {
        parsed.needs_recursive = true;
    }

    parsed
}

/// Convert a ParsedQuery into a FilterSpec for in-memory filtering.
pub fn parsed_to_filter(parsed: &ParsedQuery) -> FilterSpec {
    FilterSpec {
        query: parsed.filename_pattern.clone(),
        show_hidden: false,
        file_types: parsed.file_types.clone(),
        min_size: parsed.min_size,
        max_size: parsed.max_size,
        modified_after: parsed.modified_after,
        modified_before: parsed.modified_before,
        extensions: parsed.extensions.clone(),
    }
}

fn match_file_type(token: &str) -> Option<FileTypeFilter> {
    match token {
        "images" | "image" | "photos" | "photo" | "pictures" | "picture" => {
            Some(FileTypeFilter::Images)
        }
        "videos" | "video" | "movies" | "movie" | "clips" => Some(FileTypeFilter::Videos),
        "audio" | "music" | "songs" | "song" | "sounds" => Some(FileTypeFilter::Audio),
        "documents" | "document" | "docs" | "doc" | "text" => Some(FileTypeFilter::Documents),
        "archives" | "archive" | "compressed" | "zipped" => Some(FileTypeFilter::Archives),
        "directories" | "directory" | "folders" | "folder" | "dirs" => {
            Some(FileTypeFilter::Directories)
        }
        "symlinks" | "symlink" | "links" | "link" => Some(FileTypeFilter::Symlinks),
        _ => None,
    }
}

fn parse_size_keywords(tokens: &[&str], consumed: &mut [bool], parsed: &mut ParsedQuery) {
    for i in 0..tokens.len() {
        if consumed[i] {
            continue;
        }
        match tokens[i] {
            "large" | "big" | "huge" => {
                parsed.min_size = Some(100 * 1024 * 1024); // 100MB
                consumed[i] = true;
            }
            "small" | "tiny" => {
                parsed.max_size = Some(1024 * 1024); // 1MB
                consumed[i] = true;
            }
            "empty" => {
                parsed.max_size = Some(0);
                consumed[i] = true;
            }
            "bigger" | "larger" | "greater" => {
                // "bigger than 5mb"
                if i + 2 < tokens.len() && tokens[i + 1] == "than" {
                    if let Some(size) = parse_size_str(tokens[i + 2]) {
                        parsed.min_size = Some(size);
                        consumed[i] = true;
                        consumed[i + 1] = true;
                        consumed[i + 2] = true;
                    }
                }
            }
            "smaller" | "less" => {
                // "smaller than 10mb"
                if i + 2 < tokens.len() && tokens[i + 1] == "than" {
                    if let Some(size) = parse_size_str(tokens[i + 2]) {
                        parsed.max_size = Some(size);
                        consumed[i] = true;
                        consumed[i + 1] = true;
                        consumed[i + 2] = true;
                    }
                }
            }
            "over" | "above" => {
                // "over 5mb"
                if i + 1 < tokens.len() {
                    if let Some(size) = parse_size_str(tokens[i + 1]) {
                        parsed.min_size = Some(size);
                        consumed[i] = true;
                        consumed[i + 1] = true;
                    }
                }
            }
            "under" | "below" => {
                // "under 5mb"
                if i + 1 < tokens.len() {
                    if let Some(size) = parse_size_str(tokens[i + 1]) {
                        parsed.max_size = Some(size);
                        consumed[i] = true;
                        consumed[i + 1] = true;
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_size_str(s: &str) -> Option<u64> {
    let s = s.trim();
    // Try patterns like "5mb", "100kb", "2gb", "5m", "100k"
    let re = regex::Regex::new(r"^(\d+(?:\.\d+)?)\s*(kb|mb|gb|tb|k|m|g|t|bytes?|b)?$").ok()?;
    let caps = re.captures(s)?;
    let num: f64 = caps.get(1)?.as_str().parse().ok()?;
    let unit = caps.get(2).map(|m| m.as_str()).unwrap_or("b");
    let multiplier: u64 = match unit {
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        "t" | "tb" => 1024 * 1024 * 1024 * 1024,
        _ => 1,
    };
    Some((num * multiplier as f64) as u64)
}

fn parse_time_keywords(tokens: &[&str], consumed: &mut [bool], parsed: &mut ParsedQuery) {
    let now = Local::now();

    for i in 0..tokens.len() {
        if consumed[i] {
            continue;
        }
        match tokens[i] {
            "today" => {
                parsed.modified_after =
                    Some(now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc());
                consumed[i] = true;
            }
            "yesterday" => {
                let yesterday = now - Duration::days(1);
                let start = yesterday
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc();
                let end = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
                parsed.modified_after = Some(start);
                parsed.modified_before = Some(end);
                consumed[i] = true;
            }
            "recent" | "recently" | "new" | "newest" => {
                parsed.modified_after = Some((now - Duration::days(7)).to_utc());
                consumed[i] = true;
            }
            "old" | "oldest" | "ancient" => {
                parsed.modified_before = Some((now - Duration::days(365)).to_utc());
                consumed[i] = true;
            }
            "last" => {
                // "last week", "last month", "last year"
                if i + 1 < tokens.len() {
                    let duration = match tokens[i + 1] {
                        "week" => Some(Duration::days(7)),
                        "month" => Some(Duration::days(30)),
                        "year" => Some(Duration::days(365)),
                        "hour" => Some(Duration::hours(1)),
                        "day" => Some(Duration::days(1)),
                        _ => None,
                    };
                    if let Some(d) = duration {
                        parsed.modified_after = Some((now - d).to_utc());
                        consumed[i] = true;
                        consumed[i + 1] = true;
                    }
                }
            }
            "this" => {
                // "this week", "this month", "this year"
                if i + 1 < tokens.len() {
                    let start = match tokens[i + 1] {
                        "week" => Some((now - Duration::days(7)).to_utc()),
                        "month" => Some((now - Duration::days(30)).to_utc()),
                        "year" => {
                            let year = now.date_naive().year();
                            NaiveDate::from_ymd_opt(year, 1, 1)
                                .and_then(|d| d.and_hms_opt(0, 0, 0))
                                .map(|dt| dt.and_utc())
                        }
                        _ => None,
                    };
                    if let Some(s) = start {
                        parsed.modified_after = Some(s);
                        consumed[i] = true;
                        consumed[i + 1] = true;
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_extension_keywords(tokens: &[&str], consumed: &mut [bool], parsed: &mut ParsedQuery) {
    for i in 0..tokens.len() {
        if consumed[i] {
            continue;
        }
        // Language keywords -> extensions
        let exts: Option<Vec<&str>> = match tokens[i] {
            "python" => Some(vec!["py"]),
            "rust" => Some(vec!["rs"]),
            "javascript" | "js" => Some(vec!["js", "jsx", "mjs"]),
            "typescript" | "ts" => Some(vec!["ts", "tsx"]),
            "java" => Some(vec!["java"]),
            "go" | "golang" => Some(vec!["go"]),
            "ruby" => Some(vec!["rb"]),
            "cpp" | "c++" => Some(vec!["cpp", "cc", "cxx", "hpp", "h"]),
            "csharp" | "c#" => Some(vec!["cs"]),
            "swift" => Some(vec!["swift"]),
            "kotlin" => Some(vec!["kt", "kts"]),
            "html" => Some(vec!["html", "htm"]),
            "css" => Some(vec!["css", "scss", "sass"]),
            "markdown" | "md" => Some(vec!["md", "markdown"]),
            "json" => Some(vec!["json"]),
            "yaml" | "yml" => Some(vec!["yaml", "yml"]),
            "xml" => Some(vec!["xml"]),
            "shell" | "bash" | "sh" => Some(vec!["sh", "bash", "zsh"]),
            "pdf" | "pdfs" => Some(vec!["pdf"]),
            _ => None,
        };
        if let Some(exts) = exts {
            for ext in exts {
                parsed.extensions.push(ext.to_string());
            }
            consumed[i] = true;
            // Also consume trailing "files" if present
            if i + 1 < tokens.len() && tokens[i + 1] == "files" {
                consumed[i + 1] = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_filename() {
        let q = parse_nl_query("readme");
        assert_eq!(q.filename_pattern, "readme");
        assert!(q.file_types.is_empty());
    }

    #[test]
    fn test_parse_images() {
        let q = parse_nl_query("images");
        assert_eq!(q.file_types, vec![FileTypeFilter::Images]);
        assert!(q.filename_pattern.is_empty());
    }

    #[test]
    fn test_parse_large() {
        let q = parse_nl_query("large files");
        assert_eq!(q.min_size, Some(100 * 1024 * 1024));
    }

    #[test]
    fn test_parse_small() {
        let q = parse_nl_query("small files");
        assert_eq!(q.max_size, Some(1024 * 1024));
    }

    #[test]
    fn test_parse_bigger_than() {
        let q = parse_nl_query("bigger than 5mb");
        assert_eq!(q.min_size, Some(5 * 1024 * 1024));
    }

    #[test]
    fn test_parse_smaller_than() {
        let q = parse_nl_query("smaller than 10kb");
        assert_eq!(q.max_size, Some(10 * 1024));
    }

    #[test]
    fn test_parse_over_size() {
        let q = parse_nl_query("over 1gb");
        assert_eq!(q.min_size, Some(1024 * 1024 * 1024));
    }

    #[test]
    fn test_parse_under_size() {
        let q = parse_nl_query("under 500kb");
        assert_eq!(q.max_size, Some(500 * 1024));
    }

    #[test]
    fn test_parse_today() {
        let q = parse_nl_query("images from today");
        assert!(q.modified_after.is_some());
        assert_eq!(q.file_types, vec![FileTypeFilter::Images]);
    }

    #[test]
    fn test_parse_last_week() {
        let q = parse_nl_query("files from last week");
        assert!(q.modified_after.is_some());
        let now = Utc::now();
        let diff = now - q.modified_after.unwrap();
        assert!(diff.num_days() >= 6 && diff.num_days() <= 8);
    }

    #[test]
    fn test_parse_old_files() {
        let q = parse_nl_query("old files");
        assert!(q.modified_before.is_some());
    }

    #[test]
    fn test_parse_recent() {
        let q = parse_nl_query("recent documents");
        assert!(q.modified_after.is_some());
        assert_eq!(q.file_types, vec![FileTypeFilter::Documents]);
    }

    #[test]
    fn test_parse_python_files() {
        let q = parse_nl_query("python files");
        assert_eq!(q.extensions, vec!["py"]);
    }

    #[test]
    fn test_parse_rust_files() {
        let q = parse_nl_query("rust files");
        assert_eq!(q.extensions, vec!["rs"]);
    }

    #[test]
    fn test_parse_javascript_files() {
        let q = parse_nl_query("javascript files");
        assert_eq!(q.extensions, vec!["js", "jsx", "mjs"]);
    }

    #[test]
    fn test_parse_combined_query() {
        let q = parse_nl_query("large images from last week");
        assert_eq!(q.min_size, Some(100 * 1024 * 1024));
        assert_eq!(q.file_types, vec![FileTypeFilter::Images]);
        assert!(q.modified_after.is_some());
        assert!(q.needs_recursive);
    }

    #[test]
    fn test_parse_empty_files() {
        let q = parse_nl_query("empty files");
        assert_eq!(q.max_size, Some(0));
    }

    #[test]
    fn test_parse_videos() {
        let q = parse_nl_query("find me videos");
        assert_eq!(q.file_types, vec![FileTypeFilter::Videos]);
    }

    #[test]
    fn test_parse_archives() {
        let q = parse_nl_query("archives");
        assert_eq!(q.file_types, vec![FileTypeFilter::Archives]);
    }

    #[test]
    fn test_parse_directories() {
        let q = parse_nl_query("show directories");
        assert_eq!(q.file_types, vec![FileTypeFilter::Directories]);
    }

    #[test]
    fn test_parsed_to_filter() {
        let q = parse_nl_query("large images");
        let f = parsed_to_filter(&q);
        assert_eq!(f.min_size, Some(100 * 1024 * 1024));
        assert_eq!(f.file_types, vec![FileTypeFilter::Images]);
        assert!(f.query.is_empty());
    }

    #[test]
    fn test_parsed_to_filter_with_pattern() {
        let q = parse_nl_query("readme");
        let f = parsed_to_filter(&q);
        assert_eq!(f.query, "readme");
    }

    #[test]
    fn test_needs_recursive_for_file_types() {
        let q = parse_nl_query("images");
        assert!(q.needs_recursive);
    }

    #[test]
    fn test_needs_recursive_for_extensions() {
        let q = parse_nl_query("python files");
        assert!(q.needs_recursive);
    }

    #[test]
    fn test_parse_size_str_mb() {
        assert_eq!(parse_size_str("5mb"), Some(5 * 1024 * 1024));
    }

    #[test]
    fn test_parse_size_str_gb() {
        assert_eq!(parse_size_str("2gb"), Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_parse_size_str_kb() {
        assert_eq!(parse_size_str("100kb"), Some(100 * 1024));
    }
}
