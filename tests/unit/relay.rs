use agentmux::relay::{
    ListedBundleState, ListedSessionTransport, LookSnapshotPayload, RelayRequest, RelayResponse,
    handle_request,
};
use tempfile::TempDir;

#[path = "relay/error.rs"]
mod error;
#[path = "relay/peer_connection.rs"]
mod peer_connection;
#[path = "relay/preflight.rs"]
mod preflight;
#[path = "relay/principal_store.rs"]
mod principal_store;
#[path = "relay/raww.rs"]
mod raww;
#[path = "relay/relay_configuration.rs"]
mod relay_configuration;
#[path = "relay/request_validation.rs"]
mod request_validation;

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &std::path::Path,
    bundle_name: &str,
    tmux_socket: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    let runtime_directory = tmux_socket
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    handle_request(request, configuration_root, bundle_name, runtime_directory)
}

fn write_tui_configuration(root: &std::path::Path, policy: &str) {
    std::fs::write(
        root.join("users.toml"),
        format!(
            r#"
default-bundle = "party"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "{policy}"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
}

fn write_tui_configuration_with_session_id(root: &std::path::Path, policy: &str, session_id: &str) {
    std::fs::write(
        root.join("users.toml"),
        format!(
            r#"
default-bundle = "party"
default-session = "{session_id}"

[[sessions]]
id = "{session_id}"
policy = "{policy}"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
}

fn write_bundle(temporary: &TempDir, name: &str) -> std::path::PathBuf {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders file");
    std::fs::write(
        root.join("policies.toml"),
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

# A privileged cross-namespace operator: a relay-wide principal's home is the
# GLOBAL namespace, so reaching into a bundle requires all.
[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "all"
look = "all"
send = "all"
"#,
    )
    .expect("write policies file");
    let body = r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#;
    std::fs::write(bundles.join(format!("{name}.toml")), body).expect("write bundle file");
    root
}

fn write_single_member_bundle(temporary: &TempDir, name: &str) -> std::path::PathBuf {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders file");
    std::fs::write(
        root.join("policies.toml"),
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
    )
    .expect("write policies file");
    let body = r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"
"#;
    std::fs::write(bundles.join(format!("{name}.toml")), body).expect("write bundle file");
    root
}

fn write_acp_bundle(temporary: &TempDir, name: &str) -> std::path::PathBuf {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "sh -lc 'cat >/dev/null'"
"#,
    )
    .expect("write coders file");
    std::fs::write(
        root.join("policies.toml"),
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

# Privileged cross-namespace operator (see write_bundle).
[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "all"
look = "all"
send = "all"
"#,
    )
    .expect("write policies file");
    let body = r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "acp"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "acp"
"#;
    std::fs::write(bundles.join(format!("{name}.toml")), body).expect("write bundle file");
    root
}

fn write_bundle_with_policy(
    temporary: &TempDir,
    name: &str,
    bundle_body: &str,
    policy_body: Option<&str>,
) -> std::path::PathBuf {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders file");
    if let Some(policy_body) = policy_body {
        std::fs::write(root.join("policies.toml"), policy_body).expect("write policies file");
    }
    std::fs::write(bundles.join(format!("{name}.toml")), bundle_body).expect("write bundle file");
    root
}

#[test]
fn list_returns_all_configured_sessions_with_transport() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("list response");

    let RelayResponse::List { bundle, .. } = response else {
        panic!("expected list response");
    };
    assert_eq!(bundle.id, "party");
    assert!(
        !bundle.hosted,
        "tmux-bearing bundle without owned sessions is not hosted"
    );
    assert_eq!(bundle.state, ListedBundleState::Down);
    assert!(bundle.startup_health.is_none());
    assert_eq!(
        bundle.state_reason_code.as_deref(),
        Some("runtime_startup_failed")
    );
    assert_eq!(bundle.startup_failure_count, 0);
    assert!(bundle.recent_startup_failures.is_empty());
    assert_eq!(bundle.principals.len(), 2);
    assert_eq!(bundle.principals[0].id, "alpha@party");
    assert_eq!(bundle.principals[0].transport, ListedSessionTransport::Tmux);
    assert!(
        !bundle.principals[0].ready,
        "tmux session without resolvable pane reports ready=false"
    );
    assert_eq!(bundle.principals[1].id, "bravo@party");
    assert_eq!(bundle.principals[1].transport, ListedSessionTransport::Tmux);
    assert!(!bundle.principals[1].ready);
}

#[test]
fn list_allows_ui_sender_from_global_tui_sessions() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    // A relay-wide operator's home is GLOBAL, so listing a bundle is
    // cross-namespace and requires all (the privileged operator preset).
    write_tui_configuration(&config_root, "operator");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("user@GLOBAL".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("list response");

    let RelayResponse::List { bundle, .. } = response else {
        panic!("expected list response");
    };
    assert!(
        !bundle.hosted,
        "tmux-bearing bundle without owned sessions is not hosted"
    );
    assert_eq!(bundle.state, ListedBundleState::Down);
    assert!(bundle.startup_health.is_none());
    assert_eq!(bundle.startup_failure_count, 0);
    assert_eq!(bundle.principals.len(), 2);
    assert_eq!(bundle.principals[0].id, "alpha@party");
    assert_eq!(bundle.principals[1].id, "bravo@party");
}

#[test]
fn list_rejects_ui_sender_with_unknown_policy_reference() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "missing");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("user@GLOBAL".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("list should fail");
    assert_eq!(response.code, "validation_unknown_policy");
}

#[test]
fn list_reports_down_when_no_acp_worker_registered() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_acp_bundle(&temporary, "party");
    // Cross-namespace list from a GLOBAL operator requires all.
    write_tui_configuration(&config_root, "operator");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("user@GLOBAL".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("list response should not fail");

    let RelayResponse::List { bundle, .. } = response else {
        panic!("expected list response");
    };
    assert!(
        !bundle.hosted,
        "acp-only bundle with no ready worker is not hosted"
    );
    assert_eq!(bundle.state, ListedBundleState::Down);
    assert!(bundle.startup_health.is_none());
    assert_eq!(bundle.principals.len(), 2);
    assert!(
        bundle.principals.iter().all(|session| !session.ready),
        "every acp session without a registered worker reports ready=false"
    );
}

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
    assert_eq!(results[0].outcome, agentmux::relay::SendOutcome::Queued);
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
    assert_eq!(results[0].outcome, agentmux::relay::SendOutcome::Queued);
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
fn send_rejects_unrouted_cross_relay_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    // A bang-path target is classified as cross-relay from the address alone
    // (config-free). Until the outbound peer connection module lands it is
    // rejected with a typed, honest outcome rather than a misleading
    // unknown-bundle error.
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@other!peer-relay".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("cross-relay send should fail until forwarding lands");
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

#[test]
fn look_rejects_cross_relay_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

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
    assert_eq!(response.code, "runtime_cross_relay_unavailable");
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
