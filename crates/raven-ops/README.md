# raven-ops

File operations engine for RavenFileManager.

Handles copy, move, delete, rename, trash, and create operations with progress reporting, conflict resolution, operation queuing, and undo support.

## Components

- **OperationExecutor** - Executes file operations against a `VirtualFileSystem`
- **OperationQueue** - Manages concurrent operations with pause/resume/cancel
- **UndoStack** - Records completed operations for undo
- **CopyEngine** - Buffered file copying with progress callbacks
- **TrashFs** - Freedesktop trash specification implementation
- **Conflict** - Conflict detection and resolution strategies (skip, overwrite, rename, ask)

## Tests

25 tests covering operation execution, queuing, and conflict handling.
