use agentmux::adapter::lifecycle::{
    EventKind, Liveness, Outbox, OutboxRefusal, may_coalesce, ping_reply,
};

/// Only progress and heartbeat may coalesce or drop under flood.
#[test]
fn coalescing_class_is_progress_and_heartbeat() {
    assert!(may_coalesce(EventKind::Progress));
    assert!(may_coalesce(EventKind::Heartbeat));
    for kind in [EventKind::Ready, EventKind::Degraded, EventKind::Failed] {
        assert!(!may_coalesce(kind), "{kind:?} must never drop");
    }
}

/// Data writes past the cap fail rather than queue unboundedly.
#[test]
fn data_cap_fails_open() {
    let mut outbox = Outbox::new(16, 4);
    outbox.push_data(16).expect("cap-sized write fits");
    let error = outbox.push_data(1).expect_err("past the cap must fail");
    assert_eq!(error, OutboxRefusal::DataCapExceeded);
}

/// Released bytes free cap space for later writes.
#[test]
fn release_frees_data_cap() {
    let mut outbox = Outbox::new(16, 4);
    outbox.push_data(16).expect("fits");
    outbox.release_data(16);
    outbox.push_data(16).expect("fits again after release");
}

/// The control lane has its own bound, independent of the data cap.
#[test]
fn control_lane_has_own_bound() {
    let mut outbox = Outbox::new(0, 1);
    outbox.push_control().expect("first control signal fits");
    let error = outbox.push_control().expect_err("control bound must fail");
    assert_eq!(error, OutboxRefusal::ControlCapExceeded);
}

/// Liveness expires only past the negotiated timeout.
#[test]
fn liveness_expires_past_timeout() {
    let mut liveness = Liveness::new(1_000, 5_000);
    assert!(!liveness.is_expired(5_999));
    liveness.observe(2_000);
    assert!(!liveness.is_expired(6_999));
    assert!(liveness.is_expired(7_000));
}

/// The ping reply is the empty object: presence is the proof.
#[test]
fn ping_reply_is_empty_object() {
    let reply = ping_reply();
    assert!(reply.as_object().is_some());
    assert_eq!(reply.as_object().map(|map| map.len()), Some(0));
}
