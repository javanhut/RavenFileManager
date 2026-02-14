use std::path::{Path, PathBuf};

/// Detects git repositories by walking up the directory tree looking for `.git`.
pub struct RepoDetector;

impl RepoDetector {
    /// Find the git repository root by walking up from the given path.
    /// Returns the path containing `.git` if found.
    pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
        let mut current = if path.is_file() {
            path.parent()?.to_path_buf()
        } else {
            path.to_path_buf()
        };

        loop {
            let git_dir = current.join(".git");
            if git_dir.exists() {
                return Some(current);
            }
            if !current.pop() {
                return None;
            }
        }
    }

    /// Check if the given path is inside a git repository.
    pub fn is_in_repo(path: &Path) -> bool {
        Self::find_repo_root(path).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_repo_root_at_root() {
        // /tmp is unlikely to be a git repo
        assert!(RepoDetector::find_repo_root(Path::new("/tmp")).is_none());
    }

    #[test]
    fn test_is_in_repo() {
        assert!(!RepoDetector::is_in_repo(Path::new("/tmp")));
    }
}
