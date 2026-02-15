use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;

/// Cached directory size entry.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub size: u64,
    pub computed_at: Instant,
    pub generation: u64,
}

/// Thread-safe directory size cache with generation-gated writes and delta propagation.
#[derive(Debug, Clone)]
pub struct DirSizeCache {
    entries: Arc<DashMap<PathBuf, CacheEntry>>,
    global_generation: Arc<AtomicU64>,
}

impl DirSizeCache {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            global_generation: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Lookup a cached size.
    pub fn get(&self, path: &Path) -> Option<CacheEntry> {
        self.entries.get(path).map(|e| e.value().clone())
    }

    /// Unconditional write — used for delta updates where we know the value is correct.
    pub fn set(&self, path: &Path, size: u64) {
        let gen = self.global_generation.load(Ordering::Acquire);
        self.entries.insert(
            path.to_path_buf(),
            CacheEntry {
                size,
                computed_at: Instant::now(),
                generation: gen,
            },
        );
    }

    /// Write only if the generation matches (no newer invalidation occurred).
    /// Returns true if the write succeeded.
    pub fn insert_if_current(&self, path: &Path, size: u64, generation: u64) -> bool {
        let key = path.to_path_buf();
        // Check if a newer generation exists for this entry
        if let Some(existing) = self.entries.get(&key) {
            if existing.generation > generation {
                return false;
            }
        }
        self.entries.insert(
            key,
            CacheEntry {
                size,
                computed_at: Instant::now(),
                generation,
            },
        );
        true
    }

    /// Add/subtract from the cached size. Returns the new size if the entry existed.
    pub fn apply_delta(&self, path: &Path, delta: i64) -> Option<u64> {
        let key = path.to_path_buf();
        let mut entry = self.entries.get_mut(&key)?;
        let new_size = if delta >= 0 {
            entry.size.saturating_add(delta as u64)
        } else {
            entry.size.saturating_sub((-delta) as u64)
        };
        entry.size = new_size;
        entry.computed_at = Instant::now();
        Some(new_size)
    }

    /// Mark a path as stale by bumping its generation. Returns the new generation.
    pub fn invalidate(&self, path: &Path) -> u64 {
        let new_gen = self.global_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let key = path.to_path_buf();
        if let Some(mut entry) = self.entries.get_mut(&key) {
            entry.generation = new_gen;
        }
        new_gen
    }

    /// Walk the parent chain applying a delta to each ancestor.
    /// Returns a list of (path, new_size) for each updated ancestor.
    pub fn propagate_delta_to_ancestors(&self, child: &Path, delta: i64) -> Vec<(PathBuf, u64)> {
        let mut updated = Vec::new();
        let mut current = child.parent();
        while let Some(parent) = current {
            if let Some(new_size) = self.apply_delta(parent, delta) {
                updated.push((parent.to_path_buf(), new_size));
            } else {
                // No cached entry for this ancestor — stop propagating
                break;
            }
            current = parent.parent();
        }
        updated
    }

    /// Remove all entries under a given root path (inclusive).
    pub fn evict_tree(&self, root: &Path) {
        let root_str = root.to_string_lossy().to_string();
        self.entries.retain(|k, _| {
            let k_str = k.to_string_lossy();
            // Remove the root itself and anything that starts with root + separator
            !(k_str == root_str || k_str.starts_with(&format!("{}/", root_str)))
        });
    }

    /// Number of cached entries (for testing/diagnostics).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for DirSizeCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/home/user/docs"), 1024);

        let entry = cache.get(Path::new("/home/user/docs")).unwrap();
        assert_eq!(entry.size, 1024);
    }

    #[test]
    fn test_get_missing() {
        let cache = DirSizeCache::new();
        assert!(cache.get(Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn test_insert_if_current_succeeds() {
        let cache = DirSizeCache::new();
        let gen = cache.invalidate(Path::new("/home/user/docs"));
        assert!(cache.insert_if_current(Path::new("/home/user/docs"), 2048, gen));

        let entry = cache.get(Path::new("/home/user/docs")).unwrap();
        assert_eq!(entry.size, 2048);
    }

    #[test]
    fn test_insert_if_current_rejected_by_newer_generation() {
        let cache = DirSizeCache::new();
        let old_gen = cache.invalidate(Path::new("/home/user/docs"));

        // Simulate a newer invalidation arriving
        cache.set(Path::new("/home/user/docs"), 5000);
        let _newer_gen = cache.invalidate(Path::new("/home/user/docs"));

        // Old generation write should be rejected
        assert!(!cache.insert_if_current(Path::new("/home/user/docs"), 2048, old_gen));

        // Size should remain from the set() call
        let entry = cache.get(Path::new("/home/user/docs")).unwrap();
        assert_eq!(entry.size, 5000);
    }

    #[test]
    fn test_apply_delta_positive() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/data"), 1000);

        let new_size = cache.apply_delta(Path::new("/data"), 500).unwrap();
        assert_eq!(new_size, 1500);

        let entry = cache.get(Path::new("/data")).unwrap();
        assert_eq!(entry.size, 1500);
    }

    #[test]
    fn test_apply_delta_negative() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/data"), 1000);

        let new_size = cache.apply_delta(Path::new("/data"), -300).unwrap();
        assert_eq!(new_size, 700);
    }

    #[test]
    fn test_apply_delta_saturates_at_zero() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/data"), 100);

        let new_size = cache.apply_delta(Path::new("/data"), -500).unwrap();
        assert_eq!(new_size, 0);
    }

    #[test]
    fn test_apply_delta_missing_entry() {
        let cache = DirSizeCache::new();
        assert!(cache.apply_delta(Path::new("/missing"), 100).is_none());
    }

    #[test]
    fn test_propagate_delta_to_ancestors() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/home"), 10000);
        cache.set(Path::new("/home/user"), 5000);
        cache.set(Path::new("/home/user/docs"), 2000);

        // A file was deleted from /home/user/docs/sub — propagate -500 from the sub dir
        let updated =
            cache.propagate_delta_to_ancestors(Path::new("/home/user/docs/sub"), -500);

        assert_eq!(updated.len(), 3);
        assert_eq!(updated[0], (PathBuf::from("/home/user/docs"), 1500));
        assert_eq!(updated[1], (PathBuf::from("/home/user"), 4500));
        assert_eq!(updated[2], (PathBuf::from("/home"), 9500));
    }

    #[test]
    fn test_propagate_stops_at_uncached_ancestor() {
        let cache = DirSizeCache::new();
        // Only cache the immediate parent
        cache.set(Path::new("/home/user/docs"), 2000);

        let updated =
            cache.propagate_delta_to_ancestors(Path::new("/home/user/docs/sub"), -500);

        // Should update docs but stop at /home/user (not cached)
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0], (PathBuf::from("/home/user/docs"), 1500));
    }

    #[test]
    fn test_evict_tree() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/home/user/docs"), 2000);
        cache.set(Path::new("/home/user/docs/sub1"), 500);
        cache.set(Path::new("/home/user/docs/sub2"), 800);
        cache.set(Path::new("/home/user/music"), 3000);

        cache.evict_tree(Path::new("/home/user/docs"));

        assert!(cache.get(Path::new("/home/user/docs")).is_none());
        assert!(cache.get(Path::new("/home/user/docs/sub1")).is_none());
        assert!(cache.get(Path::new("/home/user/docs/sub2")).is_none());
        // music should be untouched
        assert_eq!(cache.get(Path::new("/home/user/music")).unwrap().size, 3000);
    }

    #[test]
    fn test_evict_tree_no_false_positives() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/home/user/docs"), 100);
        cache.set(Path::new("/home/user/docs-backup"), 200);

        cache.evict_tree(Path::new("/home/user/docs"));

        assert!(cache.get(Path::new("/home/user/docs")).is_none());
        // "docs-backup" should NOT be evicted (it's not under "docs/")
        assert_eq!(
            cache.get(Path::new("/home/user/docs-backup")).unwrap().size,
            200
        );
    }

    #[test]
    fn test_invalidate_bumps_generation() {
        let cache = DirSizeCache::new();
        cache.set(Path::new("/data"), 1000);

        let gen1 = cache.invalidate(Path::new("/data"));
        let gen2 = cache.invalidate(Path::new("/data"));

        assert!(gen2 > gen1);
    }

    #[test]
    fn test_len_and_is_empty() {
        let cache = DirSizeCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        cache.set(Path::new("/data"), 100);
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }
}
