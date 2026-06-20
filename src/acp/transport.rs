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
    /// Per-prompt token budget for envelope combining.
    max_prompt_tokens: usize,
    /// Target session id, captured at startup for permission correlation.
    target_session: String,
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
            max_prompt_tokens,
            target_session: String::new(),
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
        // Close the write channel first so the delivery task drains, processes
        // any remaining items, and exits before the runtime is dropped.
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
    fn spawn_delivery_task(&mut self) {
        let (tx, rx) = mpsc::channel::<WriteItem>(ACP_WRITE_CHANNEL_CAPACITY);
        let shared = Arc::clone(&self.shared);
        let max_prompt_tokens = self.max_prompt_tokens;
        let chooser = self.chooser.clone();
        let target_session = self.target_session.clone();

        // Take the client and session_id from the runtime. The transport keeps
        // the replay handle (already published via shared state) and no longer
        // needs the client directly — all prompt submission goes through the task.
        let runtime = self.runtime.take().expect("runtime present at task spawn");
        let client = runtime.client;
        let session_id = runtime.session_id;

        std::thread::spawn(move || {
            acp_delivery_task(
                rx,
                client,
                session_id,
                shared,
                chooser,
                max_prompt_tokens,
                target_session,
            );
        });

        self.write_tx = Some(tx);
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
            let _ = tx.try_send(WriteItem::Envelope {
                envelope,
                outcome_tx,
            });
        } else {
            let _ = outcome_tx.send(unavailable_outcome());
        }
        outcome_rx
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        if let Some(tx) = self.write_tx.as_ref() {
            let _ = tx.try_send(WriteItem::Raw {
                content,
                append_enter,
                outcome_tx,
            });
        } else {
            let _ = outcome_tx.send(unavailable_outcome());
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
        // Close the write channel so the delivery task drains and exits, then
        // drop the runtime (joins the child and reader thread).
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

/// Internal ACP delivery task. Runs on a dedicated thread, draining the write
/// channel, combining contiguous envelopes respecting the token budget, and
/// submitting turns to the ACP runtime. Exits when the channel closes (sender
/// dropped by `release_runtime()` or `shutdown()`).
fn acp_delivery_task(
    mut rx: mpsc::Receiver<WriteItem>,
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

    loop {
        // Block until at least one item arrives.
        let Some(first) = rx.blocking_recv() else {
            // Channel closed — transport shutting down or respawning.
            break;
        };

        // Drain any additional items that arrived while we were waiting.
        let mut items = vec![first];
        while let Ok(item) = rx.try_recv() {
            items.push(item);
        }

        for item in items {
            match item {
                WriteItem::Envelope {
                    envelope,
                    outcome_tx,
                } => {
                    // Combine contiguous envelopes respecting the token budget.
                    let mut rendered_batch = vec![envelope.rendered.clone()];
                    let mut message_ids = vec![envelope.message_id.clone()];
                    let mut outcome_senders = vec![outcome_tx];

                    // Peek ahead: absorb contiguous Envelope items (not Raw).
                    while let Ok(peek) = rx.try_recv() {
                        match peek {
                            WriteItem::Envelope {
                                envelope: next_env,
                                outcome_tx: next_tx,
                            } => {
                                rendered_batch.push(next_env.rendered.clone());
                                message_ids.push(next_env.message_id.clone());
                                outcome_senders.push(next_tx);
                            }
                            WriteItem::Raw {
                                content,
                                append_enter,
                                outcome_tx: raw_tx,
                            } => {
                                // Raw is a batch barrier: flush the envelope
                                // group first, then deliver the raw write.
                                if !rendered_batch.is_empty() {
                                    flush_envelope_group(
                                        &mut client,
                                        &session_id,
                                        &shared,
                                        &chooser,
                                        &batch_settings,
                                        &rendered_batch,
                                        &message_ids,
                                        outcome_senders,
                                        &target_session,
                                    );
                                    rendered_batch = Vec::new();
                                    message_ids = Vec::new();
                                    outcome_senders = Vec::new();
                                }
                                // Deliver raw immediately (no combining).
                                let result = submit_raw_turn(
                                    &mut client,
                                    &session_id,
                                    &shared,
                                    &chooser,
                                    &content,
                                    append_enter,
                                    &target_session,
                                );
                                let _ = raw_tx.send(result);
                                // Continue with empty batch.
                            }
                        }
                    }

                    // Flush any remaining envelope group.
                    if !rendered_batch.is_empty() {
                        flush_envelope_group(
                            &mut client,
                            &session_id,
                            &shared,
                            &chooser,
                            &batch_settings,
                            &rendered_batch,
                            &message_ids,
                            outcome_senders,
                            &target_session,
                        );
                    }
                }
                WriteItem::Raw {
                    content,
                    append_enter,
                    outcome_tx,
                } => {
                    // Standalone raw write (no preceding envelope group).
                    let result = submit_raw_turn(
                        &mut client,
                        &session_id,
                        &shared,
                        &chooser,
                        &content,
                        append_enter,
                        &target_session,
                    );
                    let _ = outcome_tx.send(result);
                }
            }
        }
    }
    // Task exited — channel closed. Pending items were drained above.
}

/// Combines a group of rendered envelopes into one turn prompt respecting the
/// token budget, submits each batch as a turn, and fans the outcome to the
/// senders for that batch. If the group exceeds the budget, it is split into
/// multiple batches; each batch is submitted as a separate turn and the outcome
/// is fanned to the senders whose envelopes landed in that batch.
#[allow(clippy::too_many_arguments)]
fn flush_envelope_group(
    client: &mut AcpStdioClient,
    session_id: &str,
    shared: &Arc<AcpSharedState>,
    chooser: &Option<crate::transports::Chooser>,
    batch_settings: &PromptBatchSettings,
    rendered: &[String],
    message_ids: &[String],
    outcome_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>>,
    target_session: &str,
) {
    // Partition senders into batches using the same logic as batch_envelopes.
    let budget = batch_settings.max_prompt_tokens.max(1);
    let mut batches: Vec<(
        String,
        String, // head message_id for permission correlation
        Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>>,
    )> = Vec::new();
    let mut current_prompt = String::new();
    let mut current_message_id = String::new();
    let mut current_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>> = Vec::new();

    for ((envelope, msg_id), sender) in rendered.iter().zip(message_ids.iter()).zip(outcome_senders)
    {
        if current_prompt.is_empty() {
            current_prompt.push_str(envelope);
            current_message_id = msg_id.clone();
            current_senders.push(sender);
            continue;
        }

        let candidate = format!("{current_prompt}\n\n{envelope}");
        let estimated =
            crate::envelope::estimate_prompt_tokens(&candidate, batch_settings.tokenizer_profile);
        if estimated <= budget {
            current_prompt = candidate;
            current_senders.push(sender);
        } else {
            batches.push((current_prompt, current_message_id, current_senders));
            current_prompt = envelope.clone();
            current_message_id = msg_id.clone();
            current_senders = vec![sender];
        }
    }
    if !current_prompt.is_empty() {
        batches.push((current_prompt, current_message_id, current_senders));
    }

    // Submit each batch as a turn and fan the outcome to its senders.
    for (batch_prompt, message_id, senders) in batches {
        let outcome = submit_envelope_turn(
            client,
            session_id,
            shared,
            chooser,
            &batch_prompt,
            &message_id,
            target_session,
        );
        for tx in senders {
            let _ = tx.send(outcome.clone());
        }
    }
}

/// Submits one combined prompt as an ACP turn, blocking until completion.
fn submit_envelope_turn(
    client: &mut AcpStdioClient,
    session_id: &str,
    shared: &Arc<AcpSharedState>,
    chooser: &Option<crate::transports::Chooser>,
    prompt: &str,
    message_id: &str,
    target_session: &str,
) -> SingleDeliveryOutcome {
    let pending_choice: Arc<Mutex<Option<ChoiceMade>>> = Arc::new(Mutex::new(None));
    let completion_slot: Arc<Mutex<Option<PromptCompletion>>> = Arc::new(Mutex::new(None));

    let shared_for_dispatch = Arc::clone(shared);
    let on_dispatched: DispatchHandler = Box::new(move || {
        *shared_for_dispatch
            .readiness
            .lock()
            .expect("readiness mutex") = AcpWorkerReadinessState::Busy;
    });

    // Build a permission handler if a chooser is available. The correlation
    // uses empty strings for message_id/target_session since the envelope
    // metadata is not available at the turn-submission level; the chooser
    // resolves choices regardless.
    let on_permission = if let Some(chooser) = chooser {
        let correlation = ChoiceCorrelation {
            message_id: message_id.to_string(),
            target_session: target_session.to_string(),
            decider_sessions: Vec::new(),
        };
        build_acp_permission_handler(chooser.clone(), correlation, Arc::clone(&pending_choice))
    } else {
        Box::new(|_req, mut responder: PermissionResponder| {
            // No chooser available — respond with cancelled.
            responder.respond(None);
        })
    };

    let completion_writer = Arc::clone(&completion_slot);
    let on_completion: PromptCompletionHandler = Box::new(move |completion| {
        *completion_writer.lock().expect("completion slot mutex") = Some(completion);
    });

    let dispatch = client.prompt(
        session_id,
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
                String::new(), // target_session — not available at this level
                String::new(), // message_id — not available at this level
                "",            // target_member_id — not available at this level
            );
            *shared.readiness.lock().expect("readiness mutex") = final_state;
            outcome
        }
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            *shared.readiness.lock().expect("readiness mutex") =
                AcpWorkerReadinessState::Unavailable;
            failed_outcome_with_code(
                String::new(),
                String::new(),
                ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                "ACP child stdin write failed",
                Some(json!({ "reason": reason })),
            )
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            *shared.readiness.lock().expect("readiness mutex") =
                AcpWorkerReadinessState::Unavailable;
            failed_outcome_with_code(
                String::new(),
                String::new(),
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
    session_id: &str,
    shared: &Arc<AcpSharedState>,
    chooser: &Option<crate::transports::Chooser>,
    content: &str,
    _append_enter: bool,
    target_session: &str,
) -> SingleDeliveryOutcome {
    submit_envelope_turn(
        client,
        session_id,
        shared,
        chooser,
        content,
        "",
        target_session,
    )
}

/// Outcome for writes submitted when no transport runtime is available.
fn unavailable_outcome() -> SingleDeliveryOutcome {
    failed_outcome_with_code(
        String::new(),
        String::new(),
        ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
        "ACP transport unavailable (no runtime)",
        None,
    )
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
