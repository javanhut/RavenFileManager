use serde::{Deserialize, Serialize};

/// Plugin manifest loaded from plugin.toml in each plugin directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub runtime: PluginRuntime,
    pub entry_point: String,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginRuntime {
    Lua,
    Wasm,
    /// Simple command-based plugins (shell scripts).
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    ReadFiles,
    WriteFiles,
    Network,
    Shell,
    Notifications,
}

impl PluginManifest {
    /// Load a manifest from a plugin.toml file.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read manifest at {}: {}", path.display(), e))?;
        let manifest: Self = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse manifest at {}: {}", path.display(), e))?;
        Ok(manifest)
    }

    /// Check if the plugin has a specific permission.
    pub fn has_permission(&self, perm: PluginPermission) -> bool {
        self.permissions.contains(&perm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_manifest_toml() -> String {
        r#"
id = "example-plugin"
name = "Example Plugin"
version = "0.1.0"
author = "Test Author"
description = "A test plugin"
runtime = "command"
entry_point = "run.sh"
permissions = ["read_files", "notifications"]
"#
        .to_string()
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let manifest = PluginManifest {
            id: "roundtrip-test".to_string(),
            name: "Roundtrip Test".to_string(),
            version: "1.0.0".to_string(),
            author: "Author".to_string(),
            description: "Roundtrip test plugin".to_string(),
            runtime: PluginRuntime::Command,
            entry_point: "main.sh".to_string(),
            permissions: vec![PluginPermission::ReadFiles, PluginPermission::Shell],
        };

        let toml_string = toml::to_string(&manifest).expect("Failed to serialize manifest");
        let deserialized: PluginManifest =
            toml::from_str(&toml_string).expect("Failed to deserialize manifest");

        assert_eq!(deserialized.id, manifest.id);
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.version, manifest.version);
        assert_eq!(deserialized.author, manifest.author);
        assert_eq!(deserialized.description, manifest.description);
        assert_eq!(deserialized.runtime, manifest.runtime);
        assert_eq!(deserialized.entry_point, manifest.entry_point);
        assert_eq!(deserialized.permissions, manifest.permissions);
    }

    #[test]
    fn test_load_manifest_from_file() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest_path = dir.path().join("plugin.toml");

        let mut file =
            std::fs::File::create(&manifest_path).expect("Failed to create manifest file");
        file.write_all(sample_manifest_toml().as_bytes())
            .expect("Failed to write manifest");

        let manifest = PluginManifest::load(&manifest_path).expect("Failed to load manifest");
        assert_eq!(manifest.id, "example-plugin");
        assert_eq!(manifest.name, "Example Plugin");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.author, "Test Author");
        assert_eq!(manifest.description, "A test plugin");
        assert_eq!(manifest.runtime, PluginRuntime::Command);
        assert_eq!(manifest.entry_point, "run.sh");
        assert_eq!(manifest.permissions.len(), 2);
        assert!(manifest.permissions.contains(&PluginPermission::ReadFiles));
        assert!(manifest
            .permissions
            .contains(&PluginPermission::Notifications));
    }

    #[test]
    fn test_load_manifest_missing_file() {
        let result = PluginManifest::load(std::path::Path::new("/nonexistent/plugin.toml"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to read manifest"));
    }

    #[test]
    fn test_load_manifest_invalid_toml() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let manifest_path = dir.path().join("plugin.toml");

        std::fs::write(&manifest_path, "this is not valid toml {{{{")
            .expect("Failed to write file");

        let result = PluginManifest::load(&manifest_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to parse manifest"));
    }

    #[test]
    fn test_permission_checking() {
        let manifest = PluginManifest {
            id: "perm-test".to_string(),
            name: "Permission Test".to_string(),
            version: "0.1.0".to_string(),
            author: "Author".to_string(),
            description: "Test".to_string(),
            runtime: PluginRuntime::Command,
            entry_point: "run.sh".to_string(),
            permissions: vec![PluginPermission::ReadFiles, PluginPermission::Notifications],
        };

        assert!(manifest.has_permission(PluginPermission::ReadFiles));
        assert!(manifest.has_permission(PluginPermission::Notifications));
        assert!(!manifest.has_permission(PluginPermission::WriteFiles));
        assert!(!manifest.has_permission(PluginPermission::Network));
        assert!(!manifest.has_permission(PluginPermission::Shell));
    }

    #[test]
    fn test_empty_permissions_default() {
        let toml_str = r#"
id = "no-perms"
name = "No Permissions"
version = "0.1.0"
author = "Author"
description = "A plugin with no permissions"
runtime = "lua"
entry_point = "init.lua"
"#;
        let manifest: PluginManifest =
            toml::from_str(toml_str).expect("Failed to parse manifest");
        assert!(manifest.permissions.is_empty());
        assert!(!manifest.has_permission(PluginPermission::ReadFiles));
    }

    #[test]
    fn test_all_runtimes() {
        for (runtime_str, expected) in [
            ("lua", PluginRuntime::Lua),
            ("wasm", PluginRuntime::Wasm),
            ("command", PluginRuntime::Command),
        ] {
            let toml_str = format!(
                r#"
id = "rt-test"
name = "Runtime Test"
version = "0.1.0"
author = "Author"
description = "Test"
runtime = "{}"
entry_point = "entry"
"#,
                runtime_str
            );
            let manifest: PluginManifest =
                toml::from_str(&toml_str).expect("Failed to parse manifest");
            assert_eq!(manifest.runtime, expected);
        }
    }
}
