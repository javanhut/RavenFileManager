use std::path::PathBuf;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// Watches a `.git` directory for changes and sends notifications.
pub struct GitWatcher {
    _watcher: RecommendedWatcher,
}

impl GitWatcher {
    /// Start watching the .git directory at the given repo root.
    /// Sends a notification on the channel whenever .git contents change.
    pub fn new(
        repo_root: PathBuf,
        notify_tx: mpsc::UnboundedSender<PathBuf>,
    ) -> Result<Self, notify::Error> {
        let git_dir = repo_root.join(".git");

        let root = repo_root.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {
                        let _ = notify_tx.send(root.clone());
                    }
                    _ => {}
                }
            }
        })?;

        if git_dir.exists() {
            watcher.watch(&git_dir, RecursiveMode::Recursive)?;
        }

        Ok(Self {
            _watcher: watcher,
        })
    }
}
