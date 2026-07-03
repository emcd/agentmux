//! List operation tests: configured sessions, transport reporting, policy
//! gates.

use agentmux::relay::{ListedBundleState, ListedSessionTransport, RelayRequest, RelayResponse};

use super::*;

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
