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

## Tests

40 tests covering manifest parsing, plugin discovery, loading, and lifecycle.
