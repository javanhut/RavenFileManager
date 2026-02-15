use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use raven_core::events::AppEvent;

use crate::api::PluginApi;
use crate::host::{CommandPluginHost, PluginHost};
use crate::manifest::{PluginManifest, PluginRuntime};

/// Manages plugin discovery, loading, lifecycle, and event broadcasting.
pub struct PluginManager {
    plugins: HashMap<String, Box<dyn PluginHost>>,
    plugin_dirs: Vec<PathBuf>,
    event_tx: mpsc::UnboundedSender<AppEvent>,
}

impl PluginManager {
    pub fn new(event_tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self {
            plugins: HashMap::new(),
            plugin_dirs: Vec::new(),
            event_tx,
        }
    }

    /// Add a directory to scan for plugins.
    pub fn add_plugin_dir(&mut self, dir: PathBuf) {
        self.plugin_dirs.push(dir);
    }

    /// Discover plugins in all plugin directories.
    ///
    /// Scans each plugin directory for subdirectories containing a `plugin.toml` file.
    /// Returns a list of (plugin_directory, manifest) tuples for all valid plugins found.
    pub fn discover(&self) -> Vec<(PathBuf, PluginManifest)> {
        let mut found = Vec::new();

        for dir in &self.plugin_dirs {
            if !dir.is_dir() {
                tracing::debug!(dir = %dir.display(), "Plugin directory does not exist, skipping");
                continue;
            }

            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    tracing::warn!(
                        dir = %dir.display(),
                        error = %e,
                        "Failed to read plugin directory"
                    );
                    continue;
                }
            };

            for entry in entries.flatten() {
                let entry_path = entry.path();
                if !entry_path.is_dir() {
                    continue;
                }

                let manifest_path = entry_path.join("plugin.toml");
                if !manifest_path.exists() {
                    continue;
                }

                match PluginManifest::load(&manifest_path) {
                    Ok(manifest) => {
                        tracing::info!(
                            plugin_id = %manifest.id,
                            name = %manifest.name,
                            "Discovered plugin"
                        );
                        found.push((entry_path, manifest));
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %manifest_path.display(),
                            error = %e,
                            "Failed to load plugin manifest"
                        );
                    }
                }
            }
        }

        found
    }

    /// Load a plugin from a directory.
    ///
    /// Reads the manifest from `plugin_dir/plugin.toml`, creates the appropriate
    /// plugin host based on the runtime type, loads the plugin, and emits a
    /// `PluginLoaded` event.
    pub fn load_plugin(
        &mut self,
        plugin_dir: &Path,
        api: Arc<dyn PluginApi>,
    ) -> Result<String, String> {
        let manifest_path = plugin_dir.join("plugin.toml");
        let manifest = PluginManifest::load(&manifest_path)?;
        let plugin_id = manifest.id.clone();
        let plugin_name = manifest.name.clone();

        if self.plugins.contains_key(&plugin_id) {
            return Err(format!("Plugin '{}' is already loaded", plugin_id));
        }

        let mut host: Box<dyn PluginHost> = match manifest.runtime {
            PluginRuntime::Command => {
                Box::new(CommandPluginHost::new(manifest, plugin_dir.to_path_buf()))
            }
            PluginRuntime::Lua => {
                return Err(
                    "Lua runtime is not available. Install a Lua plugin host to use Lua plugins."
                        .to_string(),
                );
            }
            PluginRuntime::Wasm => {
                return Err(
                    "WASM runtime is not available. Install a WASM plugin host to use WASM plugins."
                        .to_string(),
                );
            }
        };

        host.load(api)?;

        let _ = self.event_tx.send(AppEvent::PluginLoaded {
            plugin_id: plugin_id.clone(),
            name: plugin_name,
        });

        self.plugins.insert(plugin_id.clone(), host);

        Ok(plugin_id)
    }

    /// Unload a plugin by ID.
    pub fn unload_plugin(&mut self, plugin_id: &str) -> Result<(), String> {
        let mut host = self
            .plugins
            .remove(plugin_id)
            .ok_or_else(|| format!("Plugin '{}' is not loaded", plugin_id))?;

        host.unload()?;

        let _ = self.event_tx.send(AppEvent::PluginUnloaded {
            plugin_id: plugin_id.to_string(),
        });

        Ok(())
    }

    /// Get a list of loaded plugin IDs and names.
    pub fn loaded_plugins(&self) -> Vec<(String, String)> {
        self.plugins
            .values()
            .map(|host| {
                (
                    host.plugin_id().to_string(),
                    host.manifest().name.clone(),
                )
            })
            .collect()
    }

    /// Broadcast an event to all loaded plugins.
    ///
    /// Calls the specified hook on every loaded plugin. If a plugin fails to
    /// handle the event, a `PluginError` event is emitted but execution continues
    /// for the remaining plugins.
    pub fn broadcast_event(&self, event: &str, args: &[&str]) {
        for (plugin_id, host) in &self.plugins {
            if let Err(e) = host.call_hook(event, args) {
                tracing::error!(
                    plugin_id = %plugin_id,
                    event = %event,
                    error = %e,
                    "Plugin failed to handle event"
                );
                let _ = self.event_tx.send(AppEvent::PluginError {
                    plugin_id: plugin_id.clone(),
                    error: e,
                });
            }
        }
    }

    /// Default plugin directories.
    ///
    /// Returns standard locations where plugins may be installed:
    /// - `$XDG_CONFIG_HOME/raven/plugins` or `~/.config/raven/plugins` (user plugins)
    /// - `/usr/share/raven/plugins` (system plugins)
    pub fn default_plugin_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // Respect XDG_CONFIG_HOME, fall back to ~/.config
        if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
            dirs.push(PathBuf::from(xdg_config).join("raven").join("plugins"));
        } else if let Ok(home) = std::env::var("HOME") {
            dirs.push(
                PathBuf::from(home)
                    .join(".config")
                    .join("raven")
                    .join("plugins"),
            );
        }

        dirs.push(PathBuf::from("/usr/share/raven/plugins"));

        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MockPluginApi;
    use std::io::Write;

    fn write_manifest(dir: &Path, manifest: &str) {
        let manifest_path = dir.join("plugin.toml");
        let mut f = std::fs::File::create(&manifest_path).expect("Failed to create manifest");
        f.write_all(manifest.as_bytes())
            .expect("Failed to write manifest");
    }

    fn write_script(dir: &Path, name: &str, content: &str) {
        let script_path = dir.join(name);
        let mut f = std::fs::File::create(&script_path).expect("Failed to create script");
        f.write_all(content.as_bytes())
            .expect("Failed to write script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("Failed to set permissions");
        }
    }

    fn sample_manifest_toml(id: &str, name: &str) -> String {
        format!(
            r#"
id = "{}"
name = "{}"
version = "0.1.0"
author = "Test Author"
description = "A test plugin"
runtime = "command"
entry_point = "run.sh"
permissions = ["read_files"]
"#,
            id, name
        )
    }

    fn create_plugin_dir(parent: &Path, id: &str, name: &str) -> PathBuf {
        let plugin_dir = parent.join(id);
        std::fs::create_dir_all(&plugin_dir).expect("Failed to create plugin dir");
        write_manifest(&plugin_dir, &sample_manifest_toml(id, name));
        write_script(&plugin_dir, "run.sh", "#!/bin/sh\nexit 0\n");
        plugin_dir
    }

    #[test]
    fn test_discover_plugins() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");

        // Create two plugin directories
        create_plugin_dir(base_dir.path(), "plugin-a", "Plugin A");
        create_plugin_dir(base_dir.path(), "plugin-b", "Plugin B");

        // Create a non-plugin directory (no plugin.toml)
        let non_plugin = base_dir.path().join("not-a-plugin");
        std::fs::create_dir_all(&non_plugin).expect("Failed to create dir");

        // Create a file (not a directory) that should be skipped
        std::fs::write(base_dir.path().join("random-file.txt"), "hello")
            .expect("Failed to write file");

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        manager.add_plugin_dir(base_dir.path().to_path_buf());

        let discovered = manager.discover();
        assert_eq!(discovered.len(), 2);

        let ids: Vec<&str> = discovered.iter().map(|(_, m)| m.id.as_str()).collect();
        assert!(ids.contains(&"plugin-a"));
        assert!(ids.contains(&"plugin-b"));
    }

    #[test]
    fn test_discover_nonexistent_dir() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        manager.add_plugin_dir(PathBuf::from("/nonexistent/plugins/dir"));

        let discovered = manager.discover();
        assert!(discovered.is_empty());
    }

    #[test]
    fn test_load_and_unload_plugin() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let plugin_dir = create_plugin_dir(base_dir.path(), "my-plugin", "My Plugin");

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);

        let api = Arc::new(MockPluginApi::new());
        let plugin_id = manager
            .load_plugin(&plugin_dir, api)
            .expect("Failed to load plugin");

        assert_eq!(plugin_id, "my-plugin");

        // Check that PluginLoaded event was sent
        let event = rx.try_recv().expect("Expected PluginLoaded event");
        match event {
            AppEvent::PluginLoaded { plugin_id, name } => {
                assert_eq!(plugin_id, "my-plugin");
                assert_eq!(name, "My Plugin");
            }
            other => panic!("Expected PluginLoaded, got {:?}", other),
        }

        // Check loaded_plugins
        let loaded = manager.loaded_plugins();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, "my-plugin");
        assert_eq!(loaded[0].1, "My Plugin");

        // Unload
        manager
            .unload_plugin("my-plugin")
            .expect("Failed to unload plugin");

        // Check that PluginUnloaded event was sent
        let event = rx.try_recv().expect("Expected PluginUnloaded event");
        match event {
            AppEvent::PluginUnloaded { plugin_id } => {
                assert_eq!(plugin_id, "my-plugin");
            }
            other => panic!("Expected PluginUnloaded, got {:?}", other),
        }

        // Verify no plugins left
        assert!(manager.loaded_plugins().is_empty());
    }

    #[test]
    fn test_load_nonexistent_plugin() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        let api = Arc::new(MockPluginApi::new());

        let result = manager.load_plugin(Path::new("/nonexistent/plugin"), api);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_duplicate_plugin() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let plugin_dir = create_plugin_dir(base_dir.path(), "dup-plugin", "Dup Plugin");

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        let api = Arc::new(MockPluginApi::new());

        let api_dyn: Arc<dyn PluginApi> = api;
        manager
            .load_plugin(&plugin_dir, Arc::clone(&api_dyn))
            .expect("First load should succeed");

        let result = manager.load_plugin(&plugin_dir, api_dyn);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already loaded"));
    }

    #[test]
    fn test_unload_nonexistent_plugin() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);

        let result = manager.unload_plugin("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not loaded"));
    }

    #[test]
    fn test_loaded_plugins_empty() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let manager = PluginManager::new(tx);
        assert!(manager.loaded_plugins().is_empty());
    }

    #[test]
    fn test_broadcast_event() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let plugin_dir = create_plugin_dir(base_dir.path(), "event-plugin", "Event Plugin");

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        let api = Arc::new(MockPluginApi::new());

        manager
            .load_plugin(&plugin_dir, api)
            .expect("Failed to load plugin");

        // Broadcasting should not panic; the script exits 0
        manager.broadcast_event("on_directory_changed", &["/home/user"]);
    }

    #[test]
    fn test_broadcast_event_no_plugins() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let manager = PluginManager::new(tx);

        // Should not panic with no plugins
        manager.broadcast_event("on_directory_changed", &["/home/user"]);
    }

    #[test]
    fn test_add_multiple_plugin_dirs() {
        let dir_a = tempfile::tempdir().expect("Failed to create temp dir");
        let dir_b = tempfile::tempdir().expect("Failed to create temp dir");

        create_plugin_dir(dir_a.path(), "plugin-from-a", "Plugin From A");
        create_plugin_dir(dir_b.path(), "plugin-from-b", "Plugin From B");

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        manager.add_plugin_dir(dir_a.path().to_path_buf());
        manager.add_plugin_dir(dir_b.path().to_path_buf());

        let discovered = manager.discover();
        assert_eq!(discovered.len(), 2);

        let ids: Vec<&str> = discovered.iter().map(|(_, m)| m.id.as_str()).collect();
        assert!(ids.contains(&"plugin-from-a"));
        assert!(ids.contains(&"plugin-from-b"));
    }

    #[test]
    fn test_load_plugin_with_missing_entry_point() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let plugin_dir = base_dir.path().join("broken-plugin");
        std::fs::create_dir_all(&plugin_dir).expect("Failed to create plugin dir");

        // Write manifest but do NOT create the entry point script
        write_manifest(
            &plugin_dir,
            &sample_manifest_toml("broken-plugin", "Broken Plugin"),
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        let api = Arc::new(MockPluginApi::new());

        let result = manager.load_plugin(&plugin_dir, api);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Entry point not found"));
    }

    #[test]
    fn test_default_plugin_dirs() {
        let dirs = PluginManager::default_plugin_dirs();
        // Should always have at least the system dir
        assert!(!dirs.is_empty());
        assert!(dirs
            .iter()
            .any(|d| d == &PathBuf::from("/usr/share/raven/plugins")));
    }

    #[test]
    fn test_discover_with_invalid_manifest() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");

        // Create a valid plugin
        create_plugin_dir(base_dir.path(), "valid-plugin", "Valid Plugin");

        // Create a plugin with invalid manifest
        let bad_dir = base_dir.path().join("bad-plugin");
        std::fs::create_dir_all(&bad_dir).expect("Failed to create dir");
        std::fs::write(bad_dir.join("plugin.toml"), "this is not valid toml {{{{")
            .expect("Failed to write file");

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        manager.add_plugin_dir(base_dir.path().to_path_buf());

        let discovered = manager.discover();
        // Only the valid plugin should be discovered
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].1.id, "valid-plugin");
    }

    #[test]
    fn test_load_lua_plugin_returns_error() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let plugin_dir = base_dir.path().join("lua-plugin");
        std::fs::create_dir_all(&plugin_dir).expect("Failed to create dir");

        let manifest = r#"
id = "lua-plugin"
name = "Lua Plugin"
version = "0.1.0"
author = "Author"
description = "A Lua plugin"
runtime = "lua"
entry_point = "init.lua"
"#;
        write_manifest(&plugin_dir, manifest);

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        let api = Arc::new(MockPluginApi::new());

        let result = manager.load_plugin(&plugin_dir, api);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Lua runtime is not available"));
    }

    #[test]
    fn test_load_wasm_plugin_returns_error() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let plugin_dir = base_dir.path().join("wasm-plugin");
        std::fs::create_dir_all(&plugin_dir).expect("Failed to create dir");

        let manifest = r#"
id = "wasm-plugin"
name = "WASM Plugin"
version = "0.1.0"
author = "Author"
description = "A WASM plugin"
runtime = "wasm"
entry_point = "plugin.wasm"
"#;
        write_manifest(&plugin_dir, manifest);

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut manager = PluginManager::new(tx);
        let api = Arc::new(MockPluginApi::new());

        let result = manager.load_plugin(&plugin_dir, api);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("WASM runtime is not available"));
    }
}
