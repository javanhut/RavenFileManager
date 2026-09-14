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
    /// Select these paths in `pane_id` once its listing contains them.
    ///
    /// Sent ahead of the directory load that will bring the entries in, so the
    /// UI holds it as a pending selection rather than racing the load. Applied
    /// immediately when the pane already shows the directory.
    SelectItems {
        pane_id: u32,
        paths: Vec<RavenPath>,
        show_properties: bool,
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
    /// The full set of context-menu actions loaded plugins have registered,
    /// sent whenever a plugin load or unload changes it.
    PluginActionsChanged {
        actions: Vec<PluginActionInfo>,
    },

    // Network connections
    RemoteConnected {
        id: String,
        protocol: String,
        host: String,
        /// Where to start browsing the connection.
        initial_path: RavenPath,
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
    /// Entries in the pane's directory carrying the active tag. `tag: None` means the
    /// filter was cleared and `paths` is empty. Tag matching needs the backend's
    /// TagEngine, so the resolved paths are sent rather than a spec the UI re-evaluates.
    TagFilterApplied {
        pane_id: u32,
        /// Directory the paths were resolved against. The UI drops the event if the
        /// pane has since navigated elsewhere.
        path: RavenPath,
        tag: Option<String>,
        paths: Vec<RavenPath>,
    },
    OrganizationAnalysisComplete {
        path: RavenPath,
        suggestions: Vec<OrganizeSuggestion>,
    },

    // Filter
    FilterApplied {
        filter: crate::filter::FilterSpec,
        pane_id: u32,
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
    /// A still frame plus what ffprobe could tell. Everything but `path` is
    /// best-effort: without ffmpeg installed the UI still gets a video entry.
    Video {
        path: std::path::PathBuf,
        thumbnail: Option<std::path::PathBuf>,
        width: u32,
        height: u32,
        duration_secs: Option<f64>,
    },
    /// A paged document (PDF). The first page rendered to `thumbnail` when a
    /// renderer could manage it; page count and title when the file parses.
    Document {
        path: std::path::PathBuf,
        thumbnail: Option<std::path::PathBuf>,
        page_count: Option<u32>,
        title: Option<String>,
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

/// A context-menu action a plugin registered, as the window sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginActionInfo {
    /// The plugin to run when the action is chosen.
    pub plugin_id: String,
    /// The name handed back to that plugin.
    pub name: String,
    /// The menu label.
    pub label: String,
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
