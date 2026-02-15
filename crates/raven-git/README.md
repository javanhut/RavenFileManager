# raven-git

Git repository integration for RavenFileManager using the `gix` crate.

## Components

- **GitDetector** - Detects if a path is inside a git repository
- **GitStatusProvider** - Gets per-file status (modified, added, deleted, untracked, etc.)
- **GitWatcher** - Monitors repository for changes

## Tests

2 tests covering detection and status retrieval.
