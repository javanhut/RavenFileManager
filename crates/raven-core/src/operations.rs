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

impl std::str::FromStr for ConflictStrategy {
    type Err = String;

    /// Parse the `default_conflict_strategy` config value. Accepts the enum
    /// names in snake_case or kebab-case, case-insensitively.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key = s.trim().to_ascii_lowercase().replace('-', "_");
        Ok(match key.as_str() {
            "ask" => Self::Ask,
            "skip" => Self::Skip,
            "overwrite" | "replace" => Self::Overwrite,
            "overwrite_older" | "replace_older" => Self::OverwriteOlder,
            "rename" => Self::Rename,
            "rename_all" => Self::RenameAll,
            "skip_all" => Self::SkipAll,
            "overwrite_all" | "replace_all" => Self::OverwriteAll,
            other => return Err(format!("unknown conflict strategy '{}'", other)),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub operation_id: OperationId,
    pub source: RavenPath,
    pub destination: RavenPath,
    pub source_size: u64,
    pub dest_size: u64,
    /// The item being copied or moved is a folder.
    #[serde(default)]
    pub source_is_dir: bool,
    /// The existing item is a folder. Choosing to overwrite a folder merges
    /// into it rather than replacing it.
    #[serde(default)]
    pub dest_is_dir: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_conflict_strategy_names() {
        assert_eq!("ask".parse::<ConflictStrategy>(), Ok(ConflictStrategy::Ask));
        assert_eq!(
            " Overwrite-Older ".parse::<ConflictStrategy>(),
            Ok(ConflictStrategy::OverwriteOlder)
        );
        assert_eq!("rename".parse::<ConflictStrategy>(), Ok(ConflictStrategy::Rename));
        assert!("merge".parse::<ConflictStrategy>().is_err());
    }
}
