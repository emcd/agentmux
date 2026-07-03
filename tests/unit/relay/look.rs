//! Look operation tests: capability gates, snapshot construction,
//! rejection matrix.

use agentmux::relay::{LookSnapshotPayload, RelayRequest, RelayResponse};

use super::*;

#[test]
fn look_rejects_cross_relay_target_as_unsupported() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    // Unlike Send/Raww, Look does not forward across relays: cross-relay
    // inspection needs request/response snapshot semantics over the peer boundary
    // and is a deferred follow-on. A well-formed cross-relay Look target is a
    // permanent, honest rejection rather than a routing-unavailable outcome.
    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo@other!peer-relay".to_string(),
            lines: Some(3),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("cross-relay look should fail");
    assert_eq!(response.code, "runtime_cross_relay_unsupported");
}

#[test]
fn look_rejects_unknown_peer_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    // A peer-qualified target routes by suffix; the non-stream entry point
    // carries an empty catalog, so an unconfigured peer bundle is rejected
    // fail-closed rather than the retired cross-bundle stub.
    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo@other".to_string(),
            lines: Some(3),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");
    assert_eq!(response.code, "validation_unknown_bundle");
}

#[test]
fn look_rejects_out_of_range_lines() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo@party".to_string(),
            lines: Some(1001),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");
    assert_eq!(response.code, "validation_invalid_lines");
}

#[test]
fn look_rejects_offset_for_tmux_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "bravo".to_string(),
            target_session: "bravo@party".to_string(),
            lines: None,
            offset: Some(2),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");
    assert_eq!(response.code, "validation_offset_unsupported");
}

#[test]
fn look_allows_zero_offset_for_tmux_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    // A zero offset (explicit or defaulted) is a valid no-op for tmux targets;
    // only a nonzero offset is rejected. Pane capture fails afterward because no
    // tmux server is running, so the error code must be anything other than the
    // offset gate — proving Some(0) passed the gate rather than tripping it.
    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "bravo".to_string(),
            target_session: "bravo@party".to_string(),
            lines: None,
            offset: Some(0),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail on absent tmux server");
    assert_ne!(response.code, "validation_offset_unsupported");
}

#[test]
fn look_rejects_relay_wide_target_as_unsupported_operation() {
    // A declared relay-wide (`@GLOBAL`) principal is registered offline in the
    // unified registry at startup; look resolves its capability from that entry
    // (`ui` carries `can_be_looked = false`) whether or not it is connected,
    // reported with the target id and the failed capability flag so the
    // diagnostic is actionable.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    agentmux::relay::register_configured_relay_wide_principals(&config_root)
        .expect("register declared relay-wide principals");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "user@GLOBAL".to_string(),
            lines: Some(3),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");

    assert_eq!(response.code, "validation_unsupported_operation");
    let details = response.details.expect("capability details");
    assert_eq!(details["target_session"], "user@GLOBAL");
    assert_eq!(details["can_be_looked"], false);
}

#[test]
fn look_rejects_unconfigured_relay_wide_target_as_unknown() {
    // A `@GLOBAL` target absent from the global users configuration has no
    // session type to derive a capability from and sorts as unknown.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "stranger@GLOBAL".to_string(),
            lines: Some(3),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");

    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn look_rejects_reserved_namespace_target_as_unsupported_namespace() {
    // Reserved namespaces (`@EXTERNAL`/`@RELAY`) name no routable session at
    // all; their routing-stage rejection is unchanged by the capability gate.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "service@EXTERNAL".to_string(),
            lines: Some(3),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");

    assert_eq!(response.code, "validation_unsupported_namespace");
}

#[test]
fn look_rejects_unknown_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "missing@party".to_string(),
            lines: Some(5),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");
    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn look_returns_empty_snapshot_for_acp_target_without_recorded_updates() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_acp_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "bravo".to_string(),
            target_session: "bravo@party".to_string(),
            lines: Some(5),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("look should succeed");
    let RelayResponse::Look {
        snapshot:
            LookSnapshotPayload::StructuredEntriesV1 {
                snapshot_entries, ..
            },
        ..
    } = response
    else {
        panic!("expected look response");
    };
    assert!(snapshot_entries.is_empty());
}

#[test]
fn look_denies_same_bundle_non_self_target_under_default_self_scope() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo@party".to_string(),
            lines: Some(3),
            offset: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");
    assert_eq!(response.code, "authorization_forbidden");
    let details = response.details.expect("authorization details");
    assert_eq!(details["capability"], "look.inspect");
    assert_eq!(details["requester_session"], "alpha");
    assert_eq!(details["namespace"], "party");
    // The routing/authorization spine reports the canonical `<session>@<bundle>`
    // target uniformly (same-bundle and cross-bundle alike).
    assert_eq!(details["target_session"], "bravo@party");
}
