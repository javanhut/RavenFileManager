use serde::{Deserialize, Serialize};

use crate::path::RavenPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationKind {
    Copy,
    Move,
    Delete,
    Trash,
    Rename,
    CreateDirectory,
    CreateFile,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationPriority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationProgress {
    pub id: OperationId,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub current_file: Option<String>,
}

impl OperationProgress {
    pub fn fraction(&self) -> f64 {
        if self.bytes_total == 0 {
            0.0
        } else {
            self.bytes_done as f64 / self.bytes_total as f64
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictStrategy {
    Ask,
    Skip,
    Overwrite,
    OverwriteOlder,
    Rename,
    RenameAll,
    SkipAll,
    OverwriteAll,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub operation_id: OperationId,
    pub source: RavenPath,
    pub destination: RavenPath,
    pub source_size: u64,
    pub dest_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: OperationId,
    pub kind: OperationKind,
    pub sources: Vec<RavenPath>,
    pub destination: Option<RavenPath>,
    pub status: OperationStatus,
    pub priority: OperationPriority,
}

impl Operation {
    pub fn new(
        id: OperationId,
        kind: OperationKind,
        sources: Vec<RavenPath>,
        destination: Option<RavenPath>,
    ) -> Self {
        Self {
            id,
            kind,
            sources,
            destination,
            status: OperationStatus::Pending,
            priority: OperationPriority::Normal,
        }
    }
}
