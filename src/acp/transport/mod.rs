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

use tokio::sync::mpsc;

use crate::acp::client::{AcpGenerationHandle, SharedReplay};
use crate::acp::persistent_runtime::PersistentAcpWorkerRuntime;

use crate::envelope::PromptBatchSettings;
#[cfg(test)]
use crate::transports::SubmissionEvidence;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    DeliveryEnvelope, GenerationFence, OutputView, StartupContext, Transport, TransportError,
    TransportHealth, TransportStatus,
};
use crate::transports::{PartitionSink, WorkerReadinessState};

pub mod state;
use self::state::{
    ACP_WRITE_CHANNEL_CAPACITY, AcpSharedState, BootstrapInFlight, BootstrapRecord,
    BootstrapRegistry, NEXT_BOOTSTRAP_ID, ReadinessMirror, raise_respawn_signal,
};

pub mod output;
pub mod turn;
use self::turn::{not_submitted_outcome, set_shared_readiness};

pub mod delivery;
use self::delivery::{DeliveryChannels, DeliveryTaskIdentity, WriteItem, acp_delivery_task};

// ACP delivery failure taxonomy (see the relay delivery README for the full
// catalogue). The delivery outcomes the ACP transport now produces are typed:
// `Delivered` (framed write succeeded), `NotSubmitted` (positive non-delivery:
// active-prompt refusal or serialization failure), and `SubmissionUnknown`
// (a write/flush error without proof that zero bytes left). Connection close
// after a successful write is target-health observability, not a delivery
// outcome. A write this generation was still holding when it was stopped is
// `NotSubmitted` too — see `stopped_generation_outcome`.

/// Items enqueued onto the ACP transport's internal ordered write channel.
///
/// Both [`Transport::mailw`] and [`Transport::raww`] submit through a single
/// FIFO channel. The internal delivery task processes them in order; a `Raw`
/// item acts as a batch barrier (flushes any preceding `Envelope` group first).
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
    pub fn new(
        batch_settings: PromptBatchSettings,
        mirror_state: Option<ReadinessMirror>,
        partition_sink: Arc<dyn PartitionSink>,
    ) -> Self {
        Self {
            runtime: None,
            chooser: None,
            shared: Arc::new(AcpSharedState {
                readiness: Mutex::new(WorkerReadinessState::Initializing),
                replay: Mutex::new(None),
                mirror_state,
                partition_sink,
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

    /// Sync readiness predicate answering for itself. The async trait method
    /// delegates here so `mailw`/`raww` keep their sync call without inlining
    /// the rule. Extracted to keep the four ACP readiness call sites from
    /// diverging when the rule gains a condition.
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self.readiness(), WorkerReadinessState::Available)
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
        // Direct readiness check rather than `is_ready_for_handover().await`
        // because `mailw` is synchronous and must not block or await.
        if !self.is_available() {
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
        if !self.is_available() {
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

    async fn is_ready_for_handover(&self) -> bool {
        self.is_available()
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
        Some(Arc::new(output::AcpOutputView {
            shared: Arc::clone(&self.shared),
        }))
    }
}

#[cfg(test)]
mod handover_readiness_tests {
    use super::*;

    #[tokio::test]
    async fn handover_readiness_matrix_and_delivery_task_handle_retention() {
        // Nothing is ever declared here: the test drives readiness predicates
        // and never submits a turn, which is the only situation in which a sink
        // that binds nothing is the right stand-in.
        struct NoDeclarations;
        impl PartitionSink for NoDeclarations {
            fn declare(
                &self,
                _member_ids: &[&str],
            ) -> Result<crate::transports::PackingUnitId, crate::transports::PartitionError>
            {
                Err(crate::transports::PartitionError::MemberNotBindable)
            }
            fn record(
                &self,
                _unit: crate::transports::PackingUnitId,
                _evidence: SubmissionEvidence,
            ) {
            }
        }

        let mut transport = AcpTransport::new(
            PromptBatchSettings::default(),
            None,
            Arc::new(NoDeclarations),
        );

        // Initial state is `Initializing` — not ready for handover.
        assert_eq!(
            transport.readiness(),
            crate::transports::WorkerReadinessState::Initializing
        );
        assert!(!transport.is_ready_for_handover().await);

        // The single state that returns true: only when the transport is
        // actually idle and able to take a batch right now.
        transport.set_readiness(crate::transports::WorkerReadinessState::Available);
        assert!(transport.is_ready_for_handover().await);

        // `Busy` is intentionally NOT a handover-ready state — accepting
        // another batch while a turn is in flight would dispatch the wrong
        // message to the same turn. The readiness signal still exists
        // through the injected mirror closure, which is where observers
        // wanting the wider "runtime exists" reading read it.
        transport.set_readiness(crate::transports::WorkerReadinessState::Busy);
        assert!(!transport.is_ready_for_handover().await);

        // `Recovering` and `Unavailable` are also not ready.
        transport.set_readiness(crate::transports::WorkerReadinessState::Recovering);
        assert!(!transport.is_ready_for_handover().await);
        transport.set_readiness(crate::transports::WorkerReadinessState::Unavailable);
        assert!(!transport.is_ready_for_handover().await);

        // No delivery task has been spawned yet, so there is no handle
        // for a generation supervisor to take — the field starts empty
        // and a second `take` after the first stays empty.
        assert!(transport.take_delivery_task_handle().is_none());
        assert!(transport.take_delivery_task_handle().is_none());
    }
}
