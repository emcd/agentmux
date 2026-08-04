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

/// An envelope whose canonical payload alone exceeds what the target's transport
/// will ever accept is refused at admission rather than queued, because no
/// partition could ever carry it. The refusal is structured and names both the
/// size and the ceiling so the caller can act on it.
#[test]
fn send_rejects_a_payload_larger_than_the_transport_can_accept() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let oversized = "x".repeat(262_144 + 1);
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: oversized,
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("an oversized payload should be refused at admission");
    assert_eq!(response.code, "validation_payload_too_large");
    let details = response.details.expect("rejection carries details");
    assert_eq!(details["canonical_bytes"], 262_145);
    assert_eq!(details["canonical_bytes_max"], 262_144);
}

/// A payload at exactly the transport's ceiling is admitted: the bound is the
/// largest accepted size, not the smallest rejected one. Without this the
/// oversize test above passes against an off-by-one that refuses valid sends.
#[test]
fn send_admits_a_payload_at_exactly_the_transport_ceiling() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "x".repeat(262_144),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("a payload at the ceiling is admitted");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);
}

/// A Pubsub target is refused synchronously at admission. The request fails as a
/// whole rather than returning `queued` and resolving later, so no work is
/// authorized merely to discover the forward-declared stub.
#[test]
fn send_rejects_a_pubsub_target_at_admission() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle_with_policy(
        &temporary,
        "party",
        r#"
format-version = 1

[[sessions]]
id = "alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "feed"
directory = "/tmp"

[sessions.pubsub]
"#,
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["feed@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("a pubsub target should be refused at admission");
    assert_eq!(response.code, "runtime_session_type_not_implemented");
}

/// A request naming a Pubsub target alongside a deliverable one is refused
/// whole: admission reserves for every target before queueing any, so the
/// deliverable target is not left queued behind a refused request.
///
/// The absence of the deliverable target's `queued` inscription is the claim, so
/// the test first drives a send that does produce one. Without that control the
/// absence assertion would pass just as well against inscriptions that never
/// reached this file at all.
#[test]
fn send_rejecting_one_target_queues_none_of_the_request() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle_with_policy(
        &temporary,
        "party",
        r#"
format-version = 1

[[sessions]]
id = "alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "bravo"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "feed"
directory = "/tmp"

[sessions.pubsub]
"#,
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    // Control: a send the relay does accept queues bravo and says so.
    dispatch_request(
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
    let queued_before = count_bravo_queued_inscriptions(&inscriptions);
    assert_eq!(
        queued_before, 1,
        "control send should queue bravo exactly once"
    );

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string(), "feed@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("a request naming a pubsub target is refused whole");
    assert_eq!(response.code, "runtime_session_type_not_implemented");
    assert_eq!(
        count_bravo_queued_inscriptions(&inscriptions),
        queued_before,
        "the refused request must not have queued its deliverable target"
    );
}

/// Counts `relay.send.async.queued` inscriptions naming bravo. The `queued`
/// inscription is written synchronously before the send response returns, so its
/// absence after a refused request is decidable at that point.
fn count_bravo_queued_inscriptions(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            line.contains("\"event\":\"relay.send.async.queued\"")
                && line.contains("\"target_session\":\"bravo\"")
        })
        .count()
}
