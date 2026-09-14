# raven-plugin

Plugin system for RavenFileManager.

Handles plugin discovery, loading, lifecycle management, and the API surface exposed to plugins.

## Components

- **PluginManager** - Discovers plugins in directories, loads/unloads them
- **PluginManifest** - TOML manifest format defining plugin metadata (id, name, version, entry point)
- **PluginHost** - Trait for plugin execution backends (Lua via mlua, WASM via wasmtime - planned)
- **PluginApi** - Trait defining the file manager API exposed to plugins

## Plugin Directories

Plugins are discovered from:
- `~/.config/raven/plugins/`
- `~/.local/share/raven/plugins/`
- Additional directories configured in `config.toml`

## Manifest Format

```toml
id = "my-plugin"
name = "My Plugin"
version = "1.0.0"
description = "A sample plugin"
entry = "init.lua"
```

## Context-menu actions

Command plugins declare actions in `plugin.toml`; they appear under
**Plugins** in the file context menu while the plugin is loaded:

```toml
[[actions]]
name = "compress"
label = "Compress to .zip"
```

Choosing one runs the entry point as `run.sh on_action <name> <path>...`
with the selected local paths. `RAVEN_ACTION_PATHS_FILE` names a temporary
file holding the same paths as NUL-terminated entries; when the selection is
too large for the command line (over ~512 KiB) the paths are left off argv
and only that file carries them.

Every hook also receives the active pane's selection in
`RAVEN_SELECTION_FILE` (NUL-terminated entries, always exact). The
convenience copy `RAVEN_SELECTION` (one path per line) is unset when the
list exceeds 64 KiB or a path contains a newline. Both files are deleted
once the hook exits, e.g. `xargs -0 ... < "$RAVEN_ACTION_PATHS_FILE"`.

## Tests

40 tests covering manifest parsing, plugin discovery, loading, and lifecycle.
