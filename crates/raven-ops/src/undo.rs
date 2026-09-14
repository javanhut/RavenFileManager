use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

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
    /// The exact `(original, created)` pairs the operation produced, when
    /// known. Undo touches only these, so an item that was skipped, renamed
    /// to keep both, or written over existing data is handled correctly.
    /// `None` means each source's name inside `destination`.
    pub items: Option<Vec<(RavenPath, RavenPath)>>,
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
        self.push_transfer(OperationKind::Copy, operation_id, sources, destination, None)
            .await;
    }

    /// Record a completed copy with the exact items it created.
    pub async fn record_copy_items(
        &self,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
        destination: RavenPath,
        items: Vec<(RavenPath, RavenPath)>,
    ) {
        self.push_transfer(OperationKind::Copy, operation_id, sources, destination, Some(items))
            .await;
    }

    /// Record a completed move operation.
    pub async fn record_move(
        &self,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
        destination: RavenPath,
    ) {
        self.push_transfer(OperationKind::Move, operation_id, sources, destination, None)
            .await;
    }

    /// Record a completed move with the exact `(original, moved)` pairs.
    pub async fn record_move_items(
        &self,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
        destination: RavenPath,
        items: Vec<(RavenPath, RavenPath)>,
    ) {
        self.push_transfer(OperationKind::Move, operation_id, sources, destination, Some(items))
            .await;
    }

    async fn push_transfer(
        &self,
        kind: OperationKind,
        operation_id: OperationId,
        sources: Vec<RavenPath>,
        destination: RavenPath,
        items: Option<Vec<(RavenPath, RavenPath)>>,
    ) {
        // Nothing reversible happened (everything was skipped, replaced or
        // merged): a record would only use up an Undo doing nothing.
        if items.as_ref().is_some_and(|items| items.is_empty()) {
            debug!(id = ?operation_id, "nothing to record for undo");
            return;
        }
        self.push(UndoRecord {
            operation_id,
            kind,
            sources,
            destination: Some(destination),
            trash_entries: Vec::new(),
            items,
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
            items: None,
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
            items: None,
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

    /// Stop offering undo for every earlier copy or move whose result is
    /// `path`, lies inside it, or contains it.
    ///
    /// Called before something is written over or merged into `path`: from
    /// then on those results hold data that operation did not create, so
    /// undoing it (deleting a copy, moving an item back) would take that
    /// data along.
    pub async fn invalidate_touching(&self, path: &RavenPath) {
        let mut records = self.records.lock().await;
        for record in records.iter_mut().filter(|r| r.undoable) {
            if !matches!(record.kind, OperationKind::Copy | OperationKind::Move) {
                continue;
            }
            let Ok(pairs) = Self::transfer_pairs(record) else {
                continue;
            };
            let touched = pairs.iter().any(|(_, result)| {
                result == path || result.is_inside(path) || path.is_inside(result)
            });
            if touched {
                info!(id = ?record.operation_id, path = %path, "undo no longer offered: its result is being overwritten");
                record.undoable = false;
            }
        }
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

    /// The `(original, result)` pairs a copy or move record stands for.
    fn transfer_pairs(record: &UndoRecord) -> RavenResult<Vec<(RavenPath, RavenPath)>> {
        if let Some(items) = &record.items {
            return Ok(items.clone());
        }
        let destination = record.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: format!("{:?} undo record missing destination", record.kind),
        })?;
        record
            .sources
            .iter()
            .map(|source| {
                let file_name = source.file_name().ok_or_else(|| RavenError::Other {
                    message: format!("unable to determine file name for {}", source),
                })?;
                Ok((source.clone(), destination.join(file_name)))
            })
            .collect()
    }

    /// Undo a copy by deleting the copied files at the destination.
    async fn undo_copy(
        &self,
        vfs: &dyn VirtualFileSystem,
        record: &UndoRecord,
    ) -> RavenResult<()> {
        for (_, copied_path) in Self::transfer_pairs(record)? {
            if vfs.exists(&copied_path).await? {
                vfs.delete(&copied_path).await?;
                debug!(path = %copied_path, "deleted copied file for undo");
            }
        }

        Ok(())
    }

    /// Undo a move by moving files back from destination to their original locations.
    ///
    /// Something that has since appeared at an original location is never
    /// written over; those items stay where they are and an error says so.
    async fn undo_move(
        &self,
        vfs: &dyn VirtualFileSystem,
        record: &UndoRecord,
    ) -> RavenResult<()> {
        let mut blocked = None;

        for (source, moved_path) in Self::transfer_pairs(record)? {
            if !vfs.exists(&moved_path).await? {
                continue;
            }
            // A case-only rename on a case-insensitive share sees its own
            // item at the original name; that is not something in the way.
            if vfs.exists(&source).await?
                && !crate::conflict::same_object(vfs, &source, &moved_path).await?
            {
                warn!(from = %moved_path, to = %source, "not moving back over an existing item");
                blocked.get_or_insert(source);
                continue;
            }

            // Ensure the original parent directory exists
            if let Some(parent) = source.parent() {
                if !vfs.exists(&parent).await? {
                    vfs.create_dir(&parent).await?;
                }
            }

            vfs.rename(&moved_path, &source).await?;
            debug!(
                from = %moved_path,
                to = %source,
                "moved file back for undo"
            );
        }

        match blocked {
            None => Ok(()),
            Some(path) => Err(RavenError::AlreadyExists {
                path: path
                    .as_local_path()
                    .cloned()
                    .unwrap_or_else(|| PathBuf::from(path.to_string())),
            }),
        }
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
    use crate::test_support::{remote, MemFs};
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

    fn trash() -> TrashFs {
        let dir = std::env::temp_dir().join(format!("raven_undo_trash_{}", std::process::id()));
        TrashFs::with_trash_dir(dir).unwrap()
    }

    #[tokio::test]
    async fn test_undo_copy_items_deletes_only_what_was_created() {
        let fs = MemFs::new();
        let src = fs.file("/src/f.txt", b"new");
        let original = fs.file("/dst/f.txt", b"old");
        let kept_both = fs.file("/dst/f (1).txt", b"new");

        let stack = UndoStack::new();
        stack
            .record_copy_items(
                OperationId(1),
                vec![src.clone()],
                remote("/dst"),
                vec![(src.clone(), kept_both.clone())],
            )
            .await;
        stack.undo(&fs, &trash()).await.unwrap();

        assert_eq!(fs.contents(&original).as_deref(), Some(&b"old"[..]));
        assert!(fs.node(&kept_both).is_none());
        assert!(fs.node(&src).is_some());
    }

    #[tokio::test]
    async fn test_empty_transfer_records_are_not_kept() {
        let stack = UndoStack::new();
        stack
            .record_copy_items(OperationId(1), vec![remote("/a")], remote("/dst"), Vec::new())
            .await;
        stack
            .record_move_items(OperationId(2), vec![remote("/b")], remote("/dst"), Vec::new())
            .await;
        assert!(stack.is_empty().await);
    }

    #[tokio::test]
    async fn test_invalidate_touching_disables_overlapping_records_only() {
        let stack = UndoStack::new();
        let pair = |s: &str, d: &str| vec![(remote(s), remote(d))];
        stack.record_copy_items(OperationId(1), vec![], remote("/dst"), pair("/x/d", "/dst/d")).await;
        stack.record_copy_items(OperationId(2), vec![], remote("/dst"), pair("/x/e", "/dst/e")).await;
        stack.record_move_items(OperationId(3), vec![], remote("/dst"), pair("/y/f.txt", "/dst/d/f.txt")).await;

        stack.invalidate_touching(&remote("/dst/d")).await;

        let undoable: Vec<_> = stack
            .list()
            .await
            .into_iter()
            .filter(|r| r.undoable)
            .map(|r| r.operation_id)
            .collect();
        assert_eq!(undoable, vec![OperationId(2)]);
    }

    #[tokio::test]
    async fn test_undo_move_never_overwrites_original_location() {
        let fs = MemFs::new();
        let moved = fs.file("/dst/f.txt", b"moved");
        let original = fs.file("/src/f.txt", b"someone else's");

        let stack = UndoStack::new();
        stack
            .record_move_items(
                OperationId(1),
                vec![original.clone()],
                remote("/dst"),
                vec![(original.clone(), moved.clone())],
            )
            .await;
        let result = stack.undo(&fs, &trash()).await;

        assert!(matches!(result, Err(RavenError::AlreadyExists { .. })));
        assert_eq!(fs.contents(&original).as_deref(), Some(&b"someone else's"[..]));
        assert_eq!(fs.contents(&moved).as_deref(), Some(&b"moved"[..]));
    }
}
