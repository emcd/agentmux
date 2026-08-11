//! Pty worker delivery and write-result handling.
//!
//! Readiness is checked before authorization. Once a command reaches this
//! worker, the PTY master write is the submission boundary and its outcome is
//! resolved immediately; no prompt, prime, or quiescence wait follows it.
//!
//! A batch is partitioned into per-member packing units before the first write
//! (`[Delivery::start_envelope_group]` collects the group, then writes). Each
//! unit's full byte sequence is buffered and written as one primitive, and each
//! member resolves from its own unit's write result — outcomes are per unit,
//! never applied to the whole batch. A failure in one member's write does not
//! change what another member's write already proved.

use std::{io::Write, sync::Arc};

use tokio::sync::{mpsc, oneshot};

use crate::transports::{
    DeliveryEnvelope, PackingUnitId, PartitionSink, SendOutcome, SingleDeliveryOutcome,
    SubmissionEvidence,
};

use super::transport::DeliveryCommand;

const RECEIPT_MARKER: &str = "--- agentmux terminal-outcome receipt ---";

#[derive(Debug)]
pub struct PendingRaw {
    pub content: String,
    pub append_enter: bool,
    pub outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
}

/// Marker for "the raw write failed and the failure outcome was already
/// sent" — used by [`Delivery::start_raw`] to signal failure without
/// returning a unit `Result`.
#[derive(Debug)]
pub struct RawWriteFailed;

/// One in-flight envelope-group delivery on the worker thread.
///
/// The group's membership is fixed at partition time, before the first write;
/// each member is its own packing unit and carries the outcome its own unit's
/// write recorded. Resolution sends each member its own outcome, and a raw
/// barrier captured during partition is handed back for delivery after the
/// group.
pub struct DeliveryRun {
    members: Vec<MemberDelivery>,
    /// A raw barrier captured during partition, delivered after this group.
    pending_raw: Option<PendingRaw>,
    resolved: bool,
}

/// One partitioned member: its outcome sender and the outcome its own unit's
/// write recorded at partition time.
struct MemberDelivery {
    outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
    outcome: SingleDeliveryOutcome,
}

/// Outcome of one [`Delivery::step`] call. The worker loop uses this to drive
/// its event loop.
#[derive(Debug)]
pub enum DeliveryStep {
    /// The delivery has resolved. `pending_raw` is a raw barrier captured
    /// during this group's partition, to be delivered after the group.
    Done { pending_raw: Option<PendingRaw> },
}

/// One in-flight raw-only delivery. The raw write has already reached the
/// master when this value enters the worker step loop.
pub struct RawDelivery {
    raw: Option<PendingRaw>,
    resolved: bool,
}

/// One in-flight delivery on the worker thread. Either an envelope group or a
/// raw-only command. The worker loop holds an `Option<Delivery>` and calls
/// `step` per outer iteration.
pub enum Delivery {
    Group(DeliveryRun),
    Raw(RawDelivery),
}

impl Delivery {
    /// Start a new envelope-group delivery.
    ///
    /// Partition first: the batch's membership is fixed by draining contiguous
    /// `Envelope` commands from `write_rx`, and a `Raw` command during the scan
    /// acts as a batch barrier that ends the partition and is delivered after
    /// the group. Only then, after the partition, is each unit written: one
    /// unit per member, buffered and written as a single primitive, with each
    /// member's outcome recorded from its own unit's write result.
    #[allow(clippy::too_many_arguments)]
    pub fn start_envelope_group(
        first_envelope: Box<DeliveryEnvelope>,
        first_outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
        write_rx: &mut mpsc::Receiver<DeliveryCommand>,
        writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
        target_session: &str,
        partition_sink: &dyn PartitionSink,
    ) -> Delivery {
        // Partition: fix the batch's membership before the first write. A raw
        // item encountered during the scan ends the partition.
        let mut group = vec![(first_envelope, first_outcome_tx)];
        let mut pending_raw = None;
        loop {
            match write_rx.try_recv() {
                Ok(DeliveryCommand::Envelope {
                    envelope,
                    outcome_tx,
                }) => {
                    group.push((envelope, outcome_tx));
                }
                Ok(DeliveryCommand::Raw {
                    content,
                    append_enter,
                    outcome_tx,
                }) => {
                    pending_raw = Some(PendingRaw {
                        content,
                        append_enter,
                        outcome_tx,
                    });
                    break;
                }
                Err(_) => break,
            }
        }

        // Write after the partition: one unit per member, buffered then written.
        // Each member's outcome derives from its own unit's write result.
        //
        // One member per unit is a commitment here rather than a current
        // limitation. Coalescing several members into one write would smear a
        // partial write's evidence across all of them: the bytes of an earlier
        // member could have landed while a later member's did not, and a shared
        // unit could only report one answer for both.
        //
        // Each member is therefore declared immediately before its own write
        // rather than all of them being declared before the first. With no unit
        // spanning members there is no shared fate to establish up front, and a
        // refusal for one member must not hold back a groupmate whose write is
        // independent of it.
        let members = group
            .into_iter()
            .filter_map(|(envelope, outcome_tx)| {
                // A terminal-outcome receipt bypassed admission and holds no
                // ledger entry, so it belongs to no unit. Declaring one would be
                // refused — the ledger cannot tell a member it never had from one
                // that already terminalized — and the refusal would drop a
                // receipt the relay committed to sending.
                if envelope.is_receipt {
                    let outcome = write_unit(writer, &envelope, target_session, None);
                    return Some(MemberDelivery {
                        outcome_tx,
                        outcome,
                    });
                }
                let unit = match partition_sink.declare(&[envelope.message_id.as_str()]) {
                    Ok(unit) => unit,
                    Err(_) => {
                        // Refused: this member gets no write. Dropping its sender
                        // unresolved hands it back to the guard, which derives
                        // `not_submitted` from its being unbound — true, because
                        // nothing was written for it.
                        drop(outcome_tx);
                        return None;
                    }
                };
                let outcome = write_unit(
                    writer,
                    &envelope,
                    target_session,
                    Some((partition_sink, unit)),
                );
                Some(MemberDelivery {
                    outcome_tx,
                    outcome,
                })
            })
            .collect();

        Delivery::Group(DeliveryRun {
            members,
            pending_raw,
            resolved: false,
        })
    }

    /// Start a raw-only delivery. Writes the raw content to the PTY master and
    /// constructs a `RawDelivery` ready for the worker's step loop. On write
    /// failure, returns `Err(RawWriteFailed)` (the failure outcome has already
    /// been sent to the raw's `outcome_tx`).
    #[allow(clippy::too_many_arguments)]
    pub fn start_raw(
        content: String,
        append_enter: bool,
        outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
        writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
        target_session: &str,
    ) -> Result<Delivery, RawWriteFailed> {
        let result = (|| -> std::io::Result<()> {
            let mut guard = writer
                .lock()
                .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
            guard.write_all(content.as_bytes())?;
            if append_enter {
                guard.write_all(b"\n")?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = outcome_tx.send(failed_outcome(
                target_session,
                "pty_write_failed",
                &error.to_string(),
            ));
            return Err(RawWriteFailed);
        }
        Ok(Delivery::Raw(RawDelivery {
            raw: Some(PendingRaw {
                content,
                append_enter,
                outcome_tx,
            }),
            resolved: false,
        }))
    }

    /// Drive the delivery one step. The worker re-services channels between
    /// calls. With the writes synchronous at partition time, resolution only
    /// flushes each member's already-recorded outcome.
    pub fn step(&mut self, target_session: &str) -> DeliveryStep {
        match self {
            Delivery::Group(run) => run.resolve(target_session),
            Delivery::Raw(raw) => raw.resolve(target_session),
        }
    }

    /// Abandon an in-flight delivery. For a group, each member keeps the
    /// outcome its own unit's write already recorded — submission success
    /// terminalizes `delivered`, and a later child exit is target-health
    /// observability, not a second delivery outcome. Only the raw barrier
    /// (never written) takes the supplied failure. For a raw-only delivery, the
    /// raw is resolved with the supplied failure.
    pub fn abandon_into_failed(&mut self, target_session: &str, reason_code: &str, reason: &str) {
        match self {
            Delivery::Group(run) => {
                for member in run.members.drain(..) {
                    let _ = member.outcome_tx.send(member.outcome);
                }
                if let Some(raw) = run.pending_raw.take() {
                    let _ =
                        raw.outcome_tx
                            .send(failed_outcome(target_session, reason_code, reason));
                }
                run.resolved = true;
            }
            Delivery::Raw(raw) => {
                if let Some(item) = raw.raw.take() {
                    let _ =
                        item.outcome_tx
                            .send(failed_outcome(target_session, reason_code, reason));
                }
                raw.resolved = true;
            }
        }
    }
}

impl DeliveryRun {
    fn resolve(&mut self, _target_session: &str) -> DeliveryStep {
        self.resolved = true;
        let pending_raw = self.pending_raw.take();
        for member in self.members.drain(..) {
            let _ = member.outcome_tx.send(member.outcome);
        }
        DeliveryStep::Done { pending_raw }
    }
}

impl RawDelivery {
    fn resolve(&mut self, target_session: &str) -> DeliveryStep {
        self.resolved = true;
        if let Some(raw) = self.raw.take() {
            let _ = raw.outcome_tx.send(delivered_outcome(target_session, ""));
        }
        DeliveryStep::Done { pending_raw: None }
    }
}

/// Buffers one member's complete rendered bytes and writes them to the master
/// as a single unit. The write result is that member's own submission evidence.
fn write_unit(
    writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    envelope: &DeliveryEnvelope,
    target_session: &str,
    // `None` for a member no unit covers: a terminal-outcome receipt, which
    // bypassed admission and is resolved by its own outcome rather than the guard.
    unit: Option<(&dyn PartitionSink, PackingUnitId)>,
) -> SingleDeliveryOutcome {
    let text = envelope.message.render_pane_envelope(&envelope.message_id);
    let mut buffer = Vec::with_capacity(text.len() + RECEIPT_MARKER.len() + 2);
    if envelope.is_receipt {
        buffer.extend_from_slice(RECEIPT_MARKER.as_bytes());
        buffer.push(b'\n');
    }
    buffer.extend_from_slice(text.as_bytes());
    buffer.push(b'\n');

    let result = (|| -> std::io::Result<()> {
        let mut guard = writer
            .lock()
            .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
        guard.write_all(&buffer)
    })();
    // Recorded after the writer guard drops and before this member's outcome is
    // returned, so a step loop that never resolves the member still leaves the
    // guard something to resolve it from.
    //
    // A failed `write_all` records `SubmissionUnknown`, not `NotSubmitted`: it
    // reports how many bytes it wrote only on success, so a failure cannot
    // exclude a partial write reaching the master. The returned outcome keeps its
    // existing `failed` spelling — changing that is sender-visible and belongs to
    // the refusal-spelling task, not here.
    match result {
        Ok(()) => {
            if let Some((sink, unit)) = unit {
                sink.record(unit, SubmissionEvidence::Submitted);
            }
            delivered_outcome(target_session, &envelope.message_id)
        }
        Err(error) => {
            if let Some((sink, unit)) = unit {
                sink.record(unit, SubmissionEvidence::SubmissionUnknown);
            }
            let mut outcome =
                failed_outcome(target_session, "pty_write_failed", &error.to_string());
            outcome.message_id = envelope.message_id.clone();
            outcome
        }
    }
}

fn delivered_outcome(target_session: &str, message_id: &str) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: target_session.to_string(),
        message_id: message_id.to_string(),
        outcome: SendOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

fn failed_outcome(target_session: &str, reason_code: &str, reason: &str) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: target_session.to_string(),
        message_id: String::new(),
        outcome: SendOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.to_string()),
        details: None,
    }
}
