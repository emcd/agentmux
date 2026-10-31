use std::collections::HashMap;

use agentmux::adapter::binding::PinnedLaunch;
use agentmux::adapter::handshake::{
    LimitsProposal, NegotiatedLimits, VersionOffer, hello_request,
    negotiate_minor, validate_limits, validate_selection,
};

/// Minor selection is the min of maxes within the overlap.
#[test]
fn minor_overlap_selects_min_of_maxes() {
    assert_eq!(negotiate_minor(0, 2, 1, 3), Some(2));
}

/// Disjoint windows select nothing.
#[test]
fn disjoint_windows_select_nothing() {
    assert_eq!(negotiate_minor(0, 0, 1, 1), None);
}

/// A major outside the offer refuses, whatever the minor.
#[test]
fn major_mismatch_refuses() {
    let offer = VersionOffer {
        major: 2,
        min_minor: 0,
        max_minor: 2,
    };
    let error = validate_selection(&offer, 3, 1)
        .expect_err("major must match exactly");
    assert!(error.to_string().contains("major mismatch"));
}

/// A selected minor outside the offered window refuses.
#[test]
fn minor_outside_window_refuses() {
    let offer = VersionOffer {
        major: 2,
        min_minor: 1,
        max_minor: 2,
    };
    let error = validate_selection(&offer, 2, 0)
        .expect_err("minor must lie in the window");
    assert!(error.to_string().contains("outside the offered window"));
}

/// An in-window selection converges.
#[test]
fn in_window_selection_converges() {
    let offer = VersionOffer {
        major: 2,
        min_minor: 0,
        max_minor: 2,
    };
    validate_selection(&offer, 2, 1).expect("selection is valid");
}

fn proposal() -> LimitsProposal {
    LimitsProposal {
        max_frame_bytes: 1_048_576,
        queue_tx_bytes: 16_777_216,
        queue_rx_bytes: 16_777_216,
        max_control_pending: 32,
        max_events_per_second: 100,
        window_seconds: 1,
        max_submits_in_flight: 64,
        max_choices_in_flight: 16,
        max_looks_in_flight: 8,
        drain_wait_ms: 250,
        latency_declarations: HashMap::from([("default".to_string(), 5_000)]),
    }
}

fn negotiated() -> NegotiatedLimits {
    NegotiatedLimits {
        max_frame_bytes: 65_536,
        queue_tx_bytes: 16_777_216,
        queue_rx_bytes: 16_777_216,
        max_control_pending: 32,
        max_events_per_second: 100,
        window_seconds: 1,
        max_submits_in_flight: 64,
        max_choices_in_flight: 16,
        max_looks_in_flight: 8,
        drain_wait_ms: 250,
    }
}

/// Sane negotiated limits validate against the proposal.
#[test]
fn sane_limits_validate() {
    validate_limits(&proposal(), &negotiated()).expect("limits are valid");
}

/// A zero in-flight cap is invalid, never unlimited.
#[test]
fn zero_cap_is_invalid() {
    let mut limits = negotiated();
    limits.max_choices_in_flight = 0;
    let error = validate_limits(&proposal(), &limits)
        .expect_err("zero cap must refuse");
    assert!(error.to_string().contains("max_choices_in_flight"));
}

/// A negotiated frame bound above the proposal refuses.
#[test]
fn negotiated_above_proposal_refuses() {
    let mut limits = negotiated();
    limits.max_frame_bytes = 2_097_152;
    let error = validate_limits(&proposal(), &limits)
        .expect_err("must not exceed the proposal");
    assert!(error.to_string().contains("exceeds the proposal"));
}

/// Missing latency declarations refuse the binding.
#[test]
fn missing_latency_declarations_refuse() {
    let mut offer = proposal();
    offer.latency_declarations.clear();
    let error = validate_limits(&offer, &negotiated())
        .expect_err("declarations are required");
    assert!(error.to_string().contains("latency declarations"));
}

/// Unknown profiles refuse before any wire traffic.
#[test]
fn unknown_profile_refuses_hello() {
    let pinned = PinnedLaunch {
        target_session: "seat-1".to_string(),
        adapter_profile: "tmux".to_string(),
        adapter_executable: "agentmux-adapter".to_string(),
    };
    let offer = VersionOffer {
        major: 2,
        min_minor: 0,
        max_minor: 0,
    };
    let error = hello_request(&pinned, &offer, &proposal())
        .expect_err("profile must be known");
    assert!(error.to_string().contains("tmux"));
}

/// The hello request carries launch inputs and the version offer.
#[test]
fn hello_request_carries_launch_inputs() {
    let pinned = PinnedLaunch {
        target_session: "seat-1".to_string(),
        adapter_profile: "generic".to_string(),
        adapter_executable: "agentmux-adapter".to_string(),
    };
    let offer = VersionOffer {
        major: 2,
        min_minor: 0,
        max_minor: 1,
    };
    let request = hello_request(&pinned, &offer, &proposal())
        .expect("generic profile is known");
    assert_eq!(request["target_session"], "seat-1");
    assert_eq!(request["adapter_profile"], "generic");
    assert_eq!(request["protocol_major"], 2);
    assert_eq!(
        request["latency_declarations"]["default"], 5_000,
        "hello carries latency declarations"
    );
    assert_eq!(
        request["limits_proposal"]["max_frame_bytes"], 1_048_576,
        "hello carries the frame bound proposal"
    );
}
