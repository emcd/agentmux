//! Send operation tests: target resolution, broadcast, async dispatch,
//! cross-relay bang-path resolution.

use agentmux::relay::{RelayRequest, RelayResponse, SendOutcome};

use super::*;

#[test]
fn send_rejects_unknown_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["missing@party".to_string()],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("send should fail");
    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn send_rejects_target_by_configured_session_name_alias() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["Bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("send should fail");
    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn send_accepts_global_ui_target_not_in_bundle_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["user@GLOBAL".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "user@GLOBAL");
}

#[test]
fn send_prefers_bundle_member_when_target_id_overlaps_with_ui_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration_with_session_id(&config_root, "default", "alpha@GLOBAL");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "bravo".to_string(),
            message: "hello".to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "alpha@party");
    // Overlap precedence is bundle-member-first: the resolved target is the
    // bundle member alpha@party, not the overlapping UI session. Async
    // dispatch queues the delivery regardless of tmux runtime presence.
    assert_eq!(results[0].outcome, SendOutcome::Queued);
}

#[test]
fn send_broadcast_excludes_sender_session() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: Vec::new(),
            broadcast: true,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "bravo@party");
}

#[test]
fn send_async_returns_accepted_and_queued_outcome() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "bravo@party");
    assert_eq!(results[0].outcome, SendOutcome::Queued);
}

#[test]
fn send_broadcast_with_only_sender_returns_empty_results() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_single_member_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: Vec::new(),
            broadcast: true,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert!(results.is_empty());
}

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
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("cross-relay send has no peer manager on the non-stream path");
    assert_eq!(response.code, "runtime_cross_relay_unavailable");
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
                quiet_window_ms: Some(1),
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
