use std::path::PathBuf;
use std::sync::Arc;

use crate::api::PluginApi;
use crate::manifest::PluginManifest;

/// Trait for plugin runtime hosts.
///
/// Each plugin runtime (Lua, WASM, Command) implements this trait.
/// The trait provides a uniform lifecycle interface: load, call hooks, and unload.
pub trait PluginHost: Send + Sync {
    /// Get the plugin ID.
    fn plugin_id(&self) -> &str;

    /// Get the plugin manifest.
    fn manifest(&self) -> &PluginManifest;

    /// Initialize and load the plugin.
    fn load(&mut self, api: Arc<dyn PluginApi>) -> Result<(), String>;

    /// Call a hook/event handler in the plugin.
    fn call_hook(&self, event: &str, args: &[&str]) -> Result<(), String>;

    /// Unload/cleanup the plugin.
    fn unload(&mut self) -> Result<(), String>;

    /// Check if the plugin is currently loaded.
    fn is_loaded(&self) -> bool;
}

/// A command-based plugin host that runs shell scripts.
///
/// This is the simplest plugin runtime. It executes shell commands (scripts)
/// with the event name and arguments passed as command-line arguments.
pub struct CommandPluginHost {
    manifest: PluginManifest,
    plugin_dir: PathBuf,
    loaded: bool,
    api: Option<Arc<dyn PluginApi>>,
}

impl CommandPluginHost {
    pub fn new(manifest: PluginManifest, plugin_dir: PathBuf) -> Self {
        Self {
            manifest,
            plugin_dir,
            loaded: false,
            api: None,
        }
    }

    /// Get the full path to the entry point script.
    fn entry_point_path(&self) -> PathBuf {
        self.plugin_dir.join(&self.manifest.entry_point)
    }
}

impl PluginHost for CommandPluginHost {
    fn plugin_id(&self) -> &str {
        &self.manifest.id
    }

    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn load(&mut self, api: Arc<dyn PluginApi>) -> Result<(), String> {
        let entry_point = self.entry_point_path();
        if !entry_point.exists() {
            return Err(format!(
                "Entry point not found: {}",
                entry_point.display()
            ));
        }

        self.api = Some(api);
        self.loaded = true;

        tracing::info!(
            plugin_id = %self.manifest.id,
            entry_point = %entry_point.display(),
            "Command plugin loaded"
        );

        Ok(())
    }

    fn call_hook(&self, event: &str, args: &[&str]) -> Result<(), String> {
        if !self.loaded {
            return Err(format!(
                "Plugin '{}' is not loaded",
                self.manifest.id
            ));
        }

        let entry_point = self.entry_point_path();

        tracing::debug!(
            plugin_id = %self.manifest.id,
            event = %event,
            "Calling hook on command plugin"
        );

        let mut cmd = std::process::Command::new(&entry_point);
        cmd.arg(event);
        cmd.args(args);
        cmd.current_dir(&self.plugin_dir);

        // Pass plugin ID and directory as environment variables
        cmd.env("RAVEN_PLUGIN_ID", &self.manifest.id);
        cmd.env("RAVEN_PLUGIN_DIR", &self.plugin_dir);

        let output = cmd.output().map_err(|e| {
            format!(
                "Failed to execute plugin '{}' entry point: {}",
                self.manifest.id, e
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Plugin '{}' hook '{}' failed (exit code {:?}): {}",
                self.manifest.id,
                event,
                output.status.code(),
                stderr.trim()
            ));
        }

        Ok(())
    }

    fn unload(&mut self) -> Result<(), String> {
        if !self.loaded {
            return Err(format!(
                "Plugin '{}' is not loaded",
                self.manifest.id
            ));
        }

        self.api = None;
        self.loaded = false;

        tracing::info!(
            plugin_id = %self.manifest.id,
            "Command plugin unloaded"
        );

        Ok(())
    }

    fn is_loaded(&self) -> bool {
        self.loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MockPluginApi;
    use crate::manifest::{PluginPermission, PluginRuntime};

    fn sample_manifest() -> PluginManifest {
        PluginManifest {
            id: "test-cmd-plugin".to_string(),
            name: "Test Command Plugin".to_string(),
            version: "0.1.0".to_string(),
            author: "Tester".to_string(),
            description: "A test command plugin".to_string(),
            runtime: PluginRuntime::Command,
            entry_point: "run.sh".to_string(),
            permissions: vec![PluginPermission::ReadFiles],
        }
    }

    #[test]
    fn test_command_host_new() {
        let manifest = sample_manifest();
        let host = CommandPluginHost::new(manifest.clone(), PathBuf::from("/tmp/plugin"));

        assert_eq!(host.plugin_id(), "test-cmd-plugin");
        assert_eq!(host.manifest().name, "Test Command Plugin");
        assert!(!host.is_loaded());
    }

    #[test]
    fn test_command_host_lifecycle() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest = sample_manifest();

        // Create a dummy entry point script
        let script_path = dir.path().join("run.sh");
        std::fs::write(&script_path, "#!/bin/sh\nexit 0\n").expect("Failed to write script");

        // Make it executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("Failed to set permissions");
        }

        let mut host = CommandPluginHost::new(manifest, dir.path().to_path_buf());
        let api = Arc::new(MockPluginApi::new());

        // Initially not loaded
        assert!(!host.is_loaded());

        // Load the plugin
        host.load(api).expect("Failed to load plugin");
        assert!(host.is_loaded());

        // Unload the plugin
        host.unload().expect("Failed to unload plugin");
        assert!(!host.is_loaded());
    }

    #[test]
    fn test_command_host_load_missing_entry_point() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest = sample_manifest();

        let mut host = CommandPluginHost::new(manifest, dir.path().to_path_buf());
        let api = Arc::new(MockPluginApi::new());

        let result = host.load(api);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Entry point not found"));
    }

    #[test]
    fn test_command_host_call_hook_when_loaded() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest = sample_manifest();

        // Create a script that just exits 0
        let script_path = dir.path().join("run.sh");
        std::fs::write(&script_path, "#!/bin/sh\nexit 0\n").expect("Failed to write script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("Failed to set permissions");
        }

        let mut host = CommandPluginHost::new(manifest, dir.path().to_path_buf());
        let api = Arc::new(MockPluginApi::new());
        host.load(api).expect("Failed to load plugin");

        let result = host.call_hook("on_directory_changed", &["/home/user"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_command_host_call_hook_when_not_loaded() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest = sample_manifest();

        let host = CommandPluginHost::new(manifest, dir.path().to_path_buf());

        let result = host.call_hook("on_directory_changed", &["/home/user"]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not loaded"));
    }

    #[test]
    fn test_command_host_unload_when_not_loaded() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest = sample_manifest();

        let mut host = CommandPluginHost::new(manifest, dir.path().to_path_buf());

        let result = host.unload();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not loaded"));
    }

    #[test]
    fn test_command_host_call_hook_failing_script() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest = sample_manifest();

        // Create a script that exits with an error
        let script_path = dir.path().join("run.sh");
        std::fs::write(
            &script_path,
            "#!/bin/sh\necho 'Something went wrong' >&2\nexit 1\n",
        )
        .expect("Failed to write script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("Failed to set permissions");
        }

        let mut host = CommandPluginHost::new(manifest, dir.path().to_path_buf());
        let api = Arc::new(MockPluginApi::new());
        host.load(api).expect("Failed to load plugin");

        let result = host.call_hook("on_start", &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("failed"));
        assert!(err.contains("Something went wrong"));
    }

    #[test]
    fn test_command_host_manifest_access() {
        let manifest = sample_manifest();
        let host = CommandPluginHost::new(manifest.clone(), PathBuf::from("/tmp/test"));

        assert_eq!(host.manifest().id, "test-cmd-plugin");
        assert_eq!(host.manifest().version, "0.1.0");
        assert_eq!(host.manifest().runtime, PluginRuntime::Command);
        assert!(host.manifest().has_permission(PluginPermission::ReadFiles));
    }
}
