# raven-vfs

Virtual filesystem abstraction layer for RavenFileManager.

Provides a unified `VirtualFileSystem` trait with backends for local, SFTP, and SMB filesystems. The `VfsRouter` dispatches operations based on `RavenPath` variants.

## Backends

| Backend | Status | Description |
|---------|--------|-------------|
| `LocalFs` | Complete | Standard filesystem via `tokio::fs` |
| `SftpFs` | Complete | Remote SFTP via `russh` + `russh-sftp` |
| `SmbFs` | Stub | Returns errors, placeholder for future implementation |

## VfsRouter

The `VfsRouter` inspects the `RavenPath` variant and dispatches to the appropriate backend:

- `RavenPath::Local(path)` -> `LocalFs`
- `RavenPath::Sftp { .. }` -> `SftpFs` (after `connect_sftp()`)
- `RavenPath::Smb { .. }` -> `SmbFs`

## Tests

46 tests (2 SFTP integration tests ignored by default - require a live SSH server).
