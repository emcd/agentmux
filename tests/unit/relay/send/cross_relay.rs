//! Cross-relay bang-path targets: what resolves, what is refused, and where a
//! resolved target stops on the non-stream entry point.

use agentmux::relay::RelayRequest;

use super::*;

#[test]
fn send_without_peer_manager_reports_cross_relay_unavailable() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    // A bang-path target is classified as cross-relay from the address alone
    // (config-free). The non-stream single-bundle entry point holds no peer
    // connection manager, so forwarding cannot run and the target reports as
    // unavailable rather than a misleading unknown-bundle error. Real forwarding
    // is exercised through the stream path, which supplies the manager.
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@other!peer-relay".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("cross-relay send has no peer manager on the non-stream path");
    assert_eq!(response.code, "runtime_cross_relay_unavailable");
}

// Cross-relay forwarding attributes any verified requester, and a relay-wide
// user is one, so a delivered sender can name a `@GLOBAL` origin. Resolution has
// to accept it or that sender is unparseable as a reply target. Reaching
// `runtime_cross_relay_unavailable` is what shows it passed resolution: the
// non-stream entry point holds no peer manager, so forwarding is where it stops.
#[test]
fn send_accepts_a_relay_wide_cross_relay_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["operator@GLOBAL!peer-relay".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("cross-relay send has no peer manager on the non-stream path");
    assert_eq!(
        response.code, "runtime_cross_relay_unavailable",
        "a relay-wide origin must survive resolution and stop at forwarding"
    );
}

// The reply half of the round trip: a delivered cross-relay sender is composed as
// `<origin>!<peer>`, and this asserts the router accepts that shape back for both
// origin kinds a conforming peer can attribute. Reaching
// `runtime_cross_relay_unavailable` means it survived resolution.
//
// What this does *not* prove on its own is that composition emits this shape —
// that is asserted against the live delivered envelope in the cross-relay
// attribution tests. The round trip is the two together; neither half alone
// would catch the separator changing on one side.
#[test]
fn send_resolves_a_composed_cross_relay_sender_as_a_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    for origin in ["origin-subject@remote", "operator@GLOBAL"] {
        let composed = format!("{origin}!origin-peer");
        let response = dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: "reply".to_string(),
                targets: vec![composed.clone()],
                broadcast: false,
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect_err("cross-relay send has no peer manager on the non-stream path");
        assert_eq!(
            response.code, "runtime_cross_relay_unavailable",
            "composed sender {composed} must survive resolution as a target",
        );
    }
}

// Widening one arm of the classification is what most easily loosens the others,
// so each surviving rejection is asserted separately rather than as a group.
#[test]
fn send_rejects_a_cross_relay_target_in_a_non_routable_namespace() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    for target in ["app@EXTERNAL!peer-relay", "other@RELAY!peer-relay"] {
        let response = dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: "hello".to_string(),
                targets: vec![target.to_string()],
                broadcast: false,
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect_err("a non-routable namespace must not resolve");
        assert_eq!(
            response.code, "validation_unsupported_namespace",
            "target {target} names no routable recipient",
        );
    }
}

#[test]
fn send_rejects_malformed_cross_relay_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    for target in ["bravo@other!", "bravo!peer-relay"] {
        let response = dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: "hello".to_string(),
                targets: vec![target.to_string()],
                broadcast: false,
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect_err("malformed cross-relay target should fail at resolution");
        assert_eq!(
            response.code, "validation_malformed_cross_relay_target",
            "target {target} should be rejected as malformed",
        );
    }
}
