use std::path::PathBuf;
use std::sync::Arc;

use crate::api::PluginApi;
use crate::manager::ACTION_HOOK;
use crate::manifest::PluginManifest;

/// Largest `RAVEN_SELECTION` value we put in the environment. Linux refuses
/// to exec when any single env string exceeds MAX_ARG_STRLEN (128 KiB), and
/// every hook shares the budget, so stay well below it.
const SELECTION_ENV_MAX: usize = 64 * 1024;

/// Largest total size of action paths passed on the command line. argv and
/// the environment together are capped by ARG_MAX (2 MiB by default); the
/// inherited environment and `RAVEN_SELECTION` need room too.
const ARGV_PATHS_MAX: usize = 512 * 1024;

/// The newline-joined `RAVEN_SELECTION` value, or `None` when it cannot be
/// passed faithfully: too large for one env string, or a path contains a
/// newline and would split. Plugins then read `RAVEN_SELECTION_FILE`.
fn selection_env_value(selection: &[String]) -> Option<String> {
    if selection.iter().any(|p| p.contains('\n')) {
        return None;
    }
    let joined = selection.join("\n");
    (joined.len() <= SELECTION_ENV_MAX).then_some(joined)
}

/// Whether `paths` fit on the command line (each argv entry costs its bytes
/// plus a NUL and a pointer).
fn paths_fit_in_argv(paths: &[String]) -> bool {
    let total: usize = paths
        .iter()
        .map(|p| p.len() + 1 + std::mem::size_of::<usize>())
        .sum();
    total <= ARGV_PATHS_MAX
}

/// Encode `paths` as NUL-terminated entries, the only unambiguous separator
/// for Unix paths.
fn nul_list(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::with_capacity(paths.iter().map(|p| p.len() + 1).sum());
    for p in paths {
        out.extend_from_slice(p.as_bytes());
        out.push(0);
    }
    out
}

/// A private temp file holding a path list, removed once the hook returns.
struct ListFile(PathBuf);

impl ListFile {
    fn create(kind: &str, paths: &[String]) -> Result<Self, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "raven-plugin-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            kind
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("Failed to create {}: {}", path.display(), e))?;
        // Own the path before writing so a failed write still cleans up.
        let list = Self(path);
        file.write_all(&nul_list(paths))
            .map_err(|e| format!("Failed to write {}: {}", list.0.display(), e))?;
        Ok(list)
    }
}

impl Drop for ListFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

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

    /// Deliver the chosen action `action` on `paths` via [`ACTION_HOOK`].
    ///
    /// Hosts that can carry arbitrarily many paths some other way override
    /// this; the default passes them as ordinary hook arguments.
    fn call_action(&self, action: &str, paths: &[String]) -> Result<(), String> {
        let mut args: Vec<&str> = Vec::with_capacity(paths.len() + 1);
        args.push(action);
        args.extend(paths.iter().map(String::as_str));
        self.call_hook(ACTION_HOOK, &args)
    }

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

        for action in &self.manifest.actions {
            api.register_action(&action.name, &action.label)
                .map_err(|e| format!("Plugin '{}' action '{}': {}", self.manifest.id, action.name, e))?;
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
        self.run(event, args, None)
    }

    fn call_action(&self, action: &str, paths: &[String]) -> Result<(), String> {
        // Past the argv budget exec would fail with E2BIG, so the paths go
        // only in RAVEN_ACTION_PATHS_FILE and argv carries just the name.
        let mut args: Vec<&str> = vec![action];
        if paths_fit_in_argv(paths) {
            args.extend(paths.iter().map(String::as_str));
        }
        self.run(ACTION_HOOK, &args, Some(paths))
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

impl CommandPluginHost {
    /// Run the entry point for `event`. `action_paths` is the full path list
    /// of a chosen action, written to `RAVEN_ACTION_PATHS_FILE`.
    fn run(&self, event: &str, args: &[&str], action_paths: Option<&[String]>) -> Result<(), String> {
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
        // A script cannot call back into get_selection, so it gets the answer
        // up front. RAVEN_SELECTION_FILE always holds the exact list
        // (NUL-terminated entries); RAVEN_SELECTION is a convenience copy,
        // one path per line, left unset when it would be too large for the
        // environment or a path contains a newline.
        let mut _lists = Vec::new();
        if let Some(selection) = self.api.as_ref().and_then(|api| api.get_selection().ok()) {
            let file = ListFile::create("selection", &selection)?;
            cmd.env("RAVEN_SELECTION_FILE", &file.0);
            _lists.push(file);
            if let Some(value) = selection_env_value(&selection) {
                cmd.env("RAVEN_SELECTION", value);
            }
        }
        if let Some(paths) = action_paths {
            let file = ListFile::create("action", paths)?;
            cmd.env("RAVEN_ACTION_PATHS_FILE", &file.0);
            _lists.push(file);
        }

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
            actions: Vec::new(),
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
    fn selection_env_is_skipped_when_unsafe() {
        let small = vec!["/a".to_string(), "/b c".to_string()];
        assert_eq!(selection_env_value(&small).as_deref(), Some("/a\n/b c"));
        assert_eq!(selection_env_value(&[]).as_deref(), Some(""));
        // A newline in a name would split one path into two lines.
        assert!(selection_env_value(&["/a\nb.txt".to_string()]).is_none());
        // ~2,000 60-byte paths: over MAX_ARG_STRLEN, exec would fail.
        let big: Vec<String> = (0..2000).map(|i| format!("/{:059}", i)).collect();
        assert!(selection_env_value(&big).is_none());
    }

    #[test]
    fn argv_budget_and_nul_list() {
        assert!(paths_fit_in_argv(&["/a".to_string()]));
        let huge: Vec<String> = (0..20_000).map(|i| format!("/{:059}", i)).collect();
        assert!(!paths_fit_in_argv(&huge));
        assert_eq!(nul_list(&["/a\nb".to_string(), "/c".to_string()]), b"/a\nb\0/c\0");
    }

    #[test]
    fn oversized_action_runs_and_passes_paths_by_file() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let out = dir.path().join("out");
        let script_path = dir.path().join("run.sh");
        // Record argc, and copy the paths file, which is removed afterwards.
        std::fs::write(
            &script_path,
            format!(
                "#!/bin/sh\necho $# > '{0}.argc'\ncp \"$RAVEN_ACTION_PATHS_FILE\" '{0}.paths'\ntest -f \"$RAVEN_SELECTION_FILE\"\n",
                out.display()
            ),
        )
        .expect("Failed to write script");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("Failed to set permissions");
        }
        let mut host = CommandPluginHost::new(sample_manifest(), dir.path().to_path_buf());
        host.load(Arc::new(MockPluginApi::new())).expect("load");

        // Far past ARG_MAX if it all went on the command line.
        let paths: Vec<String> = (0..40_000).map(|i| format!("/tmp/{:0100}", i)).collect();
        host.call_action("zip", &paths).expect("oversized action must still run");

        let argc = std::fs::read_to_string(dir.path().join("out.argc")).unwrap();
        assert_eq!(argc.trim(), "2", "only the hook and action name on argv");
        let listed = std::fs::read(dir.path().join("out.paths")).unwrap();
        assert_eq!(listed, nul_list(&paths));

        // A small action still gets its paths as arguments.
        host.call_action("zip", &["/x".to_string()]).expect("small action");
        let argc = std::fs::read_to_string(dir.path().join("out.argc")).unwrap();
        assert_eq!(argc.trim(), "3");
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
