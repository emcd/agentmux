//! Forming one batch, authorizing it whole, and writing every member of it.

use std::{collections::HashMap, time::Instant};

use serde_json::json;

use tokio::task::JoinSet;

use crate::configuration::SessionType;
use crate::protocol::mailbox::MailboxPayload;
use crate::protocol::message::DeliveryEnvelope;
use crate::relay::{AsyncDeliveryTask, session_type_not_implemented};
use crate::runtime::inscriptions::emit_inscription;
use crate::transports::{PartitionError, TransportImpl};

use super::super::super::admission::canonical_payload_bytes;
use super::super::batch::HandoverWindow;
use super::gate::{TargetGate, gate_target, target_unreachable_result};
use super::intake::IntakeTask;
use super::spawn::{InflightMember, InflightOutcome};

/// The relay-owned bookkeeping one batch is submitted against.
///
/// Grouped rather than passed as parallel parameters: quota, handover budget, and
/// the dwell are the three things every member of a batch moves against, and they
/// travel together through formation, authorization and submission.
pub(super) struct SubmitContext<'worker> {
    /// How long a target may be *continuously* unreachable before a held member
    /// resolves rather than keeping its place in the queue.
    pub(super) unreachable_dwell: std::time::Duration,
    /// The relay's pending-slot counter, released at each member's terminal
    /// transition.
    pub(super) pending: &'worker std::sync::atomic::AtomicUsize,
    /// How much this target's transport is already holding. Advanced only by a
    /// batch that authorization accepted.
    pub(super) window: &'worker mut HandoverWindow,
    /// The target's activity marker as of the previous gate decision, against
    /// which this one is compared. `None` until the first observation, which
    /// records without suppressing.
    pub(super) last_activity: &'worker mut Option<u64>,
}

/// Forms one batch, authorizes it whole, and submits every member of it.
///
/// The three steps are in that order and the order is the contract: a batch's
/// membership SHALL be fixed before any member of it is authorized, because
/// mutable batch membership is what let one outcome be reported for members that
/// were written and members that were not. Fixing membership first is also what
/// makes the shared `BatchId` legal — minting one on the first member and handing
/// it to members authorized later would be absorption into an already-authorized
/// batch, which the contract forbids by name.
///
/// Returns whichever member the batch could not take, for the caller to hold. It
/// is the head of the next batch, so the target's FIFO order survives: this is
/// the only way a member leaves here without a terminal outcome.
pub(super) async fn submit_batch(
    head: IntakeTask,
    context: SubmitContext<'_>,
    transport: &mut TransportImpl,
    inflight: &mut JoinSet<InflightOutcome>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
) -> Option<IntakeTask> {
    // Pubsub is rejected rather than gated or batched. It reports unready like any
    // transport with no delivery path, so gating it would hold a member no
    // transport can ever accept, and there is nothing to authorize a batch
    // against: its `mailw`/`raww` are `unimplemented!`.
    if matches!(transport, TransportImpl::Pubsub) {
        reject_undeliverable(&head.task, context.pending);
        return None;
    }
    // Readiness gates authorization, not submission. A target that cannot take a
    // handover now must not have a batch authorized against it: authorization is
    // the linearization point, and quota releases only at the terminal
    // transition, so authorizing early would commit members to a generation that
    // cannot act on them. They stay `Pending` instead, which is a state they may
    // occupy indefinitely — how long a target stays busy is not evidence about
    // the target, and no elapsed duration converts this wait into an outcome.
    //
    // Reading the level here is deliberately advisory. It can go stale between
    // this check and the writes below, and when it does the invocation fails and
    // resolves through the guard's evidence order rather than being retried
    // behind the sender's back.
    match gate_target(transport, context.unreachable_dwell, context.last_activity).await {
        TargetGate::Open => {}
        TargetGate::Hold => return Some(head),
        TargetGate::Unreachable => {
            super::super::super::async_worker::complete_task_outcome(
                &head.task,
                Ok(target_unreachable_result(
                    &head.task,
                    context.unreachable_dwell,
                )),
            );
            super::super::super::async_worker::release_pending_slot(context.pending);
            return None;
        }
    }

    // Formation runs against a scratch copy of the window, and the real one is
    // advanced only once authorization has accepted the set. A refused batch is
    // never handed to the transport, so a window that had already recorded it
    // would be reserving room for work nothing is holding — and that room is only
    // returned when the window closes, which is gated on flight the refused
    // members never entered.
    let mut proposed = *context.window;
    let members = match form_batch(head, &mut proposed) {
        BatchFormation::Fixed(members) => members,
        BatchFormation::NoRoom(head) => return Some(*head),
    };

    // Authorization is the linearization point and the watchdog's anchor, so it
    // happens once for the whole set, before any transport-specific branch, and
    // the clock starts with it. Starting the clock after the writes would exclude
    // the synchronous rendering and submission work the bound is supposed to
    // cover. One anchor for the batch, because the batch is what was authorized
    // at that instant.
    let authorized_at = Instant::now();
    // Authorization transitions a queue entry, so it covers exactly the members
    // that have one. Relay-originated work bypasses admission by design — a
    // terminal-outcome receipt above all — and holds no entry, so there is nothing
    // to transition and the absence of one is not a refusal.
    //
    // Filtering here rather than inside `authorize_batch` is forced: from inside
    // the ledger an absent entry is ambiguous, because the terminal transition
    // removes the entry it resolves, so "never admitted" and "already resolved by
    // someone else" look identical. `AsyncDeliveryTask::admitted` is what
    // distinguishes them, and it is only in hand here. `declare_singleton_unit`
    // skips the partition step against the same flag for the same reason; this is
    // the gate that did not inherit the rule, and a receipt refused for want of an
    // authorization it can never hold is the sender's only notice of non-delivery,
    // deleted.
    let authorized = {
        let member_ids: Vec<&str> = members
            .iter()
            .filter(|member| member.task.admitted)
            .map(|member| member.task.message_id.as_str())
            .collect();
        // A set holding no admitted member is relay-originated work alone. There
        // is nothing to authorize and nothing that could refuse it, so it proceeds
        // to submission. Stated as its own arm rather than left to
        // `authorize_batch`, which rejects an empty list — correctly, since an
        // empty *authorization* is a caller error.
        if member_ids.is_empty() {
            true
        } else {
            let batch = super::super::super::admission::authorize_batch(&member_ids);
            if let Some(batch) = batch {
                // Which members were authorized together is otherwise invisible, and
                // it is the antecedent of every per-member attribution downstream: a
                // reader who can see the partition but not the batch can tell which
                // members shared a submission without being able to tell which ones
                // the relay committed to at the same instant.
                emit_inscription(
                    "relay.delivery.batch.authorized",
                    &json!({
                        "batch_id": batch.value(),
                        "member_ids": member_ids,
                        "member_count": member_ids.len(),
                    }),
                );
            }
            batch.is_some()
        }
    };
    if !authorized {
        // Nothing transitioned, so nothing may be written: a write ahead of the
        // relay's own linearization point would put an effect at the target that
        // no authorization covers. Every member is provably unwritten, and
        // `complete_task_refusal` is what turns that into an outcome — it takes
        // the spelling from the guard's evidence order, which reads
        // `not_submitted` for a member never bound to a unit, and the reason from
        // here.
        for member in &members {
            super::super::super::async_worker::complete_task_refusal(
                &member.task,
                "delivery_batch_not_authorized",
                "the relay could not authorize this batch, so no member of it was submitted",
            );
            super::super::super::async_worker::release_pending_slot(context.pending);
        }
        return None;
    }
    *context.window = proposed;

    for member in members {
        submit_member(
            member,
            authorized_at,
            transport,
            inflight,
            inflight_members,
            context.pending,
        );
    }
    None
}

/// Fixes one batch's membership, and records it against the proposed window.
///
/// # Why the set is the member in hand and nothing behind it
///
/// A batch is bounded by what **one invocation of the delivery seam** can carry,
/// and `mailw` carries one envelope. Draining a prefix of the queue into a set
/// and then submitting it therefore produces N invocations, not one — and the
/// coder transports publish `Busy` on accepting the first, so members 2..N of
/// such a set meet a `!is_ready_for_handover` refusal and resolve `not_submitted`
/// instead of being held. Held is the correct answer for them: they are still
/// owed a delivery, and the relay may not re-read readiness *inside* a batch
/// without unfixing the membership it just fixed.
///
/// So the drain is not a batching decision the relay is free to make. It waits on
/// the seam becoming a batch — `mailw` taking the set the relay authorized, with
/// the transport partitioning it internally — at which point the transport is
/// `Busy` for the whole set rather than after its first member, and this function
/// grows the drain the window is already sized for.
///
/// Returns the fixed set, or the member to hold. A head that does not fit the room
/// a batch still in flight left is held rather than skipped, because the window is
/// a prefix of this target's FIFO and letting a smaller member behind it pass
/// would reorder the queue.
fn form_batch(head: IntakeTask, proposed: &mut HandoverWindow) -> BatchFormation {
    let head_bytes = canonical_payload_bytes(head.task.message.as_str());
    if !proposed.admits(head_bytes) {
        return BatchFormation::NoRoom(Box::new(head));
    }
    proposed.record(head_bytes);
    BatchFormation::Fixed(vec![head])
}

/// What [`form_batch`] settled on.
///
/// Not a `Result`: neither arm is a failure. A window with no room for the head
/// is the bound doing its job, and the member is owed a delivery exactly as it
/// was before.
enum BatchFormation {
    /// The batch's membership, fixed and never revised. Never empty.
    Fixed(Vec<IntakeTask>),
    /// The head did not fit the room a batch still in flight had left. Boxed so
    /// the common arm does not carry a task-sized hole through every submission.
    NoRoom(Box<IntakeTask>),
}

/// Resolves a member whose target is the forward-declared `Pubsub` stub.
///
/// Not deliverable, and never authorized: producing an explicit terminal outcome
/// is the alternative to calling an `unimplemented!` write.
fn reject_undeliverable(task: &AsyncDeliveryTask, pending: &std::sync::atomic::AtomicUsize) {
    super::super::super::async_worker::complete_task_outcome(
        task,
        Err(session_type_not_implemented(
            task.target_session.as_str(),
            SessionType::Pubsub,
        )),
    );
    super::super::super::async_worker::release_pending_slot(pending);
}

/// Submits one already-authorized member to its transport via the non-blocking
/// write seam and spawns an in-flight collector for its outcome.
///
/// The artifact written is the one the mailbox holds, built at intake and carried
/// here rather than rendered again: a second envelope built at this point would
/// put something on the wire that the relay's own record of the delivery does not
/// describe. What remains transport-specific is only which seam carries it —
/// `mailw` for an envelope, `raww` for raw input — and whether a successful write
/// clears startup failures.
///
/// Every path from here is terminal for the member. Its batch was authorized
/// before this ran, so there is no path that returns it to the queue's head and
/// none that leaves it unresolved.
fn submit_member(
    member: IntakeTask,
    authorized_at: Instant,
    transport: &mut TransportImpl,
    inflight: &mut JoinSet<InflightOutcome>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    let IntakeTask { task, payload } = member;
    let payload = match payload {
        Ok(payload) => payload,
        Err(error) => {
            // Refused before any target-side effect, so nothing reached the
            // target. Routed through the guard rather than reported as an explicit
            // error: an `Err` would spell this `failed`, the undifferentiated
            // outcome, for a member the evidence order can prove was never
            // submitted. The refusal's own code and message travel with it,
            // because the guard knows the member was not written but not that its
            // target member could not be resolved.
            super::super::super::async_worker::complete_task_refusal(
                &task,
                error.code.as_str(),
                error.message.as_str(),
            );
            super::super::super::async_worker::release_pending_slot(pending);
            return;
        }
    };
    // Declared immediately before the call that can produce a target-side effect,
    // and never earlier: the gap between authorization and this point is exactly
    // the window in which the guard can still prove nothing was written.
    //
    // Every coder submission marks handover too (`record_served`). Omitting it let
    // a coder member that panicked *after* its write resolve `not_submitted` — a
    // positive claim that nothing reached the target — when bytes may well have
    // landed. That is the exact inversion the evidence order exists to prevent.
    let (future, record_served) = match &payload {
        MailboxPayload::Mail(envelope) => {
            // Skipped for a transport that reports its own partition: binding here
            // would consume the member's one write-once binding, and the
            // transport's `declare` for the group it actually pastes would then be
            // refused. UI reports no partition of its own, so its single member is
            // declared here — which is the same call the branch it replaced made
            // unconditionally.
            if !transport.reports_own_partition() && declare_singleton_unit(&task).is_err() {
                // The relay refused to bind a unit, so no write was attempted and
                // this caller has no outcome to report: the member is either
                // already terminal — someone else reported it — or the ledger
                // could not be reached, in which case uniqueness cannot be
                // established and reporting is worse than staying silent.
                super::super::super::async_worker::release_pending_slot(pending);
                return;
            }
            (
                transport.mailw(DeliveryEnvelope::clone(envelope)),
                !matches!(transport, TransportImpl::Ui(_)),
            )
        }
        MailboxPayload::Raw {
            content,
            append_enter,
        } => {
            // Raw stays relay-declared, permanently. Neither transport can name
            // the member at its raw write — ACP routes `submit_raw_turn` through
            // `submit_envelope_turn` with a synthetic empty member id, and neither
            // `Transport::raww` nor Pty's `DeliveryCommand::Raw` carries a message
            // id — so the relay is the only layer that knows which member this
            // singleton unit covers.
            if declare_singleton_unit(&task).is_err() {
                super::super::super::async_worker::release_pending_slot(pending);
                return;
            }
            (transport.raww(content.clone(), *append_enter), true)
        }
    };

    let handle = inflight.spawn(async move { future.await.ok() });
    inflight_members.insert(
        handle.id(),
        InflightMember {
            task,
            record_served,
            authorized_at,
        },
    );
}

/// Declares the one-member packing unit for a member the relay submits alone.
///
/// Every relay-side submission is a singleton unit today. A transport that
/// coalesces gets its partition from [`PartitionSink`] instead, which is the
/// point of the sink — a unit the relay mints here could only ever name the one
/// member the relay handed over.
///
/// An un-admitted task declares nothing and reports success. A terminal-outcome
/// receipt is the only one in production: it bypasses admission, so it holds no
/// ledger entry, is bound to nothing, and is resolved by its own outcome rather
/// than by the guard. Declaring it would be refused for a member the ledger never
/// had — indistinguishable, from inside the ledger, from a member that already
/// terminalized — and the refusal would drop a receipt the relay had committed to
/// sending.
fn declare_singleton_unit(task: &AsyncDeliveryTask) -> Result<(), PartitionError> {
    if !task.admitted {
        return Ok(());
    }
    super::super::super::admission::declare_packing_unit(&[task.message_id.as_str()]).map(|_| ())
}
