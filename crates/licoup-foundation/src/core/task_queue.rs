//! Small, bounded FIFO queue for local client work.
//!
//! The queue deliberately uses Rust's synchronous MPSC primitive instead of a
//! runtime-specific executor. Producers never need an async runtime, the
//! bounded buffer applies backpressure, and the single consumer preserves FIFO
//! execution for stateful local tasks.

use std::error::Error;
use std::fmt;
use std::sync::mpsc::{
    Receiver, RecvError, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueStats {
    pub capacity: usize,
    pub queued: usize,
    pub accepting: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidQueueCapacity;

impl fmt::Display for InvalidQueueCapacity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("task_queue_capacity_must_be_positive")
    }
}

impl Error for InvalidQueueCapacity {}

#[derive(Debug)]
pub enum SubmitError<T> {
    Full(T),
    Disconnected(T),
}

impl<T> SubmitError<T> {
    pub fn into_task(self) -> T {
        match self {
            Self::Full(task) | Self::Disconnected(task) => task,
        }
    }
}

impl<T> fmt::Display for SubmitError<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Full(_) => "task_queue_full",
            Self::Disconnected(_) => "task_queue_disconnected",
        })
    }
}

impl<T: fmt::Debug> Error for SubmitError<T> {}

struct QueueMetrics {
    queued: usize,
    accepting: bool,
}

struct QueueState {
    capacity: usize,
    metrics: Mutex<QueueMetrics>,
    slot_available: Condvar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReserveError {
    Full,
    Disconnected,
}

impl QueueState {
    fn lock_metrics(&self) -> MutexGuard<'_, QueueMetrics> {
        self.metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn try_reserve(&self) -> Result<(), ReserveError> {
        let mut metrics = self.lock_metrics();
        if !metrics.accepting {
            return Err(ReserveError::Disconnected);
        }
        if metrics.queued >= self.capacity {
            return Err(ReserveError::Full);
        }
        metrics.queued += 1;
        Ok(())
    }

    fn reserve(&self) -> Result<(), ReserveError> {
        let mut metrics = self.lock_metrics();
        loop {
            if !metrics.accepting {
                return Err(ReserveError::Disconnected);
            }
            if metrics.queued < self.capacity {
                metrics.queued += 1;
                return Ok(());
            }
            metrics = self
                .slot_available
                .wait(metrics)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn release_slot(&self) {
        let mut metrics = self.lock_metrics();
        metrics.queued = metrics.queued.saturating_sub(1);
        drop(metrics);
        self.slot_available.notify_one();
    }

    fn close(&self) {
        let mut metrics = self.lock_metrics();
        metrics.accepting = false;
        metrics.queued = 0;
        drop(metrics);
        self.slot_available.notify_all();
    }

    fn stats(&self) -> QueueStats {
        let metrics = self.lock_metrics();
        QueueStats {
            capacity: self.capacity,
            queued: metrics.queued,
            accepting: metrics.accepting,
        }
    }
}

/// Cloneable producer handle for a bounded local task queue.
pub struct TaskQueue<T> {
    sender: SyncSender<T>,
    state: Arc<QueueState>,
}

impl<T> Clone for TaskQueue<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<T> fmt::Debug for TaskQueue<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TaskQueue")
            .field("stats", &self.stats())
            .finish()
    }
}

/// Exclusive FIFO consumer. Keeping this non-cloneable makes task ordering
/// explicit and prevents accidental duplicate execution.
pub struct TaskWorker<T> {
    receiver: Receiver<T>,
    state: Arc<QueueState>,
}

impl<T> fmt::Debug for TaskWorker<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TaskWorker")
            .field("capacity", &self.state.capacity)
            .field("queued", &self.state.stats().queued)
            .finish()
    }
}

/// Create a bounded, multi-producer/single-consumer FIFO queue.
pub fn bounded<T>(capacity: usize) -> Result<(TaskQueue<T>, TaskWorker<T>), InvalidQueueCapacity> {
    if capacity == 0 {
        return Err(InvalidQueueCapacity);
    }
    let (sender, receiver) = sync_channel(capacity);
    let state = Arc::new(QueueState {
        capacity,
        metrics: Mutex::new(QueueMetrics {
            queued: 0,
            accepting: true,
        }),
        slot_available: Condvar::new(),
    });
    Ok((
        TaskQueue {
            sender,
            state: Arc::clone(&state),
        },
        TaskWorker { receiver, state },
    ))
}

impl<T> TaskQueue<T> {
    /// Submit without blocking the UI or a producer thread.
    pub fn try_submit(&self, task: T) -> Result<(), SubmitError<T>> {
        match self.state.try_reserve() {
            Ok(()) => {}
            Err(ReserveError::Full) => return Err(SubmitError::Full(task)),
            Err(ReserveError::Disconnected) => return Err(SubmitError::Disconnected(task)),
        }
        match self.sender.try_send(task) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(task)) => {
                self.state.release_slot();
                Err(SubmitError::Full(task))
            }
            Err(TrySendError::Disconnected(task)) => {
                self.state.close();
                Err(SubmitError::Disconnected(task))
            }
        }
    }

    /// Submit with bounded-buffer backpressure.
    pub fn submit(&self, task: T) -> Result<(), SubmitError<T>> {
        if self.state.reserve().is_err() {
            return Err(SubmitError::Disconnected(task));
        }
        self.sender.send(task).map_err(|error| {
            self.state.close();
            SubmitError::Disconnected(error.0)
        })
    }

    pub fn stats(&self) -> QueueStats {
        self.state.stats()
    }
}

impl<T> TaskWorker<T> {
    pub fn recv(&self) -> Result<T, RecvError> {
        self.receiver.recv().map(|task| {
            self.mark_received();
            task
        })
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout).map(|task| {
            self.mark_received();
            task
        })
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.receiver.try_recv().map(|task| {
            self.mark_received();
            task
        })
    }

    fn mark_received(&self) {
        self.state.release_slot();
    }
}

impl<T> Drop for TaskWorker<T> {
    fn drop(&mut self) {
        self.state.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_queue_preserves_fifo_and_reports_depth() {
        let (queue, worker) = bounded(2).unwrap();
        queue.try_submit("first").unwrap();
        queue.try_submit("second").unwrap();
        assert_eq!(queue.stats().queued, 2);
        assert_eq!(worker.try_recv().unwrap(), "first");
        assert_eq!(worker.try_recv().unwrap(), "second");
        assert_eq!(queue.stats().queued, 0);
    }

    #[test]
    fn full_queue_returns_ownership_to_the_caller() {
        let (queue, _worker) = bounded(1).unwrap();
        queue.try_submit(String::from("accepted")).unwrap();
        let error = queue.try_submit(String::from("retry-me")).unwrap_err();
        assert!(matches!(&error, SubmitError::Full(_)));
        assert_eq!(error.into_task(), "retry-me");
        assert_eq!(queue.stats().queued, 1);
    }

    #[test]
    fn blocking_submit_waits_for_capacity_without_exceeding_depth() {
        let (queue, worker) = bounded(1).unwrap();
        queue.try_submit(1).unwrap();
        let blocked_queue = queue.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = blocked_queue.submit(2);
            completed_tx.send(result).unwrap();
        });

        started_rx.recv().unwrap();
        assert!(
            completed_rx
                .recv_timeout(Duration::from_millis(20))
                .is_err()
        );
        assert_eq!(queue.stats().queued, 1);
        assert_eq!(worker.recv().unwrap(), 1);
        completed_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(queue.stats().queued, 1);
        assert_eq!(worker.recv().unwrap(), 2);
        handle.join().unwrap();
    }

    #[test]
    fn cloned_producers_share_one_bounded_fifo() {
        let (queue, worker) = bounded(2).unwrap();
        let second_producer = queue.clone();
        queue.try_submit(1).unwrap();
        second_producer.try_submit(2).unwrap();
        assert_eq!(worker.recv().unwrap(), 1);
        assert_eq!(worker.recv().unwrap(), 2);
    }

    #[test]
    fn dropping_worker_fails_closed_without_losing_the_task() {
        let (queue, worker) = bounded(1).unwrap();
        drop(worker);
        let error = queue.try_submit(7).unwrap_err();
        assert!(matches!(error, SubmitError::Disconnected(7)));
        assert!(!queue.stats().accepting);
    }

    #[test]
    fn dropping_worker_wakes_blocked_submit_and_returns_ownership() {
        let (queue, worker) = bounded(1).unwrap();
        queue.try_submit(String::from("accepted")).unwrap();
        let blocked_queue = queue.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            blocked_queue.submit(String::from("return-me"))
        });

        started_rx.recv().unwrap();
        drop(worker);
        let error = handle.join().unwrap().unwrap_err();
        assert!(matches!(&error, SubmitError::Disconnected(_)));
        assert_eq!(error.into_task(), "return-me");
        assert_eq!(queue.stats().queued, 0);
        assert!(!queue.stats().accepting);
    }

    #[test]
    fn zero_capacity_is_rejected() {
        assert!(matches!(bounded::<()>(0), Err(InvalidQueueCapacity)));
    }
}
