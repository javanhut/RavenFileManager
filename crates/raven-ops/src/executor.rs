use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};
use tracing::{debug, error, info, instrument, warn};

use raven_core::error::{RavenError, RavenResult};
use raven_core::operations::{
    ConflictInfo, ConflictStrategy, Operation, OperationId, OperationKind, OperationProgress,
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

/// Called when a conflict needs a decision the resolver's default strategy
/// cannot make. The operation blocks until the answer arrives through
/// [`OperationExecutor::resolve_pending`], or until it is cancelled.
pub type ConflictPrompt = Arc<dyn Fn(ConflictInfo) + Send + Sync>;

/// How often a blocked operation checks whether it was cancelled while
/// waiting for a conflict answer.
const CONFLICT_POLL: Duration = Duration::from_millis(100);

/// Dispatches and executes file operations (copy, move, delete, trash).
pub struct OperationExecutor {
    copy_engine: CopyEngine,
    conflict_resolver: ConflictResolver,
    trash_fs: TrashFs,
    undo_stack: Arc<UndoStack>,
    queue: Arc<OperationQueue>,
    next_id: AtomicU64,
    conflict_prompt: Option<ConflictPrompt>,
    /// Operations blocked on a conflict answer, keyed by operation.
    pending_conflicts: Mutex<HashMap<OperationId, oneshot::Sender<ConflictStrategy>>>,
    /// "Apply to all" answers, remembered for the rest of that operation.
    sticky_strategies: Mutex<HashMap<OperationId, ConflictStrategy>>,
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
            conflict_prompt: None,
            pending_conflicts: Mutex::new(HashMap::new()),
            sticky_strategies: Mutex::new(HashMap::new()),
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
            conflict_prompt: None,
            pending_conflicts: Mutex::new(HashMap::new()),
            sticky_strategies: Mutex::new(HashMap::new()),
        }
    }

    /// Install the prompt that is asked when a conflict needs user input.
    ///
    /// Without one, a conflict the default strategy leaves undecided is skipped.
    pub fn set_conflict_prompt(&mut self, prompt: ConflictPrompt) {
        self.conflict_prompt = Some(prompt);
    }

    /// Answer the conflict an operation is blocked on.
    ///
    /// Returns `false` when nothing was waiting on `id`, which happens when the
    /// operation was cancelled or finished before the answer arrived.
    pub async fn resolve_pending(&self, id: OperationId, strategy: ConflictStrategy) -> bool {
        match self.pending_conflicts.lock().await.remove(&id) {
            Some(tx) => tx.send(strategy).is_ok(),
            None => false,
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

        // Answers only ever apply to the operation they were given for.
        self.sticky_strategies.lock().await.remove(&operation.id);
        self.pending_conflicts.lock().await.remove(&operation.id);

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

    /// Check for a conflict and resolve it.
    ///
    /// An "apply to all" answer given earlier in the same operation wins;
    /// otherwise the resolver's default strategy applies, and when that is
    /// `Ask` the conflict prompt is consulted.
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

        let Some(info) = conflict else {
            return Ok(Some(destination.clone()));
        };

        let sticky = self
            .sticky_strategies
            .lock()
            .await
            .get(&operation_id)
            .copied();
        let resolution = match sticky {
            Some(strategy) => self.conflict_resolver.resolve(&info, strategy).await?,
            None => self.conflict_resolver.resolve_with_default(&info).await?,
        };
        let resolution = match resolution {
            ConflictResolution::NeedsInput { conflict } => self.ask_user(conflict).await?,
            other => other,
        };

        match resolution {
            ConflictResolution::Proceed { destination } => Ok(Some(destination)),
            ConflictResolution::Skip | ConflictResolution::NeedsInput { .. } => {
                debug!(dst = %destination, "skipped due to conflict");
                Ok(None)
            }
        }
    }

    /// Block the operation on the conflict prompt until an answer or a
    /// cancellation arrives.
    async fn ask_user(&self, conflict: ConflictInfo) -> RavenResult<ConflictResolution> {
        let Some(prompt) = &self.conflict_prompt else {
            warn!(
                dst = %conflict.destination,
                "conflict requires user input but nothing can ask, skipping"
            );
            return Ok(ConflictResolution::Skip);
        };

        let id = conflict.operation_id;
        let (tx, mut rx) = oneshot::channel();
        self.pending_conflicts.lock().await.insert(id, tx);
        prompt(conflict.clone());

        let answer = loop {
            tokio::select! {
                answer = &mut rx => {
                    // A dropped sender means the operation was cancelled or
                    // finished from elsewhere; either way there is no answer.
                    break answer.unwrap_or(ConflictStrategy::Skip);
                }
                _ = tokio::time::sleep(CONFLICT_POLL) => {
                    if self.queue.is_cancelled(id).await {
                        self.pending_conflicts.lock().await.remove(&id);
                        self.queue.clear_cancelled(id).await;
                        return Err(RavenError::Cancelled);
                    }
                }
            }
        };

        // Handing the question back is not an answer.
        let strategy = match answer {
            ConflictStrategy::Ask => ConflictStrategy::Skip,
            other => other,
        };
        if matches!(
            strategy,
            ConflictStrategy::SkipAll
                | ConflictStrategy::OverwriteAll
                | ConflictStrategy::RenameAll
        ) {
            self.sticky_strategies.lock().await.insert(id, strategy);
        }

        self.conflict_resolver.resolve(&conflict, strategy).await
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

    /// A prompt that records what it was asked and can be answered from a test.
    fn recording_prompt() -> (ConflictPrompt, Arc<std::sync::Mutex<Vec<ConflictInfo>>>) {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = asked.clone();
        let prompt: ConflictPrompt = Arc::new(move |info| sink.lock().unwrap().push(info));
        (prompt, asked)
    }

    fn copy_op(executor: &OperationExecutor, sources: Vec<PathBuf>, dst: &PathBuf) -> Operation {
        Operation {
            id: executor.next_id(),
            kind: OperationKind::Copy,
            sources: sources.into_iter().map(RavenPath::local).collect(),
            destination: Some(RavenPath::local(dst)),
            status: OperationStatus::Running,
            priority: OperationPriority::Normal,
        }
    }

    #[tokio::test]
    async fn test_ask_without_prompt_skips() {
        let dir = test_dir().await;
        let (src_dir, dst_dir) = (dir.join("src"), dir.join("dst"));
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::create_dir_all(&dst_dir).await.unwrap();
        tokio::fs::write(src_dir.join("f.txt"), b"new").await.unwrap();
        tokio::fs::write(dst_dir.join("f.txt"), b"old").await.unwrap();

        let mut executor = make_executor(dir.join("trash"));
        executor
            .conflict_resolver_mut()
            .set_default_strategy(ConflictStrategy::Ask);
        let op = copy_op(&executor, vec![src_dir.join("f.txt")], &dst_dir);
        executor.execute(&op, &LocalFs::new(), None).await.unwrap();

        let content = tokio::fs::read_to_string(dst_dir.join("f.txt")).await.unwrap();
        assert_eq!(content, "old");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_prompt_answer_overwrite() {
        let dir = test_dir().await;
        let (src_dir, dst_dir) = (dir.join("src"), dir.join("dst"));
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::create_dir_all(&dst_dir).await.unwrap();
        tokio::fs::write(src_dir.join("f.txt"), b"new").await.unwrap();
        tokio::fs::write(dst_dir.join("f.txt"), b"old").await.unwrap();

        let mut executor = make_executor(dir.join("trash"));
        executor
            .conflict_resolver_mut()
            .set_default_strategy(ConflictStrategy::Ask);
        let (prompt, asked) = recording_prompt();
        executor.set_conflict_prompt(prompt);
        let executor = Arc::new(executor);

        let op = copy_op(&executor, vec![src_dir.join("f.txt")], &dst_dir);
        let op_id = op.id;

        // Answer from "the UI" once the question has been asked.
        let answerer = {
            let executor = executor.clone();
            let asked = asked.clone();
            tokio::spawn(async move {
                while asked.lock().unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert!(executor.resolve_pending(op_id, ConflictStrategy::Overwrite).await);
            })
        };

        executor.execute(&op, &LocalFs::new(), None).await.unwrap();
        answerer.await.unwrap();

        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].operation_id, op_id);
        let content = tokio::fs::read_to_string(dst_dir.join("f.txt")).await.unwrap();
        assert_eq!(content, "new");
        // Nothing is left waiting once the operation is over.
        assert!(!executor.resolve_pending(op_id, ConflictStrategy::Skip).await);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_apply_to_all_answers_once() {
        let dir = test_dir().await;
        let (src_dir, dst_dir) = (dir.join("src"), dir.join("dst"));
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::create_dir_all(&dst_dir).await.unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            tokio::fs::write(src_dir.join(name), b"new").await.unwrap();
            tokio::fs::write(dst_dir.join(name), b"old").await.unwrap();
        }

        let mut executor = make_executor(dir.join("trash"));
        executor
            .conflict_resolver_mut()
            .set_default_strategy(ConflictStrategy::Ask);
        let (prompt, asked) = recording_prompt();
        executor.set_conflict_prompt(prompt);
        let executor = Arc::new(executor);

        let sources = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|n| src_dir.join(n))
            .collect();
        let op = copy_op(&executor, sources, &dst_dir);
        let op_id = op.id;

        let answerer = {
            let executor = executor.clone();
            let asked = asked.clone();
            tokio::spawn(async move {
                while asked.lock().unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                executor.resolve_pending(op_id, ConflictStrategy::SkipAll).await;
            })
        };

        executor.execute(&op, &LocalFs::new(), None).await.unwrap();
        answerer.await.unwrap();

        // One question for three conflicts, and every file was left alone.
        assert_eq!(asked.lock().unwrap().len(), 1);
        for name in ["a.txt", "b.txt", "c.txt"] {
            let content = tokio::fs::read_to_string(dst_dir.join(name)).await.unwrap();
            assert_eq!(content, "old", "{name}");
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_cancel_while_waiting_on_conflict() {
        let dir = test_dir().await;
        let (src_dir, dst_dir) = (dir.join("src"), dir.join("dst"));
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::create_dir_all(&dst_dir).await.unwrap();
        tokio::fs::write(src_dir.join("f.txt"), b"new").await.unwrap();
        tokio::fs::write(dst_dir.join("f.txt"), b"old").await.unwrap();

        let mut executor = make_executor(dir.join("trash"));
        executor
            .conflict_resolver_mut()
            .set_default_strategy(ConflictStrategy::Ask);
        let (prompt, asked) = recording_prompt();
        executor.set_conflict_prompt(prompt);
        let executor = Arc::new(executor);

        let op = copy_op(&executor, vec![src_dir.join("f.txt")], &dst_dir);
        let op_id = op.id;

        let canceller = {
            let executor = executor.clone();
            tokio::spawn(async move {
                while asked.lock().unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                executor.queue().cancel(op_id).await.unwrap();
            })
        };

        let result = executor.execute(&op, &LocalFs::new(), None).await;
        canceller.await.unwrap();
        assert!(matches!(result, Err(RavenError::Cancelled)));
        assert!(!executor.resolve_pending(op_id, ConflictStrategy::Skip).await);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
