pub mod detector;
pub mod ivaldi;
pub mod status;
pub mod watcher;

use std::path::Path;

use raven_core::events::GitFileStatusEntry;

/// Per-file version-control status for the names in `dir`, from whichever
/// repository contains it: Git first, then Ivaldi. Empty when `dir` is in
/// neither, or when the repository is clean.
///
/// Blocks on repository I/O; call it off the async runtime.
pub fn status_for_dir(dir: &Path) -> Vec<GitFileStatusEntry> {
    if detector::RepoDetector::is_in_repo(dir) {
        return status::GitStatusProvider::get_status_entries(dir);
    }
    ivaldi::IvaldiStatusProvider::get_status_entries(dir)
}
