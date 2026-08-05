//! ACP delivery as a [`Transport`] implementation.
//!
//! `AcpTransport` owns the per-target `PersistentAcpWorkerRuntime` (moved here
//! from the relay delivery worker, which previously threaded it through
//! `spawn_blocking`). [`Transport::mailw`] enqueues a structured delivery
//! message on the transport's internal channel and returns an outcome future;
//! the internal delivery task renders each message into pane-envelope text,
//! combines a contiguous group into one ACP turn under the token budget, drives
//! it to a terminal state (folding in what used to be the reader thread's
//! `on_completion` body), and resolves the future for each contributing task.
//!
//! Choices (tool-call permissions) resolve through the relay-injected
//! [`Chooser`] (see [`crate::acp::permission`]); the transport never calls the
//! relay choice queue directly. The `look` path reads output through the
//! [`OutputView`] handle published by [`Transport::give_output`].
//!
//! ## Readiness
//!
//! The transport owns an [`WorkerReadinessState`] signal for [`is_ready`] and
//! the [`OutputView`] prime-wait, because it cannot call relay's
//! `set_worker_readiness`. The `AcpWorkerDriver` mirrors transitions into the
//! global worker-state registry (which external observers and respawn/startup
//! gating still read).
//!
//! [`is_ready`]: Transport::is_ready

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
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
    ChoiceMade, DeliveryDiagnosticContext, DeliveryEnvelope, GenerationFence, LookMode,
    LookSnapshotPayload, OutputView, SingleDeliveryOutcome, StartupContext, Transport,
    TransportError, TransportStatus, emit_delivery_progress,
};
use crate::transports::{SendOutcome, WorkerReadinessState};

// ACP delivery failure taxonomy (see the relay delivery README for the full
// catalogue). These mirror the codes the relay completion path used before the
// transport move so the wire outcomes are unchanged.
const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
/// Prime-timeout reason code reused for the bounded-prime-wait fire. The
/// canonical mapping is recorded in `session-relay/spec.md` under
/// "ACP Stop-Reason Outcome Mapping"; the
/// `acp-prime-timeout-and-wedge-detection` proposal reuses this code on the
/// ACP delivery task's prime timer fire (a new `SendOutcome` variant is NOT
/// introduced).
const ACP_REASON_CODE_PRIME_TIMEOUT: &str = "acp_turn_timeout";
/// Prompt-dispatch failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_PROMPT_FAILED: &str = "runtime_acp_prompt_failed";
/// Connection-closed failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_CONNECTION_CLOSED: &str = "runtime_acp_connection_closed";
/// Transport-unavailable failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE: &str = "acp_child_unavailable";

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
    /// Target namespace, captured at startup for progress diagnostics.
    namespace: String,
    /// Target session id, captured at startup for permission correlation.
    target_session: String,
    /// Stable respawn-needed signal. Created once at construction (not per
    /// delivery task) so the driver-owned async respawn monitor can hold a single
    /// long-lived subscription across respawns. Driven exclusively from the
    /// transport (bootstrap failure, write-without-runtime paths); readiness
    /// latches no longer promote themselves into respawn signals. The monitor
    /// awaits the change, drives the respawn, then resets it to `false`.
    respawn_needed_tx: tokio::sync::watch::Sender<bool>,
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
    bootstraps: Arc<Mutex<Vec<BootstrapRecord>>>,
}

/// One in-flight bootstrap: its guard's identity, and the agent child it owns
/// once the spawn has happened.
#[derive(Debug)]
struct BootstrapRecord {
    id: u64,
    generation: Option<AcpGenerationHandle>,
}

static NEXT_BOOTSTRAP_ID: AtomicU64 = AtomicU64::new(1);

/// Marks a bootstrap as running for as long as it is held, and is how that
/// bootstrap hands its agent child to the fence.
///
/// Moved into the blocking closure itself, not held beside it: the closure
/// outlives any abort of the task awaiting it, so only something dropped by the
/// closure can say when that executor actually stopped.
#[derive(Debug)]
pub(crate) struct BootstrapInFlight {
    id: u64,
    bootstraps: Arc<Mutex<Vec<BootstrapRecord>>>,
}

impl BootstrapInFlight {
    /// Publishes the agent child this bootstrap owns, making it reachable by the
    /// fence's forced step for as long as this guard lives.
    pub(crate) fn publish_generation(&self, generation: AcpGenerationHandle) {
        let mut bootstraps = self.bootstraps.lock().expect("bootstrap registry mutex");
        if let Some(record) = bootstraps.iter_mut().find(|record| record.id == self.id) {
            record.generation = Some(generation);
        }
    }
}

impl Drop for BootstrapInFlight {
    fn drop(&mut self) {
        let mut bootstraps = self.bootstraps.lock().expect("bootstrap registry mutex");
        bootstraps.retain(|record| record.id != self.id);
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
            namespace: String::new(),
            target_session: String::new(),
            respawn_needed_tx: tokio::sync::watch::channel(false).0,
            delivery_task_handle: None,
            generation: None,
            fenced: Arc::new(AtomicBool::new(false)),
            bootstraps: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Subscribes to the stable respawn-needed signal. The driver-owned respawn
    /// monitor holds one subscription for the transport's whole life.
    pub fn respawn_needed_subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.respawn_needed_tx.subscribe()
    }

    /// Resets the respawn-needed signal to `false` after the monitor has handled
    /// a respawn, so a subsequent Unavailable turn re-triggers it.
    pub fn clear_respawn_signal(&self) {
        let _ = self.respawn_needed_tx.send(false);
    }

    /// Primes the respawn-needed signal directly (no delivery task running yet).
    /// Used after an initial-bootstrap failure so the driver's respawn monitor
    /// retries with backoff.
    pub fn signal_respawn(&self) {
        let _ = self.respawn_needed_tx.send(true);
    }

    /// Re-primes the respawn signal when a write arrives but no runtime is live
    /// and the worker has settled Unavailable. This preserves the prior
    /// "every delivery to a dead worker re-attempts recovery" behavior: a
    /// recoverable worker recovers, and a permanently-dead one re-publishes its
    /// Unavailable transition for observers. A transient respawn window
    /// (Recovering) is skipped so an in-flight respawn is not disturbed.
    fn resignal_respawn_if_dead(&self) {
        if matches!(self.readiness(), WorkerReadinessState::Unavailable) {
            self.signal_respawn();
        }
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
            namespace: self.namespace.clone(),
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
        namespace: String,
        target_session: String,
    ) {
        self.chooser = Some(chooser);
        self.namespace = namespace;
        self.target_session = target_session;
        // Close any existing delivery task's channel before creating a new
        // runtime; the old task drains and exits.
        self.write_tx = None;
    }

    /// Registers a bootstrap as running until the returned guard drops.
    #[must_use]
    pub(crate) fn begin_bootstrap(&self) -> BootstrapInFlight {
        let id = NEXT_BOOTSTRAP_ID.fetch_add(1, Ordering::Relaxed);
        self.bootstraps
            .lock()
            .expect("bootstrap registry mutex")
            .push(BootstrapRecord {
                id,
                generation: None,
            });
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

    /// Creates an unavailable outcome preserving the caller's message_id and
    /// this transport's target_session.
    fn unavailable_outcome_with_id(&self, message_id: &str) -> SingleDeliveryOutcome {
        failed_outcome_with_code(
            self.target_session.clone(),
            message_id.to_string(),
            ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
            "ACP transport unavailable (no runtime)",
            None,
        )
    }
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
        for record in self
            .bootstraps
            .lock()
            .expect("bootstrap registry mutex")
            .iter()
        {
            if let Some(generation) = record.generation.as_ref() {
                generation.initiate_termination();
            }
        }
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
        let no_bootstrap_running = self
            .bootstraps
            .lock()
            .expect("bootstrap registry mutex")
            .is_empty();
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
        if let Some(tx) = self.write_tx.as_ref() {
            if let Err(error) = tx.try_send(WriteItem::Envelope {
                envelope: Box::new(envelope),
                outcome_tx,
            }) {
                // Channel full or closed — resolve with a terminal outcome,
                // preserving the envelope's message_id. The rejected item is the
                // Envelope we just submitted; mailw never enqueues a Raw.
                let WriteItem::Envelope {
                    outcome_tx,
                    envelope,
                } = error.into_inner()
                else {
                    unreachable!("mailw only enqueues Envelope write items");
                };
                let _ = outcome_tx.send(self.unavailable_outcome_with_id(&envelope.message_id));
            }
        } else {
            self.resignal_respawn_if_dead();
            let _ = outcome_tx.send(self.unavailable_outcome_with_id(&envelope.message_id));
        }
        outcome_rx
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        if let Some(tx) = self.write_tx.as_ref() {
            if let Err(error) = tx.try_send(WriteItem::Raw {
                content,
                append_enter,
                outcome_tx,
            }) {
                // The rejected item is the Raw we just submitted; raww never
                // enqueues an Envelope.
                let WriteItem::Raw { outcome_tx, .. } = error.into_inner() else {
                    unreachable!("raww only enqueues Raw write items");
                };
                let _ = outcome_tx.send(self.unavailable_outcome_with_id(""));
            }
        } else {
            self.resignal_respawn_if_dead();
            let _ = outcome_tx.send(self.unavailable_outcome_with_id(""));
        }
        outcome_rx
    }

    fn is_ready(&self) -> bool {
        matches!(
            self.readiness(),
            WorkerReadinessState::Available | WorkerReadinessState::Busy
        )
    }

    fn can_accept_handover(&self) -> bool {
        matches!(self.readiness(), WorkerReadinessState::Available)
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
    namespace: &'a str,
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
///
/// The batch carries a single `prime_timeout_ms` taken from the head envelope;
/// the entire flush group's per-turn prime wait is governed by the head
/// envelope's deadline. Coalesce-during-wait does NOT extend or restart the
/// prime window — absorbed envelopes inherit the head envelope's anchor. The
/// invariance is enforced by [`EnvelopeBatch::absorb_envelope`], which never
/// touches `prime_timeout_ms` regardless of the incoming envelope's value.
pub(super) struct EnvelopeBatch {
    rendered: Vec<String>,
    message_ids: Vec<String>,
    decider_sessions: Vec<Vec<String>>,
    outcome_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>>,
    prime_timeout_ms: Option<u64>,
}

impl EnvelopeBatch {
    /// Builds a single-envelope batch from the head envelope's submitted
    /// write item. The prime anchor is taken from the head envelope; absorbed
    /// envelopes do not re-set it.
    fn from_head(
        envelope: &DeliveryEnvelope,
        outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    ) -> Self {
        Self {
            rendered: vec![envelope.message.render_pane_envelope(&envelope.message_id)],
            message_ids: vec![envelope.message_id.clone()],
            decider_sessions: vec![envelope.choice_decider_sessions.clone()],
            outcome_senders: vec![outcome_tx],
            prime_timeout_ms: envelope.prime_timeout_ms,
        }
    }

    /// Absorbs an additional envelope into this batch during the outer
    /// coalesce loop. Pushes rendered output, message id, decider
    /// sessions, and outcome sender. Deliberately does NOT touch
    /// `prime_timeout_ms`: absorbed envelopes inherit the head envelope's
    /// prime anchor (Decision 3). The absorbed envelope's own
    /// `prime_timeout_ms` is ignored for the flush group's prime timer.
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
    respawn_needed_tx: tokio::sync::watch::Sender<bool>,
}

struct DeliveryTaskIdentity {
    namespace: String,
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
        namespace: &identity.namespace,
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
/// the receipt is its own turn and is observable on its own. The
/// envelope's `prime_timeout_ms` (if any, from the sender's
/// `[coders.<id>.acp].prime-timeout-ms`) governs the singleton turn's
/// bounded prime wait, identical to a head envelope's; `quiet_window` is
/// unused on ACP and the relay's `build_coder_envelope` zeros it for
/// receipts addressed to an ACP sender so the receipt-bypasses-quiescence
/// invariant holds at the envelope seam.
fn submit_singleton_envelope(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    batch_settings: &PromptBatchSettings,
    respawn_needed_tx: &tokio::sync::watch::Sender<bool>,
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

/// True when a settled [`SingleDeliveryOutcome`] indicates the underlying
/// agent has died and the worker should respawn. The decision rides on the
/// observable failure (the outcome's `reason_code`) rather than the
/// transport's readiness state. `SerializationFailed` is intentionally NOT
/// a respawn trigger: a bad message cannot be retried by respawning the
/// agent.
///
/// On an ACP settlement, the only `SendOutcome::Timeout` source is the
/// per-turn prime timer; treating it as a respawn trigger preserves the
/// prior readiness-latch behavior on that path.
fn outcome_requires_respawn(outcome: &SingleDeliveryOutcome) -> bool {
    match outcome.outcome {
        SendOutcome::Timeout => true,
        SendOutcome::Failed => matches!(
            outcome.reason_code.as_deref(),
            Some(ACP_ERROR_CODE_CONNECTION_CLOSED) | Some(ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE),
        ),
        _ => false,
    }
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
    respawn_needed_tx: &tokio::sync::watch::Sender<bool>,
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
            let result = submit_raw_turn(client, ctx, content.as_str(), append_enter);
            let _ = outcome_tx.send(result.clone());
            // The raw path runs through `submit_envelope_turn`, so the
            // outcome is a settled `SingleDeliveryOutcome` and the same
            // observable-event rule applies.
            if outcome_requires_respawn(&result) {
                let _ = respawn_needed_tx.send(true);
            }
        }
    }
}

/// Combines a contiguous batch of rendered envelopes into token-budget-bounded
/// turn prompts via [`crate::envelope::batch_envelope_groups`], submits each
/// group as one turn, and fans that turn's outcome to the contributing senders.
/// Each sender receives its own message_id in the outcome, even when multiple
/// envelopes are combined into one turn. The group's head message_id and decider
/// sessions correlate any choice raised mid-turn.
///
/// `prime_timeout_ms` from the batch governs every turn submitted in this
/// flush group; the value is set once at head-envelope time and does NOT
/// change across coalesce iterations or token-budget splits.
fn flush_envelope_group(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    batch_settings: &PromptBatchSettings,
    respawn_needed_tx: &tokio::sync::watch::Sender<bool>,
    batch: &mut EnvelopeBatch,
) {
    let groups = crate::envelope::batch_envelope_groups(&batch.rendered, *batch_settings);
    batch.rendered.clear();
    let prime_timeout_ms = batch.prime_timeout_ms;
    let mut message_ids = batch.message_ids.drain(..);
    let mut decider_sessions = batch.decider_sessions.drain(..);
    let mut outcome_senders = batch.outcome_senders.drain(..);

    let mut last_outcome: Option<SingleDeliveryOutcome> = None;
    for group in groups {
        let group_msg_ids: Vec<String> = message_ids.by_ref().take(group.member_count).collect();
        let group_deciders: Vec<Vec<String>> =
            decider_sessions.by_ref().take(group.member_count).collect();
        let group_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>> =
            outcome_senders.by_ref().take(group.member_count).collect();
        let head_msg_id = group_msg_ids.first().cloned().unwrap_or_default();
        let head_deciders = group_deciders.into_iter().next().unwrap_or_default();
        let outcome = submit_envelope_turn(
            client,
            ctx,
            &group.combined_prompt,
            &head_msg_id,
            &group_msg_ids,
            &head_deciders,
            prime_timeout_ms,
        );
        for (sender_msg_id, tx) in group_msg_ids.into_iter().zip(group_senders) {
            let mut sender_outcome = outcome.clone();
            sender_outcome.message_id = sender_msg_id;
            let _ = tx.send(sender_outcome);
        }
        last_outcome = Some(outcome);
    }

    // The signal flows from the observable outcome (the reason a turn did
    // not land cleanly), not from the readiness state. The driver-owned
    // respawn monitor reads this and drives replacement.
    if let Some(outcome) = last_outcome
        && outcome_requires_respawn(&outcome)
    {
        let _ = respawn_needed_tx.send(true);
    }
}

/// Submits one combined prompt as an ACP turn, blocking until completion.
///
/// `prime_timeout_ms` is the head envelope's bounded-prime-window value. When
/// `Some(ms)`, the per-turn wait loop tracks a prime timer anchored at first
/// wait start (does NOT reset on coalesce iterations; this function is invoked
/// once per group, so there are no internal coalesce iterations to reset on).
/// On prime-timer fire the loop exits with `SendOutcome::Timeout` +
/// `reason_code = "acp_turn_timeout"`, latches per-target readiness to
/// `Unavailable`, and emits a `delivery_prime_timeout` inscription. The
/// returned `SendOutcome::Timeout` is the signal that `flush_envelope_group`
/// uses, via `outcome_requires_respawn`, to publish the respawn-need the
/// monitor consumes. When `None`, the prime timer is unbounded and the
/// loop exits only on completion, shutdown, or transport failure.
#[allow(clippy::too_many_arguments)]
fn submit_envelope_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    prompt: &str,
    message_id: &str,
    message_ids: &[String],
    decider_sessions: &[String],
    prime_timeout_ms: Option<u64>,
) -> SingleDeliveryOutcome {
    let pending_choice: Arc<Mutex<Option<ChoiceMade>>> = Arc::new(Mutex::new(None));
    let completion_slot: Arc<Mutex<Option<PromptCompletion>>> = Arc::new(Mutex::new(None));
    // Set to `true` by the wrapper around the real permission handler when
    // the agent raises a `session/request_permission` for this turn. Used by
    // the prime-timer suppression predicate to distinguish "no choice was
    // raised" (suppress=false; the prime timer can fire normally) from "a
    // choice is in flight, operator has not yet decided" (suppress=true; the
    // prime timer must NOT fire because the operator is mid-decision and
    // firing `Timeout` would be a false positive).
    let permission_was_raised: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    let shared_for_dispatch = Arc::clone(ctx.shared);
    let on_dispatched: DispatchHandler = Box::new(move || {
        set_shared_readiness(&shared_for_dispatch, WorkerReadinessState::Busy);
    });

    let on_permission = if let Some(chooser) = ctx.chooser {
        let correlation = ChoiceCorrelation {
            message_id: message_id.to_string(),
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
        let raised_flag = Arc::clone(&permission_was_raised);
        let wrapped: PermissionHandler = Box::new(move |req, responder| {
            raised_flag.store(true, Ordering::Release);
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

    let dispatch = client.prompt(
        ctx.session_id,
        prompt,
        Some(on_dispatched),
        Some(on_permission),
        on_completion,
    );

    match dispatch {
        PromptDispatchOutcome::Submitted => {
            // Prime timer anchor: "first wait start" per
            // `acp-prime-timeout-and-wedge-detection/design.md` Decision 3.
            // The timer starts when this loop first enters the wait; not at
            // envelope enqueue time, not at `client.prompt` call time. The
            // deadline is `None` when no per-coder prime timeout is configured,
            // which preserves today's unbounded behavior.
            let prime_started_at = Instant::now();
            let prime_deadline =
                prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));
            let timed_out = run_prime_bounded_wait(
                client,
                prime_deadline,
                &completion_slot,
                &pending_choice,
                &permission_was_raised,
            );
            if timed_out {
                // Prime timer fired before the `PromptCompletion` callback ran
                // and before any pending choice resolved. Resolve the flush
                // group as `Timeout` + `acp_turn_timeout`, latch readiness to
                // `Unavailable` (matching `PromptCompletion::ConnectionClosed`),
                // and emit the diagnostic. Do NOT cancel the in-flight prompt
                // via `client.cancel()` — the prompt may still resolve and we
                // should not assume the server honors cancellation; the
                // transport resolves this flush group and the relay does not
                // inject further messages until the worker respawns. The
                // accompanying `SendOutcome::Timeout` outcome is observed by
                // `outcome_requires_respawn` in `flush_envelope_group`, which
                // publishes the respawn signal the monitor consumes —
                // replacing the prior readiness-latch path.
                //
                // The leaked `last_prompt_signal` on the client is cleaned up
                // by the next `client.prompt` (it overwrites the slot) or by
                // `client.shutdown` (which closes the channel). The
                // `run_prime_bounded_wait` exit path skips the
                // `completion_slot.take()` below; any future completion that
                // arrives after we return is dropped by the next-turn
                // overwrite of the slot.
                let elapsed_ms = Instant::now()
                    .saturating_duration_since(prime_started_at)
                    .as_millis();
                let timeout_ms = prime_timeout_ms.unwrap_or(0);
                emit_delivery_progress(
                    "delivery_prime_timeout",
                    &DeliveryDiagnosticContext::new(
                        ctx.namespace,
                        ctx.target_session,
                        message_ids.iter().map(String::as_str),
                    ),
                    json!({
                        "timeout_ms": timeout_ms,
                        "prime_wait_elapsed_ms": elapsed_ms,
                    }),
                );
                let outcome = prime_timeout_outcome(
                    ctx.target_session.to_string(),
                    message_id.to_string(),
                    timeout_ms,
                );
                set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
                return outcome;
            }
            let completion = completion_slot
                .lock()
                .expect("completion slot mutex")
                .take();
            let pending = pending_choice.lock().expect("pending_choice mutex").take();
            let (final_state, outcome) = build_acp_completion_result(
                completion,
                pending,
                ctx.target_session.to_string(),
                message_id.to_string(),
                ctx.target_session,
            );
            set_turn_readiness(ctx, final_state);
            outcome
        }
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
            failed_outcome_with_code(
                ctx.target_session.to_string(),
                message_id.to_string(),
                ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                "ACP child stdin write failed",
                Some(json!({ "reason": reason })),
            )
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
            failed_outcome_with_code(
                ctx.target_session.to_string(),
                message_id.to_string(),
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt dispatch failed",
                Some(json!({ "reason": reason })),
            )
        }
    }
}

/// Submits raw content as an ACP turn (no envelope framing). Raw-mode writes
/// are intentionally NOT bounded by the prime timer — they preserve today's
/// unbounded behavior because there is no per-raw-write envelope to read
/// `prime_timeout_ms` from.
fn submit_raw_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    content: &str,
    _append_enter: bool,
) -> SingleDeliveryOutcome {
    submit_envelope_turn(client, ctx, content, "", &[], &[], None)
}

/// Polls the prompt completion slot under a bounded prime wait.
///
/// Returns `true` if the prime window elapsed AND no `PromptCompletion` was
/// observed AND any permission request raised for the turn has resolved by the
/// time the deadline hit (an unresolved choice mid-turn suppresses the prime
/// fire). Returns `false` for normal exits (completion observed, shutdown
/// requested).
///
/// `prime_deadline` is `None` for the unbounded case; the helper still loops
/// but the deadline check never trips. `permission_was_raised` is `true` when
/// the agent raised a `session/request_permission` and the operator decision
/// has not yet landed in `pending_choice`; in that state the prime timer
/// keeps waiting. This matches the
/// `acp-prime-timeout-and-wedge-detection/design.md` Decisions 3 and 6.
fn run_prime_bounded_wait(
    client: &mut AcpStdioClient,
    prime_deadline: Option<Instant>,
    completion_slot: &Arc<Mutex<Option<PromptCompletion>>>,
    pending_choice: &Arc<Mutex<Option<ChoiceMade>>>,
    permission_was_raised: &Arc<AtomicBool>,
) -> bool {
    loop {
        if client.wait_for_prompt_complete(ACP_PROMPT_WAIT_POLL_INTERVAL) {
            return false;
        }
        if shutdown_requested() {
            return false;
        }
        if let Some(deadline) = prime_deadline
            && Instant::now() >= deadline
        {
            let raised = permission_was_raised.load(Ordering::Acquire);
            let resolved = pending_choice
                .lock()
                .expect("pending_choice mutex")
                .is_some();
            if raised && !resolved {
                continue;
            }
            if completion_slot
                .lock()
                .expect("completion slot mutex")
                .is_some()
            {
                return false;
            }
            return true;
        }
    }
}

/// Builds the `SendOutcome::Timeout` + `reason_code = "acp_turn_timeout"`
/// outcome used when the prime timer fires. The `details` payload records
/// the operator-configured deadline so operators can correlate the failure
/// to the bundle config that produced it.
fn prime_timeout_outcome(
    target_session: String,
    message_id: String,
    timeout_ms: u64,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Timeout,
        reason_code: Some(ACP_REASON_CODE_PRIME_TIMEOUT.to_string()),
        reason: Some("ACP prime timer elapsed before prompt completion".to_string()),
        details: Some(json!({ "timeout_ms": timeout_ms })),
    }
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

fn failed_outcome(
    target_session: String,
    message_id: String,
    reason: impl Into<String>,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Failed,
        reason_code: None,
        reason: Some(reason.into()),
        details: None,
    }
}

fn failed_outcome_with_code(
    target_session: String,
    message_id: String,
    reason_code: &str,
    reason: impl Into<String>,
    details: Option<Value>,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.into()),
        details,
    }
}

fn build_acp_completion_result(
    completion: Option<PromptCompletion>,
    pending_choice_outcome: Option<ChoiceMade>,
    target_session: String,
    message_id: String,
    target_member_id: &str,
) -> (WorkerReadinessState, SingleDeliveryOutcome) {
    if let Some(ChoiceMade::Cancelled {
        reason_code,
        reason,
        ..
    }) = pending_choice_outcome
    {
        return (
            WorkerReadinessState::Available,
            failed_outcome_with_code(
                target_session,
                message_id,
                reason_code.as_str(),
                reason.unwrap_or_else(|| "choice request was cancelled".to_string()),
                Some(json!({ "target_session": target_member_id })),
            ),
        );
    }

    let Some(completion) = completion else {
        // No completion observed before the wait was abandoned: shutdown. Report
        // the dropped-on-shutdown outcome (not a generic failure), matching the
        // queued-write drop path and the tmux transport.
        return (
            WorkerReadinessState::Available,
            SingleDeliveryOutcome {
                target_session,
                message_id,
                outcome: SendOutcome::DroppedOnShutdown,
                reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                details: None,
            },
        );
    };

    match completion {
        PromptCompletion::Completed { stop_reason } => match stop_reason.as_str() {
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => (
                WorkerReadinessState::Available,
                delivered_outcome(target_session, message_id),
            ),
            "cancelled" => (
                WorkerReadinessState::Available,
                failed_outcome_with_code(
                    target_session,
                    message_id,
                    ACP_REASON_CODE_STOP_CANCELLED,
                    "ACP turn completed with stopReason=cancelled",
                    None,
                ),
            ),
            other => (
                WorkerReadinessState::Available,
                failed_outcome(
                    target_session,
                    message_id,
                    format!("ACP returned unsupported stopReason '{other}'"),
                ),
            ),
        },
        PromptCompletion::ProtocolError(reason) => (
            WorkerReadinessState::Available,
            failed_outcome_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt failed",
                Some(json!({ "target_session": target_member_id, "reason": reason })),
            ),
        ),
        PromptCompletion::ConnectionClosed { reason } => (
            WorkerReadinessState::Unavailable,
            failed_outcome_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_CONNECTION_CLOSED,
                "ACP connection closed before prompt response",
                Some(json!({ "target_session": target_member_id, "reason": reason })),
            ),
        ),
    }
}

#[cfg(test)]
mod envelope_batch_prime_anchor_tests {
    //! Inline coverage for the `EnvelopeBatch` prime-anchor invariant under
    //! outer-coalesce absorb. Crate-private by design (the batch is an
    //! internal of `acp_delivery_task`); the public surface (`Transport`)
    //! does not exercise this path end-to-end (it would require two
    //! `mailw` calls racing the delivery task's inner coalesce loop,
    //! which is non-deterministic from the harness). One test function
    //! per AGENTS.md convention.
    use super::*;
    use crate::envelope::AddressIdentity;
    use crate::transports::{DeliveryEnvelope, DeliveryMessage};

    fn head_envelope(message: &str, message_id: &str, prime: Option<u64>) -> DeliveryEnvelope {
        let party = AddressIdentity {
            session_name: "alpha".to_string(),
            display_name: None,
        };
        DeliveryEnvelope {
            message_id: message_id.to_string(),
            message: DeliveryMessage {
                body: message.to_string(),
                created_at: "1970-01-01T00:00:00Z".to_string(),
                namespace: "party".to_string(),
                sender: party.clone(),
                target: party.clone(),
                cc: vec![],
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: vec!["alpha".to_string()],
            quiet_window: Duration::ZERO,
            prime_timeout_ms: prime,
            readiness_timeout_ms: None,
            is_receipt: false,
        }
    }

    #[test]
    fn absorbed_envelope_inherits_head_prime_anchor() {
        // Head envelope anchors the flush group's prime at 100 ms. The
        // absorbed envelope carries a deliberately larger 99_999 ms
        // value; per Decision 3 the absorbed envelope MUST NOT extend
        // or restart the prime window. Constructing two envelopes with
        // distinct prime values, absorbing the second, then asserting the
        // batch's `prime_timeout_ms` is the head's value is the
        // deterministic test of that invariant.
        let head = head_envelope("hello", "msg-head", Some(100));
        let (head_tx, _head_rx) = tokio::sync::oneshot::channel();
        let mut batch = EnvelopeBatch::from_head(&head, head_tx);
        assert_eq!(batch.prime_timeout_ms, Some(100));
        assert_eq!(batch.rendered.len(), 1);
        assert_eq!(batch.message_ids, vec!["msg-head".to_string()]);

        let absorbed = head_envelope("world", "msg-absorbed", Some(99_999));
        let (absorbed_tx, _absorbed_rx) = tokio::sync::oneshot::channel();
        batch.absorb_envelope(&absorbed, absorbed_tx);

        // Prime anchor stays at the head envelope's value; the absorbed
        // envelope's own `prime_timeout_ms` is ignored.
        assert_eq!(batch.prime_timeout_ms, Some(100));
        assert_eq!(batch.message_ids, vec!["msg-head", "msg-absorbed"]);
        assert_eq!(batch.outcome_senders.len(), 2);
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
            prime_timeout_ms: None,
            readiness_timeout_ms: None,
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
    fn can_accept_handover_readiness_matrix_and_delivery_task_handle_retention() {
        let mut transport = AcpTransport::new(PromptBatchSettings::default(), None);

        // Initial state is `Initializing` — not ready for handover.
        assert_eq!(
            transport.readiness(),
            crate::transports::WorkerReadinessState::Initializing
        );
        assert!(!transport.can_accept_handover());

        // The single state that returns true: only when the transport is
        // actually idle and able to take a batch right now.
        transport.set_readiness(crate::transports::WorkerReadinessState::Available);
        assert!(transport.can_accept_handover());

        // `Busy` is intentionally NOT a handover-ready state — accepting
        // another batch while a turn is in flight would dispatch the wrong
        // message to the same turn. The readiness signal still exists
        // through the injected mirror closure.
        transport.set_readiness(crate::transports::WorkerReadinessState::Busy);
        assert!(!transport.can_accept_handover());
        // `is_ready` keeps its pre-existing semantic and continues to
        // include `Busy`; the two surfaces now differ by design.
        assert!(transport.is_ready());

        // `Recovering` and `Unavailable` are also not ready.
        transport.set_readiness(crate::transports::WorkerReadinessState::Recovering);
        assert!(!transport.can_accept_handover());
        transport.set_readiness(crate::transports::WorkerReadinessState::Unavailable);
        assert!(!transport.can_accept_handover());

        // No delivery task has been spawned yet, so there is no handle
        // for a generation supervisor to take — the field starts empty
        // and a second `take` after the first stays empty.
        assert!(transport.take_delivery_task_handle().is_none());
        assert!(transport.take_delivery_task_handle().is_none());
    }
}
