use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use raven_core::ai_types::{DuplicateGroup, DuplicateScanProgress, ScanPhase};
use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::path::RavenPath;

/// Configuration for a duplicate file scan.
#[derive(Debug, Clone)]
pub struct DuplicateScanConfig {
    pub root: PathBuf,
    pub recursive: bool,
    pub min_size: u64,
    pub include_hidden: bool,
}

impl Default for DuplicateScanConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        }
    }
}

/// Scanner for finding duplicate files using a two-phase algorithm.
pub struct DuplicateScanner {
    cancelled: Arc<AtomicBool>,
}

impl DuplicateScanner {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    /// Scan for duplicate files, streaming progress updates.
    pub async fn scan(
        &self,
        config: DuplicateScanConfig,
        progress_tx: tokio::sync::mpsc::UnboundedSender<DuplicateScanProgress>,
    ) -> Result<Vec<DuplicateGroup>, String> {
        // Phase 1: Walk and group by size
        let _ = progress_tx.send(DuplicateScanProgress {
            files_scanned: 0,
            total_files: 0,
            bytes_hashed: 0,
            duplicates_found: 0,
            phase: ScanPhase::Walking,
        });

        let size_groups = self.walk_and_group_by_size(&config, &progress_tx)?;

        if self.cancelled.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }

        // Phase 2: Hash files in same-size groups
        let _ = progress_tx.send(DuplicateScanProgress {
            files_scanned: 0,
            total_files: size_groups.values().map(|v| v.len() as u64).sum(),
            bytes_hashed: 0,
            duplicates_found: 0,
            phase: ScanPhase::Hashing,
        });

        let groups = self.hash_and_group(&size_groups, &progress_tx).await?;

        let _ = progress_tx.send(DuplicateScanProgress {
            files_scanned: 0,
            total_files: 0,
            bytes_hashed: 0,
            duplicates_found: groups.len() as u64,
            phase: ScanPhase::Complete,
        });

        Ok(groups)
    }

    fn walk_and_group_by_size(
        &self,
        config: &DuplicateScanConfig,
        progress_tx: &tokio::sync::mpsc::UnboundedSender<DuplicateScanProgress>,
    ) -> Result<HashMap<u64, Vec<PathBuf>>, String> {
        let mut size_groups: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        let mut files_scanned: u64 = 0;

        let walker = ignore::WalkBuilder::new(&config.root)
            .hidden(!config.include_hidden)
            .max_depth(if config.recursive { None } else { Some(1) })
            .build();

        for result in walker {
            if self.cancelled.load(Ordering::Relaxed) {
                return Ok(HashMap::new());
            }

            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().map_or(false, |ft| ft.is_file()) {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            let size = metadata.len();
            if size < config.min_size {
                continue;
            }

            size_groups
                .entry(size)
                .or_default()
                .push(entry.into_path());

            files_scanned += 1;
            if files_scanned % 1000 == 0 {
                let _ = progress_tx.send(DuplicateScanProgress {
                    files_scanned,
                    total_files: 0,
                    bytes_hashed: 0,
                    duplicates_found: 0,
                    phase: ScanPhase::Walking,
                });
            }
        }

        // Remove size groups with only one file (no possible duplicates)
        size_groups.retain(|_, paths| paths.len() > 1);

        Ok(size_groups)
    }

    async fn hash_and_group(
        &self,
        size_groups: &HashMap<u64, Vec<PathBuf>>,
        progress_tx: &tokio::sync::mpsc::UnboundedSender<DuplicateScanProgress>,
    ) -> Result<Vec<DuplicateGroup>, String> {
        let mut hash_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut files_hashed: u64 = 0;
        let mut bytes_hashed: u64 = 0;
        let total_files: u64 = size_groups.values().map(|v| v.len() as u64).sum();

        for (size, paths) in size_groups {
            for path in paths {
                if self.cancelled.load(Ordering::Relaxed) {
                    return Ok(Vec::new());
                }

                match hash_file(path).await {
                    Ok(hash) => {
                        hash_groups.entry(hash).or_default().push(path.clone());
                        bytes_hashed += size;
                    }
                    Err(_) => continue,
                }

                files_hashed += 1;
                if files_hashed % 100 == 0 {
                    let _ = progress_tx.send(DuplicateScanProgress {
                        files_scanned: files_hashed,
                        total_files,
                        bytes_hashed,
                        duplicates_found: 0,
                        phase: ScanPhase::Hashing,
                    });
                }
            }
        }

        // Remove hash groups with only one file
        hash_groups.retain(|_, paths| paths.len() > 1);

        // Convert to DuplicateGroup
        let groups: Vec<DuplicateGroup> = hash_groups
            .into_iter()
            .map(|(hash, paths)| {
                let size = std::fs::metadata(&paths[0])
                    .map(|m| m.len())
                    .unwrap_or(0);
                let entries = paths
                    .into_iter()
                    .map(|p| {
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        FileEntry {
                            name,
                            path: RavenPath::Local(p),
                            kind: EntryKind::File,
                            metadata: EntryMetadata {
                                size,
                                ..EntryMetadata::default()
                            },
                        }
                    })
                    .collect();
                DuplicateGroup {
                    hash,
                    size,
                    entries,
                }
            })
            .collect();

        Ok(groups)
    }
}

async fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let data = tokio::fs::read(path).await?;
    let hash = blake3::hash(&data);
    Ok(hash.to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        // Create some duplicate files
        fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        fs::write(dir.path().join("b.txt"), "hello world").unwrap();
        fs::write(dir.path().join("c.txt"), "different content").unwrap();
        fs::write(dir.path().join("d.txt"), "different content").unwrap();
        fs::write(dir.path().join("unique.txt"), "unique file content here").unwrap();
        dir
    }

    #[tokio::test]
    async fn test_scan_finds_duplicates() {
        let dir = create_test_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert_eq!(groups.len(), 2); // Two groups of duplicates
    }

    #[tokio::test]
    async fn test_scan_each_group_has_correct_count() {
        let dir = create_test_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        for group in &groups {
            assert_eq!(group.entries.len(), 2);
        }
    }

    #[tokio::test]
    async fn test_scan_cancelled() {
        let dir = create_test_dir();
        let cancel = Arc::new(AtomicBool::new(true));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn test_scan_min_size_filter() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("small1.txt"), "hi").unwrap();
        fs::write(dir.path().join("small2.txt"), "hi").unwrap();
        fs::write(dir.path().join("big1.txt"), "a".repeat(1000)).unwrap();
        fs::write(dir.path().join("big2.txt"), "a".repeat(1000)).unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 100,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].size >= 100);
    }

    #[tokio::test]
    async fn test_scan_no_duplicates() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "unique a").unwrap();
        fs::write(dir.path().join("b.txt"), "unique b").unwrap();
        fs::write(dir.path().join("c.txt"), "unique c").unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn test_scan_empty_dir() {
        let dir = TempDir::new().unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn test_scan_recursive() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        fs::write(dir.path().join("a.txt"), "same content").unwrap();
        fs::write(sub.join("b.txt"), "same content").unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert_eq!(groups.len(), 1);
    }

    #[tokio::test]
    async fn test_scan_non_recursive() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        fs::write(dir.path().join("a.txt"), "same content").unwrap();
        fs::write(sub.join("b.txt"), "same content").unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: false,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert!(groups.is_empty()); // Only one file at top level
    }

    #[tokio::test]
    async fn test_scan_group_has_hash() {
        let dir = create_test_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        for group in &groups {
            assert!(!group.hash.is_empty());
            assert!(group.hash.len() == 64); // blake3 hex length
        }
    }

    #[tokio::test]
    async fn test_scan_progress_sent() {
        let dir = create_test_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let _ = scanner.scan(config, tx).await.unwrap();

        // Should have received at least walking and complete phases
        let mut got_walking = false;
        let mut got_complete = false;
        while let Ok(progress) = rx.try_recv() {
            match progress.phase {
                ScanPhase::Walking => got_walking = true,
                ScanPhase::Complete => got_complete = true,
                _ => {}
            }
        }
        assert!(got_walking);
        assert!(got_complete);
    }

    #[tokio::test]
    async fn test_hash_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "test content").unwrap();
        let hash = hash_file(&path).await.unwrap();
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_default_config() {
        let config = DuplicateScanConfig::default();
        assert!(config.recursive);
        assert_eq!(config.min_size, 1);
        assert!(!config.include_hidden);
    }

    #[tokio::test]
    async fn test_three_way_duplicates() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "triple").unwrap();
        fs::write(dir.path().join("b.txt"), "triple").unwrap();
        fs::write(dir.path().join("c.txt"), "triple").unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = DuplicateScanner::new(cancel);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let config = DuplicateScanConfig {
            root: dir.path().to_path_buf(),
            recursive: true,
            min_size: 1,
            include_hidden: false,
        };

        let groups = scanner.scan(config, tx).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].entries.len(), 3);
    }
}
