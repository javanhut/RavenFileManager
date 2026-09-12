use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;

use raven_core::commands::AppCommand;
use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::events::{AppEvent, NotificationLevel};
use raven_core::path::RavenPath;

/// The host API exposed to plugins.
/// Plugins call these methods to interact with the file manager.
pub trait PluginApi: Send + Sync {
    /// List directory contents.
    fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, String>;

    /// Get the currently selected files.
    fn get_selection(&self) -> Result<Vec<String>, String>;

    /// Navigate to a path.
    fn navigate(&self, path: &str) -> Result<(), String>;

    /// Copy files to a destination.
    fn copy_files(&self, sources: &[String], dest: &str) -> Result<(), String>;

    /// Move files to a destination.
    fn move_files(&self, sources: &[String], dest: &str) -> Result<(), String>;

    /// Register a custom action in the context menu.
    fn register_action(&self, name: &str, label: &str) -> Result<(), String>;

    /// Send a notification to the user.
    fn send_notification(&self, title: &str, body: &str) -> Result<(), String>;

    /// Get a configuration value.
    fn get_config(&self, key: &str) -> Result<Option<String>, String>;
}

/// The API plugins get in the running file manager: navigation and file
/// operations go to the backend as commands, notifications to the window as
/// events, and listings are read straight from the local filesystem.
pub struct ChannelPluginApi {
    command_tx: UnboundedSender<AppCommand>,
    event_tx: UnboundedSender<AppEvent>,
    /// The pane the user is looking at, kept current by the backend.
    current_pane: Arc<AtomicU32>,
    config_dir: PathBuf,
    /// Actions plugins registered, as (name, label). Shown nowhere yet;
    /// kept so a plugin's registration is not silently lost.
    pub actions: Mutex<Vec<(String, String)>>,
}

impl ChannelPluginApi {
    pub fn new(
        command_tx: UnboundedSender<AppCommand>,
        event_tx: UnboundedSender<AppEvent>,
        current_pane: Arc<AtomicU32>,
        config_dir: PathBuf,
    ) -> Self {
        Self {
            command_tx,
            event_tx,
            current_pane,
            config_dir,
            actions: Mutex::new(Vec::new()),
        }
    }

    fn local_paths(paths: &[String]) -> Vec<RavenPath> {
        paths.iter().map(|p| RavenPath::local(PathBuf::from(p))).collect()
    }
}

/// A listing built from `std::fs`, enough for a plugin to look around.
fn read_local_dir(path: &Path) -> Result<Vec<FileEntry>, String> {
    let read = std::fs::read_dir(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut entries = Vec::new();
    for item in read {
        let item = item.map_err(|e| e.to_string())?;
        let name = item.file_name().to_string_lossy().to_string();
        let full = item.path();
        let meta = match item.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let kind = if meta.is_dir() {
            EntryKind::Directory
        } else if meta.file_type().is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::File
        };
        let metadata = EntryMetadata {
            size: meta.len(),
            is_hidden: name.starts_with('.'),
            ..EntryMetadata::default()
        };
        entries.push(FileEntry::new(name, RavenPath::local(full), kind, metadata));
    }
    Ok(entries)
}

impl PluginApi for ChannelPluginApi {
    fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, String> {
        read_local_dir(Path::new(path))
    }

    fn get_selection(&self) -> Result<Vec<String>, String> {
        // The selection lives in the window and is not mirrored to the
        // backend; plugins get it as hook arguments instead.
        Err("the selection is not available through the plugin API".to_string())
    }

    fn navigate(&self, path: &str) -> Result<(), String> {
        self.command_tx
            .send(AppCommand::Navigate {
                path: RavenPath::local(PathBuf::from(path)),
                pane_id: self.current_pane.load(Ordering::Relaxed),
            })
            .map_err(|_| "the file manager is shutting down".to_string())
    }

    fn copy_files(&self, sources: &[String], dest: &str) -> Result<(), String> {
        self.command_tx
            .send(AppCommand::CopyFiles {
                sources: Self::local_paths(sources),
                destination: RavenPath::local(PathBuf::from(dest)),
            })
            .map_err(|_| "the file manager is shutting down".to_string())
    }

    fn move_files(&self, sources: &[String], dest: &str) -> Result<(), String> {
        self.command_tx
            .send(AppCommand::MoveFiles {
                sources: Self::local_paths(sources),
                destination: RavenPath::local(PathBuf::from(dest)),
            })
            .map_err(|_| "the file manager is shutting down".to_string())
    }

    fn register_action(&self, name: &str, label: &str) -> Result<(), String> {
        self.actions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?
            .push((name.to_string(), label.to_string()));
        Ok(())
    }

    fn send_notification(&self, title: &str, body: &str) -> Result<(), String> {
        self.event_tx
            .send(AppEvent::Notification {
                title: title.to_string(),
                message: body.to_string(),
                level: NotificationLevel::Info,
            })
            .map_err(|_| "the file manager is shutting down".to_string())
    }

    fn get_config(&self, key: &str) -> Result<Option<String>, String> {
        Ok(match key {
            "config_dir" => Some(self.config_dir.to_string_lossy().to_string()),
            "plugins_dir" => Some(self.config_dir.join("plugins").to_string_lossy().to_string()),
            _ => None,
        })
    }
}

/// A no-op implementation for testing.
pub struct MockPluginApi {
    pub actions: std::sync::Mutex<Vec<(String, String)>>,
    pub notifications: std::sync::Mutex<Vec<(String, String)>>,
}

impl MockPluginApi {
    pub fn new() -> Self {
        Self {
            actions: std::sync::Mutex::new(Vec::new()),
            notifications: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Default for MockPluginApi {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginApi for MockPluginApi {
    fn list_dir(&self, _path: &str) -> Result<Vec<FileEntry>, String> {
        Ok(Vec::new())
    }

    fn get_selection(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn navigate(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }

    fn copy_files(&self, _sources: &[String], _dest: &str) -> Result<(), String> {
        Ok(())
    }

    fn move_files(&self, _sources: &[String], _dest: &str) -> Result<(), String> {
        Ok(())
    }

    fn register_action(&self, name: &str, label: &str) -> Result<(), String> {
        self.actions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?
            .push((name.to_string(), label.to_string()));
        Ok(())
    }

    fn send_notification(&self, title: &str, body: &str) -> Result<(), String> {
        self.notifications
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?
            .push((title.to_string(), body.to_string()));
        Ok(())
    }

    fn get_config(&self, _key: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_api_forwards_navigation_and_notifications() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel();
        let pane = Arc::new(AtomicU32::new(7));
        let api = ChannelPluginApi::new(cmd_tx, evt_tx, pane, PathBuf::from("/cfg"));

        api.navigate("/tmp").unwrap();
        match cmd_rx.try_recv().unwrap() {
            AppCommand::Navigate { path, pane_id } => {
                assert_eq!(path, RavenPath::local("/tmp"));
                assert_eq!(pane_id, 7);
            }
            other => panic!("unexpected {:?}", other),
        }

        api.send_notification("Hi", "there").unwrap();
        match evt_rx.try_recv().unwrap() {
            AppEvent::Notification { title, message, .. } => {
                assert_eq!(title, "Hi");
                assert_eq!(message, "there");
            }
            other => panic!("unexpected {:?}", other),
        }

        assert_eq!(api.get_config("config_dir").unwrap().as_deref(), Some("/cfg"));
        assert!(api.get_selection().is_err());
        assert!(api.list_dir("/definitely/not/here").is_err());
    }

    #[test]
    fn test_mock_api_register_action() {
        let api = MockPluginApi::new();
        api.register_action("compress", "Compress Files")
            .expect("Failed to register action");
        api.register_action("encrypt", "Encrypt Files")
            .expect("Failed to register action");

        let actions = api.actions.lock().unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0], ("compress".to_string(), "Compress Files".to_string()));
        assert_eq!(actions[1], ("encrypt".to_string(), "Encrypt Files".to_string()));
    }

    #[test]
    fn test_mock_api_send_notification() {
        let api = MockPluginApi::new();
        api.send_notification("Plugin Loaded", "The plugin has been loaded successfully")
            .expect("Failed to send notification");

        let notifications = api.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(
            notifications[0],
            (
                "Plugin Loaded".to_string(),
                "The plugin has been loaded successfully".to_string()
            )
        );
    }

    #[test]
    fn test_mock_api_list_dir_returns_empty() {
        let api = MockPluginApi::new();
        let result = api.list_dir("/some/path").expect("list_dir should succeed");
        assert!(result.is_empty());
    }

    #[test]
    fn test_mock_api_get_selection_returns_empty() {
        let api = MockPluginApi::new();
        let result = api.get_selection().expect("get_selection should succeed");
        assert!(result.is_empty());
    }

    #[test]
    fn test_mock_api_navigate_succeeds() {
        let api = MockPluginApi::new();
        api.navigate("/home/user").expect("navigate should succeed");
    }

    #[test]
    fn test_mock_api_copy_files_succeeds() {
        let api = MockPluginApi::new();
        let sources = vec!["/a/file.txt".to_string()];
        api.copy_files(&sources, "/dest")
            .expect("copy_files should succeed");
    }

    #[test]
    fn test_mock_api_move_files_succeeds() {
        let api = MockPluginApi::new();
        let sources = vec!["/a/file.txt".to_string()];
        api.move_files(&sources, "/dest")
            .expect("move_files should succeed");
    }

    #[test]
    fn test_mock_api_get_config_returns_none() {
        let api = MockPluginApi::new();
        let result = api
            .get_config("some.key")
            .expect("get_config should succeed");
        assert!(result.is_none());
    }

    #[test]
    fn test_mock_api_default() {
        let api = MockPluginApi::default();
        assert!(api.actions.lock().unwrap().is_empty());
        assert!(api.notifications.lock().unwrap().is_empty());
    }

    #[test]
    fn test_mock_api_multiple_notifications() {
        let api = MockPluginApi::new();
        for i in 0..5 {
            api.send_notification(&format!("Title {}", i), &format!("Body {}", i))
                .expect("Failed to send notification");
        }

        let notifications = api.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 5);
        for i in 0..5 {
            assert_eq!(notifications[i].0, format!("Title {}", i));
            assert_eq!(notifications[i].1, format!("Body {}", i));
        }
    }
}
