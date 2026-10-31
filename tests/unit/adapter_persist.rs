use agentmux::adapter::persist::{PersistRefusal, PersistTracker, ResumeTag};

fn tag(generation: u64) -> ResumeTag {
    ResumeTag {
        runtime_instance: "rt-1".to_string(),
        target_session: "seat-1".to_string(),
        adapter_generation: generation,
    }
}

/// A fresh write id registers under the cap.
#[test]
fn fresh_write_registers() {
    let mut tracker = PersistTracker::new(4);
    tracker
        .submit("w-1".to_string(), tag(2))
        .expect("registers");
    assert_eq!(tracker.outstanding(), 1);
}

/// A repeated write id refuses as a duplicate without touching state.
#[test]
fn duplicate_write_id_refuses() {
    let mut tracker = PersistTracker::new(4);
    tracker
        .submit("w-1".to_string(), tag(2))
        .expect("registers");
    let error = tracker
        .submit("w-1".to_string(), tag(2))
        .expect_err("duplicate must refuse");
    assert!(matches!(
        error,
        PersistRefusal::DuplicateWriteId { write_id } if write_id == "w-1"
    ));
    assert_eq!(tracker.outstanding(), 1);
}

/// Past the adapter-local cap, writes refuse rather than queue.
#[test]
fn over_cap_refuses() {
    let mut tracker = PersistTracker::new(1);
    tracker
        .submit("w-1".to_string(), tag(2))
        .expect("registers");
    let error = tracker
        .submit("w-2".to_string(), tag(2))
        .expect_err("cap must refuse");
    assert!(matches!(error, PersistRefusal::OverLocalCap { cap: 1 }));
}

/// Retiring on commit or refusal frees the id and cap space.
#[test]
fn retire_frees_id_and_cap() {
    let mut tracker = PersistTracker::new(1);
    tracker
        .submit("w-1".to_string(), tag(2))
        .expect("registers");
    tracker.retire("w-1");
    tracker
        .submit("w-1".to_string(), tag(3))
        .expect("id reusable");
    assert_eq!(tracker.outstanding(), 1);
}
