//! Cooperative connection-worker shutdown.
//!
//! The relay host signals every connection worker when shutdown begins, then
//! waits a bounded window for them to drain. A worker parked on a stream read
//! exits immediately; a worker mid-request finishes its in-flight dispatch and
//! flushes the response before exiting. Workers that do not drain within the
//! window are abandoned to runtime teardown (and ultimately the shutdown
//! watchdog), so a wedged worker can never stall SIGINT/SIGTERM handling.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

/// Sampling interval for the bounded drain wait.
const DRAIN_WAIT_POLL_INTERVAL_MS: u64 = 25;

/// Shared coordinator between the relay host (signal + bounded wait side) and
/// its connection workers (observe + report side).
#[derive(Debug)]
pub struct ConnectionDrainCoordinator {
    signal: watch::Sender<bool>,
    active_worker_count: AtomicUsize,
    serving_worker_count: AtomicUsize,
    drained_worker_count: AtomicUsize,
}

/// Worker-drain outcome reported by [`ConnectionDrainCoordinator::wait_for_drain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionDrainReport {
    /// Workers that exited after the shutdown signal fired.
    pub drained_worker_count: usize,
    /// Workers still registered when the wait ended.
    pub remaining_worker_count: usize,
    /// Subset of the remaining workers that were mid-request when the wait
    /// ended; the rest were parked on stream reads.
    pub remaining_serving_count: usize,
    /// True when the wait ended at the timeout rather than on full drain.
    pub timed_out: bool,
}

impl ConnectionDrainCoordinator {
    pub fn new() -> Arc<Self> {
        let (signal, _) = watch::channel(false);
        Arc::new(Self {
            signal,
            active_worker_count: AtomicUsize::new(0),
            serving_worker_count: AtomicUsize::new(0),
            drained_worker_count: AtomicUsize::new(0),
        })
    }

    /// Registers one connection worker. Must be called before the worker task
    /// is spawned so a shutdown signal racing the spawn still counts it.
    pub fn register_worker(self: &Arc<Self>) -> ConnectionWorkerSlot {
        self.active_worker_count.fetch_add(1, Ordering::SeqCst);
        ConnectionWorkerSlot {
            coordinator: Arc::clone(self),
            receiver: self.signal.subscribe(),
        }
    }

    /// Fires the cooperative shutdown signal observed by every registered
    /// worker. Idempotent.
    pub fn signal_shutdown(&self) {
        self.signal.send_replace(true);
    }

    /// Waits up to `timeout` for every registered worker to exit, sampling the
    /// remaining count on a short poll interval. Returns the drain outcome
    /// either way; the caller decides how to report abandoned workers.
    pub async fn wait_for_drain(&self, timeout: Duration) -> ConnectionDrainReport {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = self.active_worker_count.load(Ordering::SeqCst);
            if remaining == 0 {
                return self.drain_report(false);
            }
            let now = Instant::now();
            if now >= deadline {
                return self.drain_report(true);
            }
            let poll = (deadline - now).min(Duration::from_millis(DRAIN_WAIT_POLL_INTERVAL_MS));
            tokio::time::sleep(poll).await;
        }
    }

    fn drain_report(&self, timed_out: bool) -> ConnectionDrainReport {
        ConnectionDrainReport {
            drained_worker_count: self.drained_worker_count.load(Ordering::SeqCst),
            remaining_worker_count: self.active_worker_count.load(Ordering::SeqCst),
            remaining_serving_count: self.serving_worker_count.load(Ordering::SeqCst),
            timed_out,
        }
    }
}

/// One connection worker's registration with the drain coordinator. Dropping
/// the slot deregisters the worker; an exit after the shutdown signal counts
/// as drained.
#[derive(Debug)]
pub struct ConnectionWorkerSlot {
    coordinator: Arc<ConnectionDrainCoordinator>,
    receiver: watch::Receiver<bool>,
}

impl ConnectionWorkerSlot {
    /// True once the host has fired the shutdown signal.
    pub fn shutdown_signaled(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Resolves when the shutdown signal fires (immediately if it already
    /// has). A dropped sender means the host is gone and counts as signaled.
    pub async fn shutdown_signal(&mut self) {
        while !*self.receiver.borrow() {
            if self.receiver.changed().await.is_err() {
                return;
            }
        }
    }

    /// Marks this worker as mid-request for the guard's lifetime, so a drain
    /// timeout can report serving workers separately from parked ones.
    pub fn begin_serving(&self) -> WorkerServingGuard<'_> {
        self.coordinator
            .serving_worker_count
            .fetch_add(1, Ordering::SeqCst);
        WorkerServingGuard {
            coordinator: &self.coordinator,
        }
    }
}

impl Drop for ConnectionWorkerSlot {
    fn drop(&mut self) {
        self.coordinator
            .active_worker_count
            .fetch_sub(1, Ordering::SeqCst);
        if *self.receiver.borrow() {
            self.coordinator
                .drained_worker_count
                .fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Guard for one in-flight frame's processing window.
#[derive(Debug)]
pub struct WorkerServingGuard<'a> {
    coordinator: &'a ConnectionDrainCoordinator,
}

impl Drop for WorkerServingGuard<'_> {
    fn drop(&mut self) {
        self.coordinator
            .serving_worker_count
            .fetch_sub(1, Ordering::SeqCst);
    }
}
