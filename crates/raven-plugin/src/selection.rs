//! The selection of the pane the user is looking at, shared between the
//! window and the backend.
//!
//! The window and the backend run in the same process on different threads,
//! and the selection only exists in the window's list models. Rather than
//! round-trip a request through both channels whenever a plugin (or a D-Bus
//! client) asks for it, the window publishes it here as it changes and readers
//! take a snapshot.

use std::sync::{Arc, OnceLock, RwLock};

use raven_core::path::RavenPath;

/// The form a selection is published in: local paths as strings.
///
/// Remote entries (SFTP, SMB, trash) are left out. Plugins and D-Bus clients
/// act on what they are given with ordinary filesystem calls, and a URI or a
/// path that only means something inside the VFS would send them to the
/// wrong place.
pub fn local_path_strings<'a>(paths: impl IntoIterator<Item = &'a RavenPath>) -> Vec<String> {
    paths
        .into_iter()
        .filter_map(|p| p.as_local_path())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// A cheaply clonable handle onto one published selection.
#[derive(Debug, Clone, Default)]
pub struct SharedSelection {
    paths: Arc<RwLock<Vec<String>>>,
}

impl SharedSelection {
    pub fn new() -> Self {
        Self::default()
    }

    /// The process-wide selection the window publishes into. Each window is
    /// its own process, so one per process is one per window.
    pub fn global() -> Self {
        static GLOBAL: OnceLock<SharedSelection> = OnceLock::new();
        GLOBAL.get_or_init(SharedSelection::new).clone()
    }

    /// Replace the published selection.
    pub fn set(&self, paths: Vec<String>) {
        // A writer that panicked mid-update left a plain Vec behind, which is
        // still a valid selection; recover it rather than stop publishing.
        let mut guard = self.paths.write().unwrap_or_else(|e| e.into_inner());
        *guard = paths;
    }

    /// A snapshot of the published selection.
    pub fn get(&self) -> Vec<String> {
        self.paths
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The underlying lock, for readers that do not depend on this crate
    /// (the D-Bus service takes it as a plain `Arc<RwLock<Vec<String>>>`).
    pub fn inner(&self) -> Arc<RwLock<Vec<String>>> {
        self.paths.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_and_get_snapshots() {
        let sel = SharedSelection::new();
        assert!(sel.get().is_empty());
        sel.set(vec!["/a".into(), "/b".into()]);
        let snapshot = sel.get();
        sel.set(vec!["/c".into()]);
        assert_eq!(snapshot, vec!["/a".to_string(), "/b".to_string()]);
        assert_eq!(sel.get(), vec!["/c".to_string()]);
    }

    #[test]
    fn clones_share_one_selection() {
        let writer = SharedSelection::new();
        let reader = writer.clone();
        writer.set(vec!["/x".into()]);
        assert_eq!(reader.get(), vec!["/x".to_string()]);
        assert_eq!(*writer.inner().read().unwrap(), vec!["/x".to_string()]);
    }

    #[test]
    fn global_is_one_instance() {
        let a = SharedSelection::global();
        let b = SharedSelection::global();
        assert!(Arc::ptr_eq(&a.paths, &b.paths));
    }

    #[test]
    fn only_local_paths_are_published() {
        let paths = vec![
            RavenPath::local("/home/u/a.txt"),
            RavenPath::local("/home/u/dir with space"),
        ];
        assert_eq!(
            local_path_strings(&paths),
            vec!["/home/u/a.txt".to_string(), "/home/u/dir with space".to_string()]
        );
        assert!(local_path_strings(std::iter::empty()).is_empty());
    }

    #[test]
    fn readable_from_another_thread() {
        let sel = SharedSelection::new();
        sel.set(vec!["/t".into()]);
        let other = sel.clone();
        let got = std::thread::spawn(move || other.get()).join().unwrap();
        assert_eq!(got, vec!["/t".to_string()]);
    }
}
