//! The tmux [`Transport`] implementation with an internal delivery task.
//!
//! [`TmuxTransport`] owns an internal ordered channel and a background delivery
//! task. The relay worker submits writes via [`mailw`](Transport::mailw) and
//! [`raww`](Transport::raww) without blocking; the internal task drains the
//! channel in FIFO order, accumulates contiguous envelopes into flush groups,
//! waits for pane quiescence (using per-envelope hints from the head envelope),
//! and pastes the group. Raw writes act as batch barriers: the task flushes any
//! buffered envelope group before delivering the raw write.
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
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::runtime::signals::shutdown_requested;
use crate::transports::{
    DeliveryContext, DeliveryEnvelope, DeliveryPreparation, DeliveryResult, DeliveryWaitError,
    LookMode, LookSnapshotPayload, OutcomeFuture, OutputView, RawWriteResult, SendOutcome,
    SingleDeliveryOutcome, StartupContext, Transport, TransportError, TransportReadiness,
    TransportStatus,
};

/// Default tmux look window applied when the caller omits a window size.
const LOOK_LINES_DEFAULT: usize = 120;

use super::pane::{
    capture_pane_snapshot, capture_pane_tail_lines, emit_delivery_diagnostic, inject_literal_text,
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
    /// Relay-framed envelope with its outcome sender.
    Envelope(DeliveryEnvelope, OutcomeSender),
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
    max_prompt_tokens: usize,
    sender: Option<mpsc::Sender<WriteItem>>,
    task_context: Option<DeliveryTaskContext>,
    shutdown_flag: Arc<AtomicBool>,
}

impl std::fmt::Debug for TmuxTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TmuxTransport")
            .field("max_prompt_tokens", &self.max_prompt_tokens)
            .field("sender", &self.sender.as_ref().map(|_| "..."))
            .finish()
    }
}

impl TmuxTransport {
    #[must_use]
    pub fn new(max_prompt_tokens: usize) -> Self {
        Self {
            max_prompt_tokens,
            sender: None,
            task_context: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Per-prompt token budget captured at construction, threaded from session
    /// configuration. Available for future use by the internal delivery task.
    #[must_use]
    pub fn max_prompt_tokens(&self) -> usize {
        self.max_prompt_tokens
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
        thread::spawn(move || {
            run_delivery_task(receiver, ctx, shutdown_flag);
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
        self.enqueue(WriteItem::Envelope(envelope, sender));
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

    fn raw_write(
        &mut self,
        text: &str,
        append_enter: bool,
        context: &DeliveryContext,
    ) -> RawWriteResult {
        let tmux_socket_path =
            tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let tmux_socket = tmux_socket_path.as_path();
        let pane_target =
            match resolve_active_pane_target(tmux_socket, context.target_session.as_str()) {
                Ok(pane_target) => pane_target,
                Err(reason) => return RawWriteResult::Failed { reason },
            };
        match inject_literal_text(tmux_socket, &pane_target, text, append_enter) {
            Ok(()) => RawWriteResult::Written,
            Err(reason) => RawWriteResult::Failed { reason },
        }
    }

    fn shutdown(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        self.sender = None;
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        None
    }

    fn prepare_delivery(
        &self,
        context: &DeliveryContext,
    ) -> Result<DeliveryPreparation, DeliveryWaitError> {
        let tmux_socket_path =
            tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let prompt_readiness = match &context.target_member.target {
            TargetConfiguration::Tmux(tmux_target) => tmux_target.prompt_readiness.as_ref(),
            _ => None,
        };
        let pane = wait_for_quiescent_pane(
            tmux_socket_path.as_path(),
            context.target_session.as_str(),
            context.quiet_window,
            context.quiescence_timeout,
            prompt_readiness,
        )?;
        Ok(DeliveryPreparation {
            pre_resolved_target: Some(pane),
        })
    }

    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult {
        let target_session = context.target_session.clone();
        let message_id = envelopes
            .first()
            .map(|envelope| envelope.message_id.clone())
            .unwrap_or_default();
        let tmux_socket_path =
            tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let tmux_socket = tmux_socket_path.as_path();

        let pane_target = match context.pre_resolved_target.clone() {
            Some(pane_target) => pane_target,
            None => match resolve_active_pane_target(tmux_socket, target_session.as_str()) {
                Ok(pane_target) => pane_target,
                Err(reason) => {
                    return single_result(SingleDeliveryOutcome {
                        target_session,
                        message_id,
                        outcome: SendOutcome::Failed,
                        reason_code: Some(TMUX_TARGET_UNAVAILABLE_CODE.to_string()),
                        reason: Some(reason),
                        details: None,
                    });
                }
            },
        };

        let mut failed_reason = None::<String>;
        for envelope in &envelopes {
            if let Err(reason) = inject_literal_text(
                tmux_socket,
                &pane_target,
                envelope.rendered.as_str(),
                envelope.append_enter,
            ) {
                failed_reason = Some(reason);
                break;
            }
        }

        let outcome = match failed_reason {
            None => SingleDeliveryOutcome {
                target_session,
                message_id,
                outcome: SendOutcome::Delivered,
                reason_code: None,
                reason: None,
                details: None,
            },
            Some(reason) => SingleDeliveryOutcome {
                target_session,
                message_id,
                outcome: SendOutcome::Failed,
                reason_code: None,
                reason: Some(reason),
                details: None,
            },
        };
        single_result(outcome)
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
) {
    let tmux_socket_path = tmux_socket_path_for_runtime_directory(ctx.runtime_directory.as_path());

    let prompt_readiness = match &ctx.target_member.target {
        TargetConfiguration::Tmux(tmux_target) => tmux_target.prompt_readiness.clone(),
        _ => None,
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
                        &mut receiver,
                        &shutdown_flag,
                        &ctx.target_session,
                    );
                }
                return;
            }
        };

        // Process the item and any immediately available follow-ups.
        // Stop at the first raw item (after flushing the preceding group).
        match item {
            WriteItem::Envelope(env, sender) => {
                group.push((env, sender));
            }
            WriteItem::Raw(content, append_enter, sender) => {
                // Raw is a batch barrier: flush preceding envelopes first.
                if !group.is_empty() {
                    flush_and_resolve(
                        &mut group,
                        &tmux_socket_path,
                        &ctx.target_session,
                        prompt_readiness.as_ref(),
                        &mut receiver,
                        &shutdown_flag,
                        &ctx.target_session,
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
                    group.push((env, sender));
                }
                Ok(WriteItem::Raw(content, append_enter, sender)) => {
                    // Flush the group accumulated before this raw.
                    if !group.is_empty() {
                        flush_and_resolve(
                            &mut group,
                            &tmux_socket_path,
                            &ctx.target_session,
                            prompt_readiness.as_ref(),
                            &mut receiver,
                            &shutdown_flag,
                            &ctx.target_session,
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
                            &mut receiver,
                            &shutdown_flag,
                            &ctx.target_session,
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
                            &mut receiver,
                            &shutdown_flag,
                            &ctx.target_session,
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
            outcome: SendOutcome::Failed,
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
            outcome: SendOutcome::Failed,
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
fn flush_and_resolve(
    group: &mut Vec<(DeliveryEnvelope, OutcomeSender)>,
    tmux_socket_path: &Path,
    target_session: &str,
    prompt_readiness: Option<&PromptReadinessTemplate>,
    receiver: &mut mpsc::Receiver<WriteItem>,
    shutdown_flag: &AtomicBool,
    task_target_session: &str,
) {
    if group.is_empty() {
        return;
    }

    // Use the head envelope's quiescence hints for the entire group.
    let quiet_window = group[0].0.quiet_window;
    let quiescence_timeout = group[0].0.quiescence_timeout;

    // Deferred raw item from the post-quiescence drain. Carries across
    // coalesce loop iterations so the re-check happens before paste.
    let mut deferred_raw: Option<(String, bool, OutcomeSender)> = None;

    // Coalesce-during-wait loop.
    loop {
        if shutdown_flag.load(Ordering::Acquire) {
            for (envelope, sender) in group.drain(..) {
                let _ = sender.send(make_dropped_on_shutdown(
                    task_target_session,
                    &envelope.message_id,
                ));
            }
            return;
        }

        // Wait for quiescence (blocks).
        match wait_for_quiescent_pane(
            tmux_socket_path,
            target_session,
            quiet_window,
            quiescence_timeout,
            prompt_readiness,
        ) {
            Ok(_) => {}
            Err(DeliveryWaitError::Shutdown) => {
                for (envelope, sender) in group.drain(..) {
                    let _ = sender.send(make_dropped_on_shutdown(
                        task_target_session,
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
                        group.push((env, sender));
                        absorbed = true;
                    }
                    Ok(WriteItem::Raw(content, append_enter, sender)) => {
                        if absorbed {
                            deferred_raw = Some((content, append_enter, sender));
                            break;
                        } else {
                            paste_group(group, tmux_socket_path, target_session);
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
                        paste_group(group, tmux_socket_path, target_session);
                        drain_remaining_as_dropped(receiver, task_target_session);
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
    paste_group(group, tmux_socket_path, target_session);

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

/// Paste all envelopes in the group and resolve each sender with its own
/// message_id. Does NOT consume items from the channel.
fn paste_group(
    group: &mut Vec<(DeliveryEnvelope, OutcomeSender)>,
    tmux_socket_path: &Path,
    target_session: &str,
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

    let mut failed_reason = None::<String>;
    for (envelope, _) in group.iter() {
        if let Err(reason) = inject_literal_text(
            tmux_socket_path,
            &pane_target,
            envelope.rendered.as_str(),
            envelope.append_enter,
        ) {
            failed_reason = Some(reason);
            break;
        }
    }

    for (envelope, sender) in group.drain(..) {
        let outcome = match &failed_reason {
            None => SingleDeliveryOutcome {
                target_session: target_session.to_string(),
                message_id: envelope.message_id.clone(),
                outcome: SendOutcome::Delivered,
                reason_code: None,
                reason: None,
                details: None,
            },
            Some(reason) => SingleDeliveryOutcome {
                target_session: target_session.to_string(),
                message_id: envelope.message_id.clone(),
                outcome: SendOutcome::Failed,
                reason_code: None,
                reason: Some(reason.clone()),
                details: None,
            },
        };
        let _ = sender.send(outcome);
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
/// caller's message_id.
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
            outcome: SendOutcome::Failed,
            reason_code: Some("quiescence_timeout".to_string()),
            reason: Some(format!(
                "quiescence timeout after {}ms (readiness_mismatch={}, reason={:?})",
                timeout.as_millis(),
                readiness_mismatch,
                mismatch_reason
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
        outcome: SendOutcome::Failed,
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

/// Wraps a single combined outcome as a [`DeliveryResult`]; tmux produces one
/// outcome per `deliver` call which the relay fans out across the batch.
fn single_result(outcome: SingleDeliveryOutcome) -> DeliveryResult {
    DeliveryResult {
        outcomes: vec![outcome],
    }
}

#[derive(Debug)]
struct PromptReadinessMatcher {
    prompt_regex: Regex,
    inspect_lines: usize,
    input_idle_cursor_column: Option<usize>,
}

#[derive(Debug, Default)]
struct PromptReadinessEvaluation {
    ready: bool,
    mismatch_reason: Option<String>,
    inspected_block: Option<String>,
    regex_matched: Option<bool>,
    expected_cursor_column: Option<usize>,
    observed_cursor_column: Option<usize>,
}

/// Signature of a non-ready evaluation used to dedup `delivery_prompt_mismatch`
/// log lines emitted from the quiescence wait. When the pane is stuck on the
/// same non-matching state (for example a Claude Code tool-approval dialog
/// that the readiness regex does not match), repeated identical evaluations
/// across poll ticks collapse to a single inscription. The dialog is still
/// treated as non-quiescent and delivery still blocks until the state clears.
#[derive(Debug, PartialEq, Eq)]
struct PromptMismatchSignature {
    mismatch_reason: Option<String>,
    inspected_block: Option<String>,
    regex_matched: Option<bool>,
    expected_cursor_column: Option<usize>,
    observed_cursor_column: Option<usize>,
}

impl PromptMismatchSignature {
    fn from_evaluation(evaluation: &PromptReadinessEvaluation) -> Self {
        Self {
            mismatch_reason: evaluation.mismatch_reason.clone(),
            inspected_block: evaluation.inspected_block.clone(),
            regex_matched: evaluation.regex_matched,
            expected_cursor_column: evaluation.expected_cursor_column,
            observed_cursor_column: evaluation.observed_cursor_column,
        }
    }
}

/// Returns whether a mismatch evaluation should emit a fresh diagnostic. The
/// first call after entering the wait, and every call whose evaluation
/// signature differs from the last emitted one, returns `true` and updates
/// `last`. Repeated identical signatures return `false`.
fn should_emit_prompt_mismatch(
    last: &mut Option<PromptMismatchSignature>,
    evaluation: &PromptReadinessEvaluation,
) -> bool {
    let signature = PromptMismatchSignature::from_evaluation(evaluation);
    if last.as_ref() == Some(&signature) {
        false
    } else {
        *last = Some(signature);
        true
    }
}

/// Blocks until the target's active pane is quiescent (and, if configured,
/// matches the prompt-readiness template), returning the resolved pane.
fn wait_for_quiescent_pane(
    tmux_socket: &Path,
    target_session: &str,
    quiet_window: Duration,
    quiescence_timeout: Option<Duration>,
    prompt_readiness: Option<&PromptReadinessTemplate>,
) -> Result<String, DeliveryWaitError> {
    let readiness = build_prompt_readiness_matcher(prompt_readiness)
        .map_err(|reason| DeliveryWaitError::Failed { reason })?;
    let deadline = quiescence_timeout.map(|timeout| Instant::now() + timeout);
    let mut readiness_mismatch = false;
    let mut mismatch_reason = None::<String>;
    let mut last_mismatch_signature: Option<PromptMismatchSignature> = None;
    loop {
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }
        let pane_before = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_before = capture_pane_snapshot(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let activity_before = resolve_window_activity_marker(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;

        thread::sleep(quiet_window);
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }

        let pane_after = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_after = capture_pane_snapshot(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let activity_after = resolve_window_activity_marker(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let pane_is_quiescent = pane_before == pane_after
            && snapshot_before == snapshot_after
            && match (activity_before.as_ref(), activity_after.as_ref()) {
                (Some(before), Some(after)) => before == after,
                _ => true,
            };
        if pane_is_quiescent {
            if let Some(reason) =
                operator_interaction_active(tmux_socket, target_session, pane_after.as_str())
                    .map_err(|reason| DeliveryWaitError::Failed { reason })?
            {
                emit_delivery_diagnostic(
                    "delivery_operator_interaction",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                        "reason": reason,
                    }),
                );
                continue;
            }
            let evaluation = match prompt_readiness_matches(
                tmux_socket,
                pane_after.as_str(),
                snapshot_after.as_str(),
                readiness.as_ref(),
            ) {
                Ok(evaluation) => evaluation,
                Err(reason) => return Err(DeliveryWaitError::Failed { reason }),
            };
            if evaluation.ready {
                emit_delivery_diagnostic(
                    "delivery_ready",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                    }),
                );
                return Ok(pane_after);
            }
            readiness_mismatch = true;
            mismatch_reason = evaluation.mismatch_reason.clone();
            if should_emit_prompt_mismatch(&mut last_mismatch_signature, &evaluation) {
                emit_delivery_diagnostic(
                    "delivery_prompt_mismatch",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                        "mismatch_reason": evaluation.mismatch_reason,
                        "regex_matched": evaluation.regex_matched,
                        "inspected_block": evaluation.inspected_block,
                        "expected_cursor_column": evaluation.expected_cursor_column,
                        "observed_cursor_column": evaluation.observed_cursor_column,
                    }),
                );
            }
        }

        if deadline.is_some_and(|value| Instant::now() >= value) {
            let timeout = quiescence_timeout.unwrap_or_default();
            emit_delivery_diagnostic(
                "quiescence_timeout",
                &json!({
                    "target_session": target_session,
                    "quiescence_timeout_ms": timeout.as_millis(),
                    "readiness_mismatch": readiness_mismatch,
                    "mismatch_reason": mismatch_reason,
                }),
            );
            return Err(DeliveryWaitError::Timeout {
                timeout,
                readiness_mismatch,
                mismatch_reason,
            });
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_emits_only_on_signature_transitions() {
        let stuck = PromptReadinessEvaluation {
            mismatch_reason: Some("prompt regex did not match inspected pane tail".to_string()),
            inspected_block: Some("Do you want to proceed?".to_string()),
            regex_matched: Some(false),
            expected_cursor_column: Some(4),
            observed_cursor_column: None,
            ..PromptReadinessEvaluation::default()
        };
        let cursor_only = PromptReadinessEvaluation {
            mismatch_reason: Some("cursor column 0 did not match required 4".to_string()),
            inspected_block: Some("> ".to_string()),
            regex_matched: Some(true),
            expected_cursor_column: Some(4),
            observed_cursor_column: Some(0),
            ..PromptReadinessEvaluation::default()
        };

        let mut last = None;
        assert!(
            should_emit_prompt_mismatch(&mut last, &stuck),
            "first mismatch must emit",
        );
        assert!(
            !should_emit_prompt_mismatch(&mut last, &stuck),
            "identical follow-up must suppress",
        );
        assert!(
            !should_emit_prompt_mismatch(&mut last, &stuck),
            "second identical follow-up must suppress",
        );
        assert!(
            should_emit_prompt_mismatch(&mut last, &cursor_only),
            "signature change must re-emit",
        );
        assert!(
            !should_emit_prompt_mismatch(&mut last, &cursor_only),
            "post-change identical follow-up must suppress",
        );
    }
}
