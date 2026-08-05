//! The tmux [`Transport`] implementation with an internal delivery task.
//!
//! [`TmuxTransport`] owns an internal ordered channel and a background delivery
//! task. The relay worker submits writes via [`mailw`](Transport::mailw) and
//! [`raww`](Transport::raww) without blocking; the internal task drains the
//! channel in FIFO order, accumulates contiguous envelopes into flush groups,
//! waits for pane quiescence (using per-envelope hints from the head envelope),
//! renders each envelope's pane text and combines them into token-budget-bounded
//! prompts (the same greedy split the ACP transport applies to its turns), and
//! pastes each combined prompt. Raw writes act as batch barriers: the task
//! flushes any buffered envelope group before delivering the raw write.
//!
//! During the quiescence wait the task continues to drain the channel, absorbing
//! any envelopes that arrive into the current flush group (coalesce-during-wait).
//! If the group grows, quiescence is re-checked before pasting.
//!
//! Tmux sessions are created and owned by the [`lifecycle`](super::lifecycle)
//! primitives (driven by relay bundle reconcile/startup), so the transport owns
//! no session lifecycle. What [`startup`](Transport::startup) does own is the
//! internal delivery task, which it establishes eagerly so the transport can
//! answer [`is_ready_for_handover`](Transport::is_ready_for_handover) before it
//! has been written to. The internal task resolves the active pane per flush
//! group against the runtime's tmux socket.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::configuration::TargetConfiguration;
use crate::envelope::{PromptBatchSettings, batch_envelope_groups};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::transports::{
    DeliveryEnvelope, DeliveryWaitError, GenerationFence, LookMode, LookSnapshotPayload,
    OutcomeFuture, OutputView, SendOutcome, SingleDeliveryOutcome, StartupContext, Transport,
    TransportError, TransportReadiness, TransportStatus,
};

/// Default tmux look window applied when the caller omits a window size.
const LOOK_LINES_DEFAULT: usize = 120;

use super::pane::{
    TmuxInvocationSlot, capture_pane_tail_lines, inject_literal_text, publish_tmux_invocations,
    resolve_active_pane_target, terminate_published_invocation,
};
use super::quiescence_probe::{PaneQuiescenceProbe, RealPaneQuiescenceProbe};

const TMUX_TARGET_UNAVAILABLE_CODE: &str = "tmux_target_unavailable";
const TMUX_DELIVERY_THREAD_STOPPED_CODE: &str = "tmux_delivery_thread_stopped";

/// Capacity of the internal write channel. Sized to absorb bursts from the
/// relay worker without unbounded growth; the delivery task drains continuously.
const WRITE_CHANNEL_CAPACITY: usize = 256;

/// Marker line written immediately before a terminal-outcome receipt's
/// pane text so the receiving agent can distinguish a relay/system
/// status update from a peer message at a glance. Reuses the same
/// literal as the Pty transport (`src/pty/delivery.rs`) for
/// cross-transport consistency.
const RECEIPT_MARKER: &str = "--- agentmux terminal-outcome receipt ---";

/// Renders one envelope's pane text for tmux paste. Receipt envelopes
/// (`DeliveryEnvelope.is_receipt`) get a leading marker line so the
/// receiving agent can distinguish a relay/system status update from
/// a peer message at a glance. The marker is included in the
/// rendered text so the token-budget batching and paste-budget counts
/// stay consistent with the actual pane bytes.
///
/// Detection uses the typed `DeliveryEnvelope.is_receipt` field the
/// relay's terminal-resolution chokepoint propagates from
/// `AsyncDeliveryTask.is_receipt`; no Tmux-side sender identity
/// inference. Receipts are non-recursive at the relay-side chokepoint;
/// the Tmux transport does not enforce or check that invariant.
pub fn render_paste_text(envelope: &DeliveryEnvelope) -> String {
    let body = envelope.message.render_pane_envelope(&envelope.message_id);
    if envelope.is_receipt {
        format!("{RECEIPT_MARKER}\n{body}")
    } else {
        body
    }
}

/// Outcome sender half: the delivery task resolves this when the write reaches
/// a terminal state.
type OutcomeSender = oneshot::Sender<SingleDeliveryOutcome>;

/// One item on the transport's internal ordered channel.
enum WriteItem {
    /// Structured delivery message with its outcome sender. Boxed to keep the
    /// channel item small (the message carries full attribution), so the `Raw`
    /// variant does not inflate every queued item.
    Envelope(Box<DeliveryEnvelope>, OutcomeSender),
    /// Raw input (content, append_enter) with its outcome sender.
    Raw(String, bool, OutcomeSender),
}

/// Context captured at `startup` for the internal delivery task.
#[derive(Clone)]
struct DeliveryTaskContext {
    target_session: String,
    runtime_directory: PathBuf,
    target_member: crate::configuration::BundleMember,
}

/// Tmux pane delivery transport with an internal delivery task.
///
/// The transport owns an ordered channel carrying [`WriteItem`]s. The relay
/// worker submits writes via `mailw`/`raww` without blocking; a background
/// delivery task drains the channel, groups contiguous envelopes, waits for
/// pane quiescence, and pastes.
pub struct TmuxTransport {
    batch_settings: PromptBatchSettings,
    sender: Option<mpsc::Sender<WriteItem>>,
    task_handle: Option<thread::JoinHandle<()>>,
    task_context: Option<DeliveryTaskContext>,
    shutdown_flag: Arc<AtomicBool>,
    /// The tmux client invocation the delivery thread is currently waiting on.
    ///
    /// Retained so the fence's forced step has something to signal: dropping the
    /// channel reaches a thread between items, and nothing else reaches one
    /// parked in a tmux client call.
    invocation: TmuxInvocationSlot,
}

impl std::fmt::Debug for TmuxTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TmuxTransport")
            .field("batch_settings", &self.batch_settings)
            .field("sender", &self.sender.as_ref().map(|_| "..."))
            .field(
                "task_running",
                &self
                    .task_handle
                    .as_ref()
                    .map(|handle| !handle.is_finished()),
            )
            .finish()
    }
}

impl TmuxTransport {
    #[must_use]
    pub fn new(batch_settings: PromptBatchSettings) -> Self {
        Self {
            batch_settings,
            sender: None,
            task_handle: None,
            task_context: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            invocation: TmuxInvocationSlot::default(),
        }
    }

    /// Starts the internal delivery task if not already running and `startup()`
    /// has been called. Returns an error when startup was omitted or the task
    /// has stopped after startup.
    fn ensure_task_running(&mut self) -> Result<(), &'static str> {
        if let Some(handle) = self.task_handle.as_ref()
            && handle.is_finished()
        {
            let handle = self
                .task_handle
                .take()
                .expect("finished task handle must still be present");
            let _ = handle.join();
            self.sender = None;
            return Err(TMUX_DELIVERY_THREAD_STOPPED_CODE);
        }
        if let Some(handle) = self.task_handle.as_ref() {
            if self.sender.is_some() && !handle.is_finished() {
                return Ok(());
            }
            self.sender = None;
            return Err(TMUX_DELIVERY_THREAD_STOPPED_CODE);
        }
        if self.sender.is_some() {
            self.sender = None;
            return Err(TMUX_DELIVERY_THREAD_STOPPED_CODE);
        }
        let ctx = match self.task_context.clone() {
            Some(ctx) => ctx,
            None => return Err("transport_not_started"),
        };
        let (sender, receiver) = mpsc::channel(WRITE_CHANNEL_CAPACITY);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let batch_settings = self.batch_settings;
        let invocation = Arc::clone(&self.invocation);
        let task_handle = thread::spawn(move || {
            publish_tmux_invocations(invocation);
            run_delivery_task(receiver, ctx, shutdown_flag, batch_settings);
        });
        self.sender = Some(sender);
        self.task_handle = Some(task_handle);
        Ok(())
    }

    /// Enqueues a write item on the channel. If the channel is full or closed,
    /// resolves the sender immediately with a failed outcome.
    fn enqueue(&self, item: WriteItem) {
        if let Some(ch) = &self.sender
            && let Err(
                mpsc::error::TrySendError::Full(item) | mpsc::error::TrySendError::Closed(item),
            ) = ch.try_send(item)
        {
            let (outcome_sender, message_id) = match item {
                WriteItem::Envelope(env, sender) => (sender, env.message_id),
                WriteItem::Raw(_, _, sender) => (sender, String::new()),
            };
            let _ = outcome_sender.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id,
                outcome: SendOutcome::Failed,
                reason_code: Some("channel_full".to_string()),
                reason: Some("internal write channel full or closed".to_string()),
                details: None,
            });
        }
    }
}

impl GenerationFence for TmuxTransport {
    fn fence_generation(&mut self) {
        // The delivery thread already checks this flag between write items, so
        // marking it is the whole cooperative request.
        self.shutdown_flag.store(true, Ordering::Release);
    }

    fn terminate_generation(&mut self) {
        // Two effect paths, and the channel only reaches one of them. Dropping
        // the sender returns a thread parked waiting for its next item; it does
        // nothing at all for one blocked inside a tmux client call, which is the
        // case that made the cooperative step fail in the first place. Signalling
        // the invocation is what unblocks that thread so the observation after
        // this can succeed.
        //
        // The tmux **server** is deliberately untouched. It is not owned by this
        // generation — it holds the operator's sessions, and terminating it to
        // fence one delivery would destroy work the fence exists to protect.
        terminate_published_invocation(&self.invocation);
        self.sender = None;
    }

    fn generation_ceased(&self) -> bool {
        // A generation that never started a delivery thread owns no executor and
        // has trivially ceased.
        self.task_handle
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }
}

impl Transport for TmuxTransport {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        self.task_context = Some(DeliveryTaskContext {
            target_session: context.target_member.id.clone(),
            runtime_directory: context.runtime_directory,
            target_member: context.target_member,
        });
        // Start the delivery task here rather than on the first write. Readiness
        // is now read before anything is submitted, and a transport whose runtime
        // only appears when written to cannot answer that question: it would
        // report unready forever, and the write that would have started the task
        // is exactly what the readiness gate withholds.
        self.ensure_task_running().map_err(|code| TransportError {
            code: code.to_string(),
            reason: "tmux delivery task could not be established at startup".to_string(),
            details: None,
        })?;
        Ok(TransportStatus {
            readiness: TransportReadiness::Ready,
        })
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let (sender, receiver) = oneshot::channel();
        if let Err(reason_code) = self.ensure_task_running() {
            let reason = if reason_code == "transport_not_started" {
                "mailw called before startup()"
            } else {
                "mailw called after the delivery thread stopped"
            };
            let _ = sender.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: envelope.message_id.clone(),
                outcome: SendOutcome::Failed,
                reason_code: Some(reason_code.to_string()),
                reason: Some(reason.to_string()),
                details: None,
            });
            return receiver;
        }
        self.enqueue(WriteItem::Envelope(Box::new(envelope), sender));
        receiver
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let (sender, receiver) = oneshot::channel();
        if let Err(reason_code) = self.ensure_task_running() {
            let reason = if reason_code == "transport_not_started" {
                "raww called before startup()"
            } else {
                "raww called after the delivery thread stopped"
            };
            let _ = sender.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: String::new(),
                outcome: SendOutcome::Failed,
                reason_code: Some(reason_code.to_string()),
                reason: Some(reason.to_string()),
                details: None,
            });
            return receiver;
        }
        self.enqueue(WriteItem::Raw(content, append_enter, sender));
        receiver
    }

    fn is_ready_for_handover(&self) -> bool {
        let Some(sender) = self.sender.as_ref() else {
            return false;
        };
        let Some(handle) = self.task_handle.as_ref() else {
            return false;
        };
        if handle.is_finished() || sender.is_closed() {
            return false;
        }
        let Some(context) = self.task_context.as_ref() else {
            return false;
        };
        let TargetConfiguration::Tmux(target) = &context.target_member.target else {
            return false;
        };
        let socket = tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let Ok(mut probe) = RealPaneQuiescenceProbe::new(
            socket.as_path(),
            context.target_session.as_str(),
            target.prompt_readiness.as_ref(),
        ) else {
            return false;
        };
        probe
            .next_evaluation()
            .is_ok_and(|evaluation| evaluation.ready)
    }

    fn shutdown(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        self.sender = None;
        self.task_handle = None;
        self.task_context = None;
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        None
    }
}

// ---------------------------------------------------------------------------
// Internal delivery task
// ---------------------------------------------------------------------------

/// Background delivery task: drains the write channel in FIFO order, groups
/// contiguous envelopes into flush groups, waits for quiescence, and pastes.
///
/// Items are processed as a FIFO stream: accumulate envelopes until a raw item
/// is encountered, flush the group, deliver the raw, then continue. This
/// preserves interleaving order and treats raw items as batch barriers.
fn run_delivery_task(
    mut receiver: mpsc::Receiver<WriteItem>,
    ctx: DeliveryTaskContext,
    shutdown_flag: Arc<AtomicBool>,
    batch_settings: PromptBatchSettings,
) {
    let tmux_socket_path = tmux_socket_path_for_runtime_directory(ctx.runtime_directory.as_path());

    // Current envelope group. Accumulates contiguous envelopes; flushed
    // before each raw item (batch barrier) and at channel close / shutdown.
    let mut group: Vec<(DeliveryEnvelope, OutcomeSender)> = Vec::new();

    loop {
        // Check shutdown before blocking.
        if shutdown_flag.load(Ordering::Acquire) {
            drain_group_as_dropped(&mut group, &ctx.target_session);
            drain_remaining_as_dropped(&mut receiver, &ctx.target_session);
            return;
        }

        // Block until at least one item arrives (or channel closes).
        let item = match receiver.blocking_recv() {
            Some(item) => item,
            None => {
                // Channel closed. Flush remaining group and exit.
                if !group.is_empty() {
                    flush_and_resolve(
                        &mut group,
                        &tmux_socket_path,
                        &ctx.target_session,
                        &shutdown_flag,
                        batch_settings,
                    );
                }
                return;
            }
        };

        // Process the item and any immediately available follow-ups.
        // Stop at the first raw item (after flushing the preceding group).
        match item {
            WriteItem::Envelope(env, sender) => {
                group.push((*env, sender));
            }
            WriteItem::Raw(content, append_enter, sender) => {
                // Raw is a batch barrier: flush preceding envelopes first.
                if !group.is_empty() {
                    flush_and_resolve(
                        &mut group,
                        &tmux_socket_path,
                        &ctx.target_session,
                        &shutdown_flag,
                        batch_settings,
                    );
                }
                let _ = sender.send(deliver_raw(
                    &tmux_socket_path,
                    &ctx.target_session,
                    &content,
                    append_enter,
                ));
            }
        }

        // Drain any additional immediately available items. Envelopes join
        // the current group; the first raw item triggers a flush and is
        // handled inline, preserving FIFO order.
        loop {
            match receiver.try_recv() {
                Ok(WriteItem::Envelope(env, sender)) => {
                    group.push((*env, sender));
                }
                Ok(WriteItem::Raw(content, append_enter, sender)) => {
                    // Flush the group accumulated before this raw.
                    if !group.is_empty() {
                        flush_and_resolve(
                            &mut group,
                            &tmux_socket_path,
                            &ctx.target_session,
                            &shutdown_flag,
                            batch_settings,
                        );
                    }
                    let _ = sender.send(deliver_raw(
                        &tmux_socket_path,
                        &ctx.target_session,
                        &content,
                        append_enter,
                    ));
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    // No more items immediately available. Flush the
                    // accumulated envelope group before blocking again.
                    if !group.is_empty() {
                        flush_and_resolve(
                            &mut group,
                            &tmux_socket_path,
                            &ctx.target_session,
                            &shutdown_flag,
                            batch_settings,
                        );
                    }
                    break;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Channel closed. Flush remaining and exit.
                    if !group.is_empty() {
                        flush_and_resolve(
                            &mut group,
                            &tmux_socket_path,
                            &ctx.target_session,
                            &shutdown_flag,
                            batch_settings,
                        );
                    }
                    drain_remaining_as_dropped(&mut receiver, &ctx.target_session);
                    return;
                }
            }
        }

        // Check shutdown after processing.
        if shutdown_flag.load(Ordering::Acquire) {
            drain_group_as_dropped(&mut group, &ctx.target_session);
            drain_remaining_as_dropped(&mut receiver, &ctx.target_session);
            return;
        }
    }
}

/// Drain all remaining items from the channel and resolve their senders with
/// DroppedOnShutdown, preserving each item's message_id.
fn drain_remaining_as_dropped(receiver: &mut mpsc::Receiver<WriteItem>, target_session: &str) {
    while let Ok(item) = receiver.try_recv() {
        let (sender, message_id) = match item {
            WriteItem::Envelope(env, sender) => (sender, env.message_id),
            WriteItem::Raw(_, _, sender) => (sender, String::new()),
        };
        let _ = sender.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id,
            outcome: SendOutcome::DroppedOnShutdown,
            reason_code: Some("dropped_on_shutdown".to_string()),
            reason: Some("delivery dropped due to relay shutdown".to_string()),
            details: None,
        });
    }
}

/// Drain a pending envelope group with DroppedOnShutdown, preserving message_ids.
fn drain_group_as_dropped(
    group: &mut Vec<(DeliveryEnvelope, OutcomeSender)>,
    target_session: &str,
) {
    for (envelope, sender) in group.drain(..) {
        let _ = sender.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: envelope.message_id,
            outcome: SendOutcome::DroppedOnShutdown,
            reason_code: Some("dropped_on_shutdown".to_string()),
            reason: Some("delivery dropped due to relay shutdown".to_string()),
            details: None,
        });
    }
}

/// Flush an envelope group with coalesce-during-wait semantics:
/// 1. Wait for quiescence using the head envelope's hints.
/// 2. Drain the channel — absorb new envelopes into the group, defer raw items.
/// 3. If the group grew, re-check quiescence (loop to step 1).
/// 4. Paste all envelopes and resolve each sender with its own message_id.
///
/// On shutdown at any point, resolves the group as DroppedOnShutdown and returns.
#[allow(clippy::too_many_arguments)]
fn flush_and_resolve(
    group: &mut Vec<(DeliveryEnvelope, OutcomeSender)>,
    tmux_socket_path: &Path,
    target_session: &str,
    shutdown_flag: &AtomicBool,
    batch_settings: PromptBatchSettings,
) {
    if shutdown_flag.load(Ordering::Acquire) {
        drain_group_as_dropped(group, target_session);
        return;
    }
    paste_group(group, tmux_socket_path, target_session, batch_settings);
}

/// Renders the group's structured messages into pane-envelope text, combines
/// them into token-budget-bounded prompts, and pastes each combined prompt as
/// one injection — the same greedy split the ACP transport applies to its
/// combined turns (via [`batch_envelope_groups`]). Each contributing sender is
/// resolved with its own message_id and the outcome of the prompt it rode in.
/// Does NOT consume items from the channel.
fn paste_group(
    group: &mut Vec<(DeliveryEnvelope, OutcomeSender)>,
    tmux_socket_path: &Path,
    target_session: &str,
    batch_settings: PromptBatchSettings,
) {
    if group.is_empty() {
        return;
    }

    let pane_target = match resolve_active_pane_target(tmux_socket_path, target_session) {
        Ok(pane) => pane,
        Err(reason) => {
            for (envelope, sender) in group.drain(..) {
                let _ = sender.send(SingleDeliveryOutcome {
                    target_session: target_session.to_string(),
                    message_id: envelope.message_id.clone(),
                    outcome: SendOutcome::Failed,
                    reason_code: Some(TMUX_TARGET_UNAVAILABLE_CODE.to_string()),
                    reason: Some(reason.clone()),
                    details: None,
                });
            }
            return;
        }
    };

    // Render each structured message into pane-envelope text, then split the
    // contiguous group into token-budget-bounded prompts. A lone envelope over
    // budget forms its own prompt; envelope order is preserved across prompts.
    // Receipt envelopes get a leading marker line (see `render_paste_text`)
    // included in the rendered text so the token-budget batching and
    // paste-budget counts stay consistent with the actual pane bytes.
    let rendered: Vec<String> = group
        .iter()
        .map(|(envelope, _)| render_paste_text(envelope))
        .collect();
    let budget_groups = batch_envelope_groups(&rendered, batch_settings);

    let mut members = group.drain(..);
    for budget_group in budget_groups {
        // Slice the parallel sender vector to this prompt's contributing members.
        let prompt_members: Vec<(String, OutcomeSender)> = members
            .by_ref()
            .take(budget_group.member_count)
            .map(|(envelope, sender)| (envelope.message_id, sender))
            .collect();
        // Envelope-mode writes always submit with Enter; the combined prompt is
        // pasted once for the whole budget group.
        let inject_result = inject_literal_text(
            tmux_socket_path,
            &pane_target,
            budget_group.combined_prompt.as_str(),
            true,
        );
        for (message_id, sender) in prompt_members {
            let outcome = match &inject_result {
                Ok(()) => SingleDeliveryOutcome {
                    target_session: target_session.to_string(),
                    message_id,
                    outcome: SendOutcome::Delivered,
                    reason_code: None,
                    reason: None,
                    details: None,
                },
                Err(reason) => SingleDeliveryOutcome {
                    target_session: target_session.to_string(),
                    message_id,
                    outcome: SendOutcome::Failed,
                    reason_code: None,
                    reason: Some(reason.clone()),
                    details: None,
                },
            };
            let _ = sender.send(outcome);
        }
    }
}

/// Deliver a raw write: resolve pane, inject text, return outcome.
fn deliver_raw(
    tmux_socket_path: &Path,
    target_session: &str,
    content: &str,
    append_enter: bool,
) -> SingleDeliveryOutcome {
    let pane_target = match resolve_active_pane_target(tmux_socket_path, target_session) {
        Ok(pane) => pane,
        Err(reason) => {
            return SingleDeliveryOutcome {
                target_session: target_session.to_string(),
                message_id: String::new(),
                outcome: SendOutcome::Failed,
                reason_code: Some(TMUX_TARGET_UNAVAILABLE_CODE.to_string()),
                reason: Some(reason),
                details: None,
            };
        }
    };
    match inject_literal_text(tmux_socket_path, &pane_target, content, append_enter) {
        Ok(()) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: SendOutcome::Delivered,
            reason_code: None,
            reason: None,
            details: None,
        },
        Err(reason) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: SendOutcome::Failed,
            reason_code: None,
            reason: Some(reason),
            details: None,
        },
    }
}

/// Convert a [`DeliveryWaitError`] into a [`SingleDeliveryOutcome`] with the
/// caller's message_id. Exposed under a `_for_test` name and `#[doc(hidden)]`
/// so the separate `tests/unit` crate can exercise the Wedged / Timeout
/// outcome mapping without expanding the public runtime API. Not intended
/// for use outside the crate's own test surface.
#[doc(hidden)]
pub fn wait_error_to_outcome_for_test(
    target_session: &str,
    error: &DeliveryWaitError,
    message_id: &str,
) -> SingleDeliveryOutcome {
    wait_error_to_outcome(target_session, error, message_id)
}

fn wait_error_to_outcome(
    target_session: &str,
    error: &DeliveryWaitError,
    message_id: &str,
) -> SingleDeliveryOutcome {
    match error {
        DeliveryWaitError::Timeout {
            timeout,
            readiness_mismatch,
            mismatch_reason,
        } => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: message_id.to_string(),
            outcome: SendOutcome::Timeout,
            reason_code: Some("delivery_prime_timeout".to_string()),
            reason: Some(format!(
                "prime wait timed out after {}ms (readiness_mismatch={}, reason={:?})",
                timeout.as_millis(),
                readiness_mismatch,
                mismatch_reason
            )),
            details: None,
        },
        DeliveryWaitError::ReadinessTimeout {
            reason_code,
            elapsed,
            mismatch_reason,
        } => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: message_id.to_string(),
            outcome: SendOutcome::Timeout,
            reason_code: Some(reason_code.code().to_string()),
            reason: Some(format!(
                "tmux target did not become ready within {}ms (reason={:?})",
                elapsed.as_millis(),
                mismatch_reason
            )),
            details: None,
        },
        // Tmux passes `wedge_detection: false`, so the shared classifier cannot
        // return this. It is mapped rather than asserted because this runs inside
        // a tokio delivery task: a panic here would be isolated to that task and
        // swallowed, leaving the sender with no outcome at all and the process
        // reporting success. A structured failure is strictly more observable
        // than an invariant assertion that nothing can hear. The reason code is
        // the generic unavailable one — `pane_wedged` is deliberately not
        // resurrected for Tmux.
        DeliveryWaitError::Wedged { reason } => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: message_id.to_string(),
            outcome: SendOutcome::Failed,
            reason_code: Some(TMUX_TARGET_UNAVAILABLE_CODE.to_string()),
            reason: Some(format!(
                "tmux classifier returned a wedge verdict it cannot produce \
                 (wedge detection is off for tmux): {reason}"
            )),
            details: None,
        },
        DeliveryWaitError::Failed { reason } => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: message_id.to_string(),
            outcome: SendOutcome::Failed,
            reason_code: Some(TMUX_TARGET_UNAVAILABLE_CODE.to_string()),
            reason: Some(reason.clone()),
            details: None,
        },
        DeliveryWaitError::Shutdown => make_dropped_on_shutdown(target_session, message_id),
    }
}

/// Build a DroppedOnShutdown outcome for a target, preserving the message_id.
fn make_dropped_on_shutdown(target_session: &str, message_id: &str) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: target_session.to_string(),
        message_id: message_id.to_string(),
        outcome: SendOutcome::DroppedOnShutdown,
        reason_code: Some("dropped_on_shutdown".to_string()),
        reason: Some("delivery dropped due to relay shutdown".to_string()),
        details: None,
    }
}

// ---------------------------------------------------------------------------
// OutputView
// ---------------------------------------------------------------------------

/// A config-constructed [`OutputView`] over a tmux session's active pane.
///
/// Unlike the ACP view, this holds no worker-owned state: it captures the tmux
/// pane directly through the socket, so it is valid before any delivery has
/// spawned a worker for the session. The relay's `get_output_view` accessor
/// constructs it from the socket path and session id.
pub struct TmuxOutputView {
    socket_path: PathBuf,
    session_id: String,
}

impl TmuxOutputView {
    /// Builds a view over the active pane of `session_id` on `socket_path`.
    #[must_use]
    pub fn new(socket_path: PathBuf, session_id: String) -> Self {
        Self {
            socket_path,
            session_id,
        }
    }
}

impl OutputView for TmuxOutputView {
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        if mode.offset.unwrap_or(0) > 0 {
            return Err(TransportError {
                code: "validation_offset_unsupported".to_string(),
                reason: "offset is only supported for ACP look targets".to_string(),
                details: Some(json!({ "offset": mode.offset })),
            });
        }
        let requested_lines = mode
            .lines
            .map(|lines| lines as usize)
            .unwrap_or(LOOK_LINES_DEFAULT);
        let pane_target =
            resolve_active_pane_target(self.socket_path.as_path(), self.session_id.as_str())
                .map_err(|reason| TransportError {
                    code: "internal_unexpected_failure".to_string(),
                    reason: "failed to resolve active pane for look target".to_string(),
                    details: Some(json!({ "cause": reason })),
                })?;
        let snapshot_lines = capture_pane_tail_lines(
            self.socket_path.as_path(),
            pane_target.as_str(),
            requested_lines,
        )
        .map_err(|reason| TransportError {
            code: "internal_unexpected_failure".to_string(),
            reason: "failed to capture look snapshot".to_string(),
            details: Some(json!({ "cause": reason })),
        })?;
        Ok(LookSnapshotPayload::Lines { snapshot_lines })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::AddressIdentity;

    fn test_envelope() -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: "stopped-thread-message".to_string(),
            message: crate::transports::DeliveryMessage {
                body: "test body".to_string(),
                created_at: "2026-08-01T00:00:00Z".to_string(),
                namespace: "test-ns".to_string(),
                sender: AddressIdentity {
                    session_name: "sender@test-ns".to_string(),
                    display_name: None,
                },
                target: AddressIdentity {
                    session_name: "target@test-ns".to_string(),
                    display_name: None,
                },
                cc: Vec::new(),
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            quiet_window: std::time::Duration::from_millis(50),
            prime_timeout_ms: None,
            readiness_timeout_ms: None,
            is_receipt: false,
        }
    }

    #[test]
    fn mailw_resolves_when_delivery_thread_has_stopped() {
        let (sender, _receiver) = mpsc::channel(WRITE_CHANNEL_CAPACITY);
        let task_handle = thread::spawn(|| {});
        while !task_handle.is_finished() {
            thread::yield_now();
        }

        let mut transport = TmuxTransport::new(PromptBatchSettings::default());
        transport.sender = Some(sender);
        transport.task_handle = Some(task_handle);

        let outcome = Transport::mailw(&mut transport, test_envelope())
            .blocking_recv()
            .expect("stopped delivery thread must resolve mailw");
        assert_eq!(outcome.outcome, SendOutcome::Failed);
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some("tmux_delivery_thread_stopped")
        );
    }
}
