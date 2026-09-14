use std::collections::HashMap;
use std::path::PathBuf;
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

use crate::conflict::{path_exists, same_object, ConflictResolution, ConflictResolver};
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

/// Makes the temporary names of same-object probes unique.
static PROBE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Refuse to put a folder inside itself: the copy would keep walking into
/// the copies it makes, and a move has nowhere to go.
fn refuse_into_itself(source: &RavenPath, dest: &RavenPath) -> RavenResult<()> {
    if dest.is_inside(source) {
        return Err(RavenError::Other {
            message: format!("cannot put {} inside itself ({})", source, dest),
        });
    }
    Ok(())
}

/// Where one item of a copy or move goes once conflicts are settled.
#[derive(Debug)]
struct Target {
    path: RavenPath,
    /// Something already exists at `path` and writing over it (merging, for
    /// folders) was agreed to. Without this nothing may replace `path`.
    replaces: bool,
}

/// The local path of `path`, or its display form for other backends.
fn error_path(path: &RavenPath) -> PathBuf {
    path.as_local_path()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(path.to_string()))
}

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

    /// Stop on cancellation and wait while paused, between items.
    async fn checkpoint(&self, id: OperationId) -> RavenResult<()> {
        if self.queue.is_cancelled(id).await {
            self.queue.clear_cancelled(id).await;
            return Err(RavenError::Cancelled);
        }
        while self.queue.is_paused(id).await {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Copy one item (file or folder) to `dest`, reporting progress.
    ///
    /// Local to local goes through the copy engine; anything involving
    /// another backend goes through the VFS. Either way a folder copied onto
    /// an existing folder merges into it.
    ///
    /// Unless `replaces` is set, every file is created exclusively: a name
    /// taken since the conflict check (another client writing to the share
    /// during a long transfer, say) fails the copy instead of being written
    /// over.
    async fn copy_item(
        &self,
        operation: &Operation,
        idx: usize,
        source: &RavenPath,
        dest: &RavenPath,
        replaces: bool,
        vfs: &dyn VirtualFileSystem,
        progress_reporter: Option<&ProgressReporter>,
    ) -> RavenResult<()> {
        let op_id = operation.id;
        let files_total = operation.sources.len() as u64;
        let files_done = idx as u64;
        let report = move |reporter: &ProgressReporter, bytes_done, bytes_total| {
            reporter(OperationProgress {
                id: op_id,
                bytes_done,
                bytes_total,
                files_done,
                files_total,
                current_file: None,
            });
        };

        match (source.as_local_path(), dest.as_local_path()) {
            (Some(src), Some(dst)) => {
                let reporter = progress_reporter.cloned();
                let cb: ProgressCallback = Arc::new(move |bytes_done, bytes_total| {
                    if let Some(ref reporter) = reporter {
                        report(reporter, bytes_done, bytes_total);
                    }
                });
                if replaces {
                    self.copy_engine.copy_recursive(src, dst, Some(&cb)).await?;
                } else {
                    self.copy_engine.copy_recursive_new(src, dst, Some(&cb)).await?;
                }
            }
            _ => {
                let progress_cb = progress_reporter.cloned().map(|r| {
                    let cb: Box<dyn Fn(u64, u64) + Send + Sync> =
                        Box::new(move |bytes_done, bytes_total| report(&r, bytes_done, bytes_total));
                    cb
                });
                if replaces {
                    vfs.copy(source, dest, progress_cb).await?;
                } else {
                    vfs.copy_new(source, dest, progress_cb).await?;
                }
            }
        }
        Ok(())
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

        let mut created = Vec::new();

        for (idx, source) in operation.sources.iter().enumerate() {
            self.checkpoint(operation.id).await?;

            let file_name = source.file_name().ok_or_else(|| RavenError::Other {
                message: format!("unable to determine file name for {}", source),
            })?;
            let dest_path = destination.join(file_name);
            refuse_into_itself(source, &dest_path)?;

            // When pasting into the same directory the file already lives in,
            // always auto-rename (e.g. "file.txt" -> "file (1).txt").
            // For cross-directory copies, use the normal conflict resolver.
            let target = if source == &dest_path {
                Target {
                    path: self
                        .conflict_resolver
                        .generate_unique_name(vfs, &dest_path)
                        .await?,
                    replaces: false,
                }
            } else {
                match self
                    .resolve_conflict(vfs, operation.id, source, &dest_path)
                    .await?
                {
                    Some(target) => target,
                    None => continue, // Skipped
                }
            };

            if target.replaces {
                // "Replacing" an item with itself (reached through a symlinked
                // folder, say) would truncate it before reading it.
                if same_object(vfs, source, &target.path).await? {
                    debug!(src = %source, dst = %target.path, "same item, nothing to copy");
                    continue;
                }
                self.undo_stack.invalidate_touching(&target.path).await;
            }

            self.copy_item(
                operation,
                idx,
                source,
                &target.path,
                target.replaces,
                vfs,
                progress_reporter,
            )
            .await?;

            // Undoing a copy deletes what it made. Something written over
            // existing data is left out: deleting it would also delete what
            // was there before (a merged folder's other contents, say).
            if !target.replaces {
                created.push((source.clone(), target.path));
            }
        }

        self.undo_stack
            .record_copy_items(
                operation.id,
                operation.sources.clone(),
                destination.clone(),
                created,
            )
            .await;

        Ok(())
    }

    /// Whether `source` and `target` are one item under two names, checked
    /// before a copy+delete move would delete the only copy.
    ///
    /// Beyond what [`same_object`] can tell from names, remote items are
    /// probed: the source is briefly renamed within its own folder, and if
    /// the target disappears with it they are the same. It is then renamed
    /// back. A probe that cannot run fails the move, since the source could
    /// not be deleted afterwards either.
    async fn is_same_item(
        &self,
        vfs: &dyn VirtualFileSystem,
        source: &RavenPath,
        target: &RavenPath,
    ) -> RavenResult<bool> {
        if same_object(vfs, source, target).await? {
            return Ok(true);
        }
        if source.is_local() || target.is_local() {
            return Ok(false);
        }
        let (Some(parent), Some(name)) = (source.parent(), source.file_name()) else {
            return Ok(false);
        };
        let probe = parent.join(&format!(
            ".{}.raven-move-{}-{}",
            name,
            std::process::id(),
            PROBE_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        vfs.rename(source, &probe).await.map_err(|e| RavenError::Other {
            message: format!(
                "could not check that {} is not {} under another name, so it was not moved: {}",
                source, target, e
            ),
        })?;
        let target_remains = vfs.exists(target).await;
        if let Err(e) = vfs.rename(&probe, source).await {
            return Err(RavenError::Other {
                message: format!(
                    "{} was left at {} while checking the move, and could not be renamed back: {}",
                    source, probe, e
                ),
            });
        }
        Ok(!target_remains?)
    }

    /// Move one item to `target`: rename when the backend can, otherwise
    /// copy and then delete the source. Returns `false` when nothing had to
    /// move because the target already is the source under another name.
    ///
    /// The fallback never replaces a destination that was not agreed to be
    /// replaced (rename also fails when the name is taken, on SFTP and SMB,
    /// or when something appeared there since the conflict check), and the
    /// source is only deleted once the copy has fully succeeded.
    async fn move_item(
        &self,
        operation: &Operation,
        idx: usize,
        source: &RavenPath,
        target: &Target,
        vfs: &dyn VirtualFileSystem,
        progress_reporter: Option<&ProgressReporter>,
    ) -> RavenResult<bool> {
        let rename_error = match vfs.rename(source, &target.path).await {
            Ok(()) => {
                debug!(src = %source, dst = %target.path, "moved via rename");
                return Ok(true);
            }
            Err(e) => e,
        };

        if !target.replaces && path_exists(vfs, &target.path).await? {
            warn!(
                src = %source,
                dst = %target.path,
                error = %rename_error,
                "rename failed and the destination exists; not overwriting it"
            );
            return Err(RavenError::AlreadyExists {
                path: error_path(&target.path),
            });
        }

        // Something the copy would skip (a dangling link) must not be
        // "moved" by deleting it, even with an old destination in place.
        if !path_exists(vfs, source).await? {
            return Err(RavenError::Other {
                message: format!(
                    "{} cannot be read (a broken link?), so it was not moved",
                    source
                ),
            });
        }

        if target.replaces && self.is_same_item(vfs, source, &target.path).await? {
            debug!(src = %source, dst = %target.path, "same item under another name, nothing to move");
            return Ok(false);
        }

        // Other backends cannot hold a link: the copy would upload the whole
        // folder it points to, and deleting the link then loses nothing
        // but surprises everyone. Copying it is the honest way.
        if let (Some(src), false) = (source.as_local_path(), target.path.is_local()) {
            let is_link = tokio::fs::symlink_metadata(src)
                .await
                .map(|m| m.is_symlink())
                .unwrap_or(false);
            let to_dir = tokio::fs::metadata(src).await.map(|m| m.is_dir()).unwrap_or(false);
            if is_link && to_dir {
                return Err(RavenError::Other {
                    message: format!(
                        "{} is a link to a folder and {} cannot hold links; copy it instead",
                        source, target.path
                    ),
                });
            }
        }

        debug!(
            src = %source,
            dst = %target.path,
            error = %rename_error,
            "rename failed, moving via copy and delete"
        );
        // A failed copy returns here, before the source is touched.
        self.copy_item(
            operation,
            idx,
            source,
            &target.path,
            target.replaces,
            vfs,
            progress_reporter,
        )
        .await?;

        // A copy that reports success but produced nothing (a skipped
        // dangling link, say) must not cost the user the source.
        if !path_exists(vfs, &target.path).await? {
            return Err(RavenError::Other {
                message: format!(
                    "moving {} did not produce {}; the source was kept",
                    source, target.path
                ),
            });
        }

        vfs.delete(source).await?;
        debug!(src = %source, dst = %target.path, "moved via copy+delete");
        Ok(true)
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

        let mut moved = Vec::new();

        for (idx, source) in operation.sources.iter().enumerate() {
            self.checkpoint(operation.id).await?;

            let file_name = source.file_name().ok_or_else(|| RavenError::Other {
                message: format!("unable to determine file name for {}", source),
            })?;
            let dest_path = destination.join(file_name);
            refuse_into_itself(source, &dest_path)?;

            // Moving an item to where it already is changes nothing. Letting
            // it through could "overwrite" the item with itself, and a
            // copy+delete fallback would then delete it.
            if source == &dest_path {
                debug!(path = %source, "already in place, nothing to move");
                continue;
            }

            let Some(target) = self
                .resolve_conflict(vfs, operation.id, source, &dest_path)
                .await?
            else {
                continue;
            };

            if target.replaces {
                self.undo_stack.invalidate_touching(&target.path).await;
            }

            let moved_it = self
                .move_item(operation, idx, source, &target, vfs, progress_reporter)
                .await?;

            // Moving back something that replaced existing data would carry
            // off what was merged into it, so undo leaves those alone.
            if moved_it && !target.replaces {
                moved.push((source.clone(), target.path));
            }
        }

        self.undo_stack
            .record_move_items(
                operation.id,
                operation.sources.clone(),
                destination.clone(),
                moved,
            )
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
        // Renaming onto a name that is taken would replace that item on the
        // local filesystem, silently. A different spelling of the item's own
        // name (a case-only rename) is not taken.
        if source != destination
            && path_exists(vfs, destination).await?
            && !same_object(vfs, source, destination).await?
        {
            return Err(RavenError::AlreadyExists {
                path: error_path(destination),
            });
        }
        vfs.rename(source, destination).await?;

        // Record as a move for undo purposes. The destination is the new
        // path itself, not a folder, so the pair is recorded exactly.
        self.undo_stack
            .record_move_items(
                operation.id,
                operation.sources.clone(),
                destination.clone(),
                vec![(source.clone(), destination.clone())],
            )
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

    /// Check for a conflict and resolve it, on whatever backend the
    /// destination lives on.
    ///
    /// An "apply to all" answer given earlier in the same operation wins;
    /// otherwise the resolver's default strategy applies, and when that is
    /// `Ask` the conflict prompt is consulted.
    ///
    /// Returns `Some(target)` if the operation should proceed, or `None`
    /// if it should be skipped.
    async fn resolve_conflict(
        &self,
        vfs: &dyn VirtualFileSystem,
        operation_id: OperationId,
        source: &RavenPath,
        destination: &RavenPath,
    ) -> RavenResult<Option<Target>> {
        let conflict = self
            .conflict_resolver
            .check_conflict(vfs, operation_id, source, destination)
            .await?;

        let Some(info) = conflict else {
            return Ok(Some(Target {
                path: destination.clone(),
                replaces: false,
            }));
        };

        let sticky = self
            .sticky_strategies
            .lock()
            .await
            .get(&operation_id)
            .copied();
        let resolution = match sticky {
            Some(strategy) => self.conflict_resolver.resolve(vfs, &info, strategy).await?,
            None => self.conflict_resolver.resolve_with_default(vfs, &info).await?,
        };
        let resolution = match resolution {
            ConflictResolution::NeedsInput { conflict } => self.ask_user(vfs, conflict).await?,
            other => other,
        };

        match resolution {
            ConflictResolution::Proceed { destination: resolved } => {
                // Keeping the conflicting name means writing over what is there.
                let replaces = &resolved == destination;
                Ok(Some(Target {
                    path: resolved,
                    replaces,
                }))
            }
            ConflictResolution::Skip | ConflictResolution::NeedsInput { .. } => {
                debug!(dst = %destination, "skipped due to conflict");
                Ok(None)
            }
        }
    }

    /// Block the operation on the conflict prompt until an answer or a
    /// cancellation arrives.
    async fn ask_user(
        &self,
        vfs: &dyn VirtualFileSystem,
        conflict: ConflictInfo,
    ) -> RavenResult<ConflictResolution> {
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

        self.conflict_resolver.resolve(vfs, &conflict, strategy).await
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
    use crate::test_support::{remote, MemFs};
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

    // --- Remote destinations (through an in-memory VFS) ---

    type Asked = Arc<std::sync::Mutex<Vec<ConflictInfo>>>;

    /// An executor that asks about every conflict, and what it asked.
    async fn asking_executor() -> (Arc<OperationExecutor>, Asked) {
        let dir = test_dir().await;
        let mut executor = make_executor(dir.join("trash"));
        executor
            .conflict_resolver_mut()
            .set_default_strategy(ConflictStrategy::Ask);
        let (prompt, asked) = recording_prompt();
        executor.set_conflict_prompt(prompt);
        (Arc::new(executor), asked)
    }

    fn answer_when_asked(
        executor: &Arc<OperationExecutor>,
        asked: &Asked,
        id: OperationId,
        strategy: ConflictStrategy,
    ) -> tokio::task::JoinHandle<()> {
        let executor = executor.clone();
        let asked = asked.clone();
        tokio::spawn(async move {
            while asked.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(executor.resolve_pending(id, strategy).await);
        })
    }

    fn remote_op(
        executor: &OperationExecutor,
        kind: OperationKind,
        sources: Vec<RavenPath>,
        dst: RavenPath,
    ) -> Operation {
        Operation {
            id: executor.next_id(),
            kind,
            sources,
            destination: Some(dst),
            status: OperationStatus::Running,
            priority: OperationPriority::Normal,
        }
    }

    #[tokio::test]
    async fn test_remote_conflict_prompts_and_overwrite() {
        let fs = MemFs::new();
        let src = fs.file("/src/f.txt", b"newer");
        let dst = fs.file("/dst/f.txt", b"old");
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Copy, vec![src], remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, op.id, ConflictStrategy::Overwrite);
        executor.execute(&op, &fs, None).await.unwrap();
        answer.await.unwrap();

        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].destination, dst);
        assert_eq!((asked[0].source_size, asked[0].dest_size), (5, 3));
        assert_eq!(fs.contents(&dst).as_deref(), Some(&b"newer"[..]));
    }

    #[tokio::test]
    async fn test_remote_conflict_skip_leaves_destination_even_after_undo() {
        let fs = MemFs::new();
        let src = fs.file("/src/f.txt", b"new");
        let dst = fs.file("/dst/f.txt", b"old");
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Copy, vec![src.clone()], remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, op.id, ConflictStrategy::Skip);
        executor.execute(&op, &fs, None).await.unwrap();
        answer.await.unwrap();

        assert_eq!(fs.contents(&dst).as_deref(), Some(&b"old"[..]));
        // Undoing the copy must not delete the file it never wrote. Nothing
        // was copied, so there is nothing to undo at all.
        assert!(executor.undo_stack().undo(&fs, executor.trash_fs()).await.is_err());
        assert_eq!(fs.contents(&dst).as_deref(), Some(&b"old"[..]));
        assert!(fs.node(&src).is_some());
    }

    #[tokio::test]
    async fn test_remote_keep_both_picks_a_free_remote_name() {
        let fs = MemFs::new();
        let src = fs.file("/src/f.txt", b"new");
        let original = fs.file("/dst/f.txt", b"old");
        let taken = fs.file("/dst/f (1).txt", b"older");
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Copy, vec![src], remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, op.id, ConflictStrategy::Rename);
        executor.execute(&op, &fs, None).await.unwrap();
        answer.await.unwrap();

        let kept = remote("/dst/f (2).txt");
        assert_eq!(fs.contents(&kept).as_deref(), Some(&b"new"[..]));
        assert_eq!(fs.contents(&original).as_deref(), Some(&b"old"[..]));
        assert_eq!(fs.contents(&taken).as_deref(), Some(&b"older"[..]));

        // Undo removes the copy it made and nothing else.
        executor.undo_stack().undo(&fs, executor.trash_fs()).await.unwrap();
        assert!(fs.node(&kept).is_none());
        assert_eq!(fs.contents(&original).as_deref(), Some(&b"old"[..]));
        assert_eq!(fs.contents(&taken).as_deref(), Some(&b"older"[..]));
    }

    #[tokio::test]
    async fn test_remote_apply_to_all_asks_once() {
        let fs = MemFs::new();
        let mut sources = Vec::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            sources.push(fs.file(&format!("/src/{name}"), b"new"));
            fs.file(&format!("/dst/{name}"), b"old");
        }
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Copy, sources, remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, op.id, ConflictStrategy::OverwriteAll);
        executor.execute(&op, &fs, None).await.unwrap();
        answer.await.unwrap();

        assert_eq!(asked.lock().unwrap().len(), 1);
        for name in ["a.txt", "b.txt", "c.txt"] {
            let content = fs.contents(&remote(&format!("/dst/{name}")));
            assert_eq!(content.as_deref(), Some(&b"new"[..]), "{name}");
        }
    }

    #[tokio::test]
    async fn test_remote_folder_conflict_merges() {
        let fs = MemFs::new();
        let src = fs.dir("/src/d");
        fs.file("/src/d/a.txt", b"new");
        fs.file("/src/d/b.txt", b"b");
        fs.file("/dst/d/a.txt", b"old");
        fs.file("/dst/d/c.txt", b"c");
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Copy, vec![src], remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, op.id, ConflictStrategy::Overwrite);
        executor.execute(&op, &fs, None).await.unwrap();
        answer.await.unwrap();

        assert_eq!(asked.lock().unwrap()[0].destination, remote("/dst/d"));
        assert_eq!(fs.contents(&remote("/dst/d/a.txt")).as_deref(), Some(&b"new"[..]));
        assert_eq!(fs.contents(&remote("/dst/d/b.txt")).as_deref(), Some(&b"b"[..]));
        assert_eq!(fs.contents(&remote("/dst/d/c.txt")).as_deref(), Some(&b"c"[..]));

        // Undo does not delete the merged folder with its other contents;
        // a merge leaves no undo entry to use up.
        assert!(executor.undo_stack().undo(&fs, executor.trash_fs()).await.is_err());
        assert_eq!(fs.contents(&remote("/dst/d/c.txt")).as_deref(), Some(&b"c"[..]));
    }

    #[tokio::test]
    async fn test_move_rename_fallback_does_not_clobber() {
        let fs = MemFs::new();
        let src = fs.file("/src/f.txt", b"mine");
        fs.dir("/dst");
        let dst = remote("/dst/f.txt");
        // Free at the conflict check, taken by the time the rename runs.
        *fs.appears_on_rename.lock().unwrap() = Some((dst.clone(), b"theirs".to_vec()));
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Move, vec![src.clone()], remote("/dst"));
        let result = executor.execute(&op, &fs, None).await;

        assert!(matches!(result, Err(RavenError::AlreadyExists { .. })), "{result:?}");
        assert!(asked.lock().unwrap().is_empty());
        assert_eq!(fs.contents(&dst).as_deref(), Some(&b"theirs"[..]));
        assert_eq!(fs.contents(&src).as_deref(), Some(&b"mine"[..]));
    }

    #[tokio::test]
    async fn test_move_fallback_overwrites_only_when_agreed() {
        let fs = MemFs::new();
        fs.rename_fails.store(true, Ordering::SeqCst);
        let src = fs.file("/src/f.txt", b"new");
        let dst = fs.file("/dst/f.txt", b"old");
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Move, vec![src.clone()], remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, op.id, ConflictStrategy::Overwrite);
        executor.execute(&op, &fs, None).await.unwrap();
        answer.await.unwrap();

        assert_eq!(fs.contents(&dst).as_deref(), Some(&b"new"[..]));
        assert!(fs.node(&src).is_none());
    }

    #[tokio::test]
    async fn test_move_fallback_without_conflict_copies_then_deletes() {
        let fs = MemFs::new();
        fs.rename_fails.store(true, Ordering::SeqCst);
        let src = fs.dir("/src/d");
        fs.file("/src/d/a.txt", b"a");
        fs.dir("/dst");
        let executor = make_executor(test_dir().await.join("trash"));

        let op = remote_op(&executor, OperationKind::Move, vec![src.clone()], remote("/dst"));
        executor.execute(&op, &fs, None).await.unwrap();

        assert_eq!(fs.contents(&remote("/dst/d/a.txt")).as_deref(), Some(&b"a"[..]));
        assert!(fs.node(&src).is_none());
    }

    #[tokio::test]
    async fn test_move_keeps_source_when_copy_fails() {
        let fs = MemFs::new();
        fs.rename_fails.store(true, Ordering::SeqCst);
        *fs.fail_writes_named.lock().unwrap() = Some("b.txt".into());
        let src = fs.dir("/src/d");
        fs.file("/src/d/a.txt", b"a");
        fs.file("/src/d/b.txt", b"b");
        fs.dir("/dst");
        let executor = make_executor(test_dir().await.join("trash"));

        let op = remote_op(&executor, OperationKind::Move, vec![src], remote("/dst"));
        let result = executor.execute(&op, &fs, None).await;

        assert!(result.is_err());
        assert_eq!(fs.contents(&remote("/src/d/a.txt")).as_deref(), Some(&b"a"[..]));
        assert_eq!(fs.contents(&remote("/src/d/b.txt")).as_deref(), Some(&b"b"[..]));
    }

    #[tokio::test]
    async fn test_move_onto_itself_does_nothing() {
        let fs = MemFs::new();
        fs.rename_fails.store(true, Ordering::SeqCst);
        let file = fs.file("/dst/f.txt", b"keep");
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Move, vec![file.clone()], remote("/dst"));
        executor.execute(&op, &fs, None).await.unwrap();

        assert!(asked.lock().unwrap().is_empty());
        assert_eq!(fs.contents(&file).as_deref(), Some(&b"keep"[..]));
    }

    #[tokio::test]
    async fn test_replace_move_disables_undo_of_the_copy_it_merged_into() {
        let fs = MemFs::new();
        let first = fs.dir("/x/d");
        fs.file("/x/d/a.txt", b"a");
        let second = fs.dir("/y/d");
        fs.file("/y/d/b.txt", b"only copy");
        fs.dir("/dst");
        fs.rename_fails.store(true, Ordering::SeqCst);
        let (executor, asked) = asking_executor().await;

        let copy = remote_op(&executor, OperationKind::Copy, vec![first], remote("/dst"));
        executor.execute(&copy, &fs, None).await.unwrap();
        let mv = remote_op(&executor, OperationKind::Move, vec![second.clone()], remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, mv.id, ConflictStrategy::Overwrite);
        executor.execute(&mv, &fs, None).await.unwrap();
        answer.await.unwrap();
        assert!(fs.node(&second).is_none());

        // Neither the merge nor the older copy can be undone any more:
        // undoing the copy would delete the moved file's only copy.
        assert!(executor.undo_stack().undo(&fs, executor.trash_fs()).await.is_err());
        assert_eq!(fs.contents(&remote("/dst/d/b.txt")).as_deref(), Some(&b"only copy"[..]));
        assert_eq!(fs.contents(&remote("/dst/d/a.txt")).as_deref(), Some(&b"a"[..]));
    }

    #[tokio::test]
    async fn test_copy_or_move_into_itself_is_refused() {
        let fs = MemFs::new();
        let a = fs.dir("/a");
        fs.dir("/a/sub");
        let executor = make_executor(test_dir().await.join("trash"));

        for kind in [OperationKind::Copy, OperationKind::Move] {
            let op = remote_op(&executor, kind, vec![a.clone()], remote("/a/sub"));
            let result = executor.execute(&op, &fs, None).await;
            assert!(result.is_err(), "{kind:?}");
            assert!(fs.node(&remote("/a/sub/a")).is_none(), "{kind:?}");
            assert!(fs.node(&a).is_some(), "{kind:?}");
        }
    }

    #[tokio::test]
    async fn test_remote_replace_move_probe_keeps_real_replace_working() {
        // Rename across folders fails, so the move goes through the probe
        // and copy+delete; different items are still replaced as agreed.
        let fs = MemFs::new();
        fs.rename_fails.store(true, Ordering::SeqCst);
        let src = fs.file("/src/f.txt", b"new");
        fs.file("/src/other.txt", b"o");
        let dst = fs.file("/dst/f.txt", b"old");
        let (executor, asked) = asking_executor().await;

        let op = remote_op(&executor, OperationKind::Move, vec![src.clone()], remote("/dst"));
        let answer = answer_when_asked(&executor, &asked, op.id, ConflictStrategy::Overwrite);
        executor.execute(&op, &fs, None).await.unwrap();
        answer.await.unwrap();

        assert_eq!(fs.contents(&dst).as_deref(), Some(&b"new"[..]));
        assert!(fs.node(&src).is_none());
        // The probe left no temporary names behind.
        let left: Vec<_> = fs.list_dir(&remote("/src")).await.unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(left, vec!["other.txt".to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_local_replace_through_symlinked_folder_keeps_the_file() {
        let dir = test_dir().await;
        tokio::fs::create_dir_all(dir.join("real")).await.unwrap();
        tokio::fs::write(dir.join("real/f.txt"), b"precious").await.unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let vfs = LocalFs::new();

        for kind in [OperationKind::Copy, OperationKind::Move] {
            let executor = make_executor(dir.join("trash")); // default: Overwrite
            let op = Operation {
                kind,
                ..copy_op(&executor, vec![dir.join("real/f.txt")], &dir.join("link"))
            };
            executor.execute(&op, &vfs, None).await.unwrap();
            let content = tokio::fs::read(dir.join("real/f.txt")).await.unwrap();
            assert_eq!(content, b"precious", "{kind:?}");
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_rename_onto_existing_name_is_refused() {
        let dir = test_dir().await;
        tokio::fs::write(dir.join("a.txt"), b"a").await.unwrap();
        tokio::fs::write(dir.join("b.txt"), b"b").await.unwrap();
        let executor = make_executor(dir.join("trash"));
        let vfs = LocalFs::new();

        let op = Operation {
            id: executor.next_id(),
            kind: OperationKind::Rename,
            sources: vec![RavenPath::local(dir.join("a.txt"))],
            destination: Some(RavenPath::local(dir.join("b.txt"))),
            status: OperationStatus::Running,
            priority: OperationPriority::Normal,
        };
        let result = executor.execute(&op, &vfs, None).await;
        assert!(matches!(result, Err(RavenError::AlreadyExists { .. })), "{result:?}");
        assert_eq!(tokio::fs::read(dir.join("a.txt")).await.unwrap(), b"a");
        assert_eq!(tokio::fs::read(dir.join("b.txt")).await.unwrap(), b"b");

        // A free name still works.
        let op = Operation {
            id: executor.next_id(),
            destination: Some(RavenPath::local(dir.join("c.txt"))),
            ..op
        };
        executor.execute(&op, &vfs, None).await.unwrap();
        assert_eq!(tokio::fs::read(dir.join("c.txt")).await.unwrap(), b"a");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_local_copy_without_replace_never_truncates_a_file_that_appeared() {
        // The executor's no-conflict path uses the exclusive copy: if the
        // name is taken by the time the file is written, it fails.
        let dir = test_dir().await;
        tokio::fs::create_dir_all(dir.join("src/d")).await.unwrap();
        tokio::fs::write(dir.join("src/d/f.txt"), b"new").await.unwrap();
        tokio::fs::create_dir_all(dir.join("dst/d")).await.unwrap();
        tokio::fs::write(dir.join("dst/d/f.txt"), b"theirs").await.unwrap();

        let engine = CopyEngine::new();
        let result = engine
            .copy_recursive_new(&dir.join("src/d"), &dir.join("dst/d"), None)
            .await;
        assert!(result.is_err());
        assert_eq!(tokio::fs::read(dir.join("dst/d/f.txt")).await.unwrap(), b"theirs");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_local_move_skip_is_not_undone_onto_the_source() {
        let dir = test_dir().await;
        let (src_dir, dst_dir) = (dir.join("src"), dir.join("dst"));
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::create_dir_all(&dst_dir).await.unwrap();
        tokio::fs::write(src_dir.join("f.txt"), b"mine").await.unwrap();
        tokio::fs::write(dst_dir.join("f.txt"), b"theirs").await.unwrap();

        let mut executor = make_executor(dir.join("trash"));
        executor
            .conflict_resolver_mut()
            .set_default_strategy(ConflictStrategy::Skip);
        let vfs = LocalFs::new();
        let op = Operation {
            kind: OperationKind::Move,
            ..copy_op(&executor, vec![src_dir.join("f.txt")], &dst_dir)
        };
        executor.execute(&op, &vfs, None).await.unwrap();
        let _ = executor.undo_stack().undo(&vfs, executor.trash_fs()).await;

        let mine = tokio::fs::read_to_string(src_dir.join("f.txt")).await.unwrap();
        let theirs = tokio::fs::read_to_string(dst_dir.join("f.txt")).await.unwrap();
        assert_eq!((mine.as_str(), theirs.as_str()), ("mine", "theirs"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
