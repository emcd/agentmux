use agentmux::adapter::submit::{
    OutcomeLiteral, TurnGate, WriteEvidence, outcome_for_attempt,
};

/// The first submit opens the gate; the second while held refuses.
#[test]
fn single_flight_refuses_busy_turn() {
    let mut gate = TurnGate::new();
    gate.begin("c-1".to_string()).expect("first turn opens");
    let holder = gate.begin("c-2".to_string()).expect_err("busy refuses");
    assert_eq!(holder, "c-1");
    assert!(gate.is_held());
}

/// Completing the turn releases the gate for the next correlation.
#[test]
fn completed_turn_releases_gate() {
    let mut gate = TurnGate::new();
    gate.begin("c-1".to_string()).expect("turn opens");
    assert_eq!(gate.complete().as_deref(), Some("c-1"));
    assert!(!gate.is_held());
    gate.begin("c-2".to_string()).expect("gate is free");
}

/// Observed submission resolves Submitted on transport_ack evidence.
#[test]
fn observed_write_is_submitted() {
    let (literal, evidence) =
        outcome_for_attempt(WriteEvidence::SubmittedOutbox);
    assert_eq!(literal, OutcomeLiteral::Submitted);
    assert_eq!(literal.as_wire(), "Submitted");
    assert_eq!(evidence, "transport_ack");
}

/// Each provable non-submission resolves TransportUnavailable.
#[test]
fn provable_non_submission_is_unavailable() {
    for evidence in [
        WriteEvidence::SerializationFailedUnbuffered,
        WriteEvidence::PromptRefusedUnbuffered,
        WriteEvidence::NoRuntime,
    ] {
        let (literal, _) = outcome_for_attempt(evidence);
        assert_eq!(literal.as_wire(), "TransportUnavailable");
    }
}

/// An unproven write resolves unknown, never success.
#[test]
fn unproven_write_is_unknown() {
    let (literal, evidence) =
        outcome_for_attempt(WriteEvidence::FailedUnknown);
    assert_eq!(literal.as_wire(), "SubmissionUnknown");
    assert_eq!(evidence, "unproven_write");
}
