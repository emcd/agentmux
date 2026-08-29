//! Delivery channel, batching, planning, and execution for the ACP transport.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::acp::client::AcpStdioClient;
use crate::envelope::PromptBatchSettings;
use crate::transports::{
    DeliveryEnvelope, SingleDeliveryOutcome, stopped_before_submission_outcome,
};

use super::state::AcpSharedState;
use super::turn::{TurnContext, TurnUnit, submit_envelope_turn, submit_raw_turn};

/// Items enqueued onto the ACP transport's internal ordered write channel.
///
/// Both [`crate::acp::transport::AcpTransport::mailw`] and
/// [`crate::acp::transport::AcpTransport::raww`] submit through a single
/// FIFO channel. The internal delivery task processes them in order; a `Raw`
/// item acts as a batch barrier (flushes any preceding `Envelope` group first).
pub(crate) enum WriteItem {
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

/// A batch of rendered envelopes with their metadata, ready for combining.
pub(crate) struct EnvelopeBatch {
    pub(crate) rendered: Vec<String>,
    pub(crate) message_ids: Vec<String>,
    pub(crate) decider_sessions: Vec<Vec<String>>,
    pub(crate) outcome_senders: Vec<tokio::sync::oneshot::Sender<SingleDeliveryOutcome>>,
}

impl EnvelopeBatch {
    /// Builds a single-envelope batch from the head envelope's submitted
    /// write item.
    pub(crate) fn from_head(
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
    pub(crate) fn absorb_envelope(
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
pub(crate) struct DeliveryChannels {
    pub(crate) rx: mpsc::Receiver<WriteItem>,
    pub(crate) shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    pub(crate) respawn_needed_tx: tokio::sync::watch::Sender<u64>,
}

pub(crate) struct DeliveryTaskIdentity {
    pub(crate) target_session: String,
}

/// Internal ACP delivery task. Runs on a dedicated thread, draining the write
/// channel, combining contiguous envelopes respecting the token budget, and
/// submitting turns to the ACP runtime. Exits when the channel closes (sender
/// dropped by `release_runtime()` or `shutdown()`).
pub(crate) fn acp_delivery_task(
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
            drain_and_resolve_stopped(&mut rx);
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
            resolve_head_as_stopped(head);
            drain_and_resolve_stopped(&mut rx);
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

/// Resolves the head item's outcome sender as stopped before submission when the
/// generation is stopped after the head was received but before plan execution.
/// Centralizes the post-receive resolution so the outer loop can hoist the check
/// above the plan dispatch.
/// Takes `head` by value so the moved `oneshot::Sender` can be consumed.
fn resolve_head_as_stopped(head: WriteItem) {
    let outcome_tx = match head {
        WriteItem::Envelope { outcome_tx, .. } => outcome_tx,
        WriteItem::Raw { outcome_tx, .. } => outcome_tx,
    };
    let _ = outcome_tx.send(stopped_generation_outcome());
}

/// Drains all remaining items from the write channel and resolves their outcome
/// senders as stopped before submission. Called when the generation's stop
/// signal fires.
fn drain_and_resolve_stopped(rx: &mut mpsc::Receiver<WriteItem>) {
    while let Ok(item) = rx.try_recv() {
        let outcome_tx = match item {
            WriteItem::Envelope { outcome_tx, .. } => outcome_tx,
            WriteItem::Raw { outcome_tx, .. } => outcome_tx,
        };
        let _ = outcome_tx.send(stopped_generation_outcome());
    }
}

/// The outcome for a write this generation was still holding when it was told to
/// stop. Nothing was rendered into a turn and no `session/prompt` was issued for
/// it, so `not_submitted` is what the transport can prove.
///
/// The identity fields are left empty exactly as they were when this resolved a
/// shutdown: the relay looks the member up by the collector it awaited rather
/// than by anything quoted back here. (Respawn invalidation is a distinct path:
/// it closes the channel, and the worker maps the dropped future to its own
/// outcome.)
fn stopped_generation_outcome() -> SingleDeliveryOutcome {
    stopped_before_submission_outcome(String::new(), String::new())
}

/// What a stopped generation tells the writes it was still holding.
///
/// Inline because both resolution points are internal to the delivery task and
/// the window that reaches them cannot be arranged from outside it: the head
/// case needs the stop signal to land between a receive and the plan dispatch
/// that follows it immediately, and the queued case needs items behind a head
/// that is being resolved. Calling the two functions directly is the same code
/// with those races removed.
///
/// The regression being pinned is a false statement rather than a missing one.
/// `fence_generation` clears the shutdown sender exactly as `shutdown()` does,
/// so the delivery task read every fenced generation as a relay shutdown and
/// told the sender so — while the relay carried on running. That is why the
/// reason code is asserted alongside the outcome: they reach a sender as
/// separate fields, and a corrected verdict beside the old explanation would
/// still be wrong.
#[cfg(test)]
mod stopped_generation_tests {
    use super::*;

    use crate::envelope::AddressIdentity;
    use crate::transports::DeliveryMessage;

    /// An **envelope** write, deliberately not a raw one. Raw is relay-declared
    /// before `raww` is ever called, so a raw item stopped in this channel is
    /// already bound to a singleton unit and the relay reconciles its result to
    /// that unit's record. Only an envelope is unbound here, so only an envelope
    /// carries the producer's spelling through to the sender unchanged.
    fn write_item(
        message_id: &str,
    ) -> (
        WriteItem,
        tokio::sync::oneshot::Receiver<SingleDeliveryOutcome>,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let envelope = DeliveryEnvelope {
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
            is_receipt: false,
        };
        (
            WriteItem::Envelope {
                envelope: Box::new(envelope),
                outcome_tx: tx,
            },
            rx,
        )
    }

    /// Resolves a head and one queued write through both stop paths, and reports
    /// what each was told.
    fn stop_and_collect() -> Vec<(&'static str, SingleDeliveryOutcome)> {
        let (head, head_rx) = write_item("head");
        let (queued, queued_rx) = write_item("queued");
        let (tx, mut rx) = mpsc::channel(4);
        tx.try_send(queued)
            .expect("queue one write behind the head");

        resolve_head_as_stopped(head);
        drain_and_resolve_stopped(&mut rx);

        [("head", head_rx), ("queued", queued_rx)]
            .into_iter()
            .map(|(label, receiver)| {
                let outcome = receiver
                    .blocking_recv()
                    .unwrap_or_else(|_| panic!("the {label} write is resolved, not dropped"));
                (label, outcome)
            })
            .collect()
    }

    /// Both post-receive resolution points report `not_submitted`, and name the
    /// cause without inventing one.
    ///
    /// The genuine shutdown is driven in this same fixture rather than left to
    /// the integration tests, because the claim under test is a
    /// *discrimination*: a fixture that only ever observes a fence cannot tell a
    /// transport that reads the cause from a transport that hardcodes one. The
    /// pairing asserts an unchanged **outcome** and a changed **reason** —
    /// deliberately not the `dropped_on_shutdown` spelling, which an authorized
    /// member may not resolve to at all. Only members the relay still holds as
    /// `Pending` carry that, and they never reach a transport channel, which is
    /// why the fixture pairing has to take this shape.
    #[test]
    fn a_stopped_generation_resolves_held_writes_as_not_submitted() {
        for (label, outcome) in stop_and_collect() {
            assert_eq!(
                outcome.outcome,
                crate::transports::SendOutcome::NotSubmitted,
                "the {label} write was never rendered into a turn",
            );
            assert_eq!(
                outcome.reason_code.as_deref(),
                Some("generation_fenced"),
                "the {label} write must not report the relay as shut down",
            );
        }

        // The positive control. Handlers are installed first, so the signal is
        // caught rather than terminating the run; the guard clears the flag when
        // it drops. Safe process-wide because nextest gives each test its own
        // process.
        let _guard = crate::runtime::signals::install_shutdown_signal_handlers()
            .expect("install shutdown signal handlers");
        let self_pid = i32::try_from(std::process::id()).expect("pid fits i32");
        assert_eq!(
            unsafe { libc::kill(self_pid, libc::SIGTERM) },
            0,
            "failed to signal this process"
        );
        while !crate::runtime::signals::shutdown_requested() {
            std::thread::yield_now();
        }

        for (label, outcome) in stop_and_collect() {
            assert_eq!(
                outcome.outcome,
                crate::transports::SendOutcome::NotSubmitted,
                "shutdown is a trigger; it does not choose the {label} outcome",
            );
            assert_eq!(
                outcome.reason_code.as_deref(),
                Some("relay_shutdown"),
                "the {label} write names the cause it was actually stopped by",
            );
        }
    }
}

/// Submits a single envelope as its own ACP turn with no batch coalescing.
/// Used by the receipt rendering path so a terminal-outcome receipt (a
/// relay/system-originated informational turn back to the original sender)
/// never absorbs peer envelopes and never lands inside a peer flush group:
/// the receipt is its own turn and is observable on its own. The receipt
/// resolves on completion, agent close, dispatcher refusal, serialization
/// failure, or shutdown — no elapsed-time bound is applied here.
fn submit_singleton_envelope(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    batch_settings: &PromptBatchSettings,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    envelope: Box<DeliveryEnvelope>,
    outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
) {
    // A receipt bypassed admission, so no unit exists for it and declaring one
    // would be refused — which, before this was distinguished, silently dropped
    // every terminal-outcome receipt routed back to an ACP sender.
    let unit = if envelope.is_receipt {
        TurnUnit::Untracked
    } else {
        TurnUnit::DeclareHere
    };
    let mut batch = EnvelopeBatch::from_head(&envelope, outcome_tx);
    flush_envelope_group(
        client,
        ctx,
        batch_settings,
        respawn_needed_tx,
        &mut batch,
        unit,
    );
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
/// Stop handling: between flush and boundary execution (and before
/// any blocking submit), the helper checks `is_shutdown(&mut shutdown_rx)`.
/// On a stop-during-execution, the pending batch's outcome senders
/// are resolved with `stopped_generation_outcome`, the channel is
/// drained via `drain_and_resolve_stopped`, and the helper returns so
/// the outer loop can break. Stop checks resolve held senders and
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
                let _ = tx.send(stopped_generation_outcome());
            }
            drain_and_resolve_stopped(rx);
            return;
        }
        // Peer traffic only: receipts are a flush barrier and never coalesce
        // into this path, so every member here holds a ledger entry.
        flush_envelope_group(
            client,
            ctx,
            batch_settings,
            respawn_needed_tx,
            &mut batch,
            TurnUnit::DeclareHere,
        );
    }

    // Execute the boundary action.
    match plan.boundary {
        BoundaryAction::ReturnToOuterLoop => {}
        BoundaryAction::SubmitReceiptSingleton {
            envelope,
            outcome_tx,
        } => {
            if is_shutdown(shutdown_rx) {
                let _ = outcome_tx.send(stopped_generation_outcome());
                drain_and_resolve_stopped(rx);
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
                let _ = outcome_tx.send(stopped_generation_outcome());
                drain_and_resolve_stopped(rx);
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
    unit: TurnUnit,
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
            unit,
        );
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
