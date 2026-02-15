# raven-search

File search functionality for RavenFileManager.

Provides three search modes: fast typeahead filtering, recursive filename search, and file content search.

## Search Modes

| Mode | Type | Description |
|------|------|-------------|
| `TypeaheadFilter` | In-memory | Fast substring filtering of current directory entries |
| `RecursiveSearcher` | Async | Walks directory trees matching filenames (uses `ignore` crate) |
| `ContentSearcher` | Async | Searches file contents using `grep-searcher` + `grep-regex` |

Both `RecursiveSearcher` and `ContentSearcher` return results via a tokio mpsc channel for streaming to the UI.

## Tests

17 tests covering all search modes.
