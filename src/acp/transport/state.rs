//! Shared state and generation fencing for the ACP transport.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::acp::client::{AcpGenerationHandle, SharedReplay};
use crate::transports::{PartitionSink, WorkerReadinessState};

/// Slice length for the single-flight ACP prompt-completion wait. Bounds how
/// long the blocking thread parks before re-checking the shutdown gate.
pub(crate) const ACP_PROMPT_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Poll cadence for the look prime-wait.
pub(crate) const ACP_LOOK_PRIME_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Default ACP look window applied when the caller omits a window size. ACP
/// replay entries are far larger than tmux lines (each can be a full message or
/// tool invocation), so a small default keeps the response under the MCP payload
/// limit while still showing recent context.
pub(crate) const ACP_LOOK_ENTRIES_DEFAULT: usize = 50;

/// Channel capacity for the internal ACP delivery task's write queue.
pub(crate) const ACP_WRITE_CHANNEL_CAPACITY: usize = 256;

/// State shared between an [`crate::acp::transport::AcpTransport`] and the
/// [`crate::transports::OutputView`] handle it publishes. Held behind an `Arc`
/// so the handle stays valid across the transport's whole life — including the
/// initial-startup and respawn windows when there is no live runtime yet.
pub(crate) struct AcpSharedState {
    pub(crate) readiness: Mutex<WorkerReadinessState>,
    pub(crate) replay: Mutex<Option<SharedReplay>>,
    /// Mirrors per-turn readiness transitions into the relay global registry.
    pub(crate) mirror_state: Option<ReadinessMirror>,
    /// The relay's guard, for reporting which members share one `session/prompt`.
    pub(crate) partition_sink: Arc<dyn PartitionSink>,
    /// Handles for the permission resolver threads this generation has spawned.
    pub(crate) permission_executors: Mutex<Vec<JoinHandle<()>>>,
}

impl AcpSharedState {
    /// Records a permission resolver's handle, dropping any that have already
    /// finished so a long-lived generation does not accumulate one per decision
    /// it ever made. Only live executors need observing.
    pub(crate) fn note_permission_executor(&self, handle: JoinHandle<()>) {
        let mut executors = self
            .permission_executors
            .lock()
            .expect("permission executors mutex");
        executors.retain(|handle| !handle.is_finished());
        executors.push(handle);
    }

    pub(crate) fn permission_executors_ceased(&self) -> bool {
        self.permission_executors
            .lock()
            .map(|executors| executors.iter().all(JoinHandle::is_finished))
            .unwrap_or(false)
    }
}

/// Mirrors a per-turn readiness transition into the relay's global worker-state
/// registry. Injected by the `AcpWorkerDriver` (structurally identical to its
/// `MirrorStateFn`), so the internal delivery task mirrors its own Busy/settled
/// transitions and the relay worker no longer drives `mark_busy` /
/// `mirror_settled_readiness`.
pub(crate) type ReadinessMirror = Arc<dyn Fn(WorkerReadinessState) + Send + Sync>;

/// The in-flight bootstraps of one generation, and whether that generation has
/// been told to end.
#[derive(Debug, Default)]
pub(crate) struct BootstrapRegistry {
    pub(crate) records: Mutex<Vec<BootstrapRecord>>,
    /// Set once and never cleared: a generation told to end does not resume.
    pub(crate) terminating: AtomicBool,
}

impl BootstrapRegistry {
    pub(crate) fn records(&self) -> std::sync::MutexGuard<'_, Vec<BootstrapRecord>> {
        self.records.lock().expect("bootstrap registry mutex")
    }

    pub(crate) fn is_terminating(&self) -> bool {
        self.terminating.load(Ordering::Acquire)
    }

    /// Latches termination and makes one non-blocking pass over *every* record.
    pub(crate) fn initiate_termination(&self) {
        self.terminating.store(true, Ordering::Release);
        if let Ok(records) = self.records.try_lock() {
            for record in records.iter() {
                if let Some(generation) = record.generation.as_ref() {
                    generation.initiate_termination();
                }
            }
        }
    }

    /// Called by every mutating holder of the records lock, after releasing it.
    pub(crate) fn serve_pending_termination(&self) {
        if self.is_terminating() {
            self.initiate_termination();
        }
    }
}

/// One in-flight bootstrap: its guard's identity, and the agent child it owns
/// once the spawn has happened.
#[derive(Debug)]
pub(crate) struct BootstrapRecord {
    pub(crate) id: u64,
    pub(crate) generation: Option<AcpGenerationHandle>,
}

pub(crate) static NEXT_BOOTSTRAP_ID: AtomicU64 = AtomicU64::new(1);

/// Publishes a fresh respawn cause on the shared signal.
pub(crate) fn raise_respawn_signal(sender: &tokio::sync::watch::Sender<u64>) {
    sender.send_modify(|epoch| *epoch += 1);
}

/// Marks a bootstrap as running for as long as it is held, and is how that
/// bootstrap hands its agent child to the fence.
#[derive(Debug)]
pub(crate) struct BootstrapInFlight {
    pub(crate) id: u64,
    pub(crate) bootstraps: Arc<BootstrapRegistry>,
}

impl BootstrapInFlight {
    /// Publishes the agent child this bootstrap owns, making it reachable by the
    /// fence's forced step for as long as this guard lives.
    pub(crate) fn publish_generation(&self, generation: AcpGenerationHandle) {
        {
            let mut records = self.bootstraps.records();
            if let Some(record) = records.iter_mut().find(|record| record.id == self.id) {
                record.generation = Some(generation);
            }
        }
        self.bootstraps.serve_pending_termination();
    }
}

impl Drop for BootstrapInFlight {
    fn drop(&mut self) {
        self.bootstraps
            .records()
            .retain(|record| record.id != self.id);
        self.bootstraps.serve_pending_termination();
    }
}
