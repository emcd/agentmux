//! ACP delivery as a [`Transport`] implementation.
//!
//! `AcpTransport` owns the per-target `PersistentAcpWorkerRuntime` (moved here
//! from the relay delivery worker, which previously threaded it through
//! `spawn_blocking`). [`Transport::deliver`] submits one ACP prompt and BLOCKS
//! until the turn reaches a terminal state, folding in what used to be the
//! reader thread's `on_completion` body; the relay worker fans the single
//! terminal outcome out to the coalesced tasks.
//!
//! Choices (tool-call permissions) resolve through the relay-injected
//! [`Chooser`] (see [`crate::acp::permission`]); the transport never calls the
//! relay choice queue directly. The `look` path reads output through the
//! [`OutputView`] handle published by [`Transport::give_output`].
//!
//! ## Readiness
//!
//! The transport owns an [`AcpWorkerReadinessState`] signal for [`is_ready`] and
//! the [`OutputView`] prime-wait, because it cannot call relay's
//! `set_acp_worker_state`. The `AcpWorkerDriver` mirrors transitions into the
//! global worker-state registry (which external observers and respawn/startup
//! gating still read).
//!
//! [`is_ready`]: Transport::is_ready

use std::path::Path;
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
    AcpStdioClient, DispatchHandler, PermissionResponder, PromptCompletion,
    PromptCompletionHandler, PromptDispatchOutcome,
};

use crate::configuration::{AcpChannel, AcpTargetConfiguration, BundleMember, TargetConfiguration};
use crate::envelope::PromptBatchSettings;
use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{AcpWorkerReadinessState, SendOutcome};
use crate::transports::{
    ChoiceMade, DeliveryContext, DeliveryEnvelope, DeliveryPreparation, DeliveryResult,
    DeliveryWaitError, LookMode, LookSnapshotPayload, OutputView, RawWriteResult,
    SingleDeliveryOutcome, StartupContext, Transport, TransportError, TransportReadiness,
    TransportStatus,
};

// ACP delivery failure taxonomy (see the relay delivery README for the full
// catalogue). These mirror the codes the relay completion path used before the
// transport move so the wire outcomes are unchanged.
const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
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
const ACP_ERROR_CODE_WORKER_UNAVAILABLE: &str = "runtime_acp_worker_unavailable";

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
    readiness: Mutex<AcpWorkerReadinessState>,
    replay: Mutex<Option<SharedReplay>>,
}

/// Channel capacity for the internal ACP delivery task's write queue.
const ACP_WRITE_CHANNEL_CAPACITY: usize = 256;

/// Items enqueued onto the ACP transport's internal ordered write channel.
///
/// Both [`Transport::mailw`] and [`Transport::raww`] submit through a single
/// FIFO channel. The internal delivery task processes them in order; a `Raw`
/// item acts as a batch barrier (flushes any preceding `Envelope` group first).
enum WriteItem {
    /// Relay-framed envelope for buffered combining and turn submission.
    Envelope {
        envelope: DeliveryEnvelope,
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
    /// Per-prompt token budget for envelope combining.
    max_prompt_tokens: usize,
    /// Target session id, captured at startup for permission correlation.
    target_session: String,
    /// Receiver for respawn-needed signals from the delivery task. The driver
    /// checks this after each delivery; if `true`, calls `maybe_respawn_after_delivery`.
    /// Paired with the `respawn_needed_tx` cloned into the delivery task.
    respawn_needed_rx: Option<tokio::sync::watch::Receiver<bool>>,
}

impl std::fmt::Debug for AcpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpTransport")
            .field("has_runtime", &self.runtime.is_some())
            .field("readiness", &self.readiness())
            .field("has_write_channel", &self.write_tx.is_some())
            .field("max_prompt_tokens", &self.max_prompt_tokens)
            .finish()
    }
}

impl AcpTransport {
    #[must_use]
    pub fn new(max_prompt_tokens: usize) -> Self {
        Self {
            runtime: None,
            chooser: None,
            shared: Arc::new(AcpSharedState {
                readiness: Mutex::new(AcpWorkerReadinessState::Initializing),
                replay: Mutex::new(None),
            }),
            write_tx: None,
            shutdown_tx: None,
            max_prompt_tokens,
            target_session: String::new(),
            respawn_needed_rx: None,
        }
    }

    /// Current readiness, mirrored by the `AcpWorkerDriver` into the global registry.
    #[must_use]
    pub fn readiness(&self) -> AcpWorkerReadinessState {
        *self.shared.readiness.lock().expect("readiness mutex")
    }

    fn set_readiness(&self, state: AcpWorkerReadinessState) {
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
        self.set_readiness(AcpWorkerReadinessState::Recovering);
    }

    /// Spawns the internal delivery task that drains the write channel, combines
    /// contiguous envelopes respecting the token budget, and submits turns to the
    /// ACP runtime. Called from [`Transport::startup`] after the runtime is
    /// established. Takes the client and session_id from the runtime so the task
    /// owns them exclusively — the transport only needs the shared replay handle
    /// and readiness state after startup.
    /// Checks if the delivery task signaled that a respawn is needed (the turn
    /// ended with Unavailable readiness). Returns `true` once per signal; the
    /// driver calls `maybe_respawn_after_delivery` when this returns `true`.
    pub fn check_respawn_needed(&mut self) -> Option<String> {
        if let Some(rx) = self.respawn_needed_rx.as_mut()
            && *rx.borrow_and_update()
        {
            return Some("worker_unavailable".to_string());
        }
        None
    }

    fn spawn_delivery_task(&mut self) {
        let (tx, rx) = mpsc::channel::<WriteItem>(ACP_WRITE_CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (respawn_needed_tx, respawn_needed_rx) = tokio::sync::watch::channel(false);
        let shared = Arc::clone(&self.shared);
        let max_prompt_tokens = self.max_prompt_tokens;
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
                max_prompt_tokens,
                target_session,
            );
        });

        self.write_tx = Some(tx);
        self.shutdown_tx = Some(shutdown_tx);
        self.respawn_needed_rx = Some(respawn_needed_rx);
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
        self.chooser = Some(context.choose);
        self.target_session = context.target_member.id.clone();
        self.set_readiness(AcpWorkerReadinessState::Initializing);
        // Close any existing delivery task's channel before creating a new runtime.
        // The old task will drain and exit; we'll spawn a fresh one below.
        self.write_tx = None;
        match bootstrap_acp_worker_runtime(&context.runtime_directory, &context.target_member) {
            Ok(runtime) => {
                // Repoint the published handle's replay slot at the new runtime's
                // buffer before marking ready, so a look that was prime-waiting
                // through startup returns the fresh buffer.
                self.set_replay(Some(runtime.client.replay_buffer_handle()));
                self.runtime = Some(runtime);
                self.set_readiness(AcpWorkerReadinessState::Available);
                // Spawn the internal delivery task for this runtime.
                self.spawn_delivery_task();
                Ok(TransportStatus {
                    readiness: TransportReadiness::Ready,
                })
            }
            Err(error) => {
                self.runtime = None;
                self.set_replay(None);
                self.set_readiness(AcpWorkerReadinessState::Unavailable);
                Err(TransportError {
                    code: error.code,
                    reason: error.reason,
                    details: None,
                })
            }
        }
    }

    fn prepare_delivery(
        &self,
        context: &DeliveryContext,
    ) -> Result<DeliveryPreparation, DeliveryWaitError> {
        // ACP has no pre-delivery wait — the internal delivery task handles
        // quiescence internally. Echo any pre-resolved target back unchanged.
        Ok(DeliveryPreparation {
            pre_resolved_target: context.pre_resolved_target.clone(),
        })
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        if let Some(tx) = self.write_tx.as_ref() {
            if let Err(e) = tx.try_send(WriteItem::Envelope {
                envelope,
                outcome_tx,
            }) {
                // Channel full or closed — resolve with terminal outcome,
                // preserving the envelope's message_id and target_session.
                match e.into_inner() {
                    WriteItem::Envelope {
                        outcome_tx,
                        envelope,
                    } => {
                        let _ =
                            outcome_tx.send(self.unavailable_outcome_with_id(&envelope.message_id));
                    }
                    WriteItem::Raw { outcome_tx, .. } => {
                        let _ = outcome_tx.send(self.unavailable_outcome_with_id(""));
                    }
                }
            }
        } else {
            let _ = outcome_tx.send(self.unavailable_outcome_with_id(&envelope.message_id));
        }
        outcome_rx
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        if let Some(tx) = self.write_tx.as_ref() {
            if let Err(e) = tx.try_send(WriteItem::Raw {
                content,
                append_enter,
                outcome_tx,
            }) {
                match e.into_inner() {
                    WriteItem::Envelope {
                        outcome_tx,
                        envelope,
                    } => {
                        let _ =
                            outcome_tx.send(self.unavailable_outcome_with_id(&envelope.message_id));
                    }
                    WriteItem::Raw { outcome_tx, .. } => {
                        let _ = outcome_tx.send(self.unavailable_outcome_with_id(""));
                    }
                }
            }
        } else {
            let _ = outcome_tx.send(self.unavailable_outcome_with_id(""));
        }
        outcome_rx
    }

    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult {
        // Legacy synchronous path: delegate to the write channel and block on
        // the outcome. This preserves backward compatibility during the
        // write-interface transition; it will be removed once all relay callsites
        // move to mailw/raww.
        let Some(envelope) = envelopes.into_iter().next() else {
            return single(failed_outcome(
                context.target_session.clone(),
                String::new(),
                "ACP delivery received no envelope",
            ));
        };
        let target_session = context.target_session.clone();
        let message_id = envelope.message_id.clone();
        if self.write_tx.is_none() {
            return single(worker_unavailable_outcome(
                target_session,
                message_id,
                context.target_member.id.as_str(),
            ));
        }
        let rx = self.mailw(envelope);
        match rx.blocking_recv() {
            Ok(outcome) => single(outcome),
            Err(_) => single(failed_outcome_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                "ACP delivery task dropped outcome sender",
                None,
            )),
        }
    }

    fn is_ready(&self) -> bool {
        matches!(
            self.readiness(),
            AcpWorkerReadinessState::Available | AcpWorkerReadinessState::Busy
        )
    }

    fn raw_write(
        &mut self,
        text: &str,
        _append_enter: bool,
        _context: &DeliveryContext,
    ) -> RawWriteResult {
        // Legacy synchronous path: delegate to the write channel and block.
        // This preserves backward compatibility during the transition.
        let rx = self.raww(text.to_string(), _append_enter);
        match rx.blocking_recv() {
            Ok(outcome) if matches!(outcome.outcome, SendOutcome::Delivered) => {
                RawWriteResult::Written
            }
            Ok(outcome) => RawWriteResult::Failed {
                reason: outcome
                    .reason
                    .unwrap_or_else(|| "ACP raw write failed".to_string()),
            },
            Err(_) => RawWriteResult::Failed {
                reason: "ACP delivery task dropped outcome sender".to_string(),
            },
        }
    }

    fn shutdown(&mut self) {
        // Signal the delivery task to drain and exit, then drop the runtime.
        self.shutdown_tx = None;
        self.write_tx = None;
        self.runtime = None;
        self.set_replay(None);
        self.set_readiness(AcpWorkerReadinessState::Unavailable);
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
            if !matches!(state, AcpWorkerReadinessState::Initializing) {
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

/// A batch of rendered envelopes with their metadata, ready for combining.
struct EnvelopeBatch {
    rendered: Vec<String>,
    message_ids: Vec<String>,
    decider_sessions: Vec<Vec<String>>,
    outcome_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>>,
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
    max_prompt_tokens: usize,
    target_session: String,
) {
    let batch_settings = PromptBatchSettings {
        max_prompt_tokens,
        ..Default::default()
    };
    let ctx = TurnContext {
        session_id: &session_id,
        shared: &shared,
        chooser: &chooser,
        target_session: &target_session,
    };

    let TurnContext {
        session_id: _,
        shared: _,
        chooser: _,
        target_session: _,
    } = ctx;
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
                let mut batch = EnvelopeBatch {
                    rendered: vec![envelope.rendered.clone()],
                    message_ids: vec![envelope.message_id.clone()],
                    decider_sessions: vec![envelope.choice_decider_sessions.clone()],
                    outcome_senders: vec![outcome_tx],
                };

                loop {
                    match rx.try_recv() {
                        Ok(WriteItem::Envelope {
                            envelope: next_env,
                            outcome_tx: next_tx,
                        }) => {
                            batch.rendered.push(next_env.rendered.clone());
                            batch.message_ids.push(next_env.message_id.clone());
                            batch
                                .decider_sessions
                                .push(next_env.choice_decider_sessions.clone());
                            batch.outcome_senders.push(next_tx);
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
    if matches!(readiness, AcpWorkerReadinessState::Unavailable) {
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

/// A DroppedOnShutdown outcome for items resolved during shutdown/respawn.
fn dropped_on_shutdown_outcome() -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: String::new(),
        message_id: String::new(),
        outcome: SendOutcome::Failed,
        reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
        reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
        details: None,
    }
}

/// Combines a group of rendered envelopes into one turn prompt respecting the
/// token budget, submits each batch as a turn, and fans the outcome to the
/// senders for that batch. Each sender receives its own message_id in the
/// outcome, even when multiple envelopes are combined into one turn.
#[allow(clippy::type_complexity)]
fn flush_envelope_group(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    batch_settings: &PromptBatchSettings,
    batch: &mut EnvelopeBatch,
) {
    let budget = batch_settings.max_prompt_tokens.max(1);
    // Each group: (combined prompt, head message_id, head decider_sessions,
    //               per-sender message_ids, per-sender outcome senders)
    let mut groups: Vec<(
        String,
        String,
        Vec<String>,
        Vec<String>,
        Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>>,
    )> = Vec::new();
    let mut cur_prompt = String::new();
    let mut cur_head_msg_id = String::new();
    let mut cur_head_deciders: Vec<String> = Vec::new();
    let mut cur_msg_ids: Vec<String> = Vec::new();
    let mut cur_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>> = Vec::new();

    for (((rendered, msg_id), deciders), sender) in batch
        .rendered
        .drain(..)
        .zip(batch.message_ids.drain(..))
        .zip(batch.decider_sessions.drain(..))
        .zip(batch.outcome_senders.drain(..))
    {
        if cur_prompt.is_empty() {
            cur_prompt = rendered;
            cur_head_msg_id = msg_id.clone();
            cur_head_deciders = deciders;
            cur_msg_ids.push(msg_id);
            cur_senders.push(sender);
            continue;
        }
        let candidate = format!("{cur_prompt}\n\n{rendered}");
        let est =
            crate::envelope::estimate_prompt_tokens(&candidate, batch_settings.tokenizer_profile);
        if est <= budget {
            cur_prompt = candidate;
            cur_msg_ids.push(msg_id);
            cur_senders.push(sender);
        } else {
            groups.push((
                cur_prompt,
                cur_head_msg_id,
                cur_head_deciders,
                cur_msg_ids,
                cur_senders,
            ));
            cur_prompt = rendered;
            cur_head_msg_id = msg_id.clone();
            cur_head_deciders = deciders;
            cur_msg_ids = vec![msg_id];
            cur_senders = vec![sender];
        }
    }
    if !cur_prompt.is_empty() {
        groups.push((
            cur_prompt,
            cur_head_msg_id,
            cur_head_deciders,
            cur_msg_ids,
            cur_senders,
        ));
    }

    for (prompt, msg_id, deciders, msg_ids, senders) in groups {
        let outcome = submit_envelope_turn(client, ctx, &prompt, &msg_id, &deciders);
        for (sender_msg_id, tx) in msg_ids.into_iter().zip(senders) {
            let mut sender_outcome = outcome.clone();
            sender_outcome.message_id = sender_msg_id;
            let _ = tx.send(sender_outcome);
        }
    }
}

/// Submits one combined prompt as an ACP turn, blocking until completion.
fn submit_envelope_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    prompt: &str,
    message_id: &str,
    decider_sessions: &[String],
) -> SingleDeliveryOutcome {
    let pending_choice: Arc<Mutex<Option<ChoiceMade>>> = Arc::new(Mutex::new(None));
    let completion_slot: Arc<Mutex<Option<PromptCompletion>>> = Arc::new(Mutex::new(None));

    let shared_for_dispatch = Arc::clone(ctx.shared);
    let on_dispatched: DispatchHandler = Box::new(move || {
        *shared_for_dispatch
            .readiness
            .lock()
            .expect("readiness mutex") = AcpWorkerReadinessState::Busy;
    });

    let on_permission = if let Some(chooser) = ctx.chooser {
        let correlation = ChoiceCorrelation {
            message_id: message_id.to_string(),
            target_session: ctx.target_session.to_string(),
            decider_sessions: decider_sessions.to_vec(),
        };
        build_acp_permission_handler(chooser.clone(), correlation, Arc::clone(&pending_choice))
    } else {
        Box::new(|_req, mut responder: PermissionResponder| {
            responder.respond(None);
        })
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
            loop {
                if client.wait_for_prompt_complete(ACP_PROMPT_WAIT_POLL_INTERVAL) {
                    break;
                }
                if shutdown_requested() {
                    break;
                }
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
            *ctx.shared.readiness.lock().expect("readiness mutex") = final_state;
            outcome
        }
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            *ctx.shared.readiness.lock().expect("readiness mutex") =
                AcpWorkerReadinessState::Unavailable;
            failed_outcome_with_code(
                ctx.target_session.to_string(),
                message_id.to_string(),
                ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                "ACP child stdin write failed",
                Some(json!({ "reason": reason })),
            )
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            *ctx.shared.readiness.lock().expect("readiness mutex") =
                AcpWorkerReadinessState::Unavailable;
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

/// Submits raw content as an ACP turn (no envelope framing).
fn submit_raw_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    content: &str,
    _append_enter: bool,
) -> SingleDeliveryOutcome {
    submit_envelope_turn(client, ctx, content, "", &[])
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

fn single(outcome: SingleDeliveryOutcome) -> DeliveryResult {
    DeliveryResult {
        outcomes: vec![outcome],
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

fn worker_unavailable_outcome(
    target_session: String,
    message_id: String,
    target_member_id: &str,
) -> SingleDeliveryOutcome {
    failed_outcome_with_code(
        target_session,
        message_id,
        ACP_ERROR_CODE_WORKER_UNAVAILABLE,
        "ACP worker is unavailable for target session",
        Some(json!({ "target_session": target_member_id })),
    )
}

fn build_acp_completion_result(
    completion: Option<PromptCompletion>,
    pending_choice_outcome: Option<ChoiceMade>,
    target_session: String,
    message_id: String,
    target_member_id: &str,
) -> (AcpWorkerReadinessState, SingleDeliveryOutcome) {
    if let Some(ChoiceMade::Cancelled {
        reason_code,
        reason,
        ..
    }) = pending_choice_outcome
    {
        return (
            AcpWorkerReadinessState::Available,
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
        // No completion observed before the wait was abandoned: shutdown.
        return (
            AcpWorkerReadinessState::Available,
            failed_outcome_with_code(
                target_session,
                message_id,
                DROPPED_ON_SHUTDOWN_REASON_CODE,
                DROPPED_ON_SHUTDOWN_REASON,
                None,
            ),
        );
    };

    match completion {
        PromptCompletion::Completed { stop_reason } => match stop_reason.as_str() {
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => (
                AcpWorkerReadinessState::Available,
                delivered_outcome(target_session, message_id),
            ),
            "cancelled" => (
                AcpWorkerReadinessState::Available,
                failed_outcome_with_code(
                    target_session,
                    message_id,
                    ACP_REASON_CODE_STOP_CANCELLED,
                    "ACP turn completed with stopReason=cancelled",
                    None,
                ),
            ),
            other => (
                AcpWorkerReadinessState::Available,
                failed_outcome(
                    target_session,
                    message_id,
                    format!("ACP returned unsupported stopReason '{other}'"),
                ),
            ),
        },
        PromptCompletion::ProtocolError(reason) => (
            AcpWorkerReadinessState::Available,
            failed_outcome_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt failed",
                Some(json!({ "target_session": target_member_id, "reason": reason })),
            ),
        ),
        PromptCompletion::ConnectionClosed { reason } => (
            AcpWorkerReadinessState::Unavailable,
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
