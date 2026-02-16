```
                                ___
                            .-'`   `'-.
                           /           \
                          ;    RAVEN    ;
                          |   ___   ___|
                          |  /   \ /   |
                           \  \__/ \__/
                            '.  ____  .'
                         ___/ /`    `\ \___
                    _--~`   _/  FILE  \_   `~--_
                  /`  _--~``  MANAGER   ``~--_  `\
                 / _~`       ___________       `~_ \
                |/`       _-~`    |    `~-_       `\|
                         /  ___   |   ___  \
                        / -`   `--|--`   `- \
                       /___       |       ___\
                            `~-___|___-~`
```

# Raven File Manager

A modern, feature-rich file manager for Linux built with GTK4 and Libadwaita, written in Rust.

## Features

- **Multiple view modes** - list, icon grid, and preview thumbnails
- **Tabbed browsing** with drag-and-drop support
- **Dual-pane navigation** with breadcrumb path bar
- **Drag-and-drop bookmarks** - pin folders to the sidebar by dragging
- **Disk management** - mount/unmount volumes, live disk space indicators
- **Smart search** with natural language queries (e.g. "large images from last week")
- **File preview** panel for text, images, and directories
- **AI-powered tools**:
  - Duplicate file detection using blake3 hashing
  - Auto-tagging with 9 built-in smart tags (Images, Code, Documents, etc.)
  - Smart organization suggestions by file type and date
- **Automation engine** with configurable rules for file actions
- **Git integration** showing repository status
- **SFTP support** for remote file browsing
- **System integration** - package lookup, process locks, disk usage, systemd unit inspection, container detection
- **DBus interface** for external scripting
- **Configurable keybindings** and file associations
- **Theming** - Catppuccin Mocha/Latte, Nord, Dracula, Frost, Rose Pine, Adwaita Dark/Light

## Installation

### Dependencies

- Rust 1.70+ (2021 edition)
- GTK4 development libraries
- Libadwaita development libraries

**Arch Linux:**

```sh
sudo pacman -S gtk4 libadwaita base-devel
```

**Ubuntu / Debian:**

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev build-essential
```

**Fedora:**

```sh
sudo dnf install gtk4-devel libadwaita-devel gcc
```

### Build and Install

Clone the repository and install using `make`:

```sh
git clone https://github.com/ravenfilemanager/raven.git
cd RavenFileManager
make
sudo make install
```

This builds a release binary and installs:

| File | Destination |
|------|-------------|
| `ravenfilemanager` | `/usr/local/bin/ravenfilemanager` |
| Desktop entry | `/usr/local/share/applications/com.ravenfilemanager.Raven.desktop` |
| AppStream metadata | `/usr/local/share/metainfo/com.ravenfilemanager.Raven.metainfo.xml` |
| Default configs | `/usr/local/share/ravenfilemanager/config/` |
| Resources | `/usr/local/share/ravenfilemanager/resources/` |

To install to a different prefix:

```sh
sudo make install PREFIX=/usr
```

For package building with a staging root:

```sh
make install DESTDIR=/tmp/pkg PREFIX=/usr
```

### Uninstall

```sh
sudo make uninstall
```

### Build Only (without installing)

```sh
make                    # release build
make PROFILE=debug      # debug build
cargo build --release   # direct cargo
```

The binary will be at `target/release/ravenfilemanager`.

### Run Without Installing

```sh
make run
# or
cargo run --release
```

### Run Tests

```sh
make test
# or
cargo test --workspace
```

### Clean

```sh
make clean
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
| `Ctrl+C` | Copy |
| `Ctrl+X` | Cut |
| `Ctrl+V` | Paste |
| `Ctrl+A` | Select all |
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

```
RavenFileManager/
  src/main.rs              # Entry point and backend command loop
  crates/
    raven-core/            # Shared types, config, commands, events
    raven-vfs/             # Virtual filesystem (local, SFTP, SMB)
    raven-ops/             # File operations (copy, move, delete, undo)
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
  data/                    # Desktop entry, resources, AppStream metadata
```

## License

GPL-3.0-or-later
