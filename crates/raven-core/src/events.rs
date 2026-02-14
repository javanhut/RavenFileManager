use crate::entry::FileEntry;
use crate::error::RavenError;
use crate::operations::{ConflictInfo, OperationId, OperationProgress};
use crate::path::RavenPath;

/// Events sent from the backend Tokio runtime to the GTK UI thread.
#[derive(Debug, Clone)]
pub enum AppEvent {
    // Navigation
    DirectoryLoaded {
        pane_id: u32,
        path: RavenPath,
        entries: Vec<FileEntry>,
    },
    DirectoryError {
        pane_id: u32,
        path: RavenPath,
        error: String,
    },

    // Operations
    OperationStarted {
        id: OperationId,
        description: String,
    },
    OperationProgress {
        progress: OperationProgress,
    },
    OperationCompleted {
        id: OperationId,
    },
    OperationFailed {
        id: OperationId,
        error: String,
    },
    OperationConflict {
        conflict: ConflictInfo,
    },

    // Search
    SearchResult {
        path: RavenPath,
        entry: FileEntry,
    },
    SearchCompleted {
        total_matches: u64,
    },
    SearchError {
        error: String,
    },

    // Preview
    PreviewReady {
        path: RavenPath,
        preview: PreviewData,
    },
    PreviewError {
        path: RavenPath,
        error: String,
    },

    // Git
    GitStatusUpdated {
        path: RavenPath,
        statuses: Vec<GitFileStatusEntry>,
    },

    // Notifications
    Notification {
        title: String,
        message: String,
        level: NotificationLevel,
    },
}

#[derive(Debug, Clone)]
pub enum PreviewData {
    Text {
        content: String,
        language: Option<String>,
    },
    Image {
        path: std::path::PathBuf,
        width: u32,
        height: u32,
    },
    Directory {
        item_count: u64,
        total_size: u64,
    },
    Unsupported {
        mime_type: String,
    },
}

#[derive(Debug, Clone)]
pub struct GitFileStatusEntry {
    pub path: RavenPath,
    pub status: GitFileStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflict,
    Clean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

impl From<RavenError> for AppEvent {
    fn from(error: RavenError) -> Self {
        AppEvent::Notification {
            title: "Error".to_string(),
            message: error.to_string(),
            level: NotificationLevel::Error,
        }
    }
}
