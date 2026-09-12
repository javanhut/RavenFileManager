use std::collections::HashMap;
use std::path::Path;

use raven_core::events::{GitFileStatus, GitFileStatusEntry};
use raven_core::path::RavenPath;

use crate::detector::RepoDetector;

/// Provides per-file git status for a directory using gitoxide (gix).
pub struct GitStatusProvider;

impl GitStatusProvider {
    /// Get git status for all files in a directory.
    /// Returns a map from filename to GitFileStatus.
    pub fn get_status(dir: &Path) -> HashMap<String, GitFileStatus> {
        let mut result = HashMap::new();

        let repo_root = match RepoDetector::find_repo_root(dir) {
            Some(root) => root,
            None => return result,
        };

        let repo = match gix::open(&repo_root) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Failed to open git repo at {}: {}", repo_root.display(), e);
                return result;
            }
        };

        let status = match repo.status(gix::progress::Discard) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to get git status: {}", e);
                return result;
            }
        };

        let platform = match status.into_index_worktree_iter(None) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to iterate git status: {}", e);
                return result;
            }
        };

        let items = platform.filter_map(|item| item.ok()).map(|item| {
            let rela_path = match &item {
                gix::status::index_worktree::Item::Modification { rela_path, .. } => {
                    rela_path.to_string()
                }
                gix::status::index_worktree::Item::DirectoryContents { entry, .. } => {
                    entry.rela_path.to_string()
                }
                gix::status::index_worktree::Item::Rewrite { dirwalk_entry, .. } => {
                    dirwalk_entry.rela_path.to_string()
                }
            };
            (rela_path, map_item_status(&item))
        });

        result.extend(fold_into_dir(&repo_root, dir, items));
        result
    }

    /// Get status entries suitable for sending as AppEvent.
    pub fn get_status_entries(dir: &Path) -> Vec<GitFileStatusEntry> {
        let statuses = Self::get_status(dir);
        statuses
            .into_iter()
            .map(|(name, status)| {
                let path = RavenPath::local(dir.join(&name));
                GitFileStatusEntry { path, status }
            })
            .collect()
    }
}

/// Reduce repository-relative changed paths to the names shown in `dir`.
///
/// A change directly in `dir` keeps its own status. A change deeper down
/// marks the subdirectory of `dir` it lives under, with the first status
/// seen winning, so a folder with any change inside it shows as changed.
/// Paths outside `dir` are dropped.
pub fn fold_into_dir(
    repo_root: &Path,
    dir: &Path,
    items: impl Iterator<Item = (String, GitFileStatus)>,
) -> HashMap<String, GitFileStatus> {
    let mut result = HashMap::new();
    for (rela_path, status) in items {
        let full_path = repo_root.join(&rela_path);
        let Ok(rel) = full_path.strip_prefix(dir) else {
            continue;
        };
        let Some(first) = rel.components().next() else {
            continue;
        };
        let name = first.as_os_str().to_string_lossy().to_string();
        let direct = rel.components().count() == 1;
        if direct {
            result.insert(name, status);
        } else {
            result.entry(name).or_insert(status);
        }
    }
    result
}

fn map_item_status(item: &gix::status::index_worktree::Item) -> GitFileStatus {
    use gix::status::index_worktree::Item;

    match item {
        Item::Modification { status, .. } => {
            use gix::status::plumbing::index_as_worktree::EntryStatus;
            match status {
                EntryStatus::Conflict { .. } => GitFileStatus::Conflict,
                EntryStatus::Change(_) => GitFileStatus::Modified,
                EntryStatus::NeedsUpdate(_) => GitFileStatus::Modified,
                EntryStatus::IntentToAdd => GitFileStatus::Added,
            }
        }
        Item::DirectoryContents { .. } => GitFileStatus::Untracked,
        Item::Rewrite { .. } => GitFileStatus::Renamed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_keeps_direct_children_and_marks_subdirectories() {
        let root = Path::new("/repo");
        let dir = Path::new("/repo/src");
        let items = vec![
            ("src/main.rs".to_string(), GitFileStatus::Modified),
            ("src/ui/window.rs".to_string(), GitFileStatus::Untracked),
            ("src/ui/state.rs".to_string(), GitFileStatus::Deleted),
            ("README.md".to_string(), GitFileStatus::Modified),
        ];
        let folded = fold_into_dir(root, dir, items.into_iter());
        assert_eq!(folded.get("main.rs"), Some(&GitFileStatus::Modified));
        // The first change seen under a subdirectory is the one it shows.
        assert_eq!(folded.get("ui"), Some(&GitFileStatus::Untracked));
        assert!(!folded.contains_key("README.md"));
        assert_eq!(folded.len(), 2);
    }

    #[test]
    fn fold_direct_status_overrides_an_earlier_subdirectory_guess() {
        let root = Path::new("/repo");
        let items = vec![
            ("ui/window.rs".to_string(), GitFileStatus::Modified),
            ("ui".to_string(), GitFileStatus::Renamed),
        ];
        let folded = fold_into_dir(root, root, items.into_iter());
        assert_eq!(folded.get("ui"), Some(&GitFileStatus::Renamed));
    }
}
