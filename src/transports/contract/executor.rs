//! The delivery-loop executor every transport runs, and the two seams it sits
//! between.
//!
//! Under the pull model the relay never invokes a transport to deliver. Each
//! transport instance owns exactly one serial executor for its lifetime, spawned
//! during [`startup`](super::Transport::startup), which asks the relay what is
//! waiting and reports back what it did with it:
//!
//! ```text
//! peek -> decide/render/measure -> declare the decided prefix -> write -> ack
//! ```
//!
//! [`run_delivery_executor`] is that loop, written once. What differs per
//! transport is entirely inside [`DeliveryWriter`]: how a target is observed for
//! readiness, how entries are rendered, how much of a peeked run one write may
//! carry, and what the write itself is. What does *not* differ is the order of
//! the five steps, the rule that a declaration precedes any target-side effect,
//! and the rule that a declared unit is always acknowledged — which is why they
//! are here rather than repeated in four modules that would have to agree.
//!
//! # The two seams
//!
//! [`MailboxConsumer`] is the relay handle, injected the way [`Chooser`] and the
//! readiness notifier already are: the relay builds it closing over its own
//! ledger, and the transport holds an opaque `Arc<dyn MailboxConsumer>` so
//! `src/transports` and the concrete transports never import `crate::relay`.
//!
//! **The handle closes over the target and the consumer generation rather than
//! taking them per call.** The requirement names the operations
//! `declare(target, generation_id, through_seq)`, and the check that a call
//! belongs to the target's active generation is exactly as it specifies — the
//! relay still compares the binding under its own lock. What changes is only
//! where the binding is held: a transport that cannot name a target cannot name
//! the wrong one, and a generation identifier that never reaches the transport
//! cannot be retained past its replacement.
//!
//! [`Chooser`]: super::Chooser

use std::sync::Arc;
use std::time::Duration;

use crate::protocol::mailbox::{EntryRange, EntrySequence, MailboxEntry};
use crate::protocol::operations::{AckResult, DeclareResult, MemberAcknowledgment, PeekResult};
use crate::protocol::{DeliveryDoorbell, PackingUnitId, SubmissionEvidence};

use super::{PeekDimensions, TransportHealth};

/// The relay's mailbox, as a transport's delivery-loop executor sees it.
///
/// Every method is bound to one target and one consumer generation, fixed when
/// the relay built the handle. A call whose generation no longer holds the target
/// is refused rather than applied, which is what makes a superseded executor's
/// late call harmless.
pub trait MailboxConsumer: Send + Sync {
    /// Reads the head run of this target's mailbox, advancing nothing.
    ///
    /// Repeatable: two calls with no acknowledgment between them report the same
    /// run, so an executor that peeks and then decides not to write has changed
    /// nothing.
    fn peek(&self, entry_max: usize, canonical_bytes_max: u64) -> PeekResult;

    /// Records, before any target-side effect, the exact run about to be written
    /// as one packing unit.
    fn declare(&self, range: EntryRange) -> DeclareResult;

    /// Terminalizes the entries a declaration bound, from what the write
    /// observed for each of them.
    fn ack(&self, unit: PackingUnitId, members: &[MemberAcknowledgment]) -> AckResult;

    /// Resolves this target's queued-and-undeclared entries because its
    /// transport has been continuously unreachable past the relay's dwell.
    ///
    /// The relay owns the transition and the outcome: an undeclared entry was
    /// never bound to a guard, so it resolves `not_submitted` directly, and a
    /// declared one is left to the guard's evidence order. The executor's part is
    /// only to say that the condition it alone can observe has now held long
    /// enough, which is why this reports nothing back.
    fn resolve_unreachable(&self);
}

/// What one transport contributes to the shared loop.
///
/// Ordered as the loop calls them. Everything here is transport-internal by
/// contract: no method is relay-facing, and the relay reads no readiness level
/// from any of them.
pub trait DeliveryWriter {
    /// Whatever this transport rendered while planning, carried from
    /// [`plan`](Self::plan) to [`write`](Self::write) so the decision is not made
    /// twice.
    ///
    /// An associated type rather than an opaque blob because rendering is the
    /// part that genuinely differs: tmux carries token-budgeted prompt text, ACP
    /// a combined turn, Pty a byte string, UI a stream event.
    type Plan;

    /// The most one `peek` may return, in the two units the relay can evaluate
    /// without rendering.
    ///
    /// Static per transport. A token budget is deliberately not among them: only
    /// this transport can render its entries and count their tokens, so a peek
    /// bound expressed in them would ask the relay to know something it does not.
    /// The token budget is applied in [`plan`](Self::plan) instead, against what
    /// the peek actually returned.
    fn peek_dimensions(&self) -> PeekDimensions;

    /// Whether this transport can reach its target at all.
    ///
    /// The axis the relay's dwell is measured over. Distinct from readiness: a
    /// busy target and a departed one both fail a readiness check, and only the
    /// first is a reason to keep waiting.
    fn health(&self) -> TransportHealth;

    /// A monotonic marker that advances when bytes reach the target's terminal.
    ///
    /// The default is the specified fallback rather than a guess: `0` can never
    /// advance, so a transport with no such primitive is simply never deferred on
    /// this basis.
    fn activity_generation(&self) -> u64 {
        0
    }

    /// Whether this transport's own readiness allows writing right now.
    ///
    /// Where a prompt-readiness template is evaluated, where an ACP turn's
    /// completion is observed, and where a UI answers unconditionally. Takes
    /// `&mut self` because observing a target is not always a pure read — a pane
    /// capture and a snapshot handshake both happen here.
    fn is_ready(&mut self) -> bool;

    /// Decides how much of the peeked run to write as one unit, rendering it.
    ///
    /// `None` leaves every peeked entry `queued` and undeclared for the next
    /// attempt, which the relay does not treat as a refusal. `entries` is never
    /// empty and is in mailbox order; a plan may cover any non-empty prefix of it.
    fn plan(&mut self, entries: &[MailboxEntry]) -> Option<PlannedWrite<Self::Plan>>;

    /// Writes the unit the relay has now recorded, reporting what the write
    /// observed for each member.
    ///
    /// One evidence per member, in the order the plan covered them. This is the
    /// only point at which a target-side effect may be produced: the declaration
    /// is already recorded, so a member reported `NotSubmitted` here is a claim
    /// the relay will pass to its sender as positive evidence of non-delivery.
    fn write(&mut self, planned: PlannedWrite<Self::Plan>) -> Vec<SubmissionEvidence>;

    /// Whether this generation has been asked to stop.
    ///
    /// The fence's cooperative step, read at every point the loop can act on it.
    /// Returning `true` ends the executor, and the executor's thread finishing is
    /// what the fence's cessation observation reads.
    ///
    /// Takes `&mut self` because a stop signal is not always a flag: ACP's is a
    /// dropped channel, and observing one consumes from the receiver.
    fn stop_requested(&mut self) -> bool;

    /// Waits for the doorbell or for `timeout`, whichever comes first.
    ///
    /// Defaulted, because for three of the four transports there is nothing to
    /// do while idle and blocking on the doorbell is exactly right. **Pty is the
    /// exception, and is the reason this is a method rather than a line in the
    /// loop.** Its terminal is `!Send`, so the terminal lives on this thread —
    /// which makes this thread also the one that feeds the terminal its child's
    /// output and answers the snapshot requests `look` and the prompt probe
    /// make. A plain block would stall both for the length of every wait.
    ///
    /// Whatever an override does, it must return: the loop's stop check runs
    /// after this, and an implementation that waited forever would put the
    /// executor beyond the fence's cooperative step.
    fn wait_for_work(&mut self, doorbell: &DeliveryDoorbell, timeout: Duration) {
        doorbell.wait_for(timeout);
    }
}

/// One transport's decision about what to write next.
pub struct PlannedWrite<P> {
    /// How many entries from the head of the peeked run this unit covers.
    ///
    /// Never zero: a plan that covers nothing is spelled `None` from
    /// [`plan`](DeliveryWriter::plan), so that "decided not to write" and
    /// "decided to write these" are different answers rather than one answer with
    /// a degenerate case.
    pub entry_count: usize,
    /// What this transport rendered while deciding.
    pub rendered: P,
}

/// What the loop needs beyond the transport and the mailbox.
///
/// Injected once at construction, the way the chooser, the readiness notifier
/// and the partition sink before it already are: the relay builds it closing
/// over its own ledger and its own `[delivery]` policy, and the transport holds
/// it without naming a `crate::relay` type.
///
/// `Clone` because a transport whose runtime is re-established in place — ACP
/// respawns its child without the relay electing a new consumer generation —
/// spawns a fresh executor against the same mailbox and the same entitlement.
/// Cloning yields another handle on the same doorbell and the same consumer, not
/// a second binding: only the relay issues those.
#[derive(Clone)]
pub struct DeliveryExecutorContext {
    /// The relay's mailbox for this executor's target and generation.
    pub consumer: Arc<dyn MailboxConsumer>,
    /// Rung when a peek that would have come back empty would now come back with
    /// something. Correctness never depends on a ring arriving.
    pub doorbell: DeliveryDoorbell,
    /// The bounded backstop the doorbell is paired with. A missed ring costs at
    /// most this long.
    pub poll_interval: Duration,
    /// How long a target may be *continuously* unreachable before its
    /// queued-and-undeclared entries resolve. Relay `[delivery]` policy, passed
    /// down rather than read here, because the threshold is the relay's and the
    /// observation is the transport's.
    pub unreachable_dwell: Duration,
}

impl std::fmt::Debug for DeliveryExecutorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeliveryExecutorContext")
            .field("poll_interval", &self.poll_interval)
            .field("unreachable_dwell", &self.unreachable_dwell)
            .finish_non_exhaustive()
    }
}

/// Runs one transport's delivery loop until its generation is asked to stop.
///
/// Serial by construction: there is one of these per transport instance, and it
/// writes one declared unit at a time, so the ordering guarantees the mailbox
/// provides — per-target FIFO with raw as a barrier — reach the target unaltered
/// without any further coordination.
pub fn run_delivery_executor<W: DeliveryWriter>(mut writer: W, context: DeliveryExecutorContext) {
    let mut last_activity: Option<u64> = None;
    loop {
        if writer.stop_requested() {
            return;
        }
        if writable(&mut writer, &context, &mut last_activity) {
            drain_target(&mut writer, &context);
        }
        if writer.stop_requested() {
            return;
        }
        // Whichever comes first. The executor peeks either way on the next
        // iteration, so a lost ring costs the poll interval and nothing else.
        writer.wait_for_work(&context.doorbell, context.poll_interval);
    }
}

/// Whether this iteration should try to write at all, and the one place the
/// three observations are ranked against each other.
///
/// The precedence is the push model's, moved rather than rebuilt. Health first,
/// because it is what bounds the wait and an unreachable target is not written to
/// whatever readiness says. Then the activity marker, which is allowed to
/// override a matching readiness check: a pane is captured as text, so a template
/// can match transiently on a frame that happens to end in a prompt while output
/// is still flowing, and an advance is positive evidence the target is doing
/// something where the template's match is an inference from a rendering.
///
/// Activity is ordered *after* health deliberately. An advance says the target is
/// active, which is not a reason to keep holding entries behind a transport that
/// has been unreachable past its dwell — those are owed an outcome, and
/// suppression may only ever withhold a write.
fn writable<W: DeliveryWriter>(
    writer: &mut W,
    context: &DeliveryExecutorContext,
    last_activity: &mut Option<u64>,
) -> bool {
    // Read on every iteration, including the ones that return early below. An
    // unobservable target reports no marker, and the comparison treats that as
    // absence rather than as a value, so the series survives an unreachable
    // stretch instead of being reset by it.
    let advanced = activity_advanced(writer.activity_generation(), last_activity);
    match writer.health() {
        TransportHealth::Unreachable { since } if since.elapsed() >= context.unreachable_dwell => {
            // Waiting will not make this target reachable, so its queued entries
            // are owed an answer rather than a place in a queue nothing will
            // drain. Idempotent: once the relay has resolved them there is
            // nothing left for a repeat call to find, which is what lets this be
            // driven by a continuously-observed condition rather than by an edge
            // the executor would have to detect.
            context.consumer.resolve_unreachable();
            false
        }
        // Unreachable but not yet past the dwell: hold, exactly as an unready
        // target is held. An unreachability that ends in time costs nothing.
        TransportHealth::Unreachable { .. } => false,
        TransportHealth::Healthy => !advanced && writer.is_ready(),
    }
}

/// Whether the target's activity marker advanced since the previous observation,
/// recording the new value either way.
///
/// The comparison lives in the loop rather than in the transport because only the
/// loop knows which two observations bracket a write decision — a transport
/// publishes a marker and has no notion of the decision that reads it.
///
/// The first observation records without suppressing. A marker seeded from an
/// epoch would otherwise read as an advance against a zero it was never compared
/// to, costing one poll interval on the first delivery to every target for no
/// evidence at all.
fn activity_advanced(current: u64, last: &mut Option<u64>) -> bool {
    // `0` is absence, and absence carries no meaning: either the transport tracks
    // no marker at all, or this observation could not read one because the target
    // was unobservable. Neither is evidence, so it must not suppress a write *and*
    // must not enter the series — folding it in would make the next real reading
    // an advance against zero, so a target would be held for a tick on recovering
    // from any unreachable observation, on no evidence that it had written
    // anything.
    if current == 0 {
        return false;
    }
    let advanced = last.is_some_and(|previous| current > previous);
    *last = Some(current);
    advanced
}

/// Writes declared units back to back for as long as the target has entries and
/// remains willing to take them.
///
/// Readiness is re-read between units rather than once for the whole drain,
/// because the coder transports publish `Busy` on accepting a write: a drain that
/// asked once would decide the second unit's fate on an observation the target
/// had already invalidated. That is the defect this whole redesign exists to
/// close, arrived at from inside the executor.
fn drain_target<W: DeliveryWriter>(writer: &mut W, context: &DeliveryExecutorContext) {
    let dimensions = writer.peek_dimensions();
    loop {
        let Ok(peeked) = context
            .consumer
            .peek(dimensions.envelopes_max, dimensions.canonical_bytes_max)
        else {
            // The generation no longer holds the target, or the target is gone.
            // Either way this executor has nothing further to do for it, and the
            // relay resolves whatever it left behind.
            return;
        };
        if peeked.entries.is_empty() {
            return;
        }
        let Some(planned) = writer.plan(&peeked.entries) else {
            // Decided not to write. The peeked entries stay `queued` and
            // undeclared, exactly as if they had never been read.
            return;
        };
        let Some(covered) = peeked.entries.get(..planned.entry_count) else {
            // A plan reaching past what it was shown is a defect in that
            // transport, and writing a prefix of it would submit a unit the
            // transport did not decide on. Leaving the entries queued costs a
            // poll interval and reaches the same target once the plan is correct.
            return;
        };
        let Some((first, last)) = covered.first().zip(covered.last()) else {
            return;
        };
        let Some(range) = EntryRange::new(first.sequence, last.sequence) else {
            return;
        };
        let Ok(accepted) = context.consumer.declare(range) else {
            // Nothing was bound, so nothing may be written: a write ahead of the
            // relay's own record would put an effect at the target that no
            // declaration covers.
            return;
        };
        let sequences: Vec<_> = covered.iter().map(|entry| entry.sequence).collect();
        let evidence = writer.write(planned);
        let members = acknowledgment(&sequences, &evidence);
        // Unconditional, and the whole point of splitting `write` from `plan`: a
        // declared unit whose write failed observably is acknowledged with what
        // that failure proved rather than left to the execution watchdog. The
        // watchdog remains the backstop for an executor that cannot report at all
        // — one that has panicked, and is therefore not here.
        let _ = context.consumer.ack(accepted.unit, &members);
        if writer.stop_requested() {
            return;
        }
        if !writer.is_ready() {
            return;
        }
    }
}

/// Splits a peeked mail run into runs that may share one write, returning each
/// run's length in order. A terminal-outcome receipt forms a run of its own.
///
/// Shared rather than per-transport because it is not a packing *decision* — how
/// much of a run fits one write stays entirely with the transport that renders
/// it — but a correctness barrier two transports reached independently and state
/// identically. A receipt is a relay-issued notice about a delivery that failed,
/// and a write carrying it beside a peer's message renders as one message to the
/// agent reading it: the marker line that distinguishes a receipt ends up in the
/// middle of somebody else's text. Tmux and ACP both split here; Pty writes one
/// member per primitive and so needs no split at all.
#[must_use]
pub fn receipt_runs(is_receipt: &[bool]) -> Vec<usize> {
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

/// Pairs each declared position with what the write observed for it.
///
/// A transport that reports a different number of results than the unit covers
/// has a defect, and the honest reading of its report is that the relay does not
/// know what happened to the members it cannot account for. Filling those with
/// `SubmissionUnknown` keeps the acknowledgment well-formed — the relay refuses
/// one that does not cover its unit exactly — so the unit still resolves rather
/// than being left outstanding for the watchdog. It is never `NotSubmitted`,
/// which would be a positive claim about a write this loop has no report of.
fn acknowledgment(
    sequences: &[EntrySequence],
    evidence: &[SubmissionEvidence],
) -> Vec<MemberAcknowledgment> {
    sequences
        .iter()
        .enumerate()
        .map(|(index, sequence)| MemberAcknowledgment {
            sequence: *sequence,
            evidence: evidence
                .get(index)
                .copied()
                .unwrap_or(SubmissionEvidence::SubmissionUnknown),
        })
        .collect()
}
