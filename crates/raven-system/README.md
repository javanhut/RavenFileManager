# raven-system

Linux system integration for RavenFileManager.

Provides deep OS-level introspection: package ownership, process file locks, disk usage analysis, container detection, and systemd unit inspection.

## Components

| Module | Description |
|--------|-------------|
| `PackageLookup` | Queries pacman/dpkg/rpm for which package owns a file |
| `ProcessLockDetector` | Checks `/proc/*/fd` for processes holding a file open |
| `DiskUsageCalculator` | Streaming recursive disk usage with cancellation support |
| `ContainerInspector` | Detects Flatpak, Docker, and Podman container environments |
| `SystemdInspector` | Parses `.service`/`.timer` unit files, queries active state via `systemctl` |

## Tests

46 tests covering all system integration modules.
