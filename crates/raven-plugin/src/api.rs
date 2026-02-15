use raven_core::entry::FileEntry;

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
