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

        for item in platform {
            let item = match item {
                Ok(i) => i,
                Err(_) => continue,
            };

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

            let full_path = repo_root.join(&rela_path);

            // Only include files relevant to our target directory
            if let Some(parent) = full_path.parent() {
                if parent == dir {
                    // Directly in our directory
                    if let Some(file_name) = full_path.file_name() {
                        let name = file_name.to_string_lossy().to_string();
                        let status = map_item_status(&item);
                        result.insert(name, status);
                    }
                } else if let Ok(rel) = full_path.strip_prefix(dir) {
                    // In a subdirectory — mark the top-level dir
                    if let Some(first_component) = rel.components().next() {
                        let dir_name = first_component.as_os_str().to_string_lossy().to_string();
                        let status = map_item_status(&item);
                        result.entry(dir_name).or_insert(status);
                    }
                }
            }
        }

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
