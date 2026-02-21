use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{debug, error, info, instrument, warn};

use raven_core::error::{RavenError, RavenResult};
use raven_core::operations::{
    Operation, OperationId, OperationKind, OperationProgress,
};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

use crate::conflict::{ConflictResolution, ConflictResolver};
use crate::copy_engine::{CopyEngine, ProgressCallback};
use crate::queue::OperationQueue;
use crate::trash::TrashFs;
use crate::undo::{TrashEntryRecord, UndoStack};

/// Callback for reporting operation progress to the UI layer.
pub type ProgressReporter = Arc<dyn Fn(OperationProgress) + Send + Sync>;

/// Dispatches and executes file operations (copy, move, delete, trash).
pub struct OperationExecutor {
    copy_engine: CopyEngine,
    conflict_resolver: ConflictResolver,
    trash_fs: TrashFs,
    undo_stack: Arc<UndoStack>,
    queue: Arc<OperationQueue>,
    next_id: AtomicU64,
}

impl OperationExecutor {
    /// Create a new OperationExecutor.
    pub fn new(
        queue: Arc<OperationQueue>,
        undo_stack: Arc<UndoStack>,
    ) -> RavenResult<Self> {
        Ok(Self {
            copy_engine: CopyEngine::new(),
            conflict_resolver: ConflictResolver::default(),
            trash_fs: TrashFs::new()?,
            undo_stack,
            queue,
            next_id: AtomicU64::new(1),
        })
    }

    /// Create a new OperationExecutor with custom components.
    pub fn with_components(
        copy_engine: CopyEngine,
        conflict_resolver: ConflictResolver,
        trash_fs: TrashFs,
        undo_stack: Arc<UndoStack>,
        queue: Arc<OperationQueue>,
    ) -> Self {
        Self {
            copy_engine,
            conflict_resolver,
            trash_fs,
            undo_stack,
            queue,
            next_id: AtomicU64::new(1),
        }
    }

    /// Generate a new unique operation ID.
    pub fn next_id(&self) -> OperationId {
        OperationId(self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Execute an operation using the VFS layer.
    #[instrument(skip(self, vfs, progress_reporter), fields(id = ?operation.id, kind = ?operation.kind))]
    pub async fn execute(
        &self,
        operation: &Operation,
        vfs: &dyn VirtualFileSystem,
        progress_reporter: Option<ProgressReporter>,
    ) -> RavenResult<()> {
        info!("executing operation");

        // Check for cancellation before starting
        if self.queue.is_cancelled(operation.id).await {
            self.queue.clear_cancelled(operation.id).await;
            return Err(RavenError::Cancelled);
        }

        let result = match operation.kind {
            OperationKind::Copy => {
                self.execute_copy(operation, vfs, progress_reporter.as_ref())
                    .await
            }
            OperationKind::Move => {
                self.execute_move(operation, vfs, progress_reporter.as_ref())
                    .await
            }
            OperationKind::Delete => {
                self.execute_delete(operation, vfs).await
            }
            OperationKind::Trash => {
                self.execute_trash(operation, progress_reporter.as_ref())
                    .await
            }
            OperationKind::Rename => {
                self.execute_rename(operation, vfs).await
            }
            OperationKind::CreateDirectory => {
                self.execute_create_dir(operation, vfs).await
            }
            OperationKind::CreateFile => {
                self.execute_create_file(operation, vfs).await
            }
            OperationKind::Restore => {
                self.execute_restore(operation).await
            }
        };

        match &result {
            Ok(()) => info!("operation completed successfully"),
            Err(e) => error!(error = %e, "operation failed"),
        }

        result
    }

    /// Execute a copy operation.
    async fn execute_copy(
        &self,
        operation: &Operation,
        vfs: &dyn VirtualFileSystem,
        progress_reporter: Option<&ProgressReporter>,
    ) -> RavenResult<()> {
        let destination = operation.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: "copy operation requires a destination".to_string(),
        })?;

        for (idx, source) in operation.sources.iter().enumerate() {
            // Check for cancellation between files
            if self.queue.is_cancelled(operation.id).await {
                self.queue.clear_cancelled(operation.id).await;
                return Err(RavenError::Cancelled);
            }

            // Wait while paused
            while self.queue.is_paused(operation.id).await {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }

            let file_name = source.file_name().ok_or_else(|| RavenError::Other {
                message: format!("unable to determine file name for {}", source),
            })?;
            let dest_path = destination.join(file_name);

            // When pasting into the same directory the file already lives in,
            // always auto-rename (e.g. "file.txt" -> "file (1).txt").
            // For cross-directory copies, use the normal conflict resolver.
            let dest_path = if source == &dest_path {
                self.conflict_resolver
                    .generate_unique_name(&dest_path)
                    .await?
            } else {
                match self.resolve_conflict(operation.id, source, &dest_path).await? {
                    Some(p) => p,
                    None => continue, // Skipped
                }
            };

            // Use the copy engine for local paths, VFS for others
            match (source.as_local_path(), dest_path.as_local_path()) {
                (Some(src), Some(dst)) => {
                    let op_id = operation.id;
                    let files_total = operation.sources.len() as u64;
                    let files_done = idx as u64;
                    let reporter = progress_reporter.cloned();

                    let cb: ProgressCallback = Arc::new(move |bytes_done, bytes_total| {
                        if let Some(ref reporter) = reporter {
                            reporter(OperationProgress {
                                id: op_id,
                                bytes_done,
                                bytes_total,
                                files_done,
                                files_total,
                                current_file: None,
                            });
                        }
                    });

                    self.copy_engine
                        .copy_recursive(src, dst, Some(&cb))
                        .await?;
                }
                _ => {
                    let reporter_clone = progress_reporter.cloned();
                    let op_id = operation.id;
                    let files_total = operation.sources.len() as u64;
                    let files_done = idx as u64;

                    let progress_cb: Option<Box<dyn Fn(u64, u64) + Send + Sync>> =
                        reporter_clone.map(|r| {
                            let cb: Box<dyn Fn(u64, u64) + Send + Sync> =
                                Box::new(move |bytes_done, bytes_total| {
                                    r(OperationProgress {
                                        id: op_id,
                                        bytes_done,
                                        bytes_total,
                                        files_done,
                                        files_total,
                                        current_file: None,
                                    });
                                });
                            cb
                        });

                    vfs.copy(source, &dest_path, progress_cb).await?;
                }
            }
        }

        // Record for undo
        self.undo_stack
            .record_copy(operation.id, operation.sources.clone(), destination.clone())
            .await;

        Ok(())
    }

    /// Execute a move operation.
    async fn execute_move(
        &self,
        operation: &Operation,
        vfs: &dyn VirtualFileSystem,
        progress_reporter: Option<&ProgressReporter>,
    ) -> RavenResult<()> {
        let destination = operation.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: "move operation requires a destination".to_string(),
        })?;

        for (idx, source) in operation.sources.iter().enumerate() {
            if self.queue.is_cancelled(operation.id).await {
                self.queue.clear_cancelled(operation.id).await;
                return Err(RavenError::Cancelled);
            }

            while self.queue.is_paused(operation.id).await {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }

            let file_name = source.file_name().ok_or_else(|| RavenError::Other {
                message: format!("unable to determine file name for {}", source),
            })?;
            let dest_path = destination.join(file_name);

            let dest_path = self
                .resolve_conflict(operation.id, source, &dest_path)
                .await?;
            let dest_path = match dest_path {
                Some(p) => p,
                None => continue,
            };

            // Try rename first (fast path for same filesystem)
            match vfs.rename(source, &dest_path).await {
                Ok(()) => {
                    debug!(src = %source, dst = %dest_path, "moved via rename");
                }
                Err(_) => {
                    // Fall back to copy + delete
                    let reporter_clone = progress_reporter.cloned();
                    let op_id = operation.id;
                    let files_total = operation.sources.len() as u64;
                    let files_done = idx as u64;

                    let progress_cb: Option<Box<dyn Fn(u64, u64) + Send + Sync>> =
                        reporter_clone.map(|r| {
                            let cb: Box<dyn Fn(u64, u64) + Send + Sync> =
                                Box::new(move |bytes_done, bytes_total| {
                                    r(OperationProgress {
                                        id: op_id,
                                        bytes_done,
                                        bytes_total,
                                        files_done,
                                        files_total,
                                        current_file: None,
                                    });
                                });
                            cb
                        });

                    vfs.copy(source, &dest_path, progress_cb).await?;
                    vfs.delete(source).await?;
                    debug!(src = %source, dst = %dest_path, "moved via copy+delete");
                }
            }
        }

        self.undo_stack
            .record_move(operation.id, operation.sources.clone(), destination.clone())
            .await;

        Ok(())
    }

    /// Execute a delete operation.
    async fn execute_delete(
        &self,
        operation: &Operation,
        vfs: &dyn VirtualFileSystem,
    ) -> RavenResult<()> {
        for source in &operation.sources {
            if self.queue.is_cancelled(operation.id).await {
                self.queue.clear_cancelled(operation.id).await;
                return Err(RavenError::Cancelled);
            }

            vfs.delete(source).await?;
            debug!(path = %source, "deleted");
        }

        self.undo_stack
            .record_delete(operation.id, operation.sources.clone())
            .await;

        Ok(())
    }

    /// Execute a trash operation.
    async fn execute_trash(
        &self,
        operation: &Operation,
        _progress_reporter: Option<&ProgressReporter>,
    ) -> RavenResult<()> {
        let mut trash_records = Vec::new();

        for source in &operation.sources {
            if self.queue.is_cancelled(operation.id).await {
                self.queue.clear_cancelled(operation.id).await;
                return Err(RavenError::Cancelled);
            }

            let entry = self.trash_fs.trash_raven_path(source).await?;
            trash_records.push(TrashEntryRecord::from(&entry));
            debug!(path = %source, trash_name = %entry.trash_name, "trashed");
        }

        self.undo_stack
            .record_trash(
                operation.id,
                operation.sources.clone(),
                trash_records,
            )
            .await;

        Ok(())
    }

    /// Execute a rename operation.
    async fn execute_rename(
        &self,
        operation: &Operation,
        vfs: &dyn VirtualFileSystem,
    ) -> RavenResult<()> {
        let destination = operation.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: "rename operation requires a destination".to_string(),
        })?;

        if operation.sources.len() != 1 {
            return Err(RavenError::Other {
                message: "rename operation requires exactly one source".to_string(),
            });
        }

        let source = &operation.sources[0];
        vfs.rename(source, destination).await?;

        // Record as a move for undo purposes
        self.undo_stack
            .record_move(operation.id, operation.sources.clone(), destination.clone())
            .await;

        Ok(())
    }

    /// Execute a create-directory operation.
    async fn execute_create_dir(
        &self,
        operation: &Operation,
        vfs: &dyn VirtualFileSystem,
    ) -> RavenResult<()> {
        let destination = operation.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: "create directory operation requires a destination".to_string(),
        })?;

        vfs.create_dir(destination).await?;
        Ok(())
    }

    /// Execute a create-file operation.
    async fn execute_create_file(
        &self,
        operation: &Operation,
        vfs: &dyn VirtualFileSystem,
    ) -> RavenResult<()> {
        let destination = operation.destination.as_ref().ok_or_else(|| RavenError::Other {
            message: "create file operation requires a destination".to_string(),
        })?;

        vfs.write(destination, &[]).await?;
        Ok(())
    }

    /// Execute a restore-from-trash operation.
    async fn execute_restore(&self, operation: &Operation) -> RavenResult<()> {
        for source in &operation.sources {
            match source {
                RavenPath::Trash {
                    original_path: _,
                    trash_id,
                } => {
                    self.trash_fs.restore_by_name(trash_id).await?;
                    debug!(trash_id = %trash_id, "restored from trash");
                }
                _ => {
                    return Err(RavenError::Other {
                        message: format!(
                            "restore operation requires Trash paths, got {}",
                            source
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Check for a conflict and resolve it using the default strategy.
    ///
    /// Returns `Some(destination)` if the operation should proceed, or `None`
    /// if it should be skipped.
    async fn resolve_conflict(
        &self,
        operation_id: OperationId,
        source: &RavenPath,
        destination: &RavenPath,
    ) -> RavenResult<Option<RavenPath>> {
        let conflict = self
            .conflict_resolver
            .check_conflict(operation_id, source, destination)
            .await?;

        match conflict {
            None => Ok(Some(destination.clone())),
            Some(info) => {
                let resolution = self.conflict_resolver.resolve_with_default(&info).await?;
                match resolution {
                    ConflictResolution::Proceed { destination } => Ok(Some(destination)),
                    ConflictResolution::Skip => {
                        debug!(dst = %destination, "skipped due to conflict");
                        Ok(None)
                    }
                    ConflictResolution::NeedsInput { conflict: _ } => {
                        // Default behavior when user input is needed but unavailable:
                        // skip the file.
                        warn!(
                            dst = %destination,
                            "conflict requires user input, skipping"
                        );
                        Ok(None)
                    }
                }
            }
        }
    }

    /// Get a reference to the undo stack.
    pub fn undo_stack(&self) -> &Arc<UndoStack> {
        &self.undo_stack
    }

    /// Get a reference to the operation queue.
    pub fn queue(&self) -> &Arc<OperationQueue> {
        &self.queue
    }

    /// Get a reference to the trash filesystem.
    pub fn trash_fs(&self) -> &TrashFs {
        &self.trash_fs
    }

    /// Get a reference to the conflict resolver.
    pub fn conflict_resolver(&self) -> &ConflictResolver {
        &self.conflict_resolver
    }

    /// Get a mutable reference to the conflict resolver.
    pub fn conflict_resolver_mut(&mut self) -> &mut ConflictResolver {
        &mut self.conflict_resolver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use raven_core::operations::{ConflictStrategy, OperationPriority, OperationStatus};
    use raven_vfs::local::LocalFs;

    use std::sync::atomic::AtomicU64;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    async fn test_dir() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "raven_executor_test_{}_{}",
            std::process::id(),
            id
        ));
        let _ = tokio::fs::remove_dir_all(&path).await;
        tokio::fs::create_dir_all(&path).await.unwrap();
        path
    }

    fn make_executor(trash_dir: PathBuf) -> OperationExecutor {
        let queue = Arc::new(OperationQueue::new());
        let undo_stack = Arc::new(UndoStack::new());
        let trash_fs = TrashFs::with_trash_dir(trash_dir).unwrap();
        OperationExecutor::with_components(
            CopyEngine::new(),
            ConflictResolver::new(ConflictStrategy::Overwrite),
            trash_fs,
            undo_stack,
            queue,
        )
    }

    #[tokio::test]
    async fn test_execute_copy() {
        let dir = test_dir().await;
        let src_dir = dir.join("src");
        let dst_dir = dir.join("dst");
        let trash_dir = dir.join("trash");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::create_dir_all(&dst_dir).await.unwrap();
        tokio::fs::write(src_dir.join("file.txt"), b"copy me")
            .await
            .unwrap();

        let executor = make_executor(trash_dir);
        let vfs = LocalFs::new();

        let op = Operation {
            id: executor.next_id(),
            kind: OperationKind::Copy,
            sources: vec![RavenPath::local(src_dir.join("file.txt"))],
            destination: Some(RavenPath::local(&dst_dir)),
            status: OperationStatus::Running,
            priority: OperationPriority::Normal,
        };

        executor.execute(&op, &vfs, None).await.unwrap();

        let content = tokio::fs::read_to_string(dst_dir.join("file.txt"))
            .await
            .unwrap();
        assert_eq!(content, "copy me");

        // Check undo stack has a record
        assert_eq!(executor.undo_stack().len().await, 1);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_execute_delete() {
        let dir = test_dir().await;
        let trash_dir = dir.join("trash");
        let file = dir.join("to_delete.txt");
        tokio::fs::write(&file, b"bye").await.unwrap();

        let executor = make_executor(trash_dir);
        let vfs = LocalFs::new();

        let op = Operation {
            id: executor.next_id(),
            kind: OperationKind::Delete,
            sources: vec![RavenPath::local(&file)],
            destination: None,
            status: OperationStatus::Running,
            priority: OperationPriority::Normal,
        };

        executor.execute(&op, &vfs, None).await.unwrap();
        assert!(!tokio::fs::try_exists(&file).await.unwrap());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_execute_trash_and_restore() {
        let dir = test_dir().await;
        let trash_dir = dir.join("trash");
        let file = dir.join("trash_me.txt");
        tokio::fs::write(&file, b"save me").await.unwrap();

        let executor = make_executor(trash_dir);
        let vfs = LocalFs::new();

        let op = Operation {
            id: executor.next_id(),
            kind: OperationKind::Trash,
            sources: vec![RavenPath::local(&file)],
            destination: None,
            status: OperationStatus::Running,
            priority: OperationPriority::Normal,
        };

        executor.execute(&op, &vfs, None).await.unwrap();
        assert!(!tokio::fs::try_exists(&file).await.unwrap());

        // Undo the trash
        let record = executor
            .undo_stack()
            .undo(&vfs, executor.trash_fs())
            .await
            .unwrap();
        assert_eq!(record.kind, OperationKind::Trash);
        assert!(tokio::fs::try_exists(&file).await.unwrap());

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "save me");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
