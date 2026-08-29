//! Tmux delivery task — FIFO envelope grouping, batch-barrier raw, paste.
//!
//! Extracted from `transport/mod.rs` as a mechanical split — no behavior
//! change.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::sync::mpsc;

use crate::envelope::{PromptBatchSettings, batch_envelope_groups};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::transports::{
    DeliveryEnvelope, PackingUnitId, PartitionError, PartitionSink, SendOutcome,
    SingleDeliveryOutcome, SubmissionEvidence, stopped_before_submission_outcome,
};

use super::{DeliveryTaskContext, TMUX_TARGET_UNAVAILABLE_CODE, WriteItem};
use crate::tmux::pane::{inject_literal_text, resolve_active_pane_target};

/// Background delivery task: drains the write channel in FIFO order, groups
/// contiguous envelopes into flush groups, and pastes.
///
/// Items are processed as a FIFO stream: accumulate envelopes until a raw item
/// is encountered, flush the group, deliver the raw, then continue. This
/// preserves interleaving order and treats raw items as batch barriers.
pub(super) fn run_delivery_task(
    mut receiver: mpsc::Receiver<WriteItem>,
    ctx: DeliveryTaskContext,
    shutdown_flag: Arc<AtomicBool>,
    batch_settings: PromptBatchSettings,
    partition_sink: Arc<dyn PartitionSink>,
) {
    let tmux_socket_path = tmux_socket_path_for_runtime_directory(ctx.runtime_directory.as_path());

    let mut group: Vec<(
        DeliveryEnvelope,
        tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    )> = Vec::new();

    loop {
        if shutdown_flag.load(Ordering::Acquire) {
            drain_group_as_stopped(&mut group, &ctx.target_session);
            drain_remaining_as_stopped(&mut receiver, &ctx.target_session);
            return;
        }

        let item = match receiver.blocking_recv() {
            Some(item) => item,
            None => {
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

        match item {
            WriteItem::Envelope(env, sender) => {
                group.push((*env, sender));
            }
            WriteItem::Raw(content, append_enter, sender) => {
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

        loop {
            match receiver.try_recv() {
                Ok(WriteItem::Envelope(env, sender)) => {
                    group.push((*env, sender));
                }
                Ok(WriteItem::Raw(content, append_enter, sender)) => {
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
                    drain_remaining_as_stopped(&mut receiver, &ctx.target_session);
                    return;
                }
            }
        }

        if shutdown_flag.load(Ordering::Acquire) {
            drain_group_as_stopped(&mut group, &ctx.target_session);
            drain_remaining_as_stopped(&mut receiver, &ctx.target_session);
            return;
        }
    }
}

/// Drain all remaining items from the channel and resolve their senders as
/// stopped before submission, preserving each item's message_id.
fn drain_remaining_as_stopped(receiver: &mut mpsc::Receiver<WriteItem>, target_session: &str) {
    while let Ok(item) = receiver.try_recv() {
        let (sender, message_id) = match item {
            WriteItem::Envelope(env, sender) => (sender, env.message_id),
            WriteItem::Raw(_, _, sender) => (sender, String::new()),
        };
        let _ = sender.send(stopped_before_submission_outcome(
            target_session.to_string(),
            message_id,
        ));
    }
}

/// Drain a pending envelope group as stopped before submission, preserving
/// message_ids.
fn drain_group_as_stopped(
    group: &mut Vec<(
        DeliveryEnvelope,
        tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    target_session: &str,
) {
    for (envelope, sender) in group.drain(..) {
        let _ = sender.send(stopped_before_submission_outcome(
            target_session.to_string(),
            envelope.message_id,
        ));
    }
}

/// Paste an envelope group and resolve each sender with its own message id.
///
/// Handover readiness is observed before authorization; this function performs
/// only the target-side write and its immediate outcome resolution.
#[allow(clippy::too_many_arguments)]
fn flush_and_resolve(
    group: &mut Vec<(
        DeliveryEnvelope,
        tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    tmux_socket_path: &Path,
    target_session: &str,
    shutdown_flag: &AtomicBool,
    batch_settings: PromptBatchSettings,
    partition_sink: &dyn PartitionSink,
) {
    if shutdown_flag.load(Ordering::Acquire) {
        drain_group_as_stopped(group, target_session);
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
    group: &mut Vec<(
        DeliveryEnvelope,
        tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
    )>,
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
                    outcome: SendOutcome::NotSubmitted,
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
        .map(|(envelope, _)| super::render_paste_text(envelope))
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
        let prompt_members: Vec<(
            String,
            tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
            bool,
        )> = members
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

#[cfg(test)]
mod stopped_generation_tests {
    use super::*;
    use std::path::Path;

    use crate::envelope::AddressIdentity;
    use crate::transports::{PackingUnitId, PartitionError};

    fn held_envelope() -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: "held-message".to_string(),
            message: crate::transports::DeliveryMessage {
                body: "held body".to_string(),
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
            is_receipt: false,
        }
    }

    /// A group the generation still held resolves `not_submitted`, and never
    /// claims the relay shut down.
    ///
    /// The negative half is the regression: one flag served both the fence and
    /// shutdown, so a generation fenced by the execution watchdog — with the
    /// relay running perfectly well — told every sender behind it that the relay
    /// had shut down. Both halves are asserted because the outcome and the
    /// reason code reach a sender as separate fields, and pinning only the
    /// outcome would let the false explanation survive beside a corrected
    /// verdict.
    #[test]
    fn a_group_held_when_the_generation_stops_resolves_not_submitted() {
        // Nothing is declared on this path, so a sink that refuses is the
        // faithful stand-in: reaching it at all would already be the defect.
        struct NoDeclarations;
        impl PartitionSink for NoDeclarations {
            fn declare(&self, _member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
                Err(PartitionError::MemberNotBindable)
            }
            fn record(&self, _unit: PackingUnitId, _evidence: SubmissionEvidence) {}
        }

        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut group = vec![(held_envelope(), sender)];
        // Already stopped when the flush is reached: the fence's cooperative
        // step and shutdown both leave the flag in exactly this state.
        let stop_flag = AtomicBool::new(true);

        flush_and_resolve(
            &mut group,
            Path::new("/nonexistent/tmux.sock"),
            "target@test-ns",
            &stop_flag,
            PromptBatchSettings::default(),
            &NoDeclarations,
        );

        assert!(group.is_empty(), "the held group is resolved, not retained");
        let outcome = receiver
            .blocking_recv()
            .expect("a stopped generation resolves what it was holding");
        assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some("generation_fenced"),
            "the stop check runs before any pane resolution or paste"
        );

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

        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut group = vec![(held_envelope(), sender)];
        flush_and_resolve(
            &mut group,
            Path::new("/nonexistent/tmux.sock"),
            "target@test-ns",
            &stop_flag,
            PromptBatchSettings::default(),
            &NoDeclarations,
        );
        let outcome = receiver
            .blocking_recv()
            .expect("a shutdown generation resolves what it was holding");
        assert_eq!(
            outcome.outcome,
            SendOutcome::NotSubmitted,
            "shutdown is a trigger; it does not choose the outcome"
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some("relay_shutdown"),
            "the write names the cause it was actually stopped by"
        );
    }
}
