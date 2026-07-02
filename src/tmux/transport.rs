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
//! primitives (driven by relay bundle reconcile/startup), so the transport has
//! no startup/shutdown lifecycle of its own. The internal task resolves the
//! active pane per flush group against the runtime's tmux socket.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::configuration::{PromptReadinessTemplate, TargetConfiguration};
use crate::envelope::{PromptBatchSettings, batch_envelope_groups};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::runtime::signals::shutdown_requested;
use crate::transports::{
    DeliveryEnvelope, DeliveryWaitError, LookMode, LookSnapshotPayload, OutcomeFuture, OutputView,
    SendOutcome, SingleDeliveryOutcome, StartupContext, Transport, TransportError,
    TransportReadiness, TransportStatus, WedgeObservation, WedgeProbe,
    wait_for_quiescent_three_state,
};

/// Default tmux look window applied when the caller omits a window size.
const LOOK_LINES_DEFAULT: usize = 120;

use super::pane::{
    capture_pane_snapshot, capture_pane_tail_lines, inject_literal_text,
    operator_interaction_active, resolve_active_pane_target, resolve_cursor_column,
    resolve_window_activity_marker, sanitize_diagnostic_text,
};

const PROMPT_INSPECT_LINES_DEFAULT: usize = 3;
const PROMPT_INSPECT_LINES_MAX: usize = 40;
const TMUX_TARGET_UNAVAILABLE_CODE: &str = "tmux_target_unavailable";

/// Capacity of the internal write channel. Sized to absorb bursts from the
/// relay worker without unbounded growth; the delivery task drains continuously.
const WRITE_CHANNEL_CAPACITY: usize = 256;

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
    task_context: Option<DeliveryTaskContext>,
    shutdown_flag: Arc<AtomicBool>,
}

impl std::fmt::Debug for TmuxTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TmuxTransport")
            .field("batch_settings", &self.batch_settings)
            .field("sender", &self.sender.as_ref().map(|_| "..."))
            .finish()
    }
}

impl TmuxTransport {
    #[must_use]
    pub fn new(batch_settings: PromptBatchSettings) -> Self {
        Self {
            batch_settings,
            sender: None,
            task_context: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Starts the internal delivery task if not already running and `startup()`
    /// has been called. Returns `true` if the task is running (or just started).
    fn ensure_task_running(&mut self) -> bool {
        if self.sender.is_some() {
            return true;
        }
        let ctx = match self.task_context.take() {
            Some(ctx) => ctx,
            None => return false,
        };
        let (sender, receiver) = mpsc::channel(WRITE_CHANNEL_CAPACITY);
        self.sender = Some(sender);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let batch_settings = self.batch_settings;
        thread::spawn(move || {
            run_delivery_task(receiver, ctx, shutdown_flag, batch_settings);
        });
        true
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

impl Transport for TmuxTransport {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        self.task_context = Some(DeliveryTaskContext {
            target_session: context.target_member.id.clone(),
            runtime_directory: context.runtime_directory,
            target_member: context.target_member,
        });
        Ok(TransportStatus {
            readiness: TransportReadiness::Ready,
        })
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let (sender, receiver) = oneshot::channel();
        if !self.ensure_task_running() {
            let _ = sender.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: envelope.message_id.clone(),
                outcome: SendOutcome::Failed,
                reason_code: Some("transport_not_started".to_string()),
                reason: Some("mailw called before startup()".to_string()),
                details: None,
            });
            return receiver;
        }
        self.enqueue(WriteItem::Envelope(Box::new(envelope), sender));
        receiver
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let (sender, receiver) = oneshot::channel();
        if !self.ensure_task_running() {
            let _ = sender.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: String::new(),
                outcome: SendOutcome::Failed,
                reason_code: Some("transport_not_started".to_string()),
                reason: Some("raww called before startup()".to_string()),
                details: None,
            });
            return receiver;
        }
        self.enqueue(WriteItem::Raw(content, append_enter, sender));
        receiver
    }

    fn is_ready(&self) -> bool {
        true
    }

    fn shutdown(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        self.sender = None;
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

    let (prompt_readiness, wedge_detection) = match &ctx.target_member.target {
        TargetConfiguration::Tmux(tmux_target) => (
            tmux_target.prompt_readiness.clone(),
            tmux_target.wedge_detection,
        ),
        _ => (None, true),
    };

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
                        prompt_readiness.as_ref(),
                        wedge_detection,
                        &mut receiver,
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
                        prompt_readiness.as_ref(),
                        wedge_detection,
                        &mut receiver,
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
                            prompt_readiness.as_ref(),
                            wedge_detection,
                            &mut receiver,
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
                            prompt_readiness.as_ref(),
                            wedge_detection,
                            &mut receiver,
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
                            prompt_readiness.as_ref(),
                            wedge_detection,
                            &mut receiver,
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
    prompt_readiness: Option<&PromptReadinessTemplate>,
    wedge_detection: bool,
    receiver: &mut mpsc::Receiver<WriteItem>,
    shutdown_flag: &AtomicBool,
    batch_settings: PromptBatchSettings,
) {
    if group.is_empty() {
        return;
    }

    // Use the head envelope's quiescence hints for the entire group.
    // `prime_timeout_ms` is the per-coder bounded prime window the tmux
    // transport consumes from the envelope; `wedge_detection` is the
    // per-coder switch (default-on) for firing `pane_wedged` on
    // quiescent + non-prompt + no operator interaction.
    //
    // The prime timer is anchored to the START OF THIS FLUSH GROUP and
    // is NOT reset across coalesce iterations (the spec requires the
    // prime timer to be anchored to "delivery-task perspective (when
    // flush begins, not enqueue time)" and "does NOT reset on
    // coalesce-during-wait"). The deadline is computed once here and
    // threaded into every wait call below.
    let quiet_window = group[0].0.quiet_window;
    let prime_timeout_ms = group[0].0.prime_timeout_ms;
    let prime_started_at = Instant::now();
    let prime_deadline = prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));

    // Deferred raw item from the post-quiescence drain. Carries across
    // coalesce loop iterations so the re-check happens before paste.
    let mut deferred_raw: Option<(String, bool, OutcomeSender)> = None;

    // Coalesce-during-wait loop.
    loop {
        if shutdown_flag.load(Ordering::Acquire) {
            for (envelope, sender) in group.drain(..) {
                let _ = sender.send(make_dropped_on_shutdown(
                    target_session,
                    &envelope.message_id,
                ));
            }
            return;
        }

        // Build a fresh probe for each wait iteration. The probe wraps
        // the same tmux queries the legacy wait loop called directly;
        // wrapping them in a trait object lets us inject scripted probes
        // from tests.
        let mut probe = match RealPaneQuiescenceProbe::new(
            tmux_socket_path,
            target_session,
            prompt_readiness,
        ) {
            Ok(probe) => probe,
            Err(error) => {
                for (envelope, sender) in group.drain(..) {
                    let _ = sender.send(wait_error_to_outcome(
                        target_session,
                        &error,
                        &envelope.message_id,
                    ));
                }
                return;
            }
        };

        // Wait for quiescence (blocks). The same `prime_deadline` is
        // passed on every coalesce iteration, so absorbed envelopes do
        // not extend the prime window.
        match wait_for_quiescent_pane_three_state(
            &mut probe,
            target_session,
            quiet_window,
            prime_deadline,
            prime_started_at,
            prime_timeout_ms,
            wedge_detection,
        ) {
            Ok(_) => {}
            Err(DeliveryWaitError::Shutdown) => {
                for (envelope, sender) in group.drain(..) {
                    let _ = sender.send(make_dropped_on_shutdown(
                        target_session,
                        &envelope.message_id,
                    ));
                }
                return;
            }
            Err(wait_error) => {
                for (envelope, sender) in group.drain(..) {
                    let _ = sender.send(wait_error_to_outcome(
                        target_session,
                        &wait_error,
                        &envelope.message_id,
                    ));
                }
                return;
            }
        }

        // Absorb envelopes that arrived during the quiescence wait.
        // Skip draining if a raw was deferred — the group is finalized
        // (no more items should cross the raw barrier). Re-check
        // quiescence for the enlarged group, then paste and deliver.
        let mut absorbed = false;
        if deferred_raw.is_none() {
            loop {
                match receiver.try_recv() {
                    Ok(WriteItem::Envelope(env, sender)) => {
                        group.push((*env, sender));
                        absorbed = true;
                    }
                    Ok(WriteItem::Raw(content, append_enter, sender)) => {
                        if absorbed {
                            deferred_raw = Some((content, append_enter, sender));
                            break;
                        } else {
                            paste_group(group, tmux_socket_path, target_session, batch_settings);
                            let _ = sender.send(deliver_raw(
                                tmux_socket_path,
                                target_session,
                                &content,
                                append_enter,
                            ));
                            return;
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        paste_group(group, tmux_socket_path, target_session, batch_settings);
                        drain_remaining_as_dropped(receiver, target_session);
                        return;
                    }
                }
            }
        }

        if absorbed {
            // New envelopes arrived during quiescence. Re-check.
            // deferred_raw (if any) carries over to the next iteration.
            continue;
        }

        break; // No new envelopes. Ready to paste.
    }

    // Paste the group. Each sender gets its own message_id.
    paste_group(group, tmux_socket_path, target_session, batch_settings);

    // Deliver the deferred raw (batch barrier) if one was saved.
    if let Some((content, append_enter, sender)) = deferred_raw {
        let _ = sender.send(deliver_raw(
            tmux_socket_path,
            target_session,
            &content,
            append_enter,
        ));
    }
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
    let rendered: Vec<String> = group
        .iter()
        .map(|(envelope, _)| envelope.message.render_pane_envelope(&envelope.message_id))
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
        DeliveryWaitError::Wedged { reason } => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: message_id.to_string(),
            outcome: SendOutcome::Failed,
            reason_code: Some("pane_wedged".to_string()),
            reason: Some(format!(
                "tmux pane wedged (pane settled at non-prompt state with no operator interaction): {reason}"
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

// ---------------------------------------------------------------------------
// Quiescence poll loop
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PromptReadinessMatcher {
    prompt_regex: Regex,
    inspect_lines: usize,
    input_idle_cursor_column: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptReadinessEvaluation {
    pub ready: bool,
    pub mismatch_reason: Option<String>,
    pub inspected_block: Option<String>,
    pub regex_matched: Option<bool>,
    pub expected_cursor_column: Option<usize>,
    pub observed_cursor_column: Option<usize>,
}

/// Transport-internal seam for the tmux quiescence wait.
///
/// The real implementation ([`RealPaneQuiescenceProbe`]) wraps tmux queries
/// against the active pane. Tests inject scripted probes that drive the
/// three-state classifier deterministically — see the unit tests in
/// `tests/unit/tmux_transport.rs` for the five probe classes
/// (unresponsive, wedged, pending-choice, slow-prompt, normal).
///
/// `pub` to support the external test surface; the trait is not part of
/// the public runtime API (no other code outside `src/tmux` consumes it).
pub trait PaneQuiescenceProbe: Send {
    /// Resolves the current prompt-readiness evaluation for the target pane.
    /// The wait loop calls this twice per quiescence check (with a
    /// `quiet_window` sleep between) and compares results.
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String>;

    /// Reports whether operator interaction (copy-mode or active key-table)
    /// is currently active for the target session. `Some(reason)` when
    /// active (e.g. `"pane_in_mode"` or `"client_key_table=copy-mode-vi"`),
    /// `None` otherwise.
    fn operator_interaction_active(&mut self) -> Result<Option<String>, String>;

    /// Resolves the active pane target for the target session (e.g. `%0`).
    /// Used by the wait loop to record the pane on terminal outcomes and to
    /// thread through to the wedge inscription event.
    fn resolve_active_pane(&mut self) -> Result<String, String>;

    /// Blocks until the pane shows a change (the next observation differs
    /// from the previous one) or the supplied `deadline` elapses. Returns
    /// `Ok(())` on observed change; `Err(DeliveryWaitError::Timeout)` on
    /// deadline elapsed with no change; `Err(DeliveryWaitError::Failed)`
    /// on probe errors. The wait loop passes a deadline derived from the
    /// per-coder `prime_timeout_ms` so the probe bounds its wait by the
    /// same prime window the loop tracks.
    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError>;
}

/// Real [`PaneQuiescenceProbe`] backed by tmux queries. Holds the socket path
/// and target session id used by every observation; the underlying tmux
/// queries are the same primitives the legacy wait loop called directly.
pub(crate) struct RealPaneQuiescenceProbe<'a> {
    tmux_socket: &'a Path,
    target_session: &'a str,
    matcher: Option<PromptReadinessMatcher>,
}

impl<'a> RealPaneQuiescenceProbe<'a> {
    fn new(
        tmux_socket: &'a Path,
        target_session: &'a str,
        prompt_readiness: Option<&PromptReadinessTemplate>,
    ) -> Result<Self, DeliveryWaitError> {
        let matcher = build_prompt_readiness_matcher(prompt_readiness)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        Ok(Self {
            tmux_socket,
            target_session,
            matcher,
        })
    }
}

impl PaneQuiescenceProbe for RealPaneQuiescenceProbe<'_> {
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String> {
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)?;
        let snapshot = capture_pane_snapshot(self.tmux_socket, &pane_target)?;
        prompt_readiness_matches(
            self.tmux_socket,
            pane_target.as_str(),
            snapshot.as_str(),
            self.matcher.as_ref(),
        )
    }

    fn operator_interaction_active(&mut self) -> Result<Option<String>, String> {
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)?;
        operator_interaction_active(self.tmux_socket, self.target_session, pane_target.as_str())
    }

    fn resolve_active_pane(&mut self) -> Result<String, String> {
        resolve_active_pane_target(self.tmux_socket, self.target_session)
    }

    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError> {
        // Sleep in short slices, polling the activity marker and pane
        // target. Returns as soon as either changes (or the deadline
        // elapses).
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let mut last_activity = resolve_window_activity_marker(self.tmux_socket, &pane_target)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let mut last_snapshot = capture_pane_snapshot(self.tmux_socket, &pane_target)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        loop {
            if shutdown_requested() {
                return Err(DeliveryWaitError::Shutdown);
            }
            if Instant::now() >= deadline {
                return Err(DeliveryWaitError::Timeout {
                    timeout: deadline.saturating_duration_since(Instant::now()),
                    readiness_mismatch: false,
                    mismatch_reason: None,
                });
            }
            // Keep the slice short so shutdown_requested is observed promptly.
            thread::sleep(Duration::from_millis(50));
            let pane_target_now = resolve_active_pane_target(self.tmux_socket, self.target_session)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if pane_target_now != pane_target {
                return Ok(());
            }
            let activity_now = resolve_window_activity_marker(self.tmux_socket, &pane_target_now)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if activity_now != last_activity {
                return Ok(());
            }
            let snapshot_now = capture_pane_snapshot(self.tmux_socket, &pane_target_now)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if snapshot_now != last_snapshot {
                return Ok(());
            }
            last_activity = activity_now;
            last_snapshot = snapshot_now;
        }
    }
}

/// Blocks until the target's active pane is quiescent and prompt-ready, or
/// fires one of the three-state classifier outcomes: prime timeout (the
/// pane has shown no observable change for the entire `prime_timeout_ms`
/// window), wedge (the pane is quiescent + not prompt-ready + no operator
/// interaction), or shutdown.
///
/// Three-state classifier:
/// - `running` — output flowing or settled at prompt. Returns `Ok(pane)`.
/// - `unresponsive` — prime window elapsed with no observable change AND
///   no operator interaction. Returns `Err(DeliveryWaitError::Timeout)`.
/// - `wedged` — pane quiesced + not prompt-ready + no operator interaction.
///   Returns `Err(DeliveryWaitError::Wedged)` when `wedge_detection` is
///   enabled; otherwise the loop continues waiting (the prime window is
///   the only bounded-wait path).
///
/// Operator interaction (copy-mode or active key-table) indefinitely
/// suppresses BOTH the unresponsive and the wedged classification, on
/// both the prime window and the post-quiescence wait. Prime timeout
/// does NOT fire while operator interaction is active.
/// Adapter that exposes a [`PaneQuiescenceProbe`] as the cross-transport
/// [`WedgeProbe`]. Constructed per quiescence iteration by
/// [`wait_for_quiescent_pane_three_state`]; holds a `&mut` borrow so it
/// does not own the underlying probe.
///
/// The adapter calls `next_evaluation()` + `operator_interaction_active()`
/// exactly once per [`observe`](WedgeProbe::observe) call. This keeps each
/// quiescence iteration to two `observe()` calls (= two
/// `next_evaluation()` roundtrips), matching the legacy
/// `wait_for_quiescent_pane_three_state` call frequency. Scripted test
/// probes with `abort_after_calls` thresholds trip at the iteration count
/// the test expects rather than at 4x that count.
///
/// Pane target resolution is delegated to the underlying probe (which
/// returns the active tmux pane id like `%0`) so the state machine can
/// thread it through to its diagnostic inscriptions
/// (`delivery_ready`, `delivery_pane_wedged`, `delivery_prime_timeout`,
/// `delivery_prompt_mismatch`).
struct TmuxAsWedgeProbe<'a, P: PaneQuiescenceProbe> {
    inner: &'a mut P,
}

impl<'a, P: PaneQuiescenceProbe> TmuxAsWedgeProbe<'a, P> {
    fn new(inner: &'a mut P) -> Self {
        Self { inner }
    }
}

impl<'a, P: PaneQuiescenceProbe> WedgeProbe for TmuxAsWedgeProbe<'a, P> {
    fn observe(&mut self) -> Result<WedgeObservation, String> {
        let evaluation = self.inner.next_evaluation()?;
        let op_interaction = self.inner.operator_interaction_active()?;
        let pane_target = self.inner.resolve_active_pane()?;
        let mismatch = if evaluation.ready {
            None
        } else {
            Some(crate::transports::ReadinessMismatch {
                reason: evaluation.mismatch_reason.clone().unwrap_or_default(),
                regex_matched: evaluation.regex_matched,
                expected_cursor_column: evaluation
                    .expected_cursor_column
                    .and_then(|c| u16::try_from(c).ok()),
                observed_cursor_column: evaluation
                    .observed_cursor_column
                    .and_then(|c| u16::try_from(c).ok()),
            })
        };
        Ok(WedgeObservation {
            inspected_tail: evaluation.inspected_block.unwrap_or_default(),
            is_prompt_ready: evaluation.ready,
            operator_interaction_active: op_interaction.is_some(),
            pane_target: Some(pane_target),
            mismatch,
        })
    }

    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError> {
        self.inner.wait_for_change(deadline)
    }
}

/// Drives the three-state delivery classifier (running / unresponsive /
/// wedged) over a [`PaneQuiescenceProbe`]. `pub` to support the external
/// test surface in `tests/unit/tmux_transport.rs`; the function is not part
/// of the runtime API (callers reach it via `flush_and_resolve`).
///
/// This is a thin wrapper that constructs a [`TmuxAsWedgeProbe`] adapter
/// and delegates to the cross-transport
/// [`wait_for_quiescent_three_state`] in `src/transports/quiescence.rs`.
/// The signature is preserved (including the `Result<String,
/// DeliveryWaitError>` return type that callers and unit tests rely on);
/// the pane target in the `Ok` value comes from the post-wait
/// observation the state machine reports (which differs from the
/// pre-wait pane target when the active pane changed during the wait).
/// The 16-probe test surface in `tests/unit/tmux_transport.rs` is
/// unchanged — probes implement [`PaneQuiescenceProbe`] as before.
///
/// `prime_deadline`, `prime_started_at`, `prime_timeout_ms`, and
/// `wedge_detection` carry the same semantics as the underlying
/// [`wait_for_quiescent_three_state`] (see that function's docs).
pub fn wait_for_quiescent_pane_three_state<P: PaneQuiescenceProbe>(
    probe: &mut P,
    target_session: &str,
    quiet_window: Duration,
    prime_deadline: Option<Instant>,
    prime_started_at: Instant,
    prime_timeout_ms: Option<u64>,
    wedge_detection: bool,
) -> Result<String, DeliveryWaitError> {
    let mut adapter = TmuxAsWedgeProbe::new(probe);
    wait_for_quiescent_three_state(
        &mut adapter,
        target_session,
        quiet_window,
        prime_deadline,
        prime_started_at,
        prime_timeout_ms,
        wedge_detection,
    )
}

fn build_prompt_readiness_matcher(
    template: Option<&PromptReadinessTemplate>,
) -> Result<Option<PromptReadinessMatcher>, String> {
    let Some(template) = template else {
        return Ok(None);
    };

    let prompt_regex = Regex::new(template.prompt_regex.as_str())
        .map_err(|source| format!("invalid prompt_readiness.prompt_regex: {source}"))?;
    let inspect_lines = template
        .inspect_lines
        .unwrap_or(PROMPT_INSPECT_LINES_DEFAULT)
        .clamp(1, PROMPT_INSPECT_LINES_MAX);

    Ok(Some(PromptReadinessMatcher {
        prompt_regex,
        inspect_lines,
        input_idle_cursor_column: template.input_idle_cursor_column,
    }))
}

fn prompt_readiness_matches(
    tmux_socket: &Path,
    pane_target: &str,
    snapshot: &str,
    matcher: Option<&PromptReadinessMatcher>,
) -> Result<PromptReadinessEvaluation, String> {
    let Some(matcher) = matcher else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            ..PromptReadinessEvaluation::default()
        });
    };

    let inspected = snapshot
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take(matcher.inspect_lines)
        .collect::<Vec<_>>();
    if inspected.is_empty() {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(
                "inspected pane tail was empty after trimming trailing blank lines".to_string(),
            ),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }
    let mut ordered = inspected;
    ordered.reverse();
    let block = ordered.join("\n");
    if !matcher.prompt_regex.is_match(block.as_str()) {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some("prompt regex did not match inspected pane tail".to_string()),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }

    let Some(expected_cursor_column) = matcher.input_idle_cursor_column else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            ..PromptReadinessEvaluation::default()
        });
    };
    let cursor_column = resolve_cursor_column(tmux_socket, pane_target)?;
    if cursor_column != expected_cursor_column {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(format!(
                "cursor column {} did not match required {}",
                cursor_column, expected_cursor_column
            )),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            expected_cursor_column: Some(expected_cursor_column),
            observed_cursor_column: Some(cursor_column),
            ..PromptReadinessEvaluation::default()
        });
    }

    Ok(PromptReadinessEvaluation {
        ready: true,
        inspected_block: Some(sanitize_diagnostic_text(&block)),
        regex_matched: Some(true),
        expected_cursor_column: Some(expected_cursor_column),
        observed_cursor_column: Some(cursor_column),
        ..PromptReadinessEvaluation::default()
    })
}
