//! Whether a target will take a handover right now, and the outcome for one
//! that has been unreachable too long to keep waiting on.

use std::time::Duration;

use serde_json::json;

use crate::relay::{AsyncDeliveryTask, SendOutcome, SendResult};
use crate::transports::{TransportHealth, TransportImpl};

/// The outcome for a member whose target was unreachable for longer than the
/// dwell allows.
///
/// `not_submitted`, not `failed`. The member was never authorized and never
/// handed to a transport, so nothing could have reached the target — that is
/// positive evidence of non-delivery, which is exactly what `NotSubmitted`
/// asserts and what `Failed` does not.
///
/// Returned as `Ok` rather than `Err` deliberately: the error branch of the
/// terminal transition spells everything `Failed`, and only the `Ok` branch is
/// reconciled against recorded evidence. A never-authorized member has no
/// recorded evidence, so this spelling passes through as given.
pub(super) fn target_unreachable_result(task: &AsyncDeliveryTask, dwell: Duration) -> SendResult {
    SendResult {
        target_session: task.target_session.clone(),
        message_id: task.message_id.clone(),
        outcome: SendOutcome::NotSubmitted,
        reason_code: Some("delivery_target_unreachable".to_string()),
        reason: Some(
            "target could not be reached for longer than the configured dwell".to_string(),
        ),
        details: Some(json!({
            "unreachable_dwell_ms": dwell.as_millis() as u64,
        })),
    }
}

/// Whether a target will take a handover right now.
///
/// Read once per batch rather than once per member. That is what makes a batch a
/// set: every member of it was authorized against one observation of the target,
/// so there is no member whose authorization rests on a readiness its groupmates
/// never saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TargetGate {
    /// Healthy and ready. A batch may be formed and authorized against it.
    Open,
    /// Cannot take a handover yet. Nothing has happened to the member in hand —
    /// no authorization, no batch, no quota movement — so it is held.
    Hold,
    /// Continuously unreachable past the dwell.
    Unreachable,
}

/// Reads both readiness axes for a target, in the order that bounds the wait.
///
/// Both are required and neither substitutes for the other. Health is read first
/// because it is what bounds the wait, and an unreachable target is never
/// authorized whatever readiness says — a transport that cannot reach its target
/// has nothing useful to report about whether that target is at a prompt.
/// Whether the target's activity marker advanced since the last gate decision,
/// recording the new value either way.
///
/// The comparison lives here rather than in the transport because only the relay
/// knows which two observations bracket a handover decision — the transport
/// publishes a level and has no notion of the gate that reads it.
///
/// The first observation records without suppressing. A marker seeded from an
/// epoch would otherwise read as an advance against a zero it was never
/// compared to, costing one poll interval on the first delivery to every target
/// for no evidence at all.
fn activity_advanced(current: u64, last: &mut Option<u64>) -> bool {
    // `0` is absence, and absence carries no meaning: either the transport
    // tracks no marker at all, or this observation could not read one because
    // the target was unobservable. Neither is evidence, so it must not suppress
    // a handover *and* must not enter the series — folding it in would make the
    // next real reading an advance against zero, so a target would be held for a
    // tick on recovering from any unreachable observation, on no evidence that
    // it had written anything.
    if current == 0 {
        return false;
    }
    let advanced = last.is_some_and(|previous| current > previous);
    *last = Some(current);
    advanced
}

pub(super) async fn gate_target(
    transport: &TransportImpl,
    unreachable_dwell: Duration,
    last_activity: &mut Option<u64>,
) -> TargetGate {
    // Read on every gate decision, including the ones that return early below.
    // An unobservable target reports no marker, and the comparison treats that
    // as absence rather than as a value, so the series survives an unreachable
    // stretch instead of being reset by it.
    let advanced = activity_advanced(transport.activity_generation(), last_activity);
    let ready = transport.is_ready_for_handover().await;
    decide_gate(transport.health(), unreachable_dwell, advanced, ready)
}

/// The gate decision itself, over levels rather than over a transport.
///
/// Split from [`gate_target`] because reading a target and judging one are
/// different jobs: the read needs a live tmux session, an ACP child or a UI
/// broadcaster, while the judgement is three booleans and a duration. Keeping
/// them apart is what lets the precedence below be stated once and checked
/// directly, instead of being inferred from whichever transport a fixture could
/// afford to build.
fn decide_gate(
    health: TransportHealth,
    unreachable_dwell: Duration,
    activity_advanced: bool,
    ready: bool,
) -> TargetGate {
    match health {
        // Waiting will not make this target ready, so its member resolves rather
        // than keeping a place in a queue nothing will drain.
        TransportHealth::Unreachable { since } if since.elapsed() >= unreachable_dwell => {
            return TargetGate::Unreachable;
        }
        // Unreachable but not yet past the dwell: hold, exactly as an unready
        // target is held. An unreachability that ends in time costs nothing.
        TransportHealth::Unreachable { .. } => return TargetGate::Hold,
        TransportHealth::Healthy => {}
    }
    // Checked before readiness and allowed to override it. A pane is captured as
    // text, so a template can match transiently on a frame that happens to end
    // in a prompt while output is still flowing — and injecting there lands a
    // batch in the middle of an agent's turn. An advance is positive evidence
    // the target is doing something; the template's match is an inference from a
    // rendering. Where they disagree, the evidence wins.
    //
    // It is ordered *after* health deliberately. An advance says the target is
    // active, which is not a reason to keep holding a member behind a transport
    // that has been unreachable past its dwell — that member is owed an outcome,
    // and suppression may only ever withhold a handover.
    if activity_advanced {
        return TargetGate::Hold;
    }
    if ready {
        TargetGate::Open
    } else {
        TargetGate::Hold
    }
}

/// The activity comparison, which is the whole of what the relay contributes to
/// this signal.
///
/// Inline because [`activity_advanced`] is the comparison itself rather than a
/// transport surface, and reaching it from `tests/` would mean publishing the
/// gate's internals. Driving `gate_target` instead would need a `TransportImpl`,
/// which cannot be constructed without a real tmux session, an ACP child or a UI
/// broadcaster — and would then test tmux rather than the rule.
#[cfg(test)]
mod activity_signal_tests {
    use super::{TargetGate, activity_advanced, decide_gate};
    use crate::transports::TransportHealth;
    use std::time::{Duration, Instant};

    /// What suppresses a handover, and what a suppression may not outrank.
    ///
    /// One test rather than two because the comparison and the precedence are
    /// one rule: an advance is the only thing that suppresses, and suppression
    /// only ever withholds. Splitting them would let each half pass while the
    /// rule they compose was broken.
    #[test]
    fn only_a_real_advance_suppresses_a_handover_and_only_ever_withholds_it() {
        let mut last = None;
        // THE FIRST-OBSERVATION CASE. tmux seeds this marker from an epoch, so a
        // first reading is a large number against no previous value. Treating
        // that as an advance would cost a poll interval on the first delivery to
        // every target, on no evidence.
        assert!(
            !activity_advanced(1_700_000_000, &mut last),
            "the first observation has nothing to have advanced against"
        );
        assert!(
            activity_advanced(1_700_000_001, &mut last),
            "a later marker is positive evidence the target wrote something"
        );
        assert!(
            !activity_advanced(1_700_000_001, &mut last),
            "an unchanged marker is not evidence of anything, so it holds nothing back"
        );

        // THE RECOVERY CASE, and the reason absence may not enter the series. An
        // unobservable target reports no marker; folding that in as a value
        // would make the next real reading an advance against zero, holding a
        // ready target for a tick because it had once been unreachable.
        assert!(
            !activity_advanced(0, &mut last),
            "an unreadable marker is absence, and absence suppresses nothing"
        );
        assert!(
            !activity_advanced(1_700_000_001, &mut last),
            "recovering with an unchanged marker is not an advance"
        );

        // A transport with no such primitive reports the constant 0 forever. It
        // can never advance, which is what makes the default sound rather than a
        // guess: such a target is simply never suppressed on this basis.
        let mut absent = None;
        for _ in 0..4 {
            assert!(
                !activity_advanced(0, &mut absent),
                "a constant zero can never advance"
            );
        }

        let dwell = Duration::from_secs(60);

        assert_eq!(
            decide_gate(TransportHealth::Healthy, dwell, true, true),
            TargetGate::Hold,
            "an activity advance outranks a matching prompt template"
        );
        // The control, and it is what makes the assertion above mean anything: a
        // ready target with no advance is exactly the case that must still open,
        // or the signal would simply be blocking every delivery.
        assert_eq!(
            decide_gate(TransportHealth::Healthy, dwell, false, true),
            TargetGate::Open
        );
        assert_eq!(
            decide_gate(TransportHealth::Healthy, dwell, false, false),
            TargetGate::Hold
        );

        // Suppression may only withhold a handover, never keep a member from an
        // outcome it is owed. A target unreachable past its dwell resolves even
        // while its marker is advancing.
        let stale = TransportHealth::Unreachable {
            since: Instant::now() - Duration::from_secs(120),
        };
        assert_eq!(
            decide_gate(stale, dwell, true, true),
            TargetGate::Unreachable,
            "activity must not outrank a target that is owed resolution"
        );
    }
}
