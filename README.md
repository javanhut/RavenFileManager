# Raven File Manager

A modern, feature-rich file manager for Linux built with GTK4 and Libadwaita, written in Rust.

## Features

- **Tabbed browsing** with drag-and-drop support
- **Dual-pane navigation** with breadcrumb path bar
- **Smart search** with natural language queries (e.g. "large images from last week")
- **File preview** panel for text, images, and directories
- **AI-powered tools**:
  - Duplicate file detection using blake3 hashing
  - Auto-tagging with 9 built-in smart tags (Images, Code, Documents, etc.)
  - Smart organization suggestions by file type and date
- **Automation engine** with configurable rules for file actions
- **Git integration** showing repository status
- **SFTP support** for remote file browsing
- **System integration**: package lookup, process locks, disk usage, systemd unit inspection, container detection
- **Plugin system** with Lua and WASM host support (planned)
- **DBus interface** for external scripting
- **Configurable keybindings** and file associations
- **Theming** via Libadwaita with dark/light mode support

## Building

### Dependencies

- Rust 1.70+ (2021 edition)
- GTK4 development libraries
- Libadwaita development libraries

On Arch Linux:

```sh
sudo pacman -S gtk4 libadwaita
```

On Ubuntu/Debian:

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev
```

On Fedora:

```sh
sudo dnf install gtk4-devel libadwaita-devel
```

### Compile and Run

```sh
cargo build --release
./target/release/raven
```

### Run Tests

```sh
cargo test --workspace
```

## Configuration

Raven stores its configuration at `~/.config/raven/config.toml`. A default configuration is created on first launch. See [`config/default.toml`](config/default.toml) for all available options.

### Key Paths

| Path | Purpose |
|------|---------|
| `~/.config/raven/config.toml` | Main configuration |
| `~/.config/raven/tags.toml` | Tag rules and manual tags |
| `~/.config/raven/rules/` | Automation rule definitions |
| `~/.config/raven/plugins/` | User plugins |
| `~/.local/share/raven/plugins/` | System-wide plugins |

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+F` | Toggle search bar |
| `Ctrl+L` | Edit path bar |
| `Ctrl+H` | Toggle hidden files |
| `Ctrl+T` | New tab |
| `Ctrl+W` | Close tab |
| `Ctrl+R` | Refresh |
| `Ctrl+Z` | Undo |
| `Ctrl+I` | Properties |
| `Ctrl+,` | Settings |
| `F2` | Rename |
| `Delete` | Move to trash |
| `Space` | Toggle preview panel |
| `Alt+Left` | Navigate back |
| `Alt+Right` | Navigate forward |
| `Alt+Up` | Navigate to parent |

All shortcuts are customizable via Settings > Keybindings.

## Smart Search

Select "Smart" mode in the search dropdown and type natural language queries:

- `images` - find all image files
- `large files` - files over 100MB
- `python files` - files with `.py` extension
- `recent documents` - documents modified in the last 7 days
- `bigger than 5mb` - files exceeding 5MB
- `old archives` - archives older than 1 year

## Project Structure

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed architecture documentation.

```
RavenFileManager/
  src/main.rs              # Application entry point + backend command loop
  crates/
    raven-core/            # Shared types, config, commands, events
    raven-vfs/             # Virtual filesystem (local, SFTP, SMB)
    raven-ops/             # File operations engine (copy, move, delete, undo)
    raven-search/          # Search (typeahead, recursive, content)
    raven-preview/         # File preview (text, image, directory)
    raven-automation/      # Rules-based automation engine
    raven-git/             # Git status integration
    raven-system/          # OS integration (packages, processes, systemd)
    raven-plugin/          # Plugin loading and management
    raven-dbus/            # DBus service interface
    raven-ai/              # AI features (duplicates, tags, search, organize)
    raven-ui/              # GTK4 + Libadwaita UI
  config/                  # Default configuration files
  data/                    # Desktop entry, icons, AppStream metadata
```

## License

GPL-3.0-or-later
