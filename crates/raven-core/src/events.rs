use std::path::PathBuf;

use crate::ai_types::{DuplicateGroup, DuplicateScanProgress, OrganizeSuggestion};
use crate::entry::FileEntry;
use crate::error::RavenError;
use crate::operations::{ConflictInfo, OperationId, OperationProgress};
use crate::path::RavenPath;
use crate::system_types::{ContainerInfo, PackageInfo, ProcessLock, SystemdUnit};

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

    // Automation
    AutomationRuleTriggered {
        rule_id: String,
        rule_name: String,
        matched_files: Vec<String>,
    },
    AutomationActionCompleted {
        rule_id: String,
        action: String,
    },
    AutomationError {
        rule_id: String,
        error: String,
    },

    // Plugins
    PluginLoaded {
        plugin_id: String,
        name: String,
    },
    PluginUnloaded {
        plugin_id: String,
    },
    PluginError {
        plugin_id: String,
        error: String,
    },

    // Network connections
    RemoteConnected {
        id: String,
        protocol: String,
        host: String,
    },
    RemoteDisconnected {
        id: String,
    },
    RemoteError {
        id: String,
        error: String,
    },

    // System integration
    PackageOwnerResult {
        path: PathBuf,
        package: Option<PackageInfo>,
    },
    ProcessLocksResult {
        path: PathBuf,
        locks: Vec<ProcessLock>,
    },
    DiskUsageProgress {
        path: RavenPath,
        entry: crate::system_types::DiskUsageEntry,
    },
    DiskUsageCompleted {
        path: RavenPath,
        total_size: u64,
        total_items: u64,
    },
    DiskUsageError {
        path: RavenPath,
        error: String,
    },
    ContainerInfoResult {
        info: ContainerInfo,
    },
    SystemdUnitResult {
        path: PathBuf,
        unit: SystemdUnit,
    },
    SystemError {
        error: String,
    },

    // AI features
    DuplicateScanProgress {
        progress: DuplicateScanProgress,
    },
    DuplicateScanCompleted {
        groups: Vec<DuplicateGroup>,
    },
    DuplicateScanError {
        error: String,
    },
    TagCountsUpdated {
        pane_id: u32,
        counts: Vec<(String, usize)>,
    },
    OrganizationAnalysisComplete {
        path: RavenPath,
        suggestions: Vec<OrganizeSuggestion>,
    },

    // Directory size (async calculation for listing)
    DirSizeCalculated {
        pane_id: u32,
        path: RavenPath,
        size: u64,
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
