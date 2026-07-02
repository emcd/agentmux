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

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::acp::client::SharedReplay;
use crate::acp::permission::{ChoiceCorrelation, build_acp_permission_handler};
use crate::acp::state::{
    AcpLookSnapshot, derive_acp_look_snapshot, load_persisted_acp_session_id,
    persist_acp_session_id,
};
use crate::acp::{
    AcpStdioClient, DispatchHandler, PermissionHandler, PermissionResponder, PromptCompletion,
    PromptCompletionHandler, PromptDispatchOutcome,
};

use crate::configuration::{AcpChannel, AcpTargetConfiguration, BundleMember, TargetConfiguration};
use crate::envelope::PromptBatchSettings;
use crate::runtime::inscriptions::emit_delivery_diagnostic;
use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    ChoiceMade, DeliveryEnvelope, LookMode, LookSnapshotPayload, OutputView, SingleDeliveryOutcome,
    StartupContext, Transport, TransportError, TransportReadiness, TransportStatus,
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
/// Bootstrap initialize failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
const ACP_ERROR_CODE_SESSION_LOAD_FAILED: &str = "runtime_acp_session_load_failed";
const ACP_ERROR_CODE_SESSION_NEW_FAILED: &str = "runtime_acp_session_new_failed";
/// Prompt-dispatch failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_PROMPT_FAILED: &str = "runtime_acp_prompt_failed";
/// Connection-closed failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_CONNECTION_CLOSED: &str = "runtime_acp_connection_closed";
/// Transport-unavailable failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE: &str = "acp_child_unavailable";
const ACP_ERROR_CODE_MISSING_CAPABILITY: &str = "validation_missing_acp_capability";

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

/// The persistent ACP runtime owned by an [`AcpTransport`]: the stdio client and
/// the resolved session id used for every prompt.
pub struct PersistentAcpWorkerRuntime {
    pub client: AcpStdioClient,
    pub session_id: String,
}

/// A structured bootstrap failure. Surfaced to the relay worker so it can decide
/// whether respawn might recover or the failure is permanent.
#[derive(Clone, Debug)]
pub struct AcpBootstrapError {
    pub code: String,
    pub reason: String,
}

impl AcpBootstrapError {
    /// Permanent failures are conditions respawn cannot resolve: a capability gap
    /// means the agent fundamentally cannot host the session, so retrying with the
    /// same binary reproduces the failure.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        self.code == ACP_ERROR_CODE_MISSING_CAPABILITY
    }
}

#[derive(Clone, Copy, Debug)]
enum AcpLifecycleSelection {
    NewSession,
    LoadSession,
}

#[derive(Clone, Debug)]
struct AcpCapabilities {
    load_session: bool,
    prompt_session: bool,
}

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
    /// Stable respawn-needed signal. Created once at construction (not per
    /// delivery task) so the driver-owned async respawn monitor can hold a single
    /// long-lived subscription across respawns. The internal delivery task holds a
    /// clone and sets it `true` when a turn ends Unavailable; the monitor awaits
    /// the change, drives the respawn, then resets it to `false`.
    respawn_needed_tx: tokio::sync::watch::Sender<bool>,
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
            }),
            write_tx: None,
            shutdown_tx: None,
            batch_settings,
            target_session: String::new(),
            respawn_needed_tx: tokio::sync::watch::channel(false).0,
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
    /// and readiness state after startup. The task holds a clone of the stable
    /// respawn-needed sender, which it sets `true` when a turn ends Unavailable so
    /// the driver-owned respawn monitor can react.
    fn spawn_delivery_task(&mut self) {
        let (tx, rx) = mpsc::channel::<WriteItem>(ACP_WRITE_CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let respawn_needed_tx = self.respawn_needed_tx.clone();
        let shared = Arc::clone(&self.shared);
        let batch_settings = self.batch_settings;
        let chooser = self.chooser.clone();
        let target_session = self.target_session.clone();

        let runtime = self.runtime.take().expect("runtime present at task spawn");
        let client = runtime.client;
        let session_id = runtime.session_id;

        std::thread::spawn(move || {
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
                target_session,
            );
        });

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

    /// Installs a freshly bootstrapped runtime: repoints the published replay
    /// handle at the new buffer, marks the transport Available, and spawns the
    /// internal delivery task. Brief and lock-safe — the blocking child spawn
    /// already happened in `bootstrap_acp_worker_runtime`, so the respawn monitor
    /// holds the transport lock only for these fast field updates.
    pub(crate) fn install_runtime(&mut self, runtime: PersistentAcpWorkerRuntime) {
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

impl Transport for AcpTransport {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        self.prepare_for_startup(context.choose, context.target_member.id.clone());
        self.set_readiness(WorkerReadinessState::Initializing);
        match bootstrap_acp_worker_runtime(&context.runtime_directory, &context.target_member) {
            Ok(runtime) => {
                self.install_runtime(runtime);
                Ok(TransportStatus {
                    readiness: TransportReadiness::Ready,
                })
            }
            Err(error) => {
                self.mark_runtime_unavailable();
                Err(TransportError {
                    code: error.code,
                    reason: error.reason,
                    details: None,
                })
            }
        }
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
    target_session: String,
) {
    let ctx = TurnContext {
        session_id: &session_id,
        shared: &shared,
        chooser: &chooser,
        target_session: &target_session,
    };

    let mut rx = channels.rx;
    let mut shutdown_rx = channels.shutdown_rx;
    let respawn_needed_tx = channels.respawn_needed_tx;

    // Helper: check if shutdown/respawn signal fired. Returns true if shutdown
    // is active. Caller is responsible for resolving any held senders.
    let is_shutdown = |shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>| -> bool {
        matches!(
            shutdown_rx.try_recv(),
            Ok(()) | Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        )
    };

    loop {
        if is_shutdown(&mut shutdown_rx) {
            drain_and_resolve_shutdown(&mut rx);
            break;
        }

        let Some(first) = rx.blocking_recv() else {
            break;
        };

        match first {
            WriteItem::Envelope {
                envelope,
                outcome_tx,
            } => {
                // Check after receive — shutdown may have fired between the
                // pre-receive check and the actual receive.
                if is_shutdown(&mut shutdown_rx) {
                    let _ = outcome_tx.send(dropped_on_shutdown_outcome());
                    drain_and_resolve_shutdown(&mut rx);
                    break;
                }
                let mut batch = EnvelopeBatch::from_head(&envelope, outcome_tx);

                loop {
                    match rx.try_recv() {
                        Ok(WriteItem::Envelope {
                            envelope: next_env,
                            outcome_tx: next_tx,
                        }) => {
                            batch.absorb_envelope(&next_env, next_tx);
                        }
                        Ok(WriteItem::Raw {
                            content,
                            append_enter,
                            outcome_tx: raw_tx,
                        }) => {
                            if is_shutdown(&mut shutdown_rx) {
                                let _ = raw_tx.send(dropped_on_shutdown_outcome());
                                for tx in batch.outcome_senders.drain(..) {
                                    let _ = tx.send(dropped_on_shutdown_outcome());
                                }
                                drain_and_resolve_shutdown(&mut rx);
                                break;
                            }
                            flush_envelope_group(&mut client, &ctx, &batch_settings, &mut batch);
                            signal_respawn_if_needed(ctx.shared, &respawn_needed_tx);
                            let result =
                                submit_raw_turn(&mut client, &ctx, content.as_str(), append_enter);
                            let _ = raw_tx.send(result);
                            break;
                        }
                        Err(_) => {
                            if is_shutdown(&mut shutdown_rx) {
                                for tx in batch.outcome_senders.drain(..) {
                                    let _ = tx.send(dropped_on_shutdown_outcome());
                                }
                                drain_and_resolve_shutdown(&mut rx);
                                break;
                            }
                            flush_envelope_group(&mut client, &ctx, &batch_settings, &mut batch);
                            signal_respawn_if_needed(ctx.shared, &respawn_needed_tx);
                            break;
                        }
                    }
                }
            }
            WriteItem::Raw {
                content,
                append_enter,
                outcome_tx,
            } => {
                if is_shutdown(&mut shutdown_rx) {
                    let _ = outcome_tx.send(dropped_on_shutdown_outcome());
                    drain_and_resolve_shutdown(&mut rx);
                    break;
                }
                let result = submit_raw_turn(&mut client, &ctx, content.as_str(), append_enter);
                let _ = outcome_tx.send(result);
                signal_respawn_if_needed(ctx.shared, &respawn_needed_tx);
            }
        }
    }
}

/// Signals the driver if the transport's readiness is Unavailable after a turn.
fn signal_respawn_if_needed(
    shared: &Arc<AcpSharedState>,
    respawn_needed_tx: &tokio::sync::watch::Sender<bool>,
) {
    let readiness = *shared.readiness.lock().expect("readiness mutex");
    if matches!(readiness, WorkerReadinessState::Unavailable) {
        let _ = respawn_needed_tx.send(true);
    }
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
    batch: &mut EnvelopeBatch,
) {
    let groups = crate::envelope::batch_envelope_groups(&batch.rendered, *batch_settings);
    batch.rendered.clear();
    let prime_timeout_ms = batch.prime_timeout_ms;
    let mut message_ids = batch.message_ids.drain(..);
    let mut decider_sessions = batch.decider_sessions.drain(..);
    let mut outcome_senders = batch.outcome_senders.drain(..);

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
            &head_deciders,
            prime_timeout_ms,
        );
        for (sender_msg_id, tx) in group_msg_ids.into_iter().zip(group_senders) {
            let mut sender_outcome = outcome.clone();
            sender_outcome.message_id = sender_msg_id;
            let _ = tx.send(sender_outcome);
        }
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
/// `Unavailable`, emits a `delivery_prime_timeout` inscription, and signals
/// respawn-needed. When `None`, the prime timer is unbounded (today's default
/// behavior) and the loop exits only on completion, shutdown, or transport
/// failure.
#[allow(clippy::too_many_arguments)]
fn submit_envelope_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    prompt: &str,
    message_id: &str,
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
        let mut inner =
            build_acp_permission_handler(chooser.clone(), correlation, Arc::clone(&pending_choice));
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
                // outer `acp_delivery_task` loop's `signal_respawn_if_needed`
                // call publishes the respawn-needed signal once it observes
                // `readiness == Unavailable` after this `submit_envelope_turn`
                // returns, so no separate publish is needed here.
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
                emit_delivery_diagnostic(
                    "delivery_prime_timeout",
                    &json!({
                        "target_session": ctx.target_session,
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
    submit_envelope_turn(client, ctx, content, "", &[], None)
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

/// Builds the per-target ACP runtime. Used by the relay worker for initial
/// bootstrap and respawn (the worker re-publishes the [`OutputView`] handle
/// afterward via [`Transport::give_output`]).
pub fn bootstrap_acp_worker_runtime(
    runtime_directory: &Path,
    target_member: &BundleMember,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let TargetConfiguration::Acp(acp_target) = &target_member.target else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires ACP target".to_string(),
        });
    };
    let Some(working_directory) = target_member.working_directory.as_ref() else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires target working directory".to_string(),
        });
    };
    initialize_persistent_acp_worker_runtime(
        target_member,
        acp_target,
        working_directory.as_path(),
        runtime_directory,
    )
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

fn initialize_persistent_acp_worker_runtime(
    target_member: &BundleMember,
    acp: &AcpTargetConfiguration,
    working_directory: &Path,
    runtime_directory: &Path,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let mut client = match acp.channel {
        AcpChannel::Stdio => {
            let Some(command) = acp.command.as_deref() else {
                return Err(AcpBootstrapError {
                    code: "runtime_startup_failed".to_string(),
                    reason: "ACP stdio target requires command".to_string(),
                });
            };
            AcpStdioClient::spawn(
                command,
                working_directory,
                &acp.environment
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.value.clone()))
                    .collect::<Vec<_>>(),
                true,
            )
            .map_err(|reason| AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason,
            })?
        }
        AcpChannel::Http => {
            return Err(AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason: "ACP http transport is not implemented".to_string(),
            });
        }
    };

    let initialize_result = client.initialize().map_err(|reason| AcpBootstrapError {
        code: ACP_ERROR_CODE_INITIALIZE_FAILED.to_string(),
        reason: format!("ACP initialize failed: {reason}"),
    })?;

    let capabilities = AcpCapabilities {
        load_session: initialize_result
            .get("agentCapabilities")
            .and_then(|value| value.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prompt_session: initialize_result
            .get("agentCapabilities")
            .map(|value| {
                value
                    .get("promptSession")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| {
                        value
                            .get("promptCapabilities")
                            .is_some_and(serde_json::Value::is_object)
                    })
            })
            .unwrap_or(false),
    };

    let persisted_session_id = if target_member.coder_session_id.is_some() {
        None
    } else {
        load_persisted_acp_session_id(runtime_directory, target_member.id.as_str()).map_err(
            |reason| AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason: format!("failed to load persisted ACP session id: {reason}"),
            },
        )?
    };

    let (lifecycle, lifecycle_session_id) =
        if let Some(configured) = target_member.coder_session_id.as_deref() {
            (AcpLifecycleSelection::LoadSession, configured.to_string())
        } else if let Some(persisted) = persisted_session_id {
            (AcpLifecycleSelection::LoadSession, persisted)
        } else {
            (AcpLifecycleSelection::NewSession, String::new())
        };

    let session_id = match lifecycle {
        AcpLifecycleSelection::LoadSession => {
            if !capabilities.load_session {
                return Err(AcpBootstrapError {
                    code: ACP_ERROR_CODE_MISSING_CAPABILITY.to_string(),
                    reason: "ACP agent does not advertise required load capability".to_string(),
                });
            }
            client
                .load_session(lifecycle_session_id.as_str(), working_directory)
                .map_err(|reason| AcpBootstrapError {
                    code: ACP_ERROR_CODE_SESSION_LOAD_FAILED.to_string(),
                    reason: format!("ACP session/load failed: {reason}"),
                })?;
            lifecycle_session_id
        }
        AcpLifecycleSelection::NewSession => {
            client
                .new_session(working_directory)
                .map_err(|reason| AcpBootstrapError {
                    code: ACP_ERROR_CODE_SESSION_NEW_FAILED.to_string(),
                    reason: format!("ACP session/new failed: {reason}"),
                })?
        }
    };

    persist_acp_session_id(
        runtime_directory,
        target_member.id.as_str(),
        session_id.as_str(),
    )
    .map_err(|reason| AcpBootstrapError {
        code: "runtime_startup_failed".to_string(),
        reason: format!("failed to persist ACP session id: {reason}"),
    })?;

    if !capabilities.prompt_session {
        return Err(AcpBootstrapError {
            code: ACP_ERROR_CODE_MISSING_CAPABILITY.to_string(),
            reason: "ACP agent does not advertise required prompt capability".to_string(),
        });
    }

    Ok(PersistentAcpWorkerRuntime { client, session_id })
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
