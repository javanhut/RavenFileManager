//! The Recent view's contents: files from the shared recent-files list
//! (`recently-used.xbel`), which Raven and other applications add to.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::path::RavenPath;

/// How many files the Recent view lists at most.
pub const RECENT_LIMIT: usize = 200;

/// The recent list's local files with their last-used stamps, unchecked.
/// Main thread only (RecentManager is a GTK object) but touches no files.
pub fn recent_candidates() -> Vec<(PathBuf, i64)> {
    if !gtk::is_initialized_main_thread() {
        return Vec::new();
    }
    gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|info| info.is_local())
        .filter_map(|info| {
            let path = gio::File::for_uri(&info.uri()).path()?;
            // Opening a file bumps "visited"; adding it again bumps "modified".
            let stamp = info.modified().to_unix().max(info.visited().to_unix());
            Some((path, stamp))
        })
        .collect()
}

/// Entries for the `candidates` that still exist, newest first. Stats every
/// file, which can block on a stalled network mount, so run it off the main
/// thread.
pub fn recent_entries(candidates: Vec<(PathBuf, i64)>) -> Vec<FileEntry> {
    newest_existing(candidates, |p| p.is_file(), RECENT_LIMIT)
        .iter()
        .filter_map(|p| entry_for_file(p))
        .collect()
}

/// Order `items` newest first, drop duplicates and anything `exists` rejects,
/// and keep at most `limit`.
pub fn newest_existing(
    mut items: Vec<(PathBuf, i64)>,
    exists: impl Fn(&Path) -> bool,
    limit: usize,
) -> Vec<PathBuf> {
    // Stable, so equal stamps keep the list's own order.
    items.sort_by(|a, b| b.1.cmp(&a.1));
    let mut seen = HashSet::new();
    items
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| seen.insert(path.clone()))
        .filter(|path| exists(path))
        .take(limit)
        .collect()
}

/// A listing entry for a regular file, or `None` when it cannot be read.
fn entry_for_file(path: &Path) -> Option<FileEntry> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    let metadata = EntryMetadata {
        size: meta.len(),
        modified: meta.modified().ok().map(chrono::DateTime::from),
        accessed: meta.accessed().ok().map(chrono::DateTime::from),
        created: meta.created().ok().map(chrono::DateTime::from),
        permissions: meta.mode(),
        owner_uid: meta.uid(),
        group_gid: meta.gid(),
        is_hidden: name.starts_with('.'),
        is_executable: meta.mode() & 0o111 != 0,
        ..EntryMetadata::default()
    };
    Some(FileEntry::new(
        name,
        RavenPath::local(path.to_path_buf()),
        EntryKind::File,
        metadata,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_deduplicated_existing_and_capped() {
        let items = vec![
            (PathBuf::from("/a"), 10),
            (PathBuf::from("/gone"), 50),
            (PathBuf::from("/b"), 30),
            (PathBuf::from("/a"), 5),
            (PathBuf::from("/c"), 20),
        ];
        let exists = |p: &Path| p != Path::new("/gone");
        assert_eq!(
            newest_existing(items.clone(), exists, 10),
            vec![PathBuf::from("/b"), PathBuf::from("/c"), PathBuf::from("/a")]
        );
        assert_eq!(
            newest_existing(items, exists, 2),
            vec![PathBuf::from("/b"), PathBuf::from("/c")]
        );
    }

    #[test]
    fn real_files_become_entries() {
        let dir = std::env::temp_dir().join(format!("raven-recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, "hello").unwrap();

        let entry = entry_for_file(&file).unwrap();
        assert_eq!(entry.name, "note.txt");
        assert_eq!(entry.metadata.size, 5);
        assert!(entry.is_file());
        assert!(entry_for_file(&dir).is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
