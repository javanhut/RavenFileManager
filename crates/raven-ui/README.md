# raven-ui

GTK4 + Libadwaita user interface for RavenFileManager.

## Components

- **RavenApplication** (`app.rs`) - `adw::Application` subclass, manages window lifecycle and event bridge
- **RavenWindow** (`window.rs`) - Main window assembling all widgets, handling events, registering actions
- **AppState** (`state.rs`) - `Rc<RefCell<AppStateInner>>` holding tabs, panes, config, clipboard

## Widgets

| Widget | File | Description |
|--------|------|-------------|
| FileListView | `file_list.rs` | Column view with name, size, modified, type columns |
| Sidebar | `sidebar.rs` | Bookmarks, mounted volumes, and tag counts |
| TabBar | `tab_bar.rs` | Tab strip with add/close buttons |
| PathBar | `path_bar.rs` | Breadcrumb navigation + editable path entry |
| SearchBar | `search_bar.rs` | Search with Filter/Filename/Content/Smart modes |
| PreviewPanel | `preview_panel.rs` | File preview (text, image, directory) |
| OperationPanel | `operation_panel.rs` | Progress display for file operations |
| ContainerBanner | `container_banner.rs` | Dismissable banner for Flatpak/Docker/Podman |
| FileContextMenu | `context_menu.rs` | Right-click menu with open, edit, tag, AI, delete actions |
| PropertiesDialog | `properties_dialog.rs` | File properties with package, lock, disk usage, systemd info |
| SettingsDialog | `settings_dialog.rs` | Settings with General, Appearance, Keybindings, File Associations, Tags pages |
| DuplicateDialog | `duplicate_dialog.rs` | Duplicate scan progress + grouped results with trash action |
| OrganizeDialog | `organize_dialog.rs` | Organization suggestions with expandable file lists |

## Dependencies

Depends on `raven-core` for types and `raven-ai` for smart search parsing. Does not depend on any other backend crate at runtime (all communication via AppCommand/AppEvent channels).

## Notes

- GTK widgets require `gtk::prelude::*` and often `libadwaita::prelude::*`
- Closures for `connect_*` signals are `Fn`, not `FnOnce` -- wrap consumed values in `RefCell`
- No automated tests (requires GTK runtime) -- tested manually
