//! Terminal resolution for one delivery member: the single transition, and the
//! evidence order that decides what it reports.
//!
//! Every path that can finish a member routes through the `complete_task_*`
//! family here. The transition is a CAS, so exactly one caller wins and the
//! losers stay silent — that is what keeps two competing resolvers from each
//! emitting an outcome for one accepted member.
//!
//! The division of labour that the rest of this module exists to protect: a
//! *trigger* says when the owning path gave up and contributes only the reason,
//! while the guard's evidence order contributes the outcome. Publishing the
//! result is [`super::reporting`]'s job, not this module's.

use crate::relay::delivery::admission::{ResolvedMember, TerminalTransition};
use crate::relay::delivery::guard::{GuardKey, GuardTrigger, SubmissionEvidence};
use crate::relay::{AsyncDeliveryTask, RelayError, SendOutcome, SendResult};

use super::reporting::report_terminal_outcome;

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";

pub(in crate::relay::delivery) fn complete_task_on_shutdown(task: &AsyncDeliveryTask) {
    complete_task_outcome(
        task,
        Ok(SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: SendOutcome::DroppedOnShutdown,
            reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
            reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
            details: None,
        }),
    );
}

/// Attempts the terminal transition and decides whether this caller may report.
///
/// `None` means stay silent: either another caller already won the transition,
/// or no reservation exists for a task that *should* have had one — which means
/// the winner already terminalized it and cleaned the ledger entry up.
///
/// The winning transition removes the ledger entry, so absence cannot by itself
/// distinguish "already resolved by someone else" from "never admitted". Only
/// the task can: a relay-originated receipt is the one thing that legitimately
/// bypasses admission. Reporting on absence alone is what let two competing
/// resolvers each emit an outcome for a single accepted member.
///
/// `Some((None, _))` is the receipt case — reportable, with no recorded evidence
/// because nothing was ever admitted to record any against.
fn resolve_terminal_transition(task: &AsyncDeliveryTask) -> Option<TerminalResolution> {
    match crate::relay::delivery::admission::terminalize(task.message_id.as_str()) {
        TerminalTransition::Won {
            evidence,
            bound,
            guard,
        } => Some(TerminalResolution {
            evidence: Some(evidence),
            bound,
            guard,
        }),
        TerminalTransition::AlreadyTerminal => None,
        // Never admitted, so nothing else can be racing to resolve it.
        TerminalTransition::NoReservation if !task.admitted => Some(TerminalResolution {
            evidence: None,
            bound: false,
            guard: None,
        }),
        // Admitted, but its reservation is gone: the winner already terminalized
        // it and cleaned up. Reporting here would be the duplicate.
        TerminalTransition::NoReservation => None,
    }
}

/// What the terminal transition established about one member, for the callers
/// that turn it into a reported outcome.
struct TerminalResolution {
    /// `None` for a member that was never admitted — a terminal-outcome receipt,
    /// which holds no admission reservation and so no evidence recorded against
    /// it.
    evidence: Option<SubmissionEvidence>,
    /// Whether the member was bound to a packing unit, which is what decides
    /// whether its evidence may override a producer's own outcome.
    bound: bool,
    guard: Option<GuardKey>,
}

/// Terminalizes one member because a lifecycle trigger fired, with no outcome of
/// its own to report.
///
/// The trigger does not choose the outcome — the guard's evidence order does.
/// That separation is the point: a shutdown reaching a member no declaration had
/// bound resolves `not_submitted`, because the relay can prove nothing was
/// written for it, while the same shutdown reaching one already bound to a
/// packing unit resolves `submission_unknown`.
pub(in crate::relay::delivery) fn complete_task_outcome_from_trigger(
    task: &AsyncDeliveryTask,
    trigger: GuardTrigger,
) {
    // Boundness is not consulted here: a trigger brings no outcome of its own, so
    // the guard is already the only source and there is nothing for the evidence
    // to have authority *over*.
    let Some(TerminalResolution {
        evidence,
        bound: _,
        guard,
    }) = resolve_terminal_transition(task)
    else {
        return;
    };
    // A receipt carries no recorded evidence, so a trigger on one can only be
    // reported honestly as unknown.
    let evidence = evidence.unwrap_or(SubmissionEvidence::SubmissionUnknown);
    report_terminal_outcome(
        task,
        Ok(SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: evidence.outcome(),
            reason_code: Some(evidence.reason_code().to_string()),
            reason: Some(trigger.reason().to_string()),
            details: None,
        }),
        guard,
    );
}

/// Terminalizes one member refused on the delivery path *before* its packing
/// unit was declared, deriving the outcome from the guard and carrying the
/// refusal's own cause.
///
/// A refusal is not a lifecycle trigger — nothing gave up on the member, the
/// relay declined to write it — but it shares the property that makes triggers
/// safe: it must not choose its own spelling. Passing an explicit `Err` here
/// reported `failed`, the undifferentiated spelling, for a member the evidence
/// order can prove was never submitted. `failed` and `not_submitted` are both
/// true of it, and the sender was being told the weaker one.
///
/// The split of responsibilities is the point. The **outcome** comes from the
/// guard, so a member that turned out to be bound after all would report
/// `submission_unknown` rather than a non-delivery claim this call cannot
/// support. The **reason** comes from the refusal, because "no such member" and
/// "delivery channel full" are what a sender needs and the evidence order cannot
/// know either.
pub(in crate::relay::delivery) fn complete_task_refusal(
    task: &AsyncDeliveryTask,
    reason_code: &str,
    reason: &str,
) {
    // Boundness is not consulted here either, for the same reason as the trigger
    // path: the outcome already comes from the guard rather than from a producer.
    let Some(TerminalResolution {
        evidence,
        bound: _,
        guard,
    }) = resolve_terminal_transition(task)
    else {
        return;
    };
    // Unlike a trigger, a refusal fires at a known point: before any write. So a
    // member with no recorded evidence — a receipt, which was never admitted —
    // still resolves `not_submitted` rather than falling back to unknown. Nothing
    // was written, and that is a fact about where this call sits rather than an
    // inference from the ledger.
    let evidence = evidence.unwrap_or(SubmissionEvidence::NotSubmitted);
    report_terminal_outcome(
        task,
        Ok(SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: evidence.outcome(),
            reason_code: Some(reason_code.to_string()),
            reason: Some(reason.to_string()),
            details: None,
        }),
        guard,
    );
}

/// Reports a member the ledger already terminalized, without attempting the
/// transition a second time.
///
/// Every other function here begins by contesting the transition, because every
/// other caller holds only a task and has to find out whether it won. A
/// [`ResolvedMember`] is the transition's own output: it exists only for a member
/// this caller won, produced under the ledger lock by an operation that resolved
/// several members as one act. Routing it back through
/// [`complete_task_outcome`] would find the reservation already gone and stay
/// silent, which is exactly the duplicate-suppression working correctly against
/// the one caller that is not a duplicate.
///
/// The outcome comes from the evidence, as it does everywhere: the acknowledgment
/// that produced it reported what its write observed for this member, and a
/// lifecycle trigger that produced it brought none and took the guard's order.
/// The *cause* is what the evidence cannot carry — "the target has been
/// unreachable past the dwell" and "the generation was replaced" resolve to the
/// same spellings and are not the same thing to a sender — so each caller names
/// its own. Both parts default to the evidence: omitting the reason code leaves
/// the evidence's own to speak for itself, which is what an ordinary
/// acknowledgment wants, since the write is the whole story.
pub(in crate::relay::delivery) fn report_resolved_member(
    member: &ResolvedMember,
    reason_code: Option<&str>,
    reason: Option<&str>,
) {
    let reason_code = reason_code
        .map(str::to_string)
        .unwrap_or_else(|| member.evidence.reason_code().to_string());
    // A session observed serving is a session whose recorded startup failures no
    // longer describe it. This is the only place that observation is made now:
    // under the push model the collect site made it, and the collect site is what
    // the pull model retires. A relay-wide target is excluded because it has no
    // startup history to clear — it is served by the UI stream and never started.
    if member.evidence == SubmissionEvidence::Submitted
        && !crate::relay::delivery::admission::target_is_relay_wide(
            member.task.bundle.bundle_name.as_str(),
            member.task.target_session.as_str(),
        )
    {
        let _ = crate::relay::startup_state::note_session_served_successfully(
            member.task.runtime_directory.as_path(),
            member.task.target_session.as_str(),
        );
    }
    report_resolved_outcome(
        member,
        SendResult {
            target_session: member.task.target_session.clone(),
            message_id: member.message_id.clone(),
            outcome: member.evidence.outcome(),
            reason_code: Some(reason_code),
            reason: reason.map(str::to_string),
            details: None,
        },
    );
}

/// Publishes an outcome for a member the ledger already terminalized.
///
/// Split from [`report_resolved_member`] for the one caller that names its own
/// outcome rather than deriving it from the evidence: graceful shutdown spells an
/// undeclared entry `dropped_on_shutdown`, which no evidence value can express.
/// Everything downstream — the observability floor, the receipt routing, the
/// non-recursion marker — is identical, and identical is the point.
pub(in crate::relay::delivery) fn report_resolved_outcome(
    member: &ResolvedMember,
    result: SendResult,
) {
    report_terminal_outcome(&member.task, Ok(result), member.guard);
}

pub(in crate::relay::delivery) fn complete_task_outcome(
    task: &AsyncDeliveryTask,
    outcome: Result<SendResult, RelayError>,
) {
    // The single terminal transition, and the one place admission quota is
    // released. Every path that can finish a member routes through here, so
    // being told to stay silent means another path already resolved this member:
    // emitting anything below would be the duplicate resolution the guard exists
    // to prevent.
    let Some(TerminalResolution {
        evidence,
        bound,
        guard,
    }) = resolve_terminal_transition(task)
    else {
        return;
    };
    // The transport's outcome is the evidence, so it is reported as given —
    // except where the recorded evidence is strictly more honest than the
    // spelling the transport chose. An undifferentiated failure cannot support a
    // claim of non-delivery, so it surfaces as `submission_unknown` rather than
    // as `failed`.
    let outcome = outcome.map(|result| match evidence {
        Some(evidence) => reconcile_with_evidence(result, evidence, bound),
        None => result,
    });
    report_terminal_outcome(task, outcome, guard);
}

/// Resolves a member's reported outcome against its packing unit's record.
///
/// For a **bound** member the record answers, and every producer spelling gives
/// way to it — `delivered`, `not_submitted` and `dropped_on_shutdown` included.
/// The record is what its siblings resolved from, so a member disagreeing with it
/// is the split outcome the unit-owned record exists to make impossible.
///
/// For an **unbound** member the producer's spelling stands, because there is no
/// record to be authoritative with: its evidence is the guard's inference from
/// absence rather than a report of what a write proved.
fn reconcile_with_evidence(
    result: SendResult,
    evidence: SubmissionEvidence,
    bound: bool,
) -> SendResult {
    // An unbound member has no unit record to be authoritative *with*: its
    // evidence is the guard's inference from absence, not a report of what a
    // write proved. Its producer's spelling stands — safe now in a way it was not
    // before the refusal sites were routed through the guard, because those
    // spellings are already what the evidence order would derive.
    if !bound {
        return result;
    }
    // Bound: the unit's record is the answer, and the value the producer computed
    // for itself is discarded even when the two agree. Agreement was never the
    // property worth having — one record read by every member of the unit is,
    // because it makes disagreement between siblings unrepresentable rather than
    // merely absent.
    //
    // Causal metadata survives, because the record holds submission evidence and
    // not causes: `pty_write_failed` with an EPIPE behind it is worth more to a
    // sender than the generic code for whatever outcome it resolved to.
    //
    // A code that merely *labels* an outcome does not survive, because it no
    // longer describes the outcome it is attached to — `delivered` sitting on a
    // `submission_unknown` contradicts the field beside it and tells the sender
    // two different things.
    let reason_code = result
        .reason_code
        .filter(|code| !labels_an_outcome(code.as_str()))
        .unwrap_or_else(|| evidence.reason_code().to_string());
    SendResult {
        outcome: evidence.outcome(),
        reason_code: Some(reason_code),
        ..result
    }
}

/// Whether a reason code restates an outcome rather than explaining one.
///
/// Every terminal [`SendOutcome`] wire label belongs here, not just the ones a
/// producer happens to emit today: a label this misses is silent, surfacing as a
/// reason code that contradicts the outcome beside it rather than as an error.
///
/// `queued` and `peer_unavailable` are deliberately absent. Neither can reach
/// this mapping — `queued` is the synchronous admission answer rather than a
/// member's terminal outcome, and `peer_unavailable` belongs to a forwarded
/// cross-relay target, which has no local transport and so is never bound. The
/// exhaustive match in `evidence_authority_tests` is what keeps this list honest
/// as the vocabulary grows.
fn labels_an_outcome(reason_code: &str) -> bool {
    matches!(
        reason_code,
        "delivered"
            | "failed"
            | "not_submitted"
            | "submission_unknown"
            | DROPPED_ON_SHUTDOWN_REASON_CODE
    )
}

/// The evidence-authority rule, pinned on its own.
///
/// Inline because the two inputs that discriminate the arms — a transport that
/// wrote before declaring, and a bound member reached by a shutdown drain — are a
/// contract violation and an interleaving that no harness can arrange, and
/// because the alternative is making a relay-internal reconciliation rule public
/// so a test can reach it.
#[cfg(test)]
mod evidence_authority_tests {
    use super::*;

    fn result(outcome: SendOutcome, reason_code: &str) -> SendResult {
        SendResult {
            target_session: "target".to_string(),
            message_id: "message".to_string(),
            outcome,
            reason_code: Some(reason_code.to_string()),
            reason: Some("the producer's own account of what happened".to_string()),
            details: None,
        }
    }

    /// Binding, not the evidence value, decides whether a unit's record may
    /// override the outcome its producer computed.
    ///
    /// The discrimination is the whole point and it cannot be driven from any
    /// public surface: the cases that separate the two arms are contract
    /// violations and shutdown interleavings that no harness can arrange, which is
    /// why this is pinned against the mapping directly.
    #[test]
    fn only_a_bound_members_unit_record_may_override_its_producer() {
        // THE HAZARD. An unbound member's `NotSubmitted` is the guard's inference
        // from absence, not a report of a write. A transport that wrote before
        // declaring would otherwise have `delivered` replaced by a provable claim
        // that nothing was written — inventing the one direction this contract
        // exists to prevent. Its producer's spelling stands.
        let untouched = reconcile_with_evidence(
            result(SendOutcome::Delivered, "delivered"),
            SubmissionEvidence::NotSubmitted,
            false,
        );
        assert_eq!(untouched.outcome, SendOutcome::Delivered);

        // Bound: the record answers, and the producer's value is discarded even
        // where the two would have agreed. Agreement was never the property worth
        // having — one record read by every member of the unit is.
        let overridden = reconcile_with_evidence(
            result(SendOutcome::Delivered, "delivered"),
            SubmissionEvidence::SubmissionUnknown,
            true,
        );
        assert_eq!(overridden.outcome, SendOutcome::SubmissionUnknown);

        // A BOUND `dropped_on_shutdown` is overwritten too. Reachable in
        // production: the relay declares raw's singleton unit before calling
        // `raww`, so a raw write still sitting in a transport's channel when the
        // shutdown drain reaches it is bound. Task 41 requires an `Authorized`
        // member to resolve from evidence, so the policy spelling does not survive
        // binding.
        let bound_shutdown = reconcile_with_evidence(
            result(SendOutcome::DroppedOnShutdown, "dropped_on_shutdown"),
            SubmissionEvidence::SubmissionUnknown,
            true,
        );
        assert_eq!(bound_shutdown.outcome, SendOutcome::SubmissionUnknown);

        // The unbound `Pending` case task 41 established keeps its policy
        // spelling, and keeps it *because* it is unbound rather than by a special
        // case in the mapping.
        let pending_shutdown = reconcile_with_evidence(
            result(SendOutcome::DroppedOnShutdown, "dropped_on_shutdown"),
            SubmissionEvidence::NotSubmitted,
            false,
        );
        assert_eq!(pending_shutdown.outcome, SendOutcome::DroppedOnShutdown);

        // A reason code that merely labelled the superseded outcome does not
        // survive: `delivered` on a `submission_unknown` would contradict the
        // field beside it.
        assert_eq!(
            overridden.reason_code.as_deref(),
            Some("submission_unknown")
        );

        // A causal code does survive, because it says something no verdict can.
        // The record holds evidence, not causes.
        let diagnosed = reconcile_with_evidence(
            result(SendOutcome::Failed, "pty_write_failed"),
            SubmissionEvidence::SubmissionUnknown,
            true,
        );
        assert_eq!(diagnosed.outcome, SendOutcome::SubmissionUnknown);
        assert_eq!(diagnosed.reason_code.as_deref(), Some("pty_write_failed"));
        assert_eq!(
            diagnosed.reason.as_deref(),
            Some("the producer's own account of what happened")
        );

        // Every outcome label is classified, and this match is exhaustive so a
        // new `SendOutcome` variant cannot be added without deciding which side
        // it falls on. That enforcement belongs here rather than in the
        // classifier: a missed label produces contradictory metadata rather than
        // a failure, so nothing in production would notice.
        for outcome in [
            SendOutcome::Queued,
            SendOutcome::Delivered,
            SendOutcome::DroppedOnShutdown,
            SendOutcome::Failed,
            SendOutcome::NotSubmitted,
            SendOutcome::SubmissionUnknown,
            SendOutcome::PeerUnavailable,
        ] {
            let (label, is_label) = match outcome {
                SendOutcome::Delivered => ("delivered", true),
                SendOutcome::DroppedOnShutdown => ("dropped_on_shutdown", true),
                SendOutcome::Failed => ("failed", true),
                SendOutcome::NotSubmitted => ("not_submitted", true),
                SendOutcome::SubmissionUnknown => ("submission_unknown", true),
                // Cannot reach this mapping: the synchronous admission answer,
                // and a forwarded cross-relay target that is never bound.
                SendOutcome::Queued => ("queued", false),
                SendOutcome::PeerUnavailable => ("peer_unavailable", false),
            };
            assert_eq!(
                labels_an_outcome(label),
                is_label,
                "{label} is classified as an outcome label, or deliberately is not",
            );
        }
    }
}

/// The evidence order's second rung, held at the helper every trigger reaches.
///
/// Its own block on the same terms as the others, and the count is deliberate
/// rather than debt: each block has to argue its own exception, and that friction
/// is what keeps a relay-internal test from being written where a public seam
/// should have been built instead.
///
/// Scope is the whole point here, because the original task asserted this under
/// *every* trigger and that was false. Two paths reach a terminal outcome for an
/// unbound member and deliberately do not spell `not_submitted`:
/// `complete_task_on_shutdown`, which reports `dropped_on_shutdown` as the task
/// 41 policy, and the pre-write render and construction failures, which report
/// `failed` through an explicit `Err`. Neither is a bypass — both are reporting
/// semantics chosen for members whose delivery responsibility never transferred.
/// So what is coverable is the rung at this helper, and the enumeration of
/// triggers *below* it rather than of paths above it.
#[cfg(test)]
mod unbound_resolution_tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::configuration::{BundleConfiguration, SessionType};
    use crate::relay::delivery::admission::{AdmissionTargetKey, admit};
    use crate::relay::{DeliveryPayloadMode, SCHEMA_VERSION};

    /// A member never bound to a packing unit resolves `not_submitted` at the
    /// trigger helper, whichever trigger fired.
    ///
    /// This is the guard's inference from absence, and it is sound because the
    /// declaration is recorded before the first target-side effect: an unbound
    /// member provably could not have been submitted. The claim worth pinning is
    /// that the *trigger does not enter into it* — a bundle stop and a graceful
    /// shutdown resolve the same member the same way, because only the evidence
    /// order is consulted.
    ///
    /// `GracefulShutdown` is the sharp one and is included deliberately. Reaching
    /// this helper it resolves `not_submitted`, while `complete_task_on_shutdown`
    /// — a different function, for members still `Pending` — reports
    /// `dropped_on_shutdown`. That pair is exactly the counterexample that made
    /// the original "under every trigger" wording false, so covering the trigger
    /// here records the distinction rather than papering over it.
    ///
    /// The match below forces a new `GuardTrigger` variant to be named before this
    /// compiles. It does **not** force the variant into the array driving the
    /// loop, so the enumeration is a tripwire rather than a proof of completeness;
    /// stated rather than left to be assumed.
    #[test]
    fn an_unbound_member_resolves_not_submitted_under_any_trigger() {
        let temporary = tempfile::TempDir::new().expect("temporary");
        let inscriptions = temporary.path().join("inscriptions.log");
        let _ = crate::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

        for trigger in [
            GuardTrigger::GracefulShutdown,
            GuardTrigger::ExecutionBound,
            GuardTrigger::BundleStop,
        ] {
            let message_id = match trigger {
                GuardTrigger::GracefulShutdown => "unbound-graceful-shutdown",
                GuardTrigger::ExecutionBound => "unbound-execution-bound",
                GuardTrigger::BundleStop => "unbound-bundle-stop",
            };
            let target = AdmissionTargetKey::new(
                "unbound-test",
                Path::new("/nonexistent/unbound-test"),
                "target",
            );
            // Admitted and queued, which is the case a trigger realistically
            // finds: an entry waiting in its target's mailbox for an executor to
            // peek. No unit is ever declared, which is the whole fixture —
            // nothing was written and the ledger can prove it.
            admit(message_id, target, SessionType::Tmux, 1).expect("admit");

            let task = AsyncDeliveryTask {
                admitted: true,
                bundle: BundleConfiguration {
                    schema_version: SCHEMA_VERSION.to_string(),
                    bundle_name: "unbound-test".to_string(),
                    autostart: false,
                    groups: Vec::new(),
                    members: Vec::new(),
                },
                sender_namespace: "unbound-test".to_string(),
                sender: super::super::reporting::relay_system_sender_member(),
                authenticated_identity: None,
                on_behalf_of: None,
                all_target_sessions: Vec::new(),
                target_session: "target".to_string(),
                message: "body".to_string(),
                message_id: message_id.to_string(),
                runtime_directory: PathBuf::from("/nonexistent/unbound-test"),
                payload_mode: DeliveryPayloadMode::EnvelopeMessage,
                append_enter: true,
                choice_decider_sessions: Vec::new(),
                is_receipt: false,
                sender_return_route: None,
            };

            complete_task_outcome_from_trigger(&task, trigger);

            let completed = std::fs::read_to_string(&inscriptions)
                .expect("inscriptions file")
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|record| {
                    record["event"] == "relay.send.async.completed"
                        && record["details"]["message_id"] == message_id
                })
                .unwrap_or_else(|| panic!("{trigger:?} reported no terminal outcome"));

            assert_eq!(
                completed["details"]["outcome"], "not_submitted",
                "{trigger:?} resolved an unbound member as something other than a \
                 provable non-delivery",
            );
            // The trigger names why the member stopped being resolvable, and only
            // that. A trigger leaking into the outcome is the failure this pair of
            // assertions separates.
            assert_eq!(
                completed["details"]["reason"],
                trigger.reason(),
                "{trigger:?} did not contribute the reason",
            );
        }
    }
}
