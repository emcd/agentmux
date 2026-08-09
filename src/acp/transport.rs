//! ACP delivery as a [`Transport`] implementation.
//!
//! `AcpTransport` owns the per-target `PersistentAcpWorkerRuntime` (moved here
//! from the relay delivery worker, which previously threaded it through
//! `spawn_blocking`). [`Transport::mailw`] hands a structured delivery message
//! to the internal delivery task, which renders each message into pane-envelope
//! text, combines a contiguous group into one ACP turn under the token budget,
//! and resolves the future for each contributing task.
//!
//! The framed `session/prompt` write is the delivery boundary: every member of
//! the group resolves `Delivered` immediately when the write succeeds, before
//! replay-buffer locks or `on_dispatched` run. Active-prompt refusal and
//! serialization failure resolve `not_submitted`; a write or flush error that
//! cannot prove zero bytes left resolves `submission_unknown`. The turn's later
//! completion, permission requests, or connection close are target-health
//! observability — they drive readiness and the respawn signal, never a second
//! delivery outcome for an already-resolved member.
//!
//! Choices (tool-call permissions) resolve through the relay-injected
//! [`Chooser`] (see [`crate::acp::permission`]); the transport never calls the
//! relay choice queue directly. The `look` path reads output through the
//! [`OutputView`] handle published by [`Transport::give_output`].
//!
//! ## Readiness
//!
//! The transport owns an [`WorkerReadinessState`] signal for
//! [`is_ready_for_handover`] and the [`OutputView`] prime-wait, because it
//! cannot call relay's `set_worker_readiness`. The `AcpWorkerDriver` mirrors
//! transitions into the global worker-state registry (which external observers
//! and respawn/startup gating still read).
//!
//! Handover readiness is the narrow question: only `Available` qualifies, since
//! a `Busy` worker is mid-turn and cannot take another. The wider
//! "runtime exists" reading that `Busy` also satisfies is what the mirrored
//! registry state carries for those other observers.
//!
//! [`is_ready_for_handover`]: Transport::is_ready_for_handover

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::acp::client::{AcpGenerationHandle, SharedReplay};
use crate::acp::permission::{ChoiceCorrelation, build_acp_permission_handler};
use crate::acp::persistent_runtime::PersistentAcpWorkerRuntime;
use crate::acp::state::{AcpLookSnapshot, derive_acp_look_snapshot};
use crate::acp::{
    AcpStdioClient, DispatchHandler, PermissionHandler, PermissionResponder, PromptCompletion,
    PromptCompletionHandler, PromptDispatchOutcome,
};

use crate::envelope::PromptBatchSettings;
use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    ChoiceMade, DeliveryEnvelope, GenerationFence, LookMode, LookSnapshotPayload, OutputView,
    SingleDeliveryOutcome, StartupContext, Transport, TransportError, TransportHealth,
    TransportStatus,
};
use crate::transports::{SendOutcome, WorkerReadinessState};

// ACP delivery failure taxonomy (see the relay delivery README for the full
// catalogue). The delivery outcomes the ACP transport now produces are typed:
// `Delivered` (framed write succeeded), `NotSubmitted` (positive non-delivery:
// active-prompt refusal or serialization failure), and `SubmissionUnknown`
// (a write/flush error without proof that zero bytes left). Connection close
// after a successful write is target-health observability, not a delivery
// outcome.
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";

/// Slice length for the single-flight ACP prompt-completion wait. Bounds how
/// long the blocking thread parks before re-checking the shutdown gate.
const ACP_PROMPT_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Poll cadence for the look prime-wait.
const ACP_LOOK_PRIME_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Default ACP look window applied when the caller omits a window size. ACP
/// replay entries are far larger than tmux lines (each can be a full message or
/// tool invocation), so a small default keeps the response under the MCP payload
/// limit while still showing recent context.
const ACP_LOOK_ENTRIES_DEFAULT: usize = 50;

/// State shared between an [`AcpTransport`] and the [`OutputView`] handle it
/// publishes. Held behind an `Arc` so the handle stays valid across the
/// transport's whole life — including the initial-startup and respawn windows
/// when there is no live runtime yet. The transport repoints `replay` at the
/// current runtime's buffer on every successful startup; the handle reads
/// whichever buffer is current (or `None`) plus the readiness that drives its
/// bounded prime-wait. This is what lets `look` actually wait through startup.
struct AcpSharedState {
    readiness: Mutex<WorkerReadinessState>,
    replay: Mutex<Option<SharedReplay>>,
    /// Mirrors per-turn readiness transitions into the relay global registry.
    /// Travels with the readiness it mirrors so both the internal delivery task
    /// and the `on_dispatched` closure reach it through the shared `Arc`. `None`
    /// in tests constructed without a relay registry.
    mirror_state: Option<ReadinessMirror>,
    /// Handles for the permission resolver threads this generation has spawned.
    ///
    /// Retained rather than detached: a permission resolver blocks on an
    /// operator decision and can outlive the turn that raised it, so an executor
    /// whose handle was dropped would be invisible to cessation observation —
    /// and a generation cannot be fenced on executors it cannot see.
    permission_executors: Mutex<Vec<JoinHandle<()>>>,
}

impl AcpSharedState {
    /// Records a permission resolver's handle, dropping any that have already
    /// finished so a long-lived generation does not accumulate one per decision
    /// it ever made. Only live executors need observing.
    fn note_permission_executor(&self, handle: JoinHandle<()>) {
        let mut executors = self
            .permission_executors
            .lock()
            .expect("permission executors mutex");
        executors.retain(|handle| !handle.is_finished());
        executors.push(handle);
    }

    fn permission_executors_ceased(&self) -> bool {
        self.permission_executors
            .lock()
            .map(|executors| executors.iter().all(JoinHandle::is_finished))
            .unwrap_or(false)
    }
}

/// Channel capacity for the internal ACP delivery task's write queue.
const ACP_WRITE_CHANNEL_CAPACITY: usize = 256;

/// Mirrors a per-turn readiness transition into the relay's global worker-state
/// registry. Injected by the `AcpWorkerDriver` (structurally identical to its
/// `MirrorStateFn`), so the internal delivery task mirrors its own Busy/settled
/// transitions and the relay worker no longer drives `mark_busy` /
/// `mirror_settled_readiness`. `None` in tests that construct the transport
/// without a relay registry.
type ReadinessMirror = Arc<dyn Fn(WorkerReadinessState) + Send + Sync>;

/// Items enqueued onto the ACP transport's internal ordered write channel.
///
/// Both [`Transport::mailw`] and [`Transport::raww`] submit through a single
/// FIFO channel. The internal delivery task processes them in order; a `Raw`
/// item acts as a batch barrier (flushes any preceding `Envelope` group first).
enum WriteItem {
    /// Structured delivery message for buffered combining and turn submission.
    /// Boxed to keep the channel item small (the message carries full
    /// attribution), so the `Raw` variant does not inflate every queued item.
    Envelope {
        envelope: Box<DeliveryEnvelope>,
        outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    },
    /// Raw input delivered without buffering; acts as a batch barrier.
    Raw {
        content: String,
        append_enter: bool,
        outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    },
}

/// ACP delivery transport. Owns the runtime, the injected [`Chooser`], and the
/// shared state ([`AcpSharedState`]) the published [`OutputView`] reads.
///
/// The transport's internal delivery task receives [`WriteItem`]s through an
/// ordered channel, combines contiguous envelopes respecting the token budget,
/// and submits turns to the ACP runtime. `mailw`/`raww` enqueue items and return
/// [`OutcomeFuture`]s that resolve when the turn settles.
pub struct AcpTransport {
    runtime: Option<PersistentAcpWorkerRuntime>,
    chooser: Option<crate::transports::Chooser>,
    shared: Arc<AcpSharedState>,
    /// Sender for the internal delivery task's write queue. `None` before first
    /// startup or after `release_runtime()`.
    write_tx: Option<mpsc::Sender<WriteItem>>,
    /// Shutdown signal to the delivery task. Dropping this signals the task to
    /// drain pending items and exit. `None` before first startup.
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Prompt-batch settings (token budget and tokenizer profile) for envelope
    /// combining.
    batch_settings: PromptBatchSettings,
    /// Target session id, captured at startup for permission correlation.
    target_session: String,
    /// Stable respawn-needed signal, as a monotonically increasing epoch.
    ///
    /// Created once at construction (not per delivery task) so the driver-owned
    /// async respawn monitor can hold a single long-lived subscription across
    /// respawns. Driven exclusively from the transport (bootstrap failure,
    /// write-without-runtime paths); readiness latches no longer promote
    /// themselves into respawn signals.
    ///
    /// An epoch rather than a flag because a flag cannot name *which* failure
    /// asked for the respawn, and every consumer of this signal needs that. A
    /// bare `bool` makes classification and retirement two operations on the
    /// same indistinguishable value: the monitor decides an outstanding signal
    /// has been answered, and by the time it writes that decision down, the
    /// value may describe a different, live failure that the write would erase.
    /// The counter never resets, so an epoch identifies its cause for the life
    /// of the transport and a decision made about one can never land on
    /// another. Retirement is a high-water mark
    /// ([`respawn_retired`](Self::respawn_retired)), not a reset.
    respawn_needed_tx: tokio::sync::watch::Sender<u64>,
    /// Highest respawn epoch the monitor has finished with.
    ///
    /// The signal is outstanding while the raised epoch exceeds this. Retiring
    /// records a bound rather than clearing a flag, which is what makes
    /// retirement safe against a concurrent raise: a cause published after the
    /// monitor sampled its epoch carries a strictly greater one, so it stays
    /// outstanding no matter when the retirement lands.
    respawn_retired: AtomicU64,
    /// `JoinHandle` for the most recent delivery task thread. Retained so a
    /// generation supervisor can observe the task's cessation (the binding the
    /// fence requires) and detach it cleanly on `take`. The handle is replaced
    /// when `spawn_delivery_task` re-spawns, leaving the previous task's thread
    /// to exit on its own; only `take` clears the field.
    delivery_task_handle: Option<JoinHandle<()>>,
    /// Fencing surface of the generation whose client the delivery task owns.
    ///
    /// Taken from the client at the moment it is moved into that task, because
    /// `self.runtime` is emptied by the same move: reading termination and
    /// cessation off the runtime made step 3 a no-op and made the reader
    /// observation vacuously true, since both looked at a field that is `None`
    /// for the whole steady state.
    generation: Option<AcpGenerationHandle>,
    /// Whether this generation has been fenced.
    ///
    /// One-way, and read by everything that could otherwise install a
    /// replacement runtime behind the fence's back. The respawn monitor runs
    /// asynchronously and can be mid-bootstrap when a fence begins, so the
    /// decision to install has to be taken against this flag under the same lock
    /// as the install itself.
    fenced: Arc<AtomicBool>,
    /// The bootstraps this generation currently has running, and the agent child
    /// each one owns.
    ///
    /// A bootstrap spawns and owns an agent child, so it is a generation-owned
    /// executor — but it runs on a blocking pool, and aborting the async task
    /// awaiting it does not cancel the closure. Observing the wrapper therefore
    /// says nothing about whether the executor stopped, and it offers the fence
    /// nothing to signal either; the child is what both steps have to reach.
    ///
    /// A list rather than a single slot, for the same reason this was a count
    /// before it carried handles: an initial bootstrap and a respawn can overlap,
    /// and neither may clear or terminate the other's state.
    bootstraps: Arc<BootstrapRegistry>,
}

/// The in-flight bootstraps of one generation, and whether that generation has
/// been told to end.
///
/// The latch belongs here rather than beside the traversal because a bootstrap's
/// child is published from inside the closure that spawned it, at a moment
/// nothing coordinates with the fence. Reading the list once and signalling what
/// it happened to hold made termination a *moment*: a child published a
/// microsecond later was never signalled at all, and the closure went on to park
/// in a handshake with a live agent behind it. As a latched state, the ordering
/// stops mattering — publication either finds it already set, or is found by the
/// traversal.
#[derive(Debug, Default)]
struct BootstrapRegistry {
    records: Mutex<Vec<BootstrapRecord>>,
    /// Set once and never cleared: a generation told to end does not resume.
    terminating: AtomicBool,
}

impl BootstrapRegistry {
    fn records(&self) -> std::sync::MutexGuard<'_, Vec<BootstrapRecord>> {
        self.records.lock().expect("bootstrap registry mutex")
    }

    fn is_terminating(&self) -> bool {
        self.terminating.load(Ordering::Acquire)
    }

    /// Latches termination and makes one non-blocking pass over *every* record.
    ///
    /// Idempotent, and registry-wide on purpose. A per-record handoff is not
    /// enough here: this registry exists because an initial bootstrap and a
    /// respawn can overlap, so the holder that defeats a traversal attempt is
    /// generally not the owner of the records that attempt would have reached.
    /// A publisher that served only itself left every other live bootstrap
    /// unsignalled with nothing scheduled to look again.
    fn initiate_termination(&self) {
        self.terminating.store(true, Ordering::Release);
        // Attempted, never taken: step 3 is contracted to block nowhere. What
        // this cannot reach is reached by whoever is holding the lock, when they
        // release it.
        if let Ok(records) = self.records.try_lock() {
            for record in records.iter() {
                if let Some(generation) = record.generation.as_ref() {
                    generation.initiate_termination();
                }
            }
        }
    }

    /// Called by every mutating holder of the records lock, after releasing it.
    ///
    /// This is the other half of the handoff. Reading the latch only while
    /// holding the lock leaves the window where a requester latches, loses its
    /// attempt to this holder, and the holder then releases without looking
    /// again — so the read happens here, after release, and it re-runs the whole
    /// traversal rather than serving one record.
    fn serve_pending_termination(&self) {
        if self.is_terminating() {
            self.initiate_termination();
        }
    }
}

/// One in-flight bootstrap: its guard's identity, and the agent child it owns
/// once the spawn has happened.
#[derive(Debug)]
struct BootstrapRecord {
    id: u64,
    generation: Option<AcpGenerationHandle>,
}

static NEXT_BOOTSTRAP_ID: AtomicU64 = AtomicU64::new(1);

/// Publishes a fresh respawn cause on the shared signal.
///
/// The delivery paths hold a cloned sender rather than the transport, so raising
/// cannot go through `&self`. Incrementing under `send_modify` keeps epochs
/// unique per cause even when two delivery threads conclude at once: the watch's
/// own lock serializes the read-modify-write, so neither can observe the other's
/// value and reuse it.
pub(crate) fn raise_respawn_signal(sender: &tokio::sync::watch::Sender<u64>) {
    sender.send_modify(|epoch| *epoch += 1);
}

/// Marks a bootstrap as running for as long as it is held, and is how that
/// bootstrap hands its agent child to the fence.
///
/// Moved into the blocking closure itself, not held beside it: the closure
/// outlives any abort of the task awaiting it, so only something dropped by the
/// closure can say when that executor actually stopped.
#[derive(Debug)]
pub(crate) struct BootstrapInFlight {
    id: u64,
    bootstraps: Arc<BootstrapRegistry>,
}

impl BootstrapInFlight {
    /// Publishes the agent child this bootstrap owns, making it reachable by the
    /// fence's forced step for as long as this guard lives — and ends it here if
    /// the forced step has already gone past.
    ///
    /// The handoff after the release is registry-wide, not this record alone: a
    /// traversal that lost its attempt to this holder was trying to reach every
    /// live bootstrap, and serving only the one published here would leave the
    /// rest of them alive with nothing scheduled to look again.
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
        // A dropping guard holds the same lock a traversal attempt can lose to,
        // so it owes the same handoff a publisher does.
        self.bootstraps.serve_pending_termination();
    }
}

impl std::fmt::Debug for AcpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpTransport")
            .field("has_runtime", &self.runtime.is_some())
            .field("readiness", &self.readiness())
            .field("has_write_channel", &self.write_tx.is_some())
            .field("batch_settings", &self.batch_settings)
            .finish()
    }
}

impl AcpTransport {
    #[must_use]
    pub fn new(batch_settings: PromptBatchSettings, mirror_state: Option<ReadinessMirror>) -> Self {
        Self {
            runtime: None,
            chooser: None,
            shared: Arc::new(AcpSharedState {
                readiness: Mutex::new(WorkerReadinessState::Initializing),
                replay: Mutex::new(None),
                mirror_state,
                permission_executors: Mutex::new(Vec::new()),
            }),
            write_tx: None,
            shutdown_tx: None,
            batch_settings,
            target_session: String::new(),
            respawn_needed_tx: tokio::sync::watch::channel(0).0,
            respawn_retired: AtomicU64::new(0),
            delivery_task_handle: None,
            generation: None,
            fenced: Arc::new(AtomicBool::new(false)),
            bootstraps: Arc::new(BootstrapRegistry::default()),
        }
    }

    /// Subscribes to the stable respawn-needed signal. The driver-owned respawn
    /// monitor holds one subscription for the transport's whole life.
    pub fn respawn_needed_subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.respawn_needed_tx.subscribe()
    }

    /// The outstanding respawn cause's epoch, if any is still owed an answer.
    #[must_use]
    pub fn respawn_signal_outstanding(&self) -> Option<u64> {
        let raised = *self.respawn_needed_tx.borrow();
        (raised > self.respawn_retired.load(Ordering::Acquire)).then_some(raised)
    }

    /// Marks every respawn cause up to and including `epoch` as answered, and
    /// reports whether any signal remains outstanding afterwards.
    ///
    /// Retirement is a high-water mark rather than a reset, which is the whole
    /// point: a caller retires the epoch it *classified*, and a cause published
    /// between that classification and this call carries a strictly greater
    /// epoch. `fetch_max` therefore cannot erase it — the newer cause is still
    /// outstanding when this returns, and the caller learns so from the return
    /// value. Resetting a flag instead loses that cause silently, and because
    /// the readiness gate withholds the very writes that would raise it again,
    /// losing one is not a delayed recovery but a permanent one.
    ///
    /// `fetch_max` also makes the operation idempotent and order-independent,
    /// so a retirement that arrives late can never move the mark backwards.
    pub fn retire_respawn_signal(&self, epoch: u64) -> Option<u64> {
        self.respawn_retired.fetch_max(epoch, Ordering::AcqRel);
        self.respawn_signal_outstanding()
    }

    /// Publishes a fresh respawn cause. Used after an initial-bootstrap failure
    /// so the driver's respawn monitor retries with backoff, and by the delivery
    /// paths whose outcome warrants replacing the runtime.
    pub fn signal_respawn(&self) {
        raise_respawn_signal(&self.respawn_needed_tx);
    }

    /// Takes and clears the most recent delivery task's [`JoinHandle`].
    /// A generation supervisor binds the handle so it can join the thread
    /// and observe cessation as part of fence acknowledgment. Returns `None`
    /// when the transport has never spawned a delivery task or the handle
    /// has already been taken for the current generation.
    pub fn take_delivery_task_handle(&mut self) -> Option<JoinHandle<()>> {
        self.delivery_task_handle.take()
    }

    /// Current readiness, mirrored by the `AcpWorkerDriver` into the global registry.
    #[must_use]
    pub fn readiness(&self) -> WorkerReadinessState {
        *self.shared.readiness.lock().expect("readiness mutex")
    }

    fn set_readiness(&self, state: WorkerReadinessState) {
        *self.shared.readiness.lock().expect("readiness mutex") = state;
    }

    fn set_replay(&self, replay: Option<SharedReplay>) {
        *self.shared.replay.lock().expect("replay slot mutex") = replay;
    }

    /// Releases the live runtime (joining its child) and marks the transport
    /// recovering, clearing the published replay pointer. Closes the write
    /// channel so the internal delivery task drains pending items and exits
    /// cleanly before the runtime is dropped. Used by the worker before a
    /// respawn so a concurrent `look` reads a recovering/stale snapshot through
    /// the still-valid handle rather than the dead buffer.
    pub fn release_runtime(&mut self) {
        // Drop the shutdown signal first, then the write channel. The delivery
        // task detects the shutdown signal, drains any remaining write items,
        // and resolves their outcome senders with DroppedOnShutdown before exiting.
        self.shutdown_tx = None;
        self.write_tx = None;
        self.runtime = None;
        self.set_replay(None);
        self.set_readiness(WorkerReadinessState::Recovering);
    }

    /// Spawns the internal delivery task that drains the write channel, combines
    /// contiguous envelopes respecting the token budget, and submits turns to the
    /// ACP runtime. Called from [`Transport::startup`] after the runtime is
    /// established. Takes the client and session_id from the runtime so the task
    /// owns them exclusively — the transport only needs the shared replay handle
    /// and readiness state after startup. The task's [`JoinHandle`] is retained
    /// on the transport so a generation supervisor can join it and observe
    /// cessation as the fence requires; the previous generation's handle, if
    /// any, is replaced — its thread is left to exit on its own.
    fn spawn_delivery_task(&mut self) {
        let (tx, rx) = mpsc::channel::<WriteItem>(ACP_WRITE_CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let respawn_needed_tx = self.respawn_needed_tx.clone();
        let shared = Arc::clone(&self.shared);
        let batch_settings = self.batch_settings;
        let chooser = self.chooser.clone();
        let identity = DeliveryTaskIdentity {
            target_session: self.target_session.clone(),
        };

        let runtime = self.runtime.take().expect("runtime present at task spawn");
        // Before the move, not after: this is the last point at which the
        // transport can still reach the client it is about to hand away.
        self.generation = Some(runtime.client.generation_handle());
        let client = runtime.client;
        let session_id = runtime.session_id;

        let handle = thread::Builder::new()
            .name("agentmux-acp-delivery".into())
            .spawn(move || {
                let channels = DeliveryChannels {
                    rx,
                    shutdown_rx,
                    respawn_needed_tx,
                };
                acp_delivery_task(
                    channels,
                    client,
                    session_id,
                    shared,
                    chooser,
                    batch_settings,
                    identity,
                );
            })
            .expect("spawn ACP delivery task thread");

        self.delivery_task_handle = Some(handle);
        self.write_tx = Some(tx);
        self.shutdown_tx = Some(shutdown_tx);
    }

    /// Sets the chooser/target identity and clears any prior delivery channel
    /// ahead of (re-)establishing the runtime. Brief and lock-safe: it holds no
    /// blocking work, so the driver-owned respawn monitor can call it under the
    /// transport lock without stalling a concurrent `mailw`. Readiness is the
    /// caller's responsibility (initial bootstrap marks Initializing; respawn
    /// leaves the released Recovering state in place).
    pub(crate) fn prepare_for_startup(
        &mut self,
        chooser: crate::transports::Chooser,
        target_session: String,
    ) {
        self.chooser = Some(chooser);
        self.target_session = target_session;
        // Close any existing delivery task's channel before creating a new
        // runtime; the old task drains and exits.
        self.write_tx = None;
    }

    /// Registers a bootstrap as running until the returned guard drops.
    ///
    /// A bootstrap begun behind an already-terminating generation is registered
    /// like any other rather than refused: it inherits the latch, so the child it
    /// is about to spawn is ended at publication. Refusing here instead would
    /// leave the caller holding no guard and the fence with nothing counting it.
    #[must_use]
    pub(crate) fn begin_bootstrap(&self) -> BootstrapInFlight {
        let id = NEXT_BOOTSTRAP_ID.fetch_add(1, Ordering::Relaxed);
        {
            self.bootstraps.records().push(BootstrapRecord {
                id,
                generation: None,
            });
        }
        // The third registry writer, and it owes the same handoff as the other
        // two. Registration holds the lock a traversal attempt can lose to, and
        // this holder's own record is empty — so serving only itself would serve
        // nothing, while every already-published bootstrap waited on a pass that
        // would not happen until this one reached publication.
        self.bootstraps.serve_pending_termination();
        BootstrapInFlight {
            id,
            bootstraps: Arc::clone(&self.bootstraps),
        }
    }

    /// Whether this generation has been fenced.
    #[must_use]
    pub(crate) fn generation_is_fenced(&self) -> bool {
        self.fenced.load(Ordering::Acquire)
    }

    /// Installs `runtime` unless this generation has been fenced, in which case
    /// it is handed back for the caller to tear down.
    ///
    /// The check and the install happen under one lock on purpose. A bootstrap
    /// runs off the lock and takes seconds, so a fence can begin while one is in
    /// flight; deciding separately from installing would leave the window where
    /// a replacement runtime is installed into a generation already declared
    /// stopped — a second live agent for one target, which is exactly what the
    /// fence exists to prevent.
    #[must_use = "a refused runtime owns a live child and must be shut down"]
    pub(crate) fn install_runtime_unless_fenced(
        &mut self,
        runtime: PersistentAcpWorkerRuntime,
    ) -> Option<PersistentAcpWorkerRuntime> {
        if self.generation_is_fenced() {
            return Some(runtime);
        }
        self.install_runtime(runtime);
        None
    }

    /// Installs a freshly bootstrapped runtime: repoints the published replay
    /// handle at the new buffer, marks the transport Available, and spawns the
    /// internal delivery task. Brief and lock-safe — the blocking child spawn
    /// already happened in `bootstrap_acp_worker_runtime`, so a bootstrap holds
    /// the transport lock only for these fast field updates.
    ///
    /// Private because the fenced check and the install belong together; every
    /// caller goes through [`install_runtime_unless_fenced`].
    ///
    /// [`install_runtime_unless_fenced`]: Self::install_runtime_unless_fenced
    fn install_runtime(&mut self, runtime: PersistentAcpWorkerRuntime) {
        // Repoint the published handle's replay slot at the new runtime's buffer
        // before marking ready, so a look that was prime-waiting through the
        // (re-)establish returns the fresh buffer.
        self.set_replay(Some(runtime.client.replay_buffer_handle()));
        self.runtime = Some(runtime);
        self.set_readiness(WorkerReadinessState::Available);
        self.spawn_delivery_task();
    }

    /// Marks the transport Unavailable with no live runtime (initial-bootstrap
    /// failure or permanent respawn give-up).
    pub(crate) fn mark_runtime_unavailable(&mut self) {
        self.runtime = None;
        self.set_replay(None);
        self.set_readiness(WorkerReadinessState::Unavailable);
    }

    /// Installs a write channel for unit tests, so `mailw`/`raww` can reach
    /// their enqueue-refusal paths without a live delivery task.
    ///
    /// Sets the transport `Available` (the precondition `mailw`/`raww` check
    /// before `try_send`) and points `write_tx` at a fresh single-slot channel
    /// (the smallest capacity that both refusal classes need). When `prefill`
    /// is set, one raw item occupies the slot so the next `try_send` returns
    /// `Full`. Returns a guard owning the channel's receiver: retaining it
    /// keeps the channel open, dropping it closes the channel (the next
    /// `try_send` returns `Closed`). The receiver is dropped normally with the
    /// guard — the test never leaks it.
    ///
    /// This mirrors the relay's `_for_testing` export convention
    /// (e.g. `second_claim_is_live_conflict_for_testing`): a `#[doc(hidden)]`
    /// public seam so `tests/unit` can drive the public transport interface
    /// without widening production API surface.
    #[doc(hidden)]
    #[must_use]
    pub fn install_write_channel_for_testing(&mut self, prefill: bool) -> WriteChannelGuard {
        self.set_readiness(WorkerReadinessState::Available);
        let (tx, rx) = mpsc::channel::<WriteItem>(1);
        if prefill {
            tx.try_send(WriteItem::Raw {
                content: "filler".to_string(),
                append_enter: true,
                outcome_tx: tokio::sync::oneshot::channel().0,
            })
            .expect("prefill the single-slot write channel");
        }
        self.write_tx = Some(tx);
        WriteChannelGuard { rx }
    }
}

/// Guard owning the write-channel receiver installed by
/// [`AcpTransport::install_write_channel_for_testing`].
///
/// Retaining it keeps the channel open (a `prefill`ed channel stays
/// saturated, so the next `try_send` returns `Full`); dropping it closes the
/// channel (the next `try_send` returns `Closed`). Dropping the guard also
/// drops the receiver it owns, so the test never leaks the channel.
#[doc(hidden)]
pub struct WriteChannelGuard {
    // Intentionally never read: the guard exists so the receiver is dropped
    // with it, closing the channel at the end of the test's scope.
    #[expect(dead_code)]
    rx: mpsc::Receiver<WriteItem>,
}

impl GenerationFence for AcpTransport {
    fn fence_generation(&mut self) {
        // Dropping the shutdown sender is the delivery task's cooperative stop
        // signal: it drains what it holds and exits at its next check. Marking
        // the generation fenced is the same request to the respawn monitor, and
        // it is what makes a bootstrap already in flight refuse to install.
        self.fenced.store(true, Ordering::Release);
        self.shutdown_tx = None;
    }

    fn terminate_generation(&mut self) {
        // Signal the child and return. This unblocks an executor parked writing
        // into the child's stdin, which is exactly the case step 1 cannot reach.
        // Reaping happens in the observation that follows, not here.
        if let Some(generation) = self.generation.as_ref() {
            generation.initiate_termination();
        }
        // Every bootstrap still running owns an agent child of its own, and a
        // bootstrap is held where it is by that child failing to answer. Killing
        // it closes the stdio the handshake is waiting on, so the request fails,
        // the client drops, and the executor returns — where before, step 3 had
        // nothing to say to it and the fence could only wait out an operation
        // timeout it does not control.
        //
        // Latches and attempts one registry-wide pass, blocking nowhere. A
        // record this attempt cannot see, or cannot reach because someone holds
        // the lock, is reached when that holder releases and runs the same pass.
        self.bootstraps.initiate_termination();
        self.write_tx = None;
    }

    fn generation_ceased(&self) -> bool {
        let delivery_task_ceased = self
            .delivery_task_handle
            .as_ref()
            .is_none_or(JoinHandle::is_finished);
        let client_ceased = self
            .generation
            .as_ref()
            .is_none_or(AcpGenerationHandle::reader_ceased);
        // A record outlives its bootstrap's disposal of the child it owns. On
        // every path that produced no live runtime the client has been dropped
        // by the time the guard goes, and dropping it kills *and waits* the
        // child; on the path that succeeded, that child has become this
        // generation's steady-state one, which the conjunct above observes.
        // Either way an empty registry means reaped or accounted for, never
        // merely signalled.
        let no_bootstrap_running = self.bootstraps.records().is_empty();
        delivery_task_ceased
            && client_ceased
            && no_bootstrap_running
            && self.shared.permission_executors_ceased()
    }
}

impl Transport for AcpTransport {
    /// ACP establishes its runtime through the driver's supervised bootstrap, not
    /// here.
    ///
    /// This used to bootstrap synchronously on the caller's thread: a second
    /// route that spawned and owned an agent child while being counted by
    /// nothing, so a fence could see an empty in-flight count with a live child
    /// coming up behind it. Every production ACP path goes through
    /// [`AcpWorkerDriver::start_bootstrap`], so the route is gone rather than
    /// duplicated under the supervisor.
    fn startup(&mut self, _context: StartupContext) -> Result<TransportStatus, TransportError> {
        Err(TransportError {
            code: "internal_unexpected_failure".to_string(),
            reason: "ACP runtimes are established by the worker driver's supervised bootstrap"
                .to_string(),
            details: None,
        })
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        // An authorized batch starts a supervised submission executor
        // synchronously or is refused synchronously — it must never wait in the
        // transport's staging channel behind an in-flight turn. The relay only
        // authorizes when this transport reports Available, so a not-ready
        // reading here is the stale-readiness case: refuse before partition with
        // `not_submitted` (positive evidence nothing was written).
        if !self.is_ready_for_handover() {
            let _ = outcome_tx.send(not_submitted_outcome(
                self.target_session.clone(),
                envelope.message_id.clone(),
                "ACP transport is not ready to start a submission",
            ));
            return outcome_rx;
        }
        let Some(tx) = self.write_tx.as_ref() else {
            // No live delivery task: refusal before partition — nothing was
            // written, so the member resolves `not_submitted`.
            let _ = outcome_tx.send(not_submitted_outcome(
                self.target_session.clone(),
                envelope.message_id.clone(),
                "ACP transport has no runtime",
            ));
            return outcome_rx;
        };
        // Accept synchronously: mark Busy so the relay's next authorization
        // check holds the following batch in `Pending` instead of enqueueing it
        // behind this turn — the staging queue this contract removes.
        set_shared_readiness(&self.shared, WorkerReadinessState::Busy);
        if let Err(error) = tx.try_send(WriteItem::Envelope {
            envelope: Box::new(envelope),
            outcome_tx,
        }) {
            // The write channel refused the item. Classify the refusal so the
            // post-refusal readiness is truthful: a Closed channel means the
            // delivery task has exited (its receiver is gone), so Busy must not
            // linger masking a dead executor — publish Unavailable. A Full
            // channel means the delivery task is alive but saturated; Busy
            // stays truthful until the task drains and settles Available. The
            // rejected item is the Envelope we just submitted; mailw never
            // enqueues a Raw.
            let channel_closed = matches!(&error, mpsc::error::TrySendError::Closed(_));
            if channel_closed {
                set_shared_readiness(&self.shared, WorkerReadinessState::Unavailable);
            }
            let WriteItem::Envelope {
                outcome_tx,
                envelope,
            } = error.into_inner()
            else {
                unreachable!("mailw only enqueues Envelope write items");
            };
            let _ = outcome_tx.send(not_submitted_outcome(
                self.target_session.clone(),
                envelope.message_id.clone(),
                if channel_closed {
                    "ACP write channel closed (delivery task exited)"
                } else {
                    "ACP write channel full (delivery task saturated)"
                },
            ));
        }
        outcome_rx
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        if !self.is_ready_for_handover() {
            let _ = outcome_tx.send(not_submitted_outcome(
                self.target_session.clone(),
                String::new(),
                "ACP transport is not ready to start a submission",
            ));
            return outcome_rx;
        }
        let Some(tx) = self.write_tx.as_ref() else {
            let _ = outcome_tx.send(not_submitted_outcome(
                self.target_session.clone(),
                String::new(),
                "ACP transport has no runtime",
            ));
            return outcome_rx;
        };
        // Accept synchronously: mark Busy so the relay holds the next batch
        // rather than enqueuing it behind this turn.
        set_shared_readiness(&self.shared, WorkerReadinessState::Busy);
        if let Err(error) = tx.try_send(WriteItem::Raw {
            content,
            append_enter,
            outcome_tx,
        }) {
            // Classify the refusal so the post-refusal readiness is truthful: a
            // Closed channel means the delivery task has exited, so Busy must
            // not linger masking a dead executor — publish Unavailable. A Full
            // channel means the delivery task is alive but saturated; Busy
            // stays truthful until the task drains. The rejected item is the
            // Raw we just submitted; raww never enqueues an Envelope.
            let channel_closed = matches!(&error, mpsc::error::TrySendError::Closed(_));
            if channel_closed {
                set_shared_readiness(&self.shared, WorkerReadinessState::Unavailable);
            }
            let WriteItem::Raw { outcome_tx, .. } = error.into_inner() else {
                unreachable!("raww only enqueues Raw write items");
            };
            let _ = outcome_tx.send(not_submitted_outcome(
                self.target_session.clone(),
                String::new(),
                if channel_closed {
                    "ACP write channel closed (delivery task exited)"
                } else {
                    "ACP write channel full (delivery task saturated)"
                },
            ));
        }
        outcome_rx
    }

    fn is_ready_for_handover(&self) -> bool {
        matches!(self.readiness(), WorkerReadinessState::Available)
    }

    fn health(&self) -> TransportHealth {
        // The inner transport cannot answer this one. `Unavailable` is published
        // for a respawn gap and for a permanent give-up alike, so reading it here
        // would report a recoverable worker as unreachable and bounce messages a
        // respawn was about to make deliverable. Permanence is the driver's
        // knowledge, and `AcpWorkerDriver::health` is what the relay actually
        // calls — `TransportImpl::Acp` holds the driver, never this type.
        TransportHealth::Healthy
    }

    fn shutdown(&mut self) {
        // Signal the delivery task to drain and exit, then drop the runtime.
        self.shutdown_tx = None;
        self.write_tx = None;
        self.runtime = None;
        self.set_replay(None);
        self.set_readiness(WorkerReadinessState::Unavailable);
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        // Always publishes a handle, even before the first runtime exists: the
        // handle reads the shared state, which the transport repoints across
        // startup/respawn. This keeps the prime-wait reachable during the very
        // windows (initial startup, respawn gap) when there is no live runtime.
        Some(Arc::new(AcpOutputView {
            shared: Arc::clone(&self.shared),
        }))
    }
}

/// Concurrent look view over an ACP transport's output. Captures the shared
/// state ([`AcpSharedState`]) so the relay look path can read a snapshot without
/// borrowing the worker-owned transport, and so the handle stays valid across
/// startup and respawn (the transport repoints the inner replay buffer).
struct AcpOutputView {
    shared: Arc<AcpSharedState>,
}

impl OutputView for AcpOutputView {
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        // Own the bounded prime-wait: while the worker is still initializing,
        // wait up to `prime_timeout` for the first snapshot to populate.
        let deadline = Instant::now() + mode.prime_timeout;
        let prime_timed_out = loop {
            let state = *self.shared.readiness.lock().expect("readiness mutex");
            if !matches!(state, WorkerReadinessState::Initializing) {
                break false;
            }
            if Instant::now() >= deadline {
                break true;
            }
            thread::sleep(ACP_LOOK_PRIME_POLL_INTERVAL);
        };

        let worker_state = *self.shared.readiness.lock().expect("readiness mutex");
        let entries = match self
            .shared
            .replay
            .lock()
            .expect("replay slot mutex")
            .as_ref()
        {
            Some(buffer) => buffer.lock().expect("replay buffer mutex").clone(),
            None => Vec::new(),
        };
        let requested_entries = mode
            .lines
            .map(|lines| lines as usize)
            .unwrap_or(ACP_LOOK_ENTRIES_DEFAULT);
        let offset = mode.offset.map(|offset| offset as usize).unwrap_or(0);
        let snapshot = derive_acp_look_snapshot(
            Some(worker_state),
            Some(entries.as_slice()),
            requested_entries,
            offset,
            prime_timed_out,
        );
        Ok(acp_snapshot_to_payload(snapshot))
    }
}

fn acp_snapshot_to_payload(snapshot: AcpLookSnapshot) -> LookSnapshotPayload {
    LookSnapshotPayload::StructuredEntries {
        snapshot_entries: snapshot.snapshot_entries,
        entries_total: snapshot.entries_total,
        returned_entries_count: snapshot.returned_entries_count,
        freshness: snapshot.freshness,
        snapshot_source: snapshot.snapshot_source,
        stale_reason_code: snapshot.stale_reason_code,
        snapshot_age_ms: snapshot.snapshot_age_ms,
    }
}

/// ACP runtime state shared across turn submission functions.
struct TurnContext<'a> {
    session_id: &'a str,
    shared: &'a Arc<AcpSharedState>,
    chooser: &'a Option<crate::transports::Chooser>,
    target_session: &'a str,
}

/// Sets the transport-internal readiness and mirrors the transition to the relay
/// global registry when a mirror is installed. Centralizes the per-turn readiness
/// transitions inside the delivery task so the relay worker no longer drives
/// `mark_busy` / `mirror_settled_readiness`.
fn set_turn_readiness(ctx: &TurnContext, state: WorkerReadinessState) {
    set_shared_readiness(ctx.shared, state);
}

/// Writes `state` to the shared readiness slot and mirrors it to the relay global
/// registry when a mirror is installed. Shared by [`set_turn_readiness`] and the
/// `on_dispatched` Busy transition (which holds the `Arc` directly).
fn set_shared_readiness(shared: &AcpSharedState, state: WorkerReadinessState) {
    *shared.readiness.lock().expect("readiness mutex") = state;
    if let Some(mirror) = shared.mirror_state.as_ref() {
        mirror(state);
    }
}

/// A batch of rendered envelopes with their metadata, ready for combining.
pub(super) struct EnvelopeBatch {
    rendered: Vec<String>,
    message_ids: Vec<String>,
    decider_sessions: Vec<Vec<String>>,
    outcome_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>>,
}

impl EnvelopeBatch {
    /// Builds a single-envelope batch from the head envelope's submitted
    /// write item.
    fn from_head(
        envelope: &DeliveryEnvelope,
        outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    ) -> Self {
        Self {
            rendered: vec![envelope.message.render_pane_envelope(&envelope.message_id)],
            message_ids: vec![envelope.message_id.clone()],
            decider_sessions: vec![envelope.choice_decider_sessions.clone()],
            outcome_senders: vec![outcome_tx],
        }
    }

    /// Absorbs an additional envelope into this batch during the outer
    /// coalesce loop. Pushes rendered output, message id, decider
    /// sessions, and outcome sender.
    fn absorb_envelope(
        &mut self,
        envelope: &DeliveryEnvelope,
        outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    ) {
        self.rendered
            .push(envelope.message.render_pane_envelope(&envelope.message_id));
        self.message_ids.push(envelope.message_id.clone());
        self.decider_sessions
            .push(envelope.choice_decider_sessions.clone());
        self.outcome_senders.push(outcome_tx);
    }
}

/// Channels connecting the transport to its internal delivery task.
struct DeliveryChannels {
    rx: mpsc::Receiver<WriteItem>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    respawn_needed_tx: tokio::sync::watch::Sender<u64>,
}

struct DeliveryTaskIdentity {
    target_session: String,
}

/// Internal ACP delivery task. Runs on a dedicated thread, draining the write
/// channel, combining contiguous envelopes respecting the token budget, and
/// submitting turns to the ACP runtime. Exits when the channel closes (sender
/// dropped by `release_runtime()` or `shutdown()`).
fn acp_delivery_task(
    channels: DeliveryChannels,
    mut client: AcpStdioClient,
    session_id: String,
    shared: Arc<AcpSharedState>,
    chooser: Option<crate::transports::Chooser>,
    batch_settings: PromptBatchSettings,
    identity: DeliveryTaskIdentity,
) {
    let ctx = TurnContext {
        session_id: &session_id,
        shared: &shared,
        chooser: &chooser,
        target_session: &identity.target_session,
    };

    let mut rx = channels.rx;
    let mut shutdown_rx = channels.shutdown_rx;
    let respawn_needed_tx = channels.respawn_needed_tx;

    loop {
        if is_shutdown(&mut shutdown_rx) {
            drain_and_resolve_shutdown(&mut rx);
            break;
        }

        let Some(first) = rx.blocking_recv() else {
            break;
        };

        let head = first;
        // The head-receipt, head-peer, and head-raw cases are all routed
        // through `plan_inner_actions`, the production seam. The plan
        // describes the ordered actions (peer absorption + boundary
        // submission); `execute_delivery_plan` applies them against the
        // live transport. The receipt-flush-barrier rules live in the plan,
        // so changes to barrier semantics touch one function and one
        // inline test.
        //
        // Check after receive — shutdown may have fired between the
        // pre-receive check and the actual receive.
        if is_shutdown(&mut shutdown_rx) {
            resolve_head_as_shutdown(head);
            drain_and_resolve_shutdown(&mut rx);
            break;
        }
        let plan = plan_inner_actions(
            head,
            || rx.try_recv().ok(),
            is_receipt_envelope,
            is_raw_write_item,
            || is_shutdown(&mut shutdown_rx),
        );
        execute_delivery_plan(
            &mut client,
            &ctx,
            &batch_settings,
            &respawn_needed_tx,
            &mut rx,
            &mut shutdown_rx,
            plan,
        );
    }
}

/// Resolves the head item's outcome sender with `dropped_on_shutdown_outcome`
/// when shutdown fires after the head was received but before plan
/// execution. Centralizes the post-receive shutdown resolution so the
/// outer loop can hoist the shutdown check above the plan dispatch.
/// Takes `head` by value so the moved `oneshot::Sender` can be consumed.
fn resolve_head_as_shutdown(head: WriteItem) {
    let outcome_tx = match head {
        WriteItem::Envelope { outcome_tx, .. } => outcome_tx,
        WriteItem::Raw { outcome_tx, .. } => outcome_tx,
    };
    let _ = outcome_tx.send(dropped_on_shutdown_outcome());
}

/// Drains all remaining items from the write channel and resolves their outcome
/// senders with DroppedOnShutdown. Called when the shutdown/respawn signal fires.
fn drain_and_resolve_shutdown(rx: &mut mpsc::Receiver<WriteItem>) {
    while let Ok(item) = rx.try_recv() {
        let outcome_tx = match item {
            WriteItem::Envelope { outcome_tx, .. } => outcome_tx,
            WriteItem::Raw { outcome_tx, .. } => outcome_tx,
        };
        let _ = outcome_tx.send(dropped_on_shutdown_outcome());
    }
}

/// A DroppedOnShutdown outcome for writes resolved during relay shutdown. Mirrors
/// the tmux transport's shutdown-drop outcome so the relay shutdown taxonomy
/// reports `dropped_on_shutdown` (not a generic failure) uniformly across
/// transports. (Respawn invalidation is a distinct path: it closes the channel,
/// and the worker maps the dropped future to its own outcome.)
fn dropped_on_shutdown_outcome() -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: String::new(),
        message_id: String::new(),
        outcome: SendOutcome::DroppedOnShutdown,
        reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
        reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
        details: None,
    }
}

/// Submits a single envelope as its own ACP turn with no batch coalescing.
/// Used by the receipt rendering path so a terminal-outcome receipt (a
/// relay/system-originated informational turn back to the original sender)
/// never absorbs peer envelopes and never lands inside a peer flush group:
/// the receipt is its own turn and is observable on its own. The receipt
/// resolves on completion, agent close, dispatcher refusal, serialization
/// failure, or shutdown — no elapsed-time bound is applied here.
/// `quiet_window` is unused on ACP and the
/// relay's `build_coder_envelope` zeros it for receipts addressed to an
/// ACP sender so the receipt-bypasses-quiescence invariant holds at the
/// envelope seam.
fn submit_singleton_envelope(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    batch_settings: &PromptBatchSettings,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    envelope: Box<DeliveryEnvelope>,
    outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
) {
    let mut batch = EnvelopeBatch::from_head(&envelope, outcome_tx);
    flush_envelope_group(client, ctx, batch_settings, respawn_needed_tx, &mut batch);
}

/// True when the write item is an envelope flagged as a terminal-outcome
/// receipt (a relay/system-originated informational turn back to the
/// original sender). Receipts are the flush barrier: they MUST NOT
/// coalesce with peer traffic. Used by `plan_inner_actions` (the production
/// seam) and by the inline `delivery_plan_tests` module to verify the
/// plan's barrier semantics without spinning up the delivery task's
/// blocking submit path.
fn is_receipt_envelope(item: &WriteItem) -> bool {
    matches!(item, WriteItem::Envelope { envelope, .. } if envelope.is_receipt)
}

/// True when the write item is a raw input. Raw inputs are a batch
/// barrier: they terminate the current peer absorption and submit on
/// their own (via `submit_raw_turn`). Used by `plan_inner_actions` and
/// tested alongside `is_receipt_envelope`.
fn is_raw_write_item(item: &WriteItem) -> bool {
    matches!(item, WriteItem::Raw { .. })
}

/// The ordered actions `acp_delivery_task` must execute after receiving
/// one head item from the write channel. Produced by [`plan_inner_actions`];
/// consumed by `execute_delivery_plan`.
///
/// `peers_to_absorb` is the list of peer envelopes the caller must absorb
/// into its in-flight `EnvelopeBatch` before any boundary action. For a
/// head that is itself a peer, the head appears as the first entry. For a
/// head that is a receipt or a raw input, the head goes straight into the
/// boundary and `peers_to_absorb` is empty.
///
/// `boundary` is the single terminating action for this head's partition:
/// either return to the outer loop (no further submission for this head),
/// submit the carried receipt as a singleton turn, or submit the carried
/// raw as a raw turn. Receipts are NEVER absorbed into a peer batch — the
/// caller must flush any in-flight peer batch before executing a receipt
/// boundary (this is `execute_delivery_plan`'s job).
struct DeliveryPlan {
    peers_to_absorb: Vec<(
        Box<DeliveryEnvelope>,
        tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    boundary: BoundaryAction,
}

/// Single terminating action for one head partition. See [`DeliveryPlan`].
enum BoundaryAction {
    /// Return to the outer loop. The caller has already flushed the
    /// in-flight peer batch (if any); no further submission for this head.
    ReturnToOuterLoop,
    /// Submit the carried receipt as a singleton turn. The caller has
    /// already flushed the in-flight peer batch (if any).
    SubmitReceiptSingleton {
        envelope: Box<DeliveryEnvelope>,
        outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    },
    /// Submit the carried raw as a raw turn. The caller has already
    /// flushed the in-flight peer batch (if any).
    SubmitRaw {
        content: String,
        append_enter: bool,
        outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    },
}

/// Production seam for `acp_delivery_task`'s inner scan. Given the head
/// item already received via `blocking_recv`, drains subsequent items via
/// `pull_next` until either a barrier (receipt or raw input) is found or
/// `pull_next` returns `None` (channel empty/closed), and returns the
/// ordered actions to execute.
///
/// Receipt envelopes are NEVER absorbed into a peer batch. When a receipt
/// appears as a head, the plan returns it directly in the boundary as a
/// `SubmitReceiptSingleton` action with `peers_to_absorb` empty. When a
/// receipt appears mid-scan, the plan flushes the pending peer absorption
/// (signaled to the caller by ending the scan) and returns the receipt in
/// the boundary. The caller is responsible for the actual flush + singleton
/// submission; this plan only describes the actions.
///
/// Raw inputs follow the same barrier shape but submit as raw turns
/// (`submit_raw_turn`) instead of singleton envelopes.
///
/// `pull_next` is a closure the caller supplies (typically wrapping
/// `rx.try_recv().ok()`); the plan never touches the channel directly. This
/// keeps the seam testable: tests pass a closure over a `Vec<WriteItem>`
/// iterator for deterministic, in-process sequencing.
///
/// `should_stop` is consulted once per scan iteration; if it returns
/// `true`, the plan ends the scan with `ReturnToOuterLoop` (the caller is
/// expected to drain and break the outer loop). The production caller
/// wires this to `is_shutdown(...)` so a mid-scan shutdown is treated as
/// a graceful stop.
///
/// `is_receipt` and `is_raw` are the barrier predicates; extracted as
/// parameters so this seam can be reused for non-ACP transports in the
/// future without depending on `is_receipt_envelope` /
/// `is_raw_write_item` directly.
fn plan_inner_actions<PReceipt, PRaw, PStop>(
    head: WriteItem,
    mut pull_next: impl FnMut() -> Option<WriteItem>,
    is_receipt: PReceipt,
    is_raw: PRaw,
    mut should_stop: PStop,
) -> DeliveryPlan
where
    PReceipt: Fn(&WriteItem) -> bool,
    PRaw: Fn(&WriteItem) -> bool,
    PStop: FnMut() -> bool,
{
    match head {
        WriteItem::Envelope {
            envelope,
            outcome_tx,
        } if envelope.is_receipt => {
            // Head receipt: immediate singleton, no scan needed.
            DeliveryPlan {
                peers_to_absorb: Vec::new(),
                boundary: BoundaryAction::SubmitReceiptSingleton {
                    envelope,
                    outcome_tx,
                },
            }
        }
        WriteItem::Envelope {
            envelope,
            outcome_tx,
        } => {
            // Head peer: scan for the first barrier.
            let mut peers_to_absorb = vec![(envelope, outcome_tx)];
            loop {
                if should_stop() {
                    return DeliveryPlan {
                        peers_to_absorb,
                        boundary: BoundaryAction::ReturnToOuterLoop,
                    };
                }
                match pull_next() {
                    Some(item) if is_receipt(&item) => {
                        let WriteItem::Envelope {
                            envelope,
                            outcome_tx,
                        } = item
                        else {
                            unreachable!("is_receipt matched non-Envelope variant");
                        };
                        return DeliveryPlan {
                            peers_to_absorb,
                            boundary: BoundaryAction::SubmitReceiptSingleton {
                                envelope,
                                outcome_tx,
                            },
                        };
                    }
                    Some(item) if is_raw(&item) => {
                        let WriteItem::Raw {
                            content,
                            append_enter,
                            outcome_tx,
                        } = item
                        else {
                            unreachable!("is_raw matched non-Raw variant");
                        };
                        return DeliveryPlan {
                            peers_to_absorb,
                            boundary: BoundaryAction::SubmitRaw {
                                content,
                                append_enter,
                                outcome_tx,
                            },
                        };
                    }
                    Some(item) => {
                        // Peer — absorb.
                        let WriteItem::Envelope {
                            envelope,
                            outcome_tx,
                        } = item
                        else {
                            unreachable!("non-receipt/non-raw item must be an Envelope");
                        };
                        peers_to_absorb.push((envelope, outcome_tx));
                    }
                    None => {
                        return DeliveryPlan {
                            peers_to_absorb,
                            boundary: BoundaryAction::ReturnToOuterLoop,
                        };
                    }
                }
            }
        }
        WriteItem::Raw {
            content,
            append_enter,
            outcome_tx,
        } => {
            // Head raw: immediate raw submit, no scan needed.
            DeliveryPlan {
                peers_to_absorb: Vec::new(),
                boundary: BoundaryAction::SubmitRaw {
                    content,
                    append_enter,
                    outcome_tx,
                },
            }
        }
    }
}

/// True when the shutdown signal has fired (or the sender was dropped).
/// Module-level helper so `execute_delivery_plan` and `acp_delivery_task`
/// can share the predicate without plumbing closures; `oneshot::Receiver`'s
/// `try_recv` returns `Ok(())` once the signal fires and `Err(Closed)`
/// once the sender is dropped, both of which the delivery task treats
/// as "graceful stop".
fn is_shutdown(shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>) -> bool {
    matches!(
        shutdown_rx.try_recv(),
        Ok(()) | Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    )
}

/// Executes the actions in a [`DeliveryPlan`] in order. The caller (the
/// inner scan of `acp_delivery_task`) supplies the live client, transport
/// context, and shutdown signal; this helper applies the plan without
/// any further state-machine branching.
///
/// Shutdown handling: between flush and boundary execution (and before
/// any blocking submit), the helper checks `is_shutdown(&mut shutdown_rx)`.
/// On a shutdown-during-execution, the pending batch's outcome senders
/// are resolved with `dropped_on_shutdown_outcome`, the channel is
/// drained via `drain_and_resolve_shutdown`, and the helper returns so
/// the outer loop can break. Shutdown checks resolve held senders and
/// drain queued work before any boundary submission.
#[allow(clippy::too_many_arguments)]
fn execute_delivery_plan(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    batch_settings: &PromptBatchSettings,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    rx: &mut mpsc::Receiver<WriteItem>,
    shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>,
    plan: DeliveryPlan,
) {
    // Build and absorb the in-flight peer batch from the plan's collected
    // peers. Empty plans (head receipt / head raw) skip the batch entirely.
    let mut batch: Option<EnvelopeBatch> = None;
    for (envelope, outcome_tx) in plan.peers_to_absorb {
        batch = Some(match batch {
            None => EnvelopeBatch::from_head(&envelope, outcome_tx),
            Some(mut existing) => {
                existing.absorb_envelope(&envelope, outcome_tx);
                existing
            }
        });
    }
    if let Some(mut batch) = batch {
        if is_shutdown(shutdown_rx) {
            for tx in batch.outcome_senders.drain(..) {
                let _ = tx.send(dropped_on_shutdown_outcome());
            }
            drain_and_resolve_shutdown(rx);
            return;
        }
        flush_envelope_group(client, ctx, batch_settings, respawn_needed_tx, &mut batch);
    }

    // Execute the boundary action.
    match plan.boundary {
        BoundaryAction::ReturnToOuterLoop => {}
        BoundaryAction::SubmitReceiptSingleton {
            envelope,
            outcome_tx,
        } => {
            if is_shutdown(shutdown_rx) {
                let _ = outcome_tx.send(dropped_on_shutdown_outcome());
                drain_and_resolve_shutdown(rx);
                return;
            }
            submit_singleton_envelope(
                client,
                ctx,
                batch_settings,
                respawn_needed_tx,
                envelope,
                outcome_tx,
            );
        }
        BoundaryAction::SubmitRaw {
            content,
            append_enter,
            outcome_tx,
        } => {
            if is_shutdown(shutdown_rx) {
                let _ = outcome_tx.send(dropped_on_shutdown_outcome());
                drain_and_resolve_shutdown(rx);
                return;
            }
            // The raw path resolves at the framed write inside
            // `submit_envelope_turn`; respawn is raised from the observability
            // path there, not from a post-hoc outcome check here.
            submit_raw_turn(
                client,
                ctx,
                respawn_needed_tx,
                content.as_str(),
                append_enter,
                outcome_tx,
            );
        }
    }
}

/// Combines a contiguous batch of rendered envelopes into token-budget-bounded
/// turn prompts via [`crate::envelope::batch_envelope_groups`], submits each
/// group as one turn, and fans that turn's outcome to the contributing senders.
/// Each sender receives its own message_id in the outcome, even when multiple
/// envelopes are combined into one turn. The group's head message_id and decider
/// sessions correlate any choice raised mid-turn.
fn flush_envelope_group(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    batch_settings: &PromptBatchSettings,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    batch: &mut EnvelopeBatch,
) {
    let groups = crate::envelope::batch_envelope_groups(&batch.rendered, *batch_settings);
    batch.rendered.clear();
    let mut message_ids = batch.message_ids.drain(..);
    let mut decider_sessions = batch.decider_sessions.drain(..);
    let mut outcome_senders = batch.outcome_senders.drain(..);

    for group in groups {
        let group_msg_ids: Vec<String> = message_ids.by_ref().take(group.member_count).collect();
        let group_deciders: Vec<Vec<String>> =
            decider_sessions.by_ref().take(group.member_count).collect();
        let group_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>> =
            outcome_senders.by_ref().take(group.member_count).collect();
        let head_deciders = group_deciders.into_iter().next().unwrap_or_default();
        let members = group_msg_ids.into_iter().zip(group_senders).collect();
        submit_envelope_turn(
            client,
            ctx,
            respawn_needed_tx,
            &group.combined_prompt,
            members,
            &head_deciders,
        );
    }
}

/// Submits one combined prompt as an ACP turn and resolves every member of the
/// group from the submission evidence.
///
/// The framed `session/prompt` write is the delivery boundary: `Submitted`
/// (member resolves `Delivered`) is recorded immediately after the write
/// succeeds, before replay-buffer locks or `on_dispatched` run. The turn's
/// later completion, permission requests, or connection close are target-health
/// observability — they drive readiness and the respawn signal, never a second
/// delivery outcome for an already-resolved member. Active-prompt refusal and
/// serialization failure map to `not_submitted`; a stdin write or flush error
/// without proof that zero bytes left maps to `submission_unknown`. No
/// elapsed-time path bounds the wait on the ACP side; the relay's
/// submission-timeout watchdog bounds the supervised code's runtime only after
/// it is armed, which it becomes when the relay records submission evidence at
/// write time.
fn submit_envelope_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    prompt: &str,
    members: Vec<(String, tokio::sync::oneshot::Sender<SingleDeliveryOutcome>)>,
    decider_sessions: &[String],
) {
    let pending_choice: Arc<Mutex<Option<ChoiceMade>>> = Arc::new(Mutex::new(None));
    let completion_slot: Arc<Mutex<Option<PromptCompletion>>> = Arc::new(Mutex::new(None));

    let head_message_id = members
        .first()
        .map(|(message_id, _)| message_id.clone())
        .unwrap_or_default();

    let shared_for_dispatch = Arc::clone(ctx.shared);
    let on_dispatched: DispatchHandler = Box::new(move || {
        set_shared_readiness(&shared_for_dispatch, WorkerReadinessState::Busy);
    });

    let on_permission = if let Some(chooser) = ctx.chooser {
        let correlation = ChoiceCorrelation {
            message_id: head_message_id,
            target_session: ctx.target_session.to_string(),
            decider_sessions: decider_sessions.to_vec(),
        };
        let shared_for_executors = Arc::clone(ctx.shared);
        let mut inner = build_acp_permission_handler(
            chooser.clone(),
            correlation,
            Arc::clone(&pending_choice),
            Arc::new(move |handle| shared_for_executors.note_permission_executor(handle)),
        );
        let wrapped: PermissionHandler = Box::new(move |req, responder| {
            (inner)(req, responder);
        });
        wrapped
    } else {
        let wrapped: PermissionHandler = Box::new(|_req, mut responder: PermissionResponder| {
            responder.respond(None);
        });
        wrapped
    };

    let completion_writer = Arc::clone(&completion_slot);
    let on_completion: PromptCompletionHandler = Box::new(move |completion| {
        *completion_writer.lock().expect("completion slot mutex") = Some(completion);
    });

    // `Submitted` is returned immediately after the framed write succeeds,
    // before replay-buffer locks or `on_dispatched` — so the member evidence is
    // recorded (resolved below) before either can block or panic.
    let dispatch = client.prompt(ctx.session_id, prompt, Some(on_permission), on_completion);

    match dispatch {
        PromptDispatchOutcome::Submitted => {
            // The framed write succeeded: every member of this group resolves
            // `Delivered` at the write, before the replay-buffer locks or
            // `on_dispatched` below.
            for (message_id, sender) in members {
                let _ = sender.send(delivered_outcome(
                    ctx.target_session.to_string(),
                    message_id,
                ));
            }
            // Replay-buffer locks + on_dispatched (Busy) follow the evidence
            // recording; the turn lifecycle is observability only.
            client.note_prompt_dispatched(prompt, Some(on_dispatched));
            observe_acp_turn(
                client,
                ctx,
                respawn_needed_tx,
                &completion_slot,
                &pending_choice,
            );
        }
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            // A stdin write or flush error without proof that zero bytes left
            // cannot assert non-delivery: the member resolves submission_unknown.
            set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
            raise_respawn_signal(respawn_needed_tx);
            for (message_id, sender) in members {
                let _ = sender.send(submission_unknown_outcome(
                    ctx.target_session.to_string(),
                    message_id,
                    &reason,
                ));
            }
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            // Active-prompt refusal and serialization failure are positive
            // non-delivery: nothing was written, so the member resolves
            // not_submitted. The transport is healthy, so readiness stays as-is.
            for (message_id, sender) in members {
                let _ = sender.send(not_submitted_outcome(
                    ctx.target_session.to_string(),
                    message_id,
                    &reason,
                ));
            }
        }
    }
}

/// Observes the turn lifecycle after a `Submitted` framed write. The member
/// already resolved `Delivered` at the write, so this wait drives readiness and
/// the respawn signal only: a normal completion returns the worker to
/// `Available`, a connection close marks it `Unavailable` and raises the respawn
/// signal, and an abandoned wait (shutdown) leaves the worker draining.
fn observe_acp_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    completion_slot: &Arc<Mutex<Option<PromptCompletion>>>,
    pending_choice: &Arc<Mutex<Option<ChoiceMade>>>,
) {
    // No elapsed-time path bounds this wait on the ACP side. The relay's
    // submission-timeout watchdog bounds the supervised code's runtime only
    // after it is armed, which it becomes when the relay records submission
    // evidence at write time. Returns true if the prompt completed, false if
    // shutdown was requested.
    let completed = loop {
        if client.wait_for_prompt_complete(ACP_PROMPT_WAIT_POLL_INTERVAL) {
            break true;
        }
        if shutdown_requested() {
            break false;
        }
    };
    if !completed {
        let _ = completion_slot
            .lock()
            .expect("completion slot mutex")
            .take();
        let _ = pending_choice.lock().expect("pending_choice mutex").take();
        set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
        return;
    }
    let completion = completion_slot
        .lock()
        .expect("completion slot mutex")
        .take();
    let pending = pending_choice.lock().expect("pending_choice mutex").take();
    let (final_state, requires_respawn) = build_acp_turn_observability(completion, pending);
    set_turn_readiness(ctx, final_state);
    if requires_respawn {
        raise_respawn_signal(respawn_needed_tx);
    }
}

/// Submits raw content as an ACP turn (no envelope framing). The framed write
/// is the delivery boundary exactly as in the envelope path; no elapsed-time
/// bound is applied here either.
fn submit_raw_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    content: &str,
    _append_enter: bool,
    outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
) {
    submit_envelope_turn(
        client,
        ctx,
        respawn_needed_tx,
        content,
        vec![(String::new(), outcome_tx)],
        &[],
    );
}

fn delivered_outcome(target_session: String, message_id: String) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

fn not_submitted_outcome(
    target_session: String,
    message_id: String,
    reason: &str,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::NotSubmitted,
        reason_code: Some("not_submitted".to_string()),
        reason: Some(reason.to_string()),
        details: None,
    }
}

fn submission_unknown_outcome(
    target_session: String,
    message_id: String,
    reason: &str,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::SubmissionUnknown,
        reason_code: Some("submission_unknown".to_string()),
        reason: Some(reason.to_string()),
        details: None,
    }
}

/// Classifies a settled turn's lifecycle for target-health observability.
///
/// The member already resolved `Delivered` at the framed write, so the turn's
/// completion and any operator-choice outcome never produce a second delivery
/// outcome — they only drive the worker's readiness and the respawn signal. A
/// normal completion returns the worker to `Available`; a connection close
/// marks it `Unavailable` and requests a respawn; an abandoned wait (shutdown)
/// leaves the worker draining. `ProtocolError` and an unsupported stop reason
/// keep the worker `Available`, because the agent is still responsive and a
/// bad turn is not a recoverable failure.
fn build_acp_turn_observability(
    completion: Option<PromptCompletion>,
    pending_choice_outcome: Option<ChoiceMade>,
) -> (WorkerReadinessState, bool) {
    if let Some(ChoiceMade::Cancelled { .. }) = pending_choice_outcome {
        return (WorkerReadinessState::Available, false);
    }

    let Some(completion) = completion else {
        // No completion observed before the wait was abandoned: shutdown. The
        // member already resolved `Delivered` at the write; the worker is
        // draining rather than accepting more turns.
        return (WorkerReadinessState::Unavailable, false);
    };

    match completion {
        PromptCompletion::Completed { stop_reason } => match stop_reason.as_str() {
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => {
                (WorkerReadinessState::Available, false)
            }
            "cancelled" => (WorkerReadinessState::Available, false),
            _ => (WorkerReadinessState::Available, false),
        },
        PromptCompletion::ProtocolError(_) => (WorkerReadinessState::Available, false),
        PromptCompletion::ConnectionClosed { .. } => (WorkerReadinessState::Unavailable, true),
    }
}

#[cfg(test)]
mod delivery_plan_tests {
    //! Inline coverage for [`plan_inner_actions`], the production seam
    //! that drives `acp_delivery_task`'s receipt-flush-barrier rules. The
    //! test exercises the real production function with a deterministic
    //! closure over a `VecDeque<WriteItem>` so the test can both drive
    //! the plan and observe the iterator remainder (the trailing items
    //! the plan did not consume). One `#[test]` covers: head receipt is
    //! its own plan; mid-batch receipt flushes preceding peers then runs
    //! alone; receipt message_ids stay correlated to the originating
    //! items; raw-as-batch-barrier (raw ends the scan with `SubmitRaw`,
    //! not absorbed as an ordinary peer); `should_stop` mid-scan
    //! preserves the collected peers.
    use super::*;
    use crate::envelope::AddressIdentity;
    use crate::transports::{DeliveryEnvelope, DeliveryMessage};

    fn peer(message_id: &str) -> WriteItem {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        WriteItem::Envelope {
            envelope: Box::new(make_envelope(message_id, false)),
            outcome_tx: tx,
        }
    }

    fn receipt(message_id: &str) -> WriteItem {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        WriteItem::Envelope {
            envelope: Box::new(make_envelope(message_id, true)),
            outcome_tx: tx,
        }
    }

    fn raw(content: &str) -> WriteItem {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        WriteItem::Raw {
            content: content.to_string(),
            append_enter: true,
            outcome_tx: tx,
        }
    }

    /// A `pull_next` source backed by a `VecDeque` so the test can both
    /// drive the plan (via the `pull` method the closure captures) and
    /// observe the items the plan did NOT consume (via `remaining_ids`).
    /// The plan takes `pull_next` as an opaque closure; this struct
    /// hands out a `FnMut` closure that mutably borrows `self`, so the
    /// borrow is released once `plan_inner_actions` returns and the
    /// test can inspect the remainder.
    struct TestQueue {
        items: std::collections::VecDeque<WriteItem>,
    }

    impl TestQueue {
        fn new(items: Vec<WriteItem>) -> Self {
            Self {
                items: items.into(),
            }
        }

        fn pull(&mut self) -> Option<WriteItem> {
            self.items.pop_front()
        }

        fn remaining_ids(&self) -> Vec<String> {
            self.items
                .iter()
                .map(|item| match item {
                    WriteItem::Envelope { envelope, .. } => envelope.message_id.clone(),
                    WriteItem::Raw { .. } => String::from("<raw>"),
                })
                .collect()
        }
    }

    fn make_envelope(message_id: &str, is_receipt: bool) -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: message_id.to_string(),
            message: DeliveryMessage {
                body: format!("body {message_id}"),
                created_at: "1970-01-01T00:00:00Z".to_string(),
                namespace: "party".to_string(),
                sender: AddressIdentity {
                    session_name: "alpha".to_string(),
                    display_name: None,
                },
                target: AddressIdentity {
                    session_name: "beta".to_string(),
                    display_name: None,
                },
                cc: vec![],
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: vec![],
            quiet_window: Duration::ZERO,
            is_receipt,
        }
    }

    fn peer_ids(plan: &DeliveryPlan) -> Vec<String> {
        plan.peers_to_absorb
            .iter()
            .map(|(env, _)| env.message_id.clone())
            .collect()
    }

    fn boundary_receipt_id(plan: &DeliveryPlan) -> Option<String> {
        if let BoundaryAction::SubmitReceiptSingleton { envelope, .. } = &plan.boundary {
            Some(envelope.message_id.clone())
        } else {
            None
        }
    }

    fn boundary_raw_content(plan: &DeliveryPlan) -> Option<String> {
        if let BoundaryAction::SubmitRaw { content, .. } = &plan.boundary {
            Some(content.clone())
        } else {
            None
        }
    }

    #[test]
    fn plan_inner_actions_partitions_receipts_and_peers_correctly() {
        // peer + receipt + peer: one plan call absorbs the head peer,
        // ends the scan at the receipt boundary, and leaves the trailing
        // peer in the channel for the next outer-loop iteration to pick
        // up. The plan returns ONE plan (peers=[p1],
        // boundary=SubmitReceiptSingleton{r1}); the test observes that
        // continuation by inspecting the queue remainder, not by
        // synthesizing three independent plan calls.
        let mut queue = TestQueue::new(vec![receipt("r1"), peer("p2")]);
        let pull = || queue.pull();
        let plan = plan_inner_actions(
            peer("p1"),
            pull,
            is_receipt_envelope,
            is_raw_write_item,
            || false,
        );
        assert_eq!(peer_ids(&plan), vec!["p1"]);
        assert_eq!(boundary_receipt_id(&plan).as_deref(), Some("r1"));
        assert_eq!(
            queue.remaining_ids(),
            vec!["p2"],
            "trailing peer left in the channel for the next outer-loop iteration",
        );

        // Head receipt is its own plan: no scan, no peer absorption. The
        // receipt goes straight into the boundary; a trailing peer in
        // the channel is untouched (the receipt does not pull).
        let mut queue = TestQueue::new(vec![peer("p1")]);
        let pull = || queue.pull();
        let plan = plan_inner_actions(
            receipt("r1"),
            pull,
            is_receipt_envelope,
            is_raw_write_item,
            || false,
        );
        assert!(plan.peers_to_absorb.is_empty());
        assert_eq!(boundary_receipt_id(&plan).as_deref(), Some("r1"));
        assert_eq!(
            queue.remaining_ids(),
            vec!["p1"],
            "head receipt does not scan; trailing peer remains in the channel",
        );

        // Mid-batch receipt flushes preceding peers then runs alone: the
        // head peer is absorbed, the second peer is absorbed, then the
        // receipt boundary ends the scan with peers=[p1, p2] and
        // SubmitReceiptSingleton{r1}. The plan does NOT absorb the
        // receipt itself.
        let mut queue = TestQueue::new(vec![peer("p2"), receipt("r1")]);
        let pull = || queue.pull();
        let plan = plan_inner_actions(
            peer("p1"),
            pull,
            is_receipt_envelope,
            is_raw_write_item,
            || false,
        );
        assert_eq!(peer_ids(&plan), vec!["p1", "p2"]);
        assert_eq!(boundary_receipt_id(&plan).as_deref(), Some("r1"));
        assert!(
            queue.remaining_ids().is_empty(),
            "scan consumed everything up to the receipt barrier",
        );

        // Raw input is a batch barrier (not an ordinary peer): the head
        // peer absorbs the next peer, then the raw ends the scan with
        // SubmitRaw (the raw content is the boundary payload).
        let mut queue = TestQueue::new(vec![peer("p2"), raw("enter")]);
        let pull = || queue.pull();
        let plan = plan_inner_actions(
            peer("p1"),
            pull,
            is_receipt_envelope,
            is_raw_write_item,
            || false,
        );
        assert_eq!(peer_ids(&plan), vec!["p1", "p2"]);
        assert_eq!(boundary_raw_content(&plan).as_deref(), Some("enter"));

        // Head raw input: no scan, immediate SubmitRaw, peers empty.
        let mut queue = TestQueue::new(vec![peer("ignored-not-consumed")]);
        let pull = || queue.pull();
        let plan = plan_inner_actions(
            raw("hello"),
            pull,
            is_receipt_envelope,
            is_raw_write_item,
            || false,
        );
        assert!(plan.peers_to_absorb.is_empty());
        assert_eq!(boundary_raw_content(&plan).as_deref(), Some("hello"));

        // should_stop fires mid-scan: the plan ends with
        // ReturnToOuterLoop and the peers collected up to the stop
        // point are preserved for the caller to flush. The head peer
        // is always in peers_to_absorb; should_stop is consulted once
        // per scan iteration, AFTER the head is recorded. With
        // stop_calls > 1 the plan stops after absorbing one subsequent
        // peer (p2) but before pulling p3.
        let mut queue = TestQueue::new(vec![peer("p2"), peer("p3")]);
        let pull = || queue.pull();
        let mut stop_calls = 0;
        let plan = plan_inner_actions(
            peer("p1"),
            pull,
            is_receipt_envelope,
            is_raw_write_item,
            || {
                stop_calls += 1;
                stop_calls > 1
            },
        );
        assert_eq!(peer_ids(&plan), vec!["p1", "p2"]);
        assert!(matches!(plan.boundary, BoundaryAction::ReturnToOuterLoop));
    }
}

#[cfg(test)]
mod handover_readiness_tests {
    use super::*;

    #[test]
    fn handover_readiness_matrix_and_delivery_task_handle_retention() {
        let mut transport = AcpTransport::new(PromptBatchSettings::default(), None);

        // Initial state is `Initializing` — not ready for handover.
        assert_eq!(
            transport.readiness(),
            crate::transports::WorkerReadinessState::Initializing
        );
        assert!(!transport.is_ready_for_handover());

        // The single state that returns true: only when the transport is
        // actually idle and able to take a batch right now.
        transport.set_readiness(crate::transports::WorkerReadinessState::Available);
        assert!(transport.is_ready_for_handover());

        // `Busy` is intentionally NOT a handover-ready state — accepting
        // another batch while a turn is in flight would dispatch the wrong
        // message to the same turn. The readiness signal still exists
        // through the injected mirror closure, which is where observers
        // wanting the wider "runtime exists" reading read it.
        transport.set_readiness(crate::transports::WorkerReadinessState::Busy);
        assert!(!transport.is_ready_for_handover());

        // `Recovering` and `Unavailable` are also not ready.
        transport.set_readiness(crate::transports::WorkerReadinessState::Recovering);
        assert!(!transport.is_ready_for_handover());
        transport.set_readiness(crate::transports::WorkerReadinessState::Unavailable);
        assert!(!transport.is_ready_for_handover());

        // No delivery task has been spawned yet, so there is no handle
        // for a generation supervisor to take — the field starts empty
        // and a second `take` after the first stays empty.
        assert!(transport.take_delivery_task_handle().is_none());
        assert!(transport.take_delivery_task_handle().is_none());
    }
}
