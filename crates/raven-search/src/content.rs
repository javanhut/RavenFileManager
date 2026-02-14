use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use ignore::WalkBuilder;
use tracing::{debug, warn};

use raven_core::error::{RavenError, RavenResult};

/// A single line matching a content search.
#[derive(Debug, Clone)]
pub struct ContentMatch {
    /// The file where the match was found.
    pub path: PathBuf,
    /// The 1-based line number of the match.
    pub line_number: u64,
    /// The content of the matching line (trimmed of trailing newline).
    pub line: String,
}

/// Configuration for a content search operation.
#[derive(Debug, Clone)]
pub struct ContentSearchConfig {
    /// The root directory to search from.
    pub root: PathBuf,
    /// The regex pattern to search for in file contents.
    pub pattern: String,
    /// Whether the search should be case-insensitive.
    pub case_insensitive: bool,
    /// Whether to include hidden files and directories.
    pub include_hidden: bool,
    /// Maximum depth to recurse (None for unlimited).
    pub max_depth: Option<usize>,
    /// Maximum number of matching files to return (None for unlimited).
    pub max_file_matches: Option<usize>,
    /// Maximum number of total line matches to return (None for unlimited).
    pub max_line_matches: Option<usize>,
}

impl Default for ContentSearchConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            pattern: String::new(),
            case_insensitive: true,
            include_hidden: false,
            max_depth: None,
            max_file_matches: None,
            max_line_matches: None,
        }
    }
}

impl ContentSearchConfig {
    /// Create a new config with the given root and pattern.
    pub fn new(root: impl Into<PathBuf>, pattern: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            pattern: pattern.into(),
            ..Self::default()
        }
    }
}

/// Grep-like content searcher using `grep-searcher` and `grep-regex`.
///
/// Searches file contents for a regex pattern, returning matching lines
/// with file path, line number, and content.
#[derive(Debug)]
pub struct ContentSearcher {
    /// Shared cancellation flag.
    cancelled: Arc<AtomicBool>,
}

impl ContentSearcher {
    /// Create a new content searcher.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a new content searcher with a shared cancellation flag.
    pub fn with_cancellation(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    /// Get a reference to the cancellation flag.
    pub fn cancellation_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Cancel any in-progress search.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Reset the cancellation flag so the searcher can be reused.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::Relaxed);
    }

    /// Search file contents and stream results through a channel.
    pub fn search(
        &self,
        config: ContentSearchConfig,
    ) -> RavenResult<tokio::sync::mpsc::Receiver<ContentMatch>> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let cancelled = Arc::clone(&self.cancelled);

        // Validate the pattern up front.
        let _ = build_regex_matcher(&config)?;

        let config_clone = config.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = run_content_search(config_clone, tx, cancelled) {
                warn!(error = %e, "content search error");
            }
        });

        Ok(rx)
    }

    /// Collect all search results into a Vec.
    pub async fn search_collect(
        &self,
        config: ContentSearchConfig,
    ) -> RavenResult<Vec<ContentMatch>> {
        let mut rx = self.search(config)?;
        let mut results = Vec::new();
        while let Some(m) = rx.recv().await {
            results.push(m);
        }
        Ok(results)
    }

    /// Search a single file for the given pattern.
    ///
    /// Returns all matching lines in the file.
    pub async fn search_file(
        &self,
        path: PathBuf,
        pattern: String,
        case_insensitive: bool,
    ) -> RavenResult<Vec<ContentMatch>> {
        tokio::task::spawn_blocking(move || {
            search_single_file(&path, &pattern, case_insensitive)
        })
        .await
        .map_err(|e| RavenError::Search {
            message: format!("task join error: {}", e),
        })?
    }
}

impl Default for ContentSearcher {
    fn default() -> Self {
        Self::new()
    }
}

fn build_regex_matcher(config: &ContentSearchConfig) -> RavenResult<RegexMatcher> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(config.case_insensitive)
        .build(&config.pattern)
        .map_err(|e| RavenError::Search {
            message: format!("invalid content search pattern: {}", e),
        })?;
    Ok(matcher)
}

fn run_content_search(
    config: ContentSearchConfig,
    tx: tokio::sync::mpsc::Sender<ContentMatch>,
    cancelled: Arc<AtomicBool>,
) -> RavenResult<()> {
    debug!(root = %config.root.display(), pattern = %config.pattern, "starting content search");

    let matcher = build_regex_matcher(&config)?;

    let mut builder = WalkBuilder::new(&config.root);
    builder
        .hidden(!config.include_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false);

    if let Some(depth) = config.max_depth {
        builder.max_depth(Some(depth));
    }

    let walker = builder.build();
    let mut file_match_count = 0u64;
    let mut total_line_matches = 0u64;

    for result in walker {
        if cancelled.load(Ordering::Relaxed) {
            debug!("content search cancelled");
            return Ok(());
        }

        let dir_entry = match result {
            Ok(entry) => entry,
            Err(e) => {
                warn!(error = %e, "error during directory walk");
                continue;
            }
        };

        // Only search files.
        let file_type = dir_entry.file_type();
        let is_file = file_type.map(|ft| ft.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }

        let path = dir_entry.path().to_path_buf();

        // Search this file.
        let matches = match search_file_with_matcher(&path, &matcher) {
            Ok(m) => m,
            Err(_) => continue, // Skip binary files or files we cannot read.
        };

        if matches.is_empty() {
            continue;
        }

        file_match_count += 1;

        for m in matches {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(());
            }

            total_line_matches += 1;
            if tx.blocking_send(m).is_err() {
                debug!("receiver dropped, stopping content search");
                return Ok(());
            }

            if let Some(max) = config.max_line_matches {
                if total_line_matches as usize >= max {
                    debug!(total_line_matches, "reached max line matches");
                    return Ok(());
                }
            }
        }

        if let Some(max) = config.max_file_matches {
            if file_match_count as usize >= max {
                debug!(file_match_count, "reached max file matches");
                return Ok(());
            }
        }
    }

    debug!(
        file_match_count,
        total_line_matches, "content search completed"
    );
    Ok(())
}

fn search_file_with_matcher(
    path: &Path,
    matcher: &RegexMatcher,
) -> Result<Vec<ContentMatch>, ()> {
    let mut matches = Vec::new();
    let path_buf = path.to_path_buf();

    let mut searcher = Searcher::new();

    let result = searcher.search_path(
        matcher,
        path,
        UTF8(|line_number, line| {
            matches.push(ContentMatch {
                path: path_buf.clone(),
                line_number,
                line: line.trim_end_matches('\n').trim_end_matches('\r').to_string(),
            });
            Ok(true)
        }),
    );

    match result {
        Ok(()) => Ok(matches),
        Err(_) => Err(()), // Binary file or read error.
    }
}

fn search_single_file(
    path: &Path,
    pattern: &str,
    case_insensitive: bool,
) -> RavenResult<Vec<ContentMatch>> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .build(pattern)
        .map_err(|e| RavenError::Search {
            message: format!("invalid search pattern: {}", e),
        })?;

    let mut matches = Vec::new();
    let path_buf = path.to_path_buf();
    let mut searcher = Searcher::new();

    searcher
        .search_path(
            &matcher,
            path,
            UTF8(|line_number, line| {
                matches.push(ContentMatch {
                    path: path_buf.clone(),
                    line_number,
                    line: line.trim_end_matches('\n').trim_end_matches('\r').to_string(),
                });
                Ok(true)
            }),
        )
        .map_err(|e| RavenError::Search {
            message: format!("error searching file {}: {}", path.display(), e),
        })?;

    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("hello.txt"),
            "Hello World\nGoodbye World\nHello Again\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("code.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(
            dir.path().join("subdir/nested.txt"),
            "This is a nested Hello file\n",
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn content_search_basic() {
        let dir = setup_test_dir();
        let searcher = ContentSearcher::new();
        let config = ContentSearchConfig::new(dir.path(), "Hello");
        let results = searcher.search_collect(config).await.unwrap();
        // "Hello World", "Hello Again" in hello.txt, "Hello" in code.rs, "Hello" in nested.txt
        assert!(results.len() >= 3);
    }

    #[tokio::test]
    async fn content_search_case_insensitive() {
        let dir = setup_test_dir();
        let searcher = ContentSearcher::new();
        let config = ContentSearchConfig {
            root: dir.path().to_path_buf(),
            pattern: "hello".to_string(),
            case_insensitive: true,
            ..ContentSearchConfig::default()
        };
        let results = searcher.search_collect(config).await.unwrap();
        assert!(results.len() >= 3);
    }

    #[tokio::test]
    async fn content_search_line_numbers() {
        let dir = setup_test_dir();
        let searcher = ContentSearcher::new();
        let results = searcher
            .search_file(
                dir.path().join("hello.txt"),
                "Hello".to_string(),
                false,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].line_number, 1);
        assert_eq!(results[0].line, "Hello World");
        assert_eq!(results[1].line_number, 3);
        assert_eq!(results[1].line, "Hello Again");
    }

    #[tokio::test]
    async fn content_search_max_line_matches() {
        let dir = setup_test_dir();
        let searcher = ContentSearcher::new();
        let config = ContentSearchConfig {
            root: dir.path().to_path_buf(),
            pattern: "Hello".to_string(),
            case_insensitive: false,
            max_line_matches: Some(1),
            ..ContentSearchConfig::default()
        };
        let results = searcher.search_collect(config).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn content_search_cancellation() {
        let dir = setup_test_dir();
        let searcher = ContentSearcher::new();
        searcher.cancel();
        let config = ContentSearchConfig::new(dir.path(), "Hello");
        let results = searcher.search_collect(config).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn content_search_invalid_pattern() {
        let searcher = ContentSearcher::new();
        let config = ContentSearchConfig::new("/tmp", "[invalid");
        let result = searcher.search(config);
        assert!(result.is_err());
    }
}
