use crate::filter::FilterSpec;
use crate::operations::{ConflictStrategy, OperationId};
use crate::path::RavenPath;
use crate::sort::SortSpec;

/// Commands sent from the UI thread to the backend Tokio runtime.
#[derive(Debug, Clone)]
pub enum AppCommand {
    // Navigation
    Navigate {
        path: RavenPath,
        pane_id: u32,
    },
    NavigateBack {
        pane_id: u32,
    },
    NavigateForward {
        pane_id: u32,
    },
    NavigateUp {
        pane_id: u32,
    },
    Refresh {
        pane_id: u32,
    },

    // File operations
    CopyFiles {
        sources: Vec<RavenPath>,
        destination: RavenPath,
    },
    MoveFiles {
        sources: Vec<RavenPath>,
        destination: RavenPath,
    },
    DeleteFiles {
        paths: Vec<RavenPath>,
    },
    TrashFiles {
        paths: Vec<RavenPath>,
    },
    RenameFile {
        path: RavenPath,
        new_name: String,
    },
    CreateDirectory {
        parent: RavenPath,
        name: String,
    },
    CreateFile {
        parent: RavenPath,
        name: String,
    },

    // Operation control
    PauseOperation {
        id: OperationId,
    },
    ResumeOperation {
        id: OperationId,
    },
    CancelOperation {
        id: OperationId,
    },
    ResolveConflict {
        id: OperationId,
        strategy: ConflictStrategy,
    },
    Undo,

    // Search
    Search {
        query: String,
        path: RavenPath,
        search_mode: SearchMode,
    },
    CancelSearch,

    // Filter
    SetFilter {
        filter: FilterSpec,
        pane_id: u32,
    },

    // Sort
    SetSort {
        sort: SortSpec,
        pane_id: u32,
    },

    // Preview
    GeneratePreview {
        path: RavenPath,
    },
    CancelPreview,

    // Git
    RefreshGitStatus {
        path: RavenPath,
    },

    // App lifecycle
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Filename,
    Content,
    Regex,
}
