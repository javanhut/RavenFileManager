# raven-core

Core types and traits shared across all RavenFileManager crates.

This crate is intentionally lightweight with minimal dependencies (serde, chrono, thiserror, tracing, async-trait) to keep compile times fast for all downstream crates.

## Key Types

- **`AppCommand`** / **`AppEvent`** - The message protocol between UI and backend threads
- **`FileEntry`** / **`EntryMetadata`** / **`EntryKind`** - File entry representation
- **`RavenPath`** - Unified enum for local, SFTP, and SMB paths
- **`AppConfig`** - Application configuration with serde serialization
- **`FilterSpec`** / **`SortSpec`** - Filtering and sorting specifications
- **`VirtualFileSystem`** - Async trait for filesystem backends
- **`ai_types`** - Shared AI feature types (DuplicateGroup, OrganizeSuggestion, etc.)
- **`system_types`** - OS integration types (PackageInfo, ProcessLock, ContainerInfo, etc.)
- **`automation_types`** - Automation rule types (AutomationRule, SshAuth, etc.)

## Modules

| Module | Purpose |
|--------|---------|
| `ai_types` | AI feature shared types |
| `automation_types` | Automation rule definitions |
| `commands` | `AppCommand` enum (UI -> Backend) |
| `config` | `AppConfig` with toml serialization |
| `entry` | `FileEntry`, `EntryMetadata`, `EntryKind` |
| `error` | `RavenError` type |
| `events` | `AppEvent` enum (Backend -> UI) |
| `filter` | `FilterSpec` and `FileTypeFilter` |
| `operations` | Operation types (OperationId, OperationKind, etc.) |
| `path` | `RavenPath` enum |
| `sort` | `SortSpec`, `SortColumn`, `SortDirection` |
| `system_types` | System integration types |
| `vfs` | `VirtualFileSystem` trait |
