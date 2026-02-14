use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info};

use raven_core::error::RavenResult;
use raven_core::operations::{Operation, OperationId, OperationPriority, OperationStatus};

/// Default maximum number of concurrent operations.
const DEFAULT_MAX_CONCURRENCY: usize = 4;

/// An entry in the operation queue.
#[derive(Debug)]
struct QueueEntry {
    operation: Operation,
}

/// A priority-aware operation queue with bounded concurrency.
///
/// Uses a `tokio::sync::Semaphore` to limit how many operations can
/// execute simultaneously.
pub struct OperationQueue {
    /// Pending operations ordered by priority.
    pending: Arc<Mutex<VecDeque<QueueEntry>>>,
    /// Active (running) operations.
    active: Arc<Mutex<Vec<Operation>>>,
    /// Semaphore controlling max concurrency.
    semaphore: Arc<Semaphore>,
    /// Maximum concurrency.
    max_concurrency: usize,
    /// Cancelled operation IDs.
    cancelled: Arc<Mutex<Vec<OperationId>>>,
    /// Paused operation IDs.
    paused: Arc<Mutex<Vec<OperationId>>>,
}

impl OperationQueue {
    /// Create a new OperationQueue with the default concurrency limit.
    pub fn new() -> Self {
        Self::with_max_concurrency(DEFAULT_MAX_CONCURRENCY)
    }

    /// Create a new OperationQueue with a custom concurrency limit.
    pub fn with_max_concurrency(max_concurrency: usize) -> Self {
        Self {
            pending: Arc::new(Mutex::new(VecDeque::new())),
            active: Arc::new(Mutex::new(Vec::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            max_concurrency,
            cancelled: Arc::new(Mutex::new(Vec::new())),
            paused: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Submit an operation to the queue.
    ///
    /// Operations are inserted in priority order (High before Normal before Low).
    pub async fn submit(&self, operation: Operation) {
        let mut pending = self.pending.lock().await;
        let entry = QueueEntry { operation };

        // Find insertion position based on priority
        let pos = pending
            .iter()
            .position(|e| priority_rank(&e.operation.priority) < priority_rank(&entry.operation.priority))
            .unwrap_or(pending.len());

        pending.insert(pos, entry);
        info!(
            id = ?pending.back().map(|e| e.operation.id),
            queue_len = pending.len(),
            "operation submitted to queue"
        );
    }

    /// Take the next pending operation from the queue, waiting for a
    /// semaphore permit to enforce concurrency limits.
    ///
    /// Returns `None` if the queue is empty.
    pub async fn take_next(&self) -> Option<Operation> {
        // First check if there is a pending operation
        let mut op = {
            let mut pending = self.pending.lock().await;
            if pending.is_empty() {
                return None;
            }
            let entry = pending.pop_front()?;
            entry.operation
        };

        // Acquire semaphore permit (waits until a slot is available)
        let _permit = self.semaphore.acquire().await.ok()?;
        // We intentionally forget the permit here; the caller must call
        // `complete` to release it via `release_slot`.
        std::mem::forget(_permit);

        op.status = OperationStatus::Running;
        self.active.lock().await.push(op.clone());
        debug!(id = ?op.id, "operation taken from queue");
        Some(op)
    }

    /// Release a concurrency slot when an operation finishes.
    pub async fn complete(&self, id: OperationId) {
        let mut active = self.active.lock().await;
        active.retain(|op| op.id != id);
        self.semaphore.add_permits(1);
        debug!(?id, "operation completed, slot released");
    }

    /// Cancel a pending or active operation.
    pub async fn cancel(&self, id: OperationId) -> RavenResult<()> {
        // Try removing from pending
        {
            let mut pending = self.pending.lock().await;
            let len_before = pending.len();
            pending.retain(|e| e.operation.id != id);
            if pending.len() < len_before {
                info!(?id, "cancelled pending operation");
                return Ok(());
            }
        }

        // Mark as cancelled for active operations
        let mut cancelled = self.cancelled.lock().await;
        cancelled.push(id);
        info!(?id, "marked active operation for cancellation");
        Ok(())
    }

    /// Check if an operation has been cancelled.
    pub async fn is_cancelled(&self, id: OperationId) -> bool {
        let cancelled = self.cancelled.lock().await;
        cancelled.contains(&id)
    }

    /// Clear the cancelled flag for an operation.
    pub async fn clear_cancelled(&self, id: OperationId) {
        let mut cancelled = self.cancelled.lock().await;
        cancelled.retain(|&cid| cid != id);
    }

    /// Pause an operation.
    pub async fn pause(&self, id: OperationId) -> RavenResult<()> {
        let mut paused = self.paused.lock().await;
        if !paused.contains(&id) {
            paused.push(id);
            info!(?id, "operation paused");
        }
        Ok(())
    }

    /// Resume a paused operation.
    pub async fn resume(&self, id: OperationId) -> RavenResult<()> {
        let mut paused = self.paused.lock().await;
        paused.retain(|&pid| pid != id);
        info!(?id, "operation resumed");
        Ok(())
    }

    /// Check if an operation is paused.
    pub async fn is_paused(&self, id: OperationId) -> bool {
        let paused = self.paused.lock().await;
        paused.contains(&id)
    }

    /// Return the number of pending operations.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }

    /// Return the number of active operations.
    pub async fn active_count(&self) -> usize {
        self.active.lock().await.len()
    }

    /// Return the maximum concurrency.
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// List all pending operations (cloned).
    pub async fn list_pending(&self) -> Vec<Operation> {
        let pending = self.pending.lock().await;
        pending.iter().map(|e| e.operation.clone()).collect()
    }

    /// List all active operations (cloned).
    pub async fn list_active(&self) -> Vec<Operation> {
        self.active.lock().await.clone()
    }
}

impl Default for OperationQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Map priority to a numeric rank for ordering (higher = more important).
fn priority_rank(p: &OperationPriority) -> u8 {
    match p {
        OperationPriority::High => 2,
        OperationPriority::Normal => 1,
        OperationPriority::Low => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raven_core::operations::OperationKind;
    use raven_core::path::RavenPath;

    fn make_op(id: u64, priority: OperationPriority) -> Operation {
        Operation {
            id: OperationId(id),
            kind: OperationKind::Copy,
            sources: vec![RavenPath::local("/tmp/src")],
            destination: Some(RavenPath::local("/tmp/dst")),
            status: OperationStatus::Pending,
            priority,
        }
    }

    #[tokio::test]
    async fn test_submit_and_take() {
        let queue = OperationQueue::new();
        let op = make_op(1, OperationPriority::Normal);
        queue.submit(op).await;

        assert_eq!(queue.pending_count().await, 1);

        let taken = queue.take_next().await;
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().id, OperationId(1));
        assert_eq!(queue.pending_count().await, 0);

        queue.complete(OperationId(1)).await;
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let queue = OperationQueue::new();
        queue.submit(make_op(1, OperationPriority::Low)).await;
        queue.submit(make_op(2, OperationPriority::High)).await;
        queue.submit(make_op(3, OperationPriority::Normal)).await;

        let pending = queue.list_pending().await;
        assert_eq!(pending[0].id, OperationId(2)); // High first
        assert_eq!(pending[1].id, OperationId(3)); // Normal second
        assert_eq!(pending[2].id, OperationId(1)); // Low last
    }

    #[tokio::test]
    async fn test_cancel_pending() {
        let queue = OperationQueue::new();
        queue.submit(make_op(1, OperationPriority::Normal)).await;
        queue.submit(make_op(2, OperationPriority::Normal)).await;

        queue.cancel(OperationId(1)).await.unwrap();
        assert_eq!(queue.pending_count().await, 1);

        let taken = queue.take_next().await;
        assert_eq!(taken.unwrap().id, OperationId(2));
        queue.complete(OperationId(2)).await;
    }

    #[tokio::test]
    async fn test_pause_resume() {
        let queue = OperationQueue::new();
        let id = OperationId(1);

        assert!(!queue.is_paused(id).await);
        queue.pause(id).await.unwrap();
        assert!(queue.is_paused(id).await);
        queue.resume(id).await.unwrap();
        assert!(!queue.is_paused(id).await);
    }

    #[tokio::test]
    async fn test_empty_queue_returns_none() {
        let queue = OperationQueue::new();
        assert!(queue.take_next().await.is_none());
    }
}
