use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info};

use raven_core::error::{RavenError, RavenResult};
use raven_core::operations::{OperationId, OperationKind};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

use crate::trash::{TrashEntry, TrashFs};

/// Maximum number of undo records to keep.
const DEFAULT_MAX_UNDO: usize = 100;

/// A record of a completed operation that can potentially be undone.
#[derive(Debug, Clone)]
pub struct UndoRecord {
    /// The operation ID that produced this record.
    pub operation_id: OperationId,
    /// What kind of operation was performed.
    pub kind: OperationKind,
    /// Source paths involved in the operation.
    pub sources: Vec<RavenPath>,
    /// Destination path (for copy/move operations).
    pub destination: Option<RavenPath>,
    /// For trash operations, the trash entries that were created.
    pub trash_entries: Vec<TrashEntryRecord>,
    /// Whether this operation can be undone.
    pub undoable: bool,
}

/// Serializable record of a trash entry for undo purposes.
#[derive(Debug, Clone)]
pub struct TrashEntryRecord {
    pub trash_name: String,
    pub original_path: PathBuf,
    pub trash_file_path: PathBuf,
    pub trashinfo_path: PathBuf,
    pub deletion_date: String,
}

impl From<&TrashEntry> for TrashEntryRecord {
    fn from(entry: &TrashEntry) -> Self {
        Self {
            trash_name: entry.trash_name.clone(),
            original_path: entry.original_path.clone(),
            trash_file_path: entry.trash_file_path.clone(),
            trashinfo_path: entry.trashinfo_path.clone(),
            deletion_date: entry.deletion_date.clone(),
        }
    }
}

impl TrashEntryRecord {
    /// Convert back to a TrashEntry.
    pub fn to_trash_entry(&self) -> TrashEntry {
        TrashEntry {
            trash_name: self.trash_name.clone(),
            original_path: self.original_path.clone(),
            trash_file_path: self.trash_file_path.clone(),
            trashinfo_path: self.trashinfo_path.clone(),
            deletion_date: self.deletion_date.clone(),
        }
    }
}

/// An undo stack that records completed operations and supports reversing them.
pub struct UndoStack {
    records: Arc<Mutex<Vec<UndoRecord>>>,
    max_records: usize,
}

impl UndoStack {
    /// Create a new UndoStack with the default maximum size.
    pub fn new() -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
            max_records: DEFAULT_MAX_UNDO,
        }
    }

    /// Create a new UndoStack with a custom maximum size.
    pub fn with_max_records(max_records: usize) -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
            max_records,
        }
    }

    /// Record a completed copy operation.
    pub async fn record_copy(
        &self,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
        destination: RavenPath,
    ) {
        self.push(UndoRecord {
            operation_id,
            kind: OperationKind::Copy,
            sources,
            destination: Some(destination),
            trash_entries: Vec::new(),
            undoable: true,
        })
        .await;
    }

    /// Record a completed move operation.
    pub async fn record_move(
        &self,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
        destination: RavenPath,
    ) {
        self.push(UndoRecord {
            operation_id,
            kind: OperationKind::Move,
            sources,
            destination: Some(destination),
            trash_entries: Vec::new(),
            undoable: true,
        })
        .await;
    }

    /// Record a completed delete operation (cannot be undone).
    pub async fn record_delete(
        &self,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
    ) {
        self.push(UndoRecord {
            operation_id,
            kind: OperationKind::Delete,
            sources,
            destination: None,
            trash_entries: Vec::new(),
            undoable: false,
        })
        .await;
    }

    /// Record a completed trash operation.
    pub async fn record_trash(
        &self,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
        trash_entries: Vec<TrashEntryRecord>,
    ) {
        self.push(UndoRecord {
            operation_id,
            kind: OperationKind::Trash,
            sources,
            destination: None,
            trash_entries,
            undoable: true,
        })
        .await;
    }

    /// Push a record onto the stack.
    async fn push(&self, record: UndoRecord) {
        let mut records = self.records.lock().await;
        records.push(record);

        // Trim old records if over limit
        while records.len() > self.max_records {
            records.remove(0);
        }

        debug!(
            count = records.len(),
            "undo record pushed"
        );
    }

    /// Pop the most recent undoable record from the stack.
    pub async fn pop(&self) -> Option<UndoRecord> {
        let mut records = self.records.lock().await;
        // Find the last undoable record
        if let Some(pos) = records.iter().rposition(|r| r.undoable) {
            Some(records.remove(pos))
        } else {
            None
        }
    }

    /// Peek at the most recent undoable record without removing it.
    pub async fn peek(&self) -> Option<UndoRecord> {
        let records = self.records.lock().await;
        records.iter().rev().find(|r| r.undoable).cloned()
    }

    /// Execute an undo for the most recent undoable operation.
    ///
    /// - Copy: delete the copied files at the destination.
    /// - Move: move the files back from destination to source.
    /// - Delete: cannot undo (returns error).
    /// - Trash: restore files from trash to original locations.
    pub async fn undo(
        &self,
        vfs: &dyn VirtualFileSystem,
        trash_fs: &TrashFs,
    ) -> RavenResult<UndoRecord> {
        let record = self.pop().await.ok_or_else(|| RavenError::Other {
            message: "nothing to undo".to_string(),
        })?;

        match record.kind {
            OperationKind::Copy => {
                self.undo_copy(vfs, &record).await?;
            }
            OperationKind::Move => {
                self.undo_move(vfs, &record).await?;
            }
            OperationKind::Delete => {
                return Err(RavenError::Other {
                    message: "delete operations cannot be undone".to_string(),
                });
            }
            OperationKind::Trash => {
                self.undo_trash(trash_fs, &record).await?;
            }
            _ => {
                return Err(RavenError::Other {
                    message: format!("undo not supported for {:?} operations", record.kind),
                });
            }
        }

        info!(
            id = ?record.operation_id,
            kind = ?record.kind,
            "operation undone"
        );

        Ok(record)
    }

    /// Undo a copy by deleting the copied files at the destination.
    async fn undo_copy(
        &self,
        vfs: &dyn VirtualFileSystem,
        record: &UndoRecord,
    ) -> RavenResult<()> {
        let destination = record.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: "copy undo record missing destination".to_string(),
        })?;

        for source in &record.sources {
            let file_name = source.file_name().ok_or_else(|| RavenError::Other {
                message: format!("unable to determine file name for {}", source),
            })?;
            let copied_path = destination.join(file_name);
            if vfs.exists(&copied_path).await? {
                vfs.delete(&copied_path).await?;
                debug!(path = %copied_path, "deleted copied file for undo");
            }
        }

        Ok(())
    }

    /// Undo a move by moving files back from destination to their original locations.
    async fn undo_move(
        &self,
        vfs: &dyn VirtualFileSystem,
        record: &UndoRecord,
    ) -> RavenResult<()> {
        let destination = record.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: "move undo record missing destination".to_string(),
        })?;

        for source in &record.sources {
            let file_name = source.file_name().ok_or_else(|| RavenError::Other {
                message: format!("unable to determine file name for {}", source),
            })?;
            let moved_path = destination.join(file_name);

            // Ensure the original parent directory exists
            if let Some(parent) = source.parent() {
                if !vfs.exists(&parent).await? {
                    vfs.create_dir(&parent).await?;
                }
            }

            if vfs.exists(&moved_path).await? {
                vfs.rename(&moved_path, source).await?;
                debug!(
                    from = %moved_path,
                    to = %source,
                    "moved file back for undo"
                );
            }
        }

        Ok(())
    }

    /// Undo a trash operation by restoring files from the trash.
    async fn undo_trash(
        &self,
        trash_fs: &TrashFs,
        record: &UndoRecord,
    ) -> RavenResult<()> {
        for trash_record in &record.trash_entries {
            let entry = trash_record.to_trash_entry();
            trash_fs.restore(&entry).await?;
            debug!(
                name = %trash_record.trash_name,
                path = %trash_record.original_path.display(),
                "restored from trash for undo"
            );
        }
        Ok(())
    }

    /// Get the number of records in the stack.
    pub async fn len(&self) -> usize {
        self.records.lock().await.len()
    }

    /// Check if the stack is empty.
    pub async fn is_empty(&self) -> bool {
        self.records.lock().await.is_empty()
    }

    /// Clear all records.
    pub async fn clear(&self) {
        self.records.lock().await.clear();
    }

    /// List all records (most recent last).
    pub async fn list(&self) -> Vec<UndoRecord> {
        self.records.lock().await.clone()
    }
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raven_core::operations::OperationId;
    use raven_core::path::RavenPath;

    #[tokio::test]
    async fn test_record_and_pop() {
        let stack = UndoStack::new();

        stack
            .record_copy(
                OperationId(1),
                vec![RavenPath::local("/tmp/a.txt")],
                RavenPath::local("/tmp/dest"),
            )
            .await;

        stack
            .record_move(
                OperationId(2),
                vec![RavenPath::local("/tmp/b.txt")],
                RavenPath::local("/tmp/dest2"),
            )
            .await;

        assert_eq!(stack.len().await, 2);

        let record = stack.pop().await.unwrap();
        assert_eq!(record.operation_id, OperationId(2));
        assert_eq!(record.kind, OperationKind::Move);

        let record = stack.pop().await.unwrap();
        assert_eq!(record.operation_id, OperationId(1));
        assert_eq!(record.kind, OperationKind::Copy);

        assert!(stack.pop().await.is_none());
    }

    #[tokio::test]
    async fn test_delete_not_undoable() {
        let stack = UndoStack::new();

        stack
            .record_delete(
                OperationId(1),
                vec![RavenPath::local("/tmp/deleted.txt")],
            )
            .await;

        // Delete is not undoable, so pop should skip it
        assert!(stack.pop().await.is_none());
        // But the record is still in the stack
        assert_eq!(stack.len().await, 1);
    }

    #[tokio::test]
    async fn test_max_records_trimming() {
        let stack = UndoStack::with_max_records(3);

        for i in 0..5 {
            stack
                .record_copy(
                    OperationId(i),
                    vec![RavenPath::local(format!("/tmp/file{}.txt", i))],
                    RavenPath::local("/tmp/dest"),
                )
                .await;
        }

        assert_eq!(stack.len().await, 3);
        // Oldest records (0, 1) should have been trimmed
        let records = stack.list().await;
        assert_eq!(records[0].operation_id, OperationId(2));
        assert_eq!(records[2].operation_id, OperationId(4));
    }

    #[tokio::test]
    async fn test_peek_does_not_remove() {
        let stack = UndoStack::new();
        stack
            .record_copy(
                OperationId(1),
                vec![RavenPath::local("/tmp/a.txt")],
                RavenPath::local("/tmp/dest"),
            )
            .await;

        let peeked = stack.peek().await.unwrap();
        assert_eq!(peeked.operation_id, OperationId(1));
        assert_eq!(stack.len().await, 1);
    }

    #[tokio::test]
    async fn test_clear() {
        let stack = UndoStack::new();
        stack
            .record_copy(
                OperationId(1),
                vec![RavenPath::local("/tmp/a.txt")],
                RavenPath::local("/tmp/dest"),
            )
            .await;
        stack.clear().await;
        assert!(stack.is_empty().await);
    }
}
