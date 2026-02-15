use std::path::PathBuf;

use crate::ai_types::OrganizeSuggestion;
use crate::automation_types::{AutomationRule, SshAuth};
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

    // Automation
    StartAutomation,
    StopAutomation,
    AddAutomationRule {
        rule: AutomationRule,
    },
    RemoveAutomationRule {
        rule_id: String,
    },
    EnableAutomationRule {
        rule_id: String,
    },
    DisableAutomationRule {
        rule_id: String,
    },
    TriggerAutomationRule {
        rule_id: String,
    },

    // Plugins
    LoadPlugin {
        path: PathBuf,
    },
    UnloadPlugin {
        plugin_id: String,
    },

    // Network connections
    ConnectSftp {
        host: String,
        port: u16,
        user: String,
        auth: SshAuth,
    },
    ConnectSmb {
        host: String,
        share: String,
        user: Option<String>,
        password: Option<String>,
    },
    DisconnectRemote {
        id: String,
    },

    // System integration
    GetPackageOwner {
        path: PathBuf,
    },
    GetProcessLocks {
        path: PathBuf,
    },
    CalculateDiskUsage {
        path: RavenPath,
    },
    CancelDiskUsage,
    GetContainerInfo,
    InspectSystemdUnit {
        path: PathBuf,
    },

    // AI features
    ScanDuplicates {
        path: RavenPath,
        recursive: bool,
        min_size: u64,
    },
    CancelDuplicateScan,
    RefreshTagCounts {
        pane_id: u32,
    },
    AddManualTag {
        path: PathBuf,
        tag: String,
    },
    RemoveManualTag {
        path: PathBuf,
        tag: String,
    },
    FilterByTag {
        tag: String,
        pane_id: u32,
    },
    AnalyzeOrganization {
        path: RavenPath,
    },
    ApplyOrganization {
        suggestions: Vec<OrganizeSuggestion>,
    },

    // App lifecycle
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Filename,
    Content,
    Regex,
    NaturalLanguage,
}
