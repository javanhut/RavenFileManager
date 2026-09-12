//! Status for Ivaldi repositories.
//!
//! Ivaldi is the VCS Raven Linux uses in place of Git. It keeps its state in
//! a `.ivaldi` directory and has no library API, so status comes from
//! `ivaldi status --json`, run at the repository root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use raven_core::events::{GitFileStatus, GitFileStatusEntry};
use raven_core::path::RavenPath;

use crate::status::fold_into_dir;

/// Detects Ivaldi repositories by walking up looking for a `.ivaldi`
/// directory that holds a `HEAD`. The user's global configuration also
/// lives in a `.ivaldi` directory, in `$HOME`, and that one has no `HEAD`.
pub struct IvaldiDetector;

impl IvaldiDetector {
    pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
        let mut current = if path.is_file() {
            path.parent()?.to_path_buf()
        } else {
            path.to_path_buf()
        };
        loop {
            if current.join(".ivaldi").join("HEAD").is_file() {
                return Some(current);
            }
            if !current.pop() {
                return None;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct StatusReport {
    #[serde(default)]
    files: Vec<StatusFile>,
}

#[derive(Debug, Deserialize)]
struct StatusFile {
    path: String,
    state: String,
}

/// Map an Ivaldi file state to the shared status vocabulary.
pub fn map_state(state: &str) -> GitFileStatus {
    match state {
        "modified" => GitFileStatus::Modified,
        "staged" | "added" => GitFileStatus::Added,
        "deleted" | "removed" => GitFileStatus::Deleted,
        "renamed" => GitFileStatus::Renamed,
        "untracked" => GitFileStatus::Untracked,
        "ignored" => GitFileStatus::Ignored,
        "conflict" | "conflicted" => GitFileStatus::Conflict,
        _ => GitFileStatus::Modified,
    }
}

/// Parse the JSON `ivaldi status --json` prints into (path, status) pairs.
pub fn parse_status_json(json: &str) -> Result<Vec<(String, GitFileStatus)>, serde_json::Error> {
    let report: StatusReport = serde_json::from_str(json)?;
    Ok(report
        .files
        .into_iter()
        .map(|f| (f.path, map_state(&f.state)))
        .collect())
}

/// Provides per-file Ivaldi status for a directory.
pub struct IvaldiStatusProvider;

impl IvaldiStatusProvider {
    /// Status for the names in `dir`, empty when `dir` is not in an Ivaldi
    /// repository or the `ivaldi` binary is unavailable.
    ///
    /// Blocks on the child process; call it off the async runtime.
    pub fn get_status(dir: &Path) -> HashMap<String, GitFileStatus> {
        let Some(repo_root) = IvaldiDetector::find_repo_root(dir) else {
            return HashMap::new();
        };

        let output = match Command::new("ivaldi")
            .args(["status", "--json", "--quiet"])
            .current_dir(&repo_root)
            .output()
        {
            Ok(output) => output,
            Err(e) => {
                tracing::debug!("ivaldi not runnable: {}", e);
                return HashMap::new();
            }
        };
        if !output.status.success() {
            tracing::warn!(
                "ivaldi status failed in {}: {}",
                repo_root.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return HashMap::new();
        }

        let items = match parse_status_json(&String::from_utf8_lossy(&output.stdout)) {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!("could not parse ivaldi status output: {}", e);
                return HashMap::new();
            }
        };

        fold_into_dir(&repo_root, dir, items.into_iter())
    }

    /// Get status entries suitable for sending as AppEvent.
    pub fn get_status_entries(dir: &Path) -> Vec<GitFileStatusEntry> {
        Self::get_status(dir)
            .into_iter()
            .map(|(name, status)| GitFileStatusEntry {
                path: RavenPath::local(dir.join(&name)),
                status,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_status_report() {
        let json = r#"{
            "timeline": "main",
            "head": {"seal_name": "x", "hash": "y", "short_hash": "z"},
            "files": [
                {"path": "README.md", "state": "modified", "hash": "1"},
                {"path": "new.txt", "state": "untracked", "hash": "2"},
                {"path": "staged.txt", "state": "staged", "hash": "3"},
                {"path": "gone.txt", "state": "deleted", "hash": "4"}
            ]
        }"#;
        let items = parse_status_json(json).unwrap();
        assert_eq!(
            items,
            vec![
                ("README.md".to_string(), GitFileStatus::Modified),
                ("new.txt".to_string(), GitFileStatus::Untracked),
                ("staged.txt".to_string(), GitFileStatus::Added),
                ("gone.txt".to_string(), GitFileStatus::Deleted),
            ]
        );
    }

    #[test]
    fn a_clean_report_has_no_files() {
        let json = r#"{"timeline": "main", "head": {"seal_name": "x", "hash": "y", "short_hash": "z"}}"#;
        assert!(parse_status_json(json).unwrap().is_empty());
    }

    #[test]
    fn detects_the_repo_root_from_a_subdirectory() {
        let dir = std::env::temp_dir().join(format!("raven_ivaldi_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".ivaldi")).unwrap();
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        // A config-only `.ivaldi`, like the one in $HOME, is not a repository.
        assert!(IvaldiDetector::find_repo_root(&dir.join("a/b")).is_none());
        std::fs::write(dir.join(".ivaldi/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(IvaldiDetector::find_repo_root(&dir.join("a/b")), Some(dir.clone()));
        assert!(IvaldiDetector::find_repo_root(Path::new("/")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
