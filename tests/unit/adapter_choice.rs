use agentmux::adapter::choice::{
    ChoiceGate, ChoiceResolution, ChoiceResolver, DiagnosticEntry,
    GateDecision,
};

/// Resolution records the outcome; answering observes the record.
#[test]
fn resolve_records_before_answering() {
    let mut resolver = ChoiceResolver::new();
    assert!(!resolver.is_resolved());
    let recorded = resolver
        .resolve(ChoiceResolution::Chosen {
            option_id: "o-1".to_string(),
            decided_by: "op-9".to_string(),
        })
        .clone();
    assert!(resolver.is_resolved());
    assert_eq!(
        recorded,
        ChoiceResolution::Chosen {
            option_id: "o-1".to_string(),
            decided_by: "op-9".to_string(),
        }
    );
}

/// Cancelling twice keeps the first terminal: one outcome per choice.
#[test]
fn double_cancel_keeps_first_outcome() {
    let mut resolver = ChoiceResolver::new();
    let _ = resolver.cancel("shutdown");
    let second = resolver.cancel("respawn").clone();
    assert_eq!(
        second,
        ChoiceResolution::Cancelled {
            decided_by: None,
            reason_code: "shutdown".to_string(),
        }
    );
}

/// A need past the cap sheds with a typed code inscribed to the
/// diagnostic sink — never queued, never stdout.
#[test]
fn shed_choice_is_inscribed() {
    let mut gate = ChoiceGate::new(1);
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    assert_eq!(gate.request(&mut sink), GateDecision::Admit);
    let shed = gate.request(&mut sink);
    assert!(matches!(shed, GateDecision::Shed { .. }));
    assert_eq!(sink.len(), 1);
    assert_eq!(sink[0].code, "adapter_single_flight_busy");
}

/// Releasing on terminal outcomes frees cap space.
#[test]
fn release_frees_cap_space() {
    let mut gate = ChoiceGate::new(1);
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    gate.request(&mut sink);
    gate.release();
    assert_eq!(gate.outstanding(), 0);
    assert_eq!(gate.request(&mut sink), GateDecision::Admit);
}

/// Release below zero saturates rather than underflowing.
#[test]
fn release_saturates_at_zero() {
    let mut gate = ChoiceGate::new(2);
    gate.release();
    assert_eq!(gate.outstanding(), 0);
}
