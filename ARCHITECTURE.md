# Architecture

Raven File Manager is a 12-crate Rust workspace with a GTK4 + Libadwaita frontend and an async Tokio backend. This document describes the overall architecture, inter-crate relationships, and key design decisions.

## High-Level Overview

```
 ┌─────────────────────────────────────┐
 │           GTK4 UI Thread            │
 │  ┌─────────┐  ┌──────────────────┐  │
 │  │ raven-ui │  │ Widgets/Dialogs  │  │
 │  └────┬─────┘  └────────┬────────┘  │
 │       │    AppCommand    │           │
 │       ▼ (tokio mpsc tx)  │           │
 ├───────┼──────────────────┼──────────┤
 │       │                  ▲           │
 │       │    AppEvent      │           │
 │       │ (tokio mpsc rx + │           │
 │       │  glib::spawn)    │           │
 │       ▼                  │           │
 │  ┌────────────────────────────────┐  │
 │  │      Tokio Runtime Thread      │  │
 │  │  ┌──────────┐ ┌────────────┐   │  │
 │  │  │ raven-vfs│ │ raven-ops  │   │  │
 │  │  │ raven-ai │ │raven-search│   │  │
 │  │  │  ...etc  │ │  ...etc    │   │  │
 │  │  └──────────┘ └────────────┘   │  │
 │  └────────────────────────────────┘  │
 └─────────────────────────────────────┘
```

## Two-Channel Event Bridge

The UI and backend communicate through two unbounded tokio mpsc channels:

1. **AppCommand** (UI -> Backend): The UI sends commands like `Navigate`, `CopyFiles`, `Search`, `ScanDuplicates`. Defined in `raven-core/src/commands.rs`.

2. **AppEvent** (Backend -> UI): The backend sends events like `DirectoryLoaded`, `SearchResult`, `DuplicateScanCompleted`. Defined in `raven-core/src/events.rs`. Events are received in the UI thread via `glib::spawn_future_local` polling the mpsc receiver.

This design avoids any shared mutable state between threads. The GTK thread owns all UI state (`Rc<RefCell<AppStateInner>>`), and the Tokio thread owns all async I/O.

## Crate Dependency Graph

```
raven-core (shared types, zero heavy deps)
  ├── raven-vfs       (VirtualFileSystem trait + backends)
  ├── raven-ops       (depends on raven-core, raven-vfs)
  ├── raven-search    (depends on raven-core)
  ├── raven-preview   (depends on raven-core)
  ├── raven-automation(depends on raven-core)
  ├── raven-git       (depends on raven-core)
  ├── raven-system    (depends on raven-core)
  ├── raven-plugin    (depends on raven-core)
  ├── raven-dbus      (depends on raven-core)
  ├── raven-ai        (depends on raven-core)
  └── raven-ui        (depends on raven-core, raven-ai, raven-vfs, raven-ops, ...)

raven (binary) depends on all crates
```

All crates depend on `raven-core` for shared types. No crate depends on `raven-ui` (the UI is a leaf dependency). This ensures backend logic can be tested without GTK.

## Crate Descriptions

### raven-core
Foundation types shared across all crates. Contains no business logic.

- `AppCommand` / `AppEvent` - The command/event protocol between UI and backend
- `FileEntry`, `EntryMetadata`, `EntryKind` - File entry representation
- `RavenPath` - Enum for local, SFTP, and SMB paths with a unified API
- `AppConfig` - Configuration with serde serialization
- `FilterSpec`, `SortSpec` - Filtering and sorting specifications
- `ai_types` - Shared AI feature types (DuplicateGroup, OrganizeSuggestion, etc.)

### raven-vfs
Virtual filesystem abstraction. The `VirtualFileSystem` trait defines async operations (list_dir, stat, read, write, copy, move, delete, create_dir, exists). Backends:

- **LocalFs** - Standard filesystem via `tokio::fs`
- **SftpFs** - Remote SFTP via `russh` + `russh-sftp`
- **SmbFs** - SMB stub (returns errors, placeholder for future implementation)
- **VfsRouter** - Dispatches `RavenPath` variants to the correct backend

### raven-ops
File operation execution engine.

- **OperationExecutor** - Executes copy/move/delete/rename/create operations
- **OperationQueue** - Manages concurrent operations with pause/resume/cancel
- **UndoStack** - Records operations for undo support
- **CopyEngine** - Buffered file copying with progress reporting
- **TrashFs** - Freedesktop trash spec implementation

### raven-search
Three search modes:

- **TypeaheadFilter** - Fast in-memory substring filtering for current directory
- **RecursiveSearcher** - Walks directory trees matching filenames (uses `ignore` crate)
- **ContentSearcher** - Searches file contents using `grep-searcher` + `grep-regex`

### raven-preview
Generates file previews dispatched by MIME type:

- **TextPreviewProvider** - Syntax-highlighted text via `syntect`
- **ImagePreviewProvider** - Image metadata and dimensions via `image`
- **DirectoryPreviewProvider** - Item count and total size
- **PreviewRouter** - Selects provider based on file type

### raven-automation
Rules-based automation engine for automatic file actions.

- **AutomationEngine** - Manages rules, watches directories, triggers actions
- **Conditions** - Extension match, name pattern, size range, age, etc.
- **Actions** - Move, copy, rename, tag, notify
- **Scheduler** - Periodic rule evaluation
- Rules are defined as TOML files in `~/.config/raven/rules/`

### raven-git
Git repository integration using the `gix` crate.

- **GitDetector** - Detects if a path is inside a git repository
- **GitStatusProvider** - Gets file-level status (modified, added, untracked, etc.)
- **GitWatcher** - Monitors repository for changes

### raven-system
Deep Linux system integration.

- **PackageLookup** - Queries pacman/dpkg/rpm for package ownership of files
- **ProcessLockDetector** - Checks `/proc/*/fd` for open file handles
- **DiskUsageCalculator** - Streaming recursive disk usage calculation
- **ContainerInspector** - Detects Flatpak, Docker, Podman environments
- **SystemdInspector** - Parses systemd unit files and queries active state

### raven-plugin
Plugin loading and lifecycle management.

- **PluginManager** - Discovers, loads, and unloads plugins from directories
- **PluginManifest** - TOML manifest format for plugin metadata
- **PluginHost** - Trait for Lua/WASM plugin execution (stubs for now)
- **PluginApi** - Trait defining the API surface exposed to plugins

### raven-dbus
DBus service for external integration.

- **DbusService** - Registers on the session bus, handles method calls
- **DbusInterface** - Exposes navigation, file operations, and status queries

### raven-ai
Local-only AI-powered features (no LLM, no network).

- **nl_search** - Natural language query parser (keyword tables for types, sizes, dates, languages)
- **TagEngine** - Rule-based auto-tagging with 9 default smart tags + manual tags
- **DuplicateScanner** - Two-phase duplicate detection (size grouping + blake3 hashing)
- **OrganizationAnalyzer** - Suggests folder organization by file type and date

### raven-ui
GTK4 + Libadwaita user interface.

- **RavenApplication** - `adw::Application` subclass, manages window lifecycle
- **RavenWindow** - Main window with sidebar, file list, path bar, search, preview
- **AppState** - `Rc<RefCell<AppStateInner>>` holding tabs, panes, config, clipboard
- **Widgets**: file_list, sidebar, tab_bar, path_bar, search_bar, preview_panel, operation_panel, context_menu, container_banner, properties_dialog, settings_dialog, duplicate_dialog, organize_dialog

## UI State Model

The UI uses a single-threaded GTK model with `Rc<RefCell<AppStateInner>>`:

```
AppStateInner
  ├── tabs: Vec<Tab>
  │     └── Tab
  │           ├── title: String
  │           └── panes: Vec<Pane>
  │                 └── Pane
  │                       ├── id: u32
  │                       ├── current_path: RavenPath
  │                       ├── entries: Vec<FileEntry>
  │                       ├── history: Vec<RavenPath>
  │                       └── history_pos: usize
  ├── active_tab: usize
  ├── config: AppConfig
  ├── clipboard: Option<ClipboardOp>
  └── show_hidden: bool
```

## Backend Command Loop

The `src/main.rs` entry point:

1. Loads configuration
2. Creates the command/event channels
3. Spawns a Tokio runtime on a separate thread
4. Initializes all backend services (VFS, operations, automation, plugins, DBus, system, AI)
5. Enters a `while let Some(command) = command_rx.recv().await` loop dispatching commands
6. Creates the GTK application on the main thread with the channel endpoints

## Key Design Decisions

- **No shared mutable state** between UI and backend threads. All communication is via message passing.
- **raven-core has no heavy dependencies** - it only depends on serde, chrono, thiserror, tracing, async-trait. This keeps compile times low for all downstream crates.
- **RavenPath enum** unifies local, SFTP, and SMB paths with a single type that flows through the entire system.
- **AI features are local-only** - no network calls, no LLM. NL search is a keyword parser, duplicate detection uses blake3, tags use rule matching.
- **VFS trait abstraction** means file operations work identically on local and remote filesystems.
- **The UI never blocks** - all I/O is async on the Tokio thread. The UI only renders state and sends commands.

## Testing Strategy

- Backend crates have extensive unit tests (365 total, 2 ignored SFTP integration tests)
- Tests use `tempfile` for filesystem tests and mock implementations where needed
- UI crate has no automated tests (requires GTK runtime) - tested manually
- Each crate is independently testable via `cargo test -p <crate>`
