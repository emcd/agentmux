//! The tmux [`Transport`] implementation with an internal delivery task.
//!
//! [`TmuxTransport`] owns an internal ordered channel and a background delivery
//! task. The relay worker submits writes via [`mailw`](Transport::mailw) and
//! [`raww`](Transport::raww) without blocking; the internal task drains the
//! channel in FIFO order, accumulates contiguous envelopes into flush groups,
//! renders each envelope's pane text and combines them into token-budget-bounded
//! prompts (the same greedy split the ACP transport applies to its turns), and
//! pastes each combined prompt. Raw writes act as batch barriers: the task
//! flushes any buffered envelope group before delivering the raw write.
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
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::configuration::TargetConfiguration;
use crate::envelope::{PromptBatchSettings, batch_envelope_groups};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::transports::{
    DeliveryEnvelope, GenerationFence, LookMode, LookSnapshotPayload, OutcomeFuture, OutputView,
    PackingUnitId, PartitionError, PartitionSink, SendOutcome, SingleDeliveryOutcome,
    StartupContext, SubmissionEvidence, Transport, TransportError, TransportHealth,
    TransportReadiness, TransportStatus, UnreachableSince,
};

/// Default tmux look window applied when the caller omits a window size.
const LOOK_LINES_DEFAULT: usize = 120;

use super::pane::{
    TmuxInvocationSlot, capture_pane_tail_lines, inject_literal_text, publish_tmux_invocations,
    resolve_active_pane_target, terminate_published_invocation,
};
use super::prompt_probe::{PanePromptProbe, RealPanePromptProbe};

const TMUX_TARGET_UNAVAILABLE_CODE: &str = "tmux_target_unavailable";
const TMUX_DELIVERY_THREAD_STOPPED_CODE: &str = "tmux_delivery_thread_stopped";

/// Capacity of the internal write channel. Sized to absorb bursts from the
/// relay worker without unbounded growth; the delivery task drains continuously.
const WRITE_CHANNEL_CAPACITY: usize = 256;

/// How often the observer thread re-reads its pane.
///
/// Matches the relay worker's poll cadence: a level staler than the interval at
/// which it is read would make the reader wait for news that had already
/// arrived.
const OBSERVER_INTERVAL_MS: u64 = 100;

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
/// delivery task drains the channel, groups contiguous envelopes, and pastes.
pub struct TmuxTransport {
    batch_settings: PromptBatchSettings,
    /// The relay's guard, for reporting which members share one paste.
    ///
    /// Cloned into the delivery thread rather than reached through the relay:
    /// the partition is decided in `paste_group`, on that thread, between the
    /// token-budget split and the injection it brackets.
    partition_sink: Arc<dyn PartitionSink>,
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
    /// Latch for the health axis: when the pane first stopped being observable.
    unreachable_since: UnreachableSince,
    /// The most recent pane observation, published by the observer thread and
    /// read by the two contract predicates.
    ///
    /// The relay reads levels from its async worker, so the read must not block
    /// and must not spawn a tmux client. It also must not spawn one *there*
    /// specifically: `publish_tmux_invocations` is thread-local, so an
    /// invocation made from a runtime worker thread lands in no slot and
    /// `terminate_generation` cannot reach it. Observing on an owned thread that
    /// publishes into a slot fixes both at once.
    observation: Arc<Mutex<PaneObservation>>,
    /// The observer thread's own invocation slot.
    ///
    /// Separate from the delivery thread's: a slot holds one child, so two
    /// publishers would clobber each other. The fence signals both.
    observer_invocation: TmuxInvocationSlot,
    /// Handle to the observer thread, for cessation observation and shutdown.
    observer_handle: Option<thread::JoinHandle<()>>,
    /// Relay-provided closure the observer invokes when its observation changes.
    ///
    /// An opaque `Arc<dyn Fn>` supplied at construction, the same shape Pty uses
    /// for `mirror_state`, so this module names no relay type. Correctness never
    /// depends on it: the authoritative state is the level the relay reads, and a
    /// lost invocation only delays a delivery to the next poll.
    readiness_notifier: Option<ReadinessNotifier>,
}

/// Relay-provided wakeup invoked when a transport's observed level changes.
pub type ReadinessNotifier = Arc<dyn Fn() + Send + Sync>;

/// The most recent thing the observer thread learned about the pane.
#[derive(Clone, Copy, Debug)]
enum PaneObservation {
    /// Nothing observed yet. Deliberately distinct from `Unreachable`: not having
    /// looked is ignorance, not evidence, and reporting it as unreachable would
    /// start a dwell against a target nobody has examined.
    Pending,
    /// The pane could not be inspected at all — a departed session, a dead
    /// server, or a delivery task that has stopped.
    Unreachable,
    /// The pane was inspected, and this is what it said.
    Observed { ready: bool },
}

impl PaneObservation {
    /// Whether replacing `self` with `next` changes either axis the relay reads.
    fn differs_from(self, next: Self) -> bool {
        !matches!(
            (self, next),
            (Self::Pending, Self::Pending)
                | (Self::Unreachable, Self::Unreachable)
                | (
                    Self::Observed { ready: true },
                    Self::Observed { ready: true }
                )
                | (
                    Self::Observed { ready: false },
                    Self::Observed { ready: false }
                )
        )
    }
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
    /// Takes the readiness notifier at construction rather than through a
    /// setter. `startup` spawns the observer, and the observer captures the
    /// notifier as it starts; anything that installs the closure afterwards
    /// hands it to a thread that has already taken its copy, leaving a wakeup
    /// path that is wired at every point except the one that fires it. Passing
    /// it in makes that ordering unrepresentable instead of merely documented.
    #[must_use]
    pub fn new(
        batch_settings: PromptBatchSettings,
        readiness_notifier: Option<ReadinessNotifier>,
        partition_sink: Arc<dyn PartitionSink>,
    ) -> Self {
        Self {
            batch_settings,
            readiness_notifier,
            partition_sink,
            sender: None,
            task_handle: None,
            task_context: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            invocation: TmuxInvocationSlot::default(),
            unreachable_since: UnreachableSince::default(),
            observation: Arc::new(Mutex::new(PaneObservation::Pending)),
            observer_invocation: TmuxInvocationSlot::default(),
            observer_handle: None,
        }
    }

    /// Starts the observer thread if it is not already running.
    ///
    /// Owned rather than folded into the delivery thread because that thread
    /// blocks on its channel: it observes nothing while idle, which is exactly
    /// when the relay is holding a member and needs a current level.
    fn ensure_observer_running(&mut self) {
        if self
            .observer_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return;
        }
        let Some(context) = self.task_context.clone() else {
            return;
        };
        let TargetConfiguration::Tmux(target) = context.target_member.target.clone() else {
            return;
        };
        let socket = tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let observation = Arc::clone(&self.observation);
        let slot = Arc::clone(&self.observer_invocation);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let target_session = context.target_session.clone();
        let notifier = self.readiness_notifier.clone();
        self.observer_handle = Some(thread::spawn(move || {
            // Publish before the first invocation, so every tmux client this
            // thread spawns is reachable by the fence.
            publish_tmux_invocations(slot);
            while !shutdown_flag.load(Ordering::Acquire) {
                let observed = observe_pane_once(
                    socket.as_path(),
                    target_session.as_str(),
                    target.prompt_readiness.as_ref(),
                );
                let next = match observed {
                    Some(ready) => PaneObservation::Observed { ready },
                    None => PaneObservation::Unreachable,
                };
                // Notify on the edge only. A wakeup per tick would make the
                // closure a second poll rather than a substitute for one, and the
                // relay re-reads the level itself on waking.
                let changed = {
                    let mut current = observation.lock().expect("tmux observation mutex");
                    let changed = current.differs_from(next);
                    *current = next;
                    changed
                };
                if changed && let Some(notifier) = notifier.as_ref() {
                    notifier();
                }
                thread::sleep(Duration::from_millis(OBSERVER_INTERVAL_MS));
            }
        }));
    }

    /// One pane observation, separating "could not observe" from "observed".
    ///
    /// `None` means the pane could not be inspected at all — the delivery task
    /// is gone, the target is not a tmux target, or the tmux client call failed,
    /// which is what happens when the session or its server has departed.
    /// `Some(ready)` means the pane was inspected and this is what it said.
    ///
    /// The two readings feed different axes and must not be collapsed: an
    /// unobservable pane is not a busy one, and only the busy one is worth
    /// waiting on.
    /// The cached observation, plus the liveness facts only the transport holds.
    ///
    /// A stopped delivery task is unreachable however healthy the pane looks:
    /// there is nothing left to carry a write to it.
    fn observed(&self) -> PaneObservation {
        let live = self
            .sender
            .as_ref()
            .is_some_and(|sender| !sender.is_closed())
            && self
                .task_handle
                .as_ref()
                .is_some_and(|handle| !handle.is_finished())
            && self
                .observer_handle
                .as_ref()
                .is_some_and(|handle| !handle.is_finished());
        if !live {
            return PaneObservation::Unreachable;
        }
        *self.observation.lock().expect("tmux observation mutex")
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
        let partition_sink = Arc::clone(&self.partition_sink);
        let task_handle = thread::spawn(move || {
            publish_tmux_invocations(invocation);
            run_delivery_task(receiver, ctx, shutdown_flag, batch_settings, partition_sink);
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
        // The observer holds a second slot, and a fence that reached only the
        // delivery thread would leave a tmux client of this generation running.
        terminate_published_invocation(&self.observer_invocation);
        self.sender = None;
    }

    fn generation_ceased(&self) -> bool {
        // A generation that never started a delivery thread owns no executor and
        // has trivially ceased.
        self.task_handle
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
            && self
                .observer_handle
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
        self.ensure_observer_running();
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
        matches!(self.observed(), PaneObservation::Observed { ready: true })
    }

    fn health(&self) -> TransportHealth {
        // Both predicates now read one cached observation, so they cannot
        // disagree about reachability and the latch cannot survive a successful
        // look — the interleaving that needed a defensive fold in each of them.
        //
        // `Pending` reports healthy. Not having observed yet is ignorance, not
        // evidence, and starting a dwell against a target nobody has examined
        // would resolve members on the strength of not knowing. It reports
        // unready through the other axis, so such a member is held.
        let reachable = !matches!(self.observed(), PaneObservation::Unreachable);
        self.unreachable_since.fold(reachable)
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
/// contiguous envelopes into flush groups, and pastes.
///
/// Items are processed as a FIFO stream: accumulate envelopes until a raw item
/// is encountered, flush the group, deliver the raw, then continue. This
/// preserves interleaving order and treats raw items as batch barriers.
fn run_delivery_task(
    mut receiver: mpsc::Receiver<WriteItem>,
    ctx: DeliveryTaskContext,
    shutdown_flag: Arc<AtomicBool>,
    batch_settings: PromptBatchSettings,
    partition_sink: Arc<dyn PartitionSink>,
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
                        partition_sink.as_ref(),
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
                        partition_sink.as_ref(),
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
                            partition_sink.as_ref(),
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
                            partition_sink.as_ref(),
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
                            partition_sink.as_ref(),
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

/// Paste an envelope group and resolve each sender with its own message id.
///
/// Handover readiness is observed before authorization; this function performs
/// only the target-side write and its immediate outcome resolution.
#[allow(clippy::too_many_arguments)]
fn flush_and_resolve(
    group: &mut Vec<(DeliveryEnvelope, OutcomeSender)>,
    tmux_socket_path: &Path,
    target_session: &str,
    shutdown_flag: &AtomicBool,
    batch_settings: PromptBatchSettings,
    partition_sink: &dyn PartitionSink,
) {
    if shutdown_flag.load(Ordering::Acquire) {
        drain_group_as_dropped(group, target_session);
        return;
    }
    paste_group(
        group,
        tmux_socket_path,
        target_session,
        batch_settings,
        partition_sink,
    );
}

/// Splits a flush group into runs that may be coalesced into one paste,
/// returning each run's length in order. A terminal-outcome receipt forms a run
/// of its own.
///
/// This is a correctness barrier, not presentation. A receipt bypasses admission
/// and belongs to no packing unit, while its peer groupmates belong to one, so a
/// paste carrying both would tie two members to one fate that only one of them
/// has: when the peers' declaration is refused, the prompt must produce no effect
/// — and the receipt, which needed no declaration, would be dropped with it. It
/// cannot be rescued after the fact either, because the budget group's combined
/// prompt is a single string, so there is no receipt-only text left to write.
/// Separating them before budgeting is what makes the refusal path drop only
/// members the guard owns.
///
/// ACP reaches the same rule from the other direction — receipts are its flush
/// barrier so they never coalesce with peer traffic — and Pty writes one member
/// per primitive, so neither needs this.
pub fn coalescing_runs(is_receipt: &[bool]) -> Vec<usize> {
    let mut runs: Vec<usize> = Vec::new();
    let mut previous_was_peer = false;
    for &receipt in is_receipt {
        if receipt || !previous_was_peer {
            runs.push(1);
        } else if let Some(last) = runs.last_mut() {
            *last += 1;
        }
        previous_was_peer = !receipt;
    }
    runs
}

/// Declares the guard-tracked members of one prompt, or nothing when the prompt
/// carries none.
///
/// A prompt made up entirely of terminal-outcome receipts has no tracked member,
/// and an empty declaration is refused by the ledger. `Ok(None)` says the prompt
/// may be written with no unit behind it, which is correct: nothing in it is
/// resolved by the guard.
fn declare_tracked_members(
    partition_sink: &dyn PartitionSink,
    member_ids: &[&str],
) -> Result<Option<PackingUnitId>, PartitionError> {
    if member_ids.is_empty() {
        return Ok(None);
    }
    partition_sink.declare(member_ids).map(Some)
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
    partition_sink: &dyn PartitionSink,
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
    // Split at receipt boundaries before budgeting, so no prompt ever mixes a
    // receipt with peer traffic. See `coalescing_runs` for why that separation is
    // a correctness requirement rather than presentation.
    let runs = coalescing_runs(&group.iter().map(|(e, _)| e.is_receipt).collect::<Vec<_>>());
    let budget_groups: Vec<_> = {
        let mut rendered_rest = rendered.as_slice();
        let mut groups = Vec::new();
        for run_length in runs {
            let (run, rest) = rendered_rest.split_at(run_length);
            rendered_rest = rest;
            groups.extend(batch_envelope_groups(run, batch_settings));
        }
        groups
    };

    let mut members = group.drain(..);
    for budget_group in budget_groups {
        // Slice the parallel sender vector to this prompt's contributing members.
        let prompt_members: Vec<(String, OutcomeSender, bool)> = members
            .by_ref()
            .take(budget_group.member_count)
            .map(|(envelope, sender)| (envelope.message_id, sender, !envelope.is_receipt))
            .collect();
        // This is the partition the relay cannot see: one paste carries every
        // member of this budget group, so they share one fate and must resolve
        // from one record. Declared before the injection below, because after it
        // partial effect cannot be excluded for any of them.
        // Terminal-outcome receipts bypass admission, so they hold no ledger
        // entry and belong to no unit. Declaring one would be refused — the
        // ledger cannot tell a member it never had from one that already
        // terminalized — and the refusal would drop the whole prompt, receipt and
        // peer traffic alike. Receipts never reach here mixed with peer traffic —
        // `coalescing_runs` above puts each in its own run — so this filter yields
        // either every member or none, and the refusal arm below can only ever
        // drop members the guard owns.
        let member_ids: Vec<&str> = prompt_members
            .iter()
            .filter(|(_, _, tracked)| *tracked)
            .map(|(message_id, _, _)| message_id.as_str())
            .collect();
        let unit = match declare_tracked_members(partition_sink, &member_ids) {
            Ok(unit) => unit,
            Err(_) => {
                // The relay refused the whole proposed unit, so this prompt must
                // produce no effect. Dropping the senders without resolving is
                // the correct handover back: the guard owns these members now and
                // resolves each from the evidence order, which — since none of
                // them was bound — proves `not_submitted` rather than guessing.
                // Sending an outcome here would be the duplicate resolution the
                // guard exists to prevent.
                drop(prompt_members);
                continue;
            }
        };
        // Envelope-mode writes always submit with Enter; the combined prompt is
        // pasted once for the whole budget group.
        let inject_result = inject_literal_text(
            tmux_socket_path,
            &pane_target,
            budget_group.combined_prompt.as_str(),
            true,
        );
        // A paste is a body write followed by an Enter, so a failure cannot
        // exclude partial effect: the group's evidence is `SubmissionUnknown`
        // rather than `NotSubmitted`. Recorded before the fan-out below, so a
        // member the fan-out never reaches still resolves from what this paste
        // proved instead of from its own absence.
        if let Some(unit) = unit {
            partition_sink.record(
                unit,
                match &inject_result {
                    Ok(()) => SubmissionEvidence::Submitted,
                    Err(_) => SubmissionEvidence::SubmissionUnknown,
                },
            );
        }
        for (message_id, sender, _) in prompt_members {
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

/// One pane observation, separating "could not observe" from "observed".
///
/// `None` means the pane could not be inspected at all, which is what a departed
/// session or a dead tmux server looks like. `Some(ready)` means it was
/// inspected and this is what it said — a settled non-prompt frame is a *busy*
/// target and belongs to the readiness axis, not the health one.
///
/// A free function because the observer thread owns no transport: it holds only
/// what it needs to look, which is also what keeps it off the transport lock.
fn observe_pane_once(
    socket: &Path,
    target_session: &str,
    prompt_readiness: Option<&crate::configuration::PromptReadinessTemplate>,
) -> Option<bool> {
    let mut probe = RealPanePromptProbe::new(socket, target_session, prompt_readiness).ok()?;
    probe
        .next_evaluation()
        .ok()
        .map(|evaluation| evaluation.ready)
}

#[cfg(test)]
mod observation_edge_tests {
    use super::PaneObservation;

    /// The notifier fires on change, so what counts as a change is the contract.
    ///
    /// Firing every tick would make the closure a second poll rather than a
    /// substitute for one; never firing on a real transition would leave a held
    /// member waiting out the backstop interval it exists to avoid.
    #[test]
    fn an_observation_differs_only_when_an_axis_the_relay_reads_moves() {
        let states = [
            PaneObservation::Pending,
            PaneObservation::Unreachable,
            PaneObservation::Observed { ready: false },
            PaneObservation::Observed { ready: true },
        ];
        for (index, current) in states.iter().enumerate() {
            for (other, next) in states.iter().enumerate() {
                assert_eq!(
                    current.differs_from(*next),
                    index != other,
                    "{current:?} -> {next:?} should {} a change",
                    if index == other { "not be" } else { "be" }
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::AddressIdentity;
    use crate::transports::{PackingUnitId, PartitionError};

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

        // No relay-admitted member exists here — the delivery thread is already
        // stopped, so nothing is ever declared — which is the only situation in
        // which a sink that records nothing is the right stand-in.
        struct NoDeclarations;
        impl PartitionSink for NoDeclarations {
            fn declare(&self, _member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
                Err(PartitionError::MemberNotBindable)
            }
            fn record(&self, _unit: PackingUnitId, _evidence: SubmissionEvidence) {}
        }

        let mut transport = TmuxTransport::new(
            PromptBatchSettings::default(),
            None,
            Arc::new(NoDeclarations),
        );
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
