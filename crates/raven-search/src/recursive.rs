use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ignore::WalkBuilder;
use regex::Regex;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;

/// A match found during recursive filename search.
#[derive(Debug, Clone)]
pub struct FileNameMatch {
    pub entry: FileEntry,
}

/// Configuration for a recursive search operation.
#[derive(Debug, Clone)]
pub struct RecursiveSearchConfig {
    /// The root directory to search from.
    pub root: PathBuf,
    /// The search query (interpreted as substring or regex).
    pub query: String,
    /// Whether to interpret the query as a regex pattern.
    pub use_regex: bool,
    /// Whether the search should be case-insensitive.
    pub case_insensitive: bool,
    /// Whether to include hidden files and directories.
    pub include_hidden: bool,
    /// Maximum depth to recurse (None for unlimited).
    pub max_depth: Option<usize>,
    /// Maximum number of results to return (None for unlimited).
    pub max_results: Option<usize>,
}

impl Default for RecursiveSearchConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            query: String::new(),
            use_regex: false,
            case_insensitive: true,
            include_hidden: false,
            max_depth: None,
            max_results: None,
        }
    }
}

impl RecursiveSearchConfig {
    /// Create a new config with the given root and query.
    pub fn new(root: impl Into<PathBuf>, query: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            query: query.into(),
            ..Self::default()
        }
    }
}

/// Async recursive filename searcher using `ignore::WalkBuilder`.
///
/// Walks directory trees respecting `.gitignore` rules and searches for
/// filenames matching a query pattern. Results are streamed via an mpsc
/// channel.
#[derive(Debug)]
pub struct RecursiveSearcher {
    /// Shared cancellation flag.
    cancelled: Arc<AtomicBool>,
}

impl RecursiveSearcher {
    /// Create a new searcher with a fresh cancellation token.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a new searcher with a shared cancellation flag.
    pub fn with_cancellation(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    /// Get a reference to the cancellation flag.
    ///
    /// Set this to `true` from another task to cancel an in-progress search.
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

    /// Search for filenames matching the config and stream results through a channel.
    ///
    /// Returns a receiver that yields `FileNameMatch` results as they are found.
    /// The search runs on a blocking thread via `tokio::task::spawn_blocking`.
    pub fn search(
        &self,
        config: RecursiveSearchConfig,
    ) -> RavenResult<mpsc::Receiver<FileNameMatch>> {
        let (tx, rx) = mpsc::channel(256);
        let cancelled = Arc::clone(&self.cancelled);

        // Build the matcher up front so we can fail early on bad regex.
        let matcher = build_matcher(&config)?;

        tokio::task::spawn_blocking(move || {
            run_search(config, matcher, tx, cancelled);
        });

        Ok(rx)
    }

    /// Collect all search results into a Vec.
    ///
    /// This is a convenience method that consumes the channel.
    pub async fn search_collect(
        &self,
        config: RecursiveSearchConfig,
    ) -> RavenResult<Vec<FileNameMatch>> {
        let mut rx = self.search(config)?;
        let mut results = Vec::new();
        while let Some(m) = rx.recv().await {
            results.push(m);
        }
        Ok(results)
    }
}

impl Default for RecursiveSearcher {
    fn default() -> Self {
        Self::new()
    }
}

/// The internal matching strategy, pre-compiled from the search config.
enum Matcher {
    Substring { query_lower: String },
    Regex(Regex),
}

impl Matcher {
    fn is_match(&self, name: &str) -> bool {
        match self {
            Matcher::Substring { query_lower } => name.to_lowercase().contains(query_lower),
            Matcher::Regex(re) => re.is_match(name),
        }
    }
}

fn build_matcher(config: &RecursiveSearchConfig) -> RavenResult<Matcher> {
    if config.use_regex {
        let pattern = if config.case_insensitive {
            format!("(?i){}", config.query)
        } else {
            config.query.clone()
        };
        let re = Regex::new(&pattern).map_err(|e| RavenError::Search {
            message: format!("invalid regex pattern: {}", e),
        })?;
        Ok(Matcher::Regex(re))
    } else {
        let query_lower = if config.case_insensitive {
            config.query.to_lowercase()
        } else {
            config.query.clone()
        };
        Ok(Matcher::Substring { query_lower })
    }
}

fn run_search(
    config: RecursiveSearchConfig,
    matcher: Matcher,
    tx: mpsc::Sender<FileNameMatch>,
    cancelled: Arc<AtomicBool>,
) {
    debug!(root = %config.root.display(), query = %config.query, "starting recursive search");

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
    let mut count = 0u64;

    for result in walker {
        // Check cancellation.
        if cancelled.load(Ordering::Relaxed) {
            debug!("recursive search cancelled");
            return;
        }

        let dir_entry = match result {
            Ok(entry) => entry,
            Err(e) => {
                warn!(error = %e, "error during directory walk");
                continue;
            }
        };

        // Skip the root itself.
        if dir_entry.path() == config.root {
            continue;
        }

        let file_name = match dir_entry.file_name().to_str() {
            Some(name) => name.to_string(),
            None => continue,
        };

        if !matcher.is_match(&file_name) {
            continue;
        }

        let path = dir_entry.path().to_path_buf();
        let entry = path_to_file_entry(&file_name, &path);

        if tx.blocking_send(FileNameMatch { entry }).is_err() {
            // Receiver dropped; stop searching.
            debug!("receiver dropped, stopping recursive search");
            return;
        }

        count += 1;
        if let Some(max) = config.max_results {
            if count as usize >= max {
                debug!(count, "reached max results limit");
                return;
            }
        }
    }

    debug!(count, "recursive search completed");
}

fn path_to_file_entry(name: &str, path: &Path) -> FileEntry {
    let kind = if path.is_dir() {
        EntryKind::Directory
    } else if path.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };

    let metadata = match std::fs::metadata(path) {
        Ok(meta) => {
            let is_hidden = name.starts_with('.');
            #[cfg(unix)]
            let (permissions, is_executable) = {
                use std::os::unix::fs::PermissionsExt;
                let mode = meta.permissions().mode();
                (mode, mode & 0o111 != 0)
            };
            #[cfg(not(unix))]
            let (permissions, is_executable) = (0o644u32, false);

            EntryMetadata {
                size: meta.len(),
                modified: meta.modified().ok().map(chrono::DateTime::from),
                accessed: meta.accessed().ok().map(chrono::DateTime::from),
                created: meta.created().ok().map(chrono::DateTime::from),
                permissions,
                is_hidden,
                is_executable,
                ..EntryMetadata::default()
            }
        }
        Err(_) => EntryMetadata {
            is_hidden: name.starts_with('.'),
            ..EntryMetadata::default()
        },
    };

    FileEntry::new(
        name.to_string(),
        RavenPath::Local(path.to_path_buf()),
        kind,
        metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("hello.txt"), "hello").unwrap();
        fs::write(dir.path().join("world.rs"), "world").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("subdir/nested.txt"), "nested").unwrap();
        fs::write(dir.path().join(".hidden"), "hidden").unwrap();
        dir
    }

    #[tokio::test]
    async fn search_substring() {
        let dir = setup_test_dir();
        let searcher = RecursiveSearcher::new();
        let config = RecursiveSearchConfig::new(dir.path(), "hello");
        let results = searcher.search_collect(config).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.name, "hello.txt");
    }

    #[tokio::test]
    async fn search_finds_nested() {
        let dir = setup_test_dir();
        let searcher = RecursiveSearcher::new();
        let config = RecursiveSearchConfig::new(dir.path(), ".txt");
        let results = searcher.search_collect(config).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn search_respects_hidden() {
        let dir = setup_test_dir();
        let searcher = RecursiveSearcher::new();

        // Without hidden files.
        let config = RecursiveSearchConfig::new(dir.path(), "hidden");
        let results = searcher.search_collect(config).await.unwrap();
        assert!(results.is_empty());

        // With hidden files.
        searcher.reset();
        let config = RecursiveSearchConfig {
            root: dir.path().to_path_buf(),
            query: "hidden".to_string(),
            include_hidden: true,
            ..RecursiveSearchConfig::default()
        };
        let results = searcher.search_collect(config).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn search_max_results() {
        let dir = setup_test_dir();
        let searcher = RecursiveSearcher::new();
        let config = RecursiveSearchConfig {
            root: dir.path().to_path_buf(),
            query: String::new(),
            max_results: Some(2),
            ..RecursiveSearchConfig::default()
        };
        // Empty query matches everything, but capped at 2.
        let results = searcher.search_collect(config).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn search_cancellation() {
        let dir = setup_test_dir();
        let searcher = RecursiveSearcher::new();
        searcher.cancel();
        let config = RecursiveSearchConfig::new(dir.path(), "hello");
        let results = searcher.search_collect(config).await.unwrap();
        assert!(results.is_empty());
    }
}
