use agentmux::relay::{ChatDeliveryMode, RelayRequest, RelayResponse, handle_request};
use tempfile::TempDir;

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
list = "all:home"
look = "self"
send = "all:home"
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
list = "all:home"
look = "self"
send = "all:home"
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
list = "all:home"
look = "self"
send = "all:home"
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
fn list_excludes_sender_session() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("list response");

    let RelayResponse::List { recipients, .. } = response else {
        panic!("expected list response");
    };
    assert_eq!(recipients.len(), 1);
    assert_eq!(recipients[0].session_name, "bravo");
}

#[test]
fn chat_rejects_unknown_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::Chat {
            request_id: None,
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["missing".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: None,
            quiescence_timeout_ms: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("chat should fail");
    assert_eq!(response.code, "validation_unknown_recipient");
}

#[test]
fn chat_accepts_target_by_recipient_name() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::Chat {
            request_id: None,
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["Bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(1),
            quiescence_timeout_ms: Some(1),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("chat response");

    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "bravo");
}

#[test]
fn chat_broadcast_excludes_sender_session() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::Chat {
            request_id: None,
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: Vec::new(),
            broadcast: true,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(1),
            quiescence_timeout_ms: Some(1),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("chat response");

    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "bravo");
}

#[test]
fn chat_async_returns_accepted_and_queued_outcome() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::Chat {
            request_id: None,
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: Some(1),
            quiescence_timeout_ms: Some(1),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("chat response");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, agentmux::relay::ChatStatus::Accepted);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "bravo");
    assert_eq!(results[0].outcome, agentmux::relay::ChatOutcome::Queued);
}

#[test]
fn chat_rejects_zero_timeout_override() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::Chat {
            request_id: None,
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(1),
            quiescence_timeout_ms: Some(0),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("chat should fail");
    assert_eq!(response.code, "validation_invalid_quiescence_timeout");
}

#[test]
fn chat_broadcast_with_only_sender_returns_empty_results() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_single_member_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let sync_response = handle_request(
        RelayRequest::Chat {
            request_id: None,
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: Vec::new(),
            broadcast: true,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(1),
            quiescence_timeout_ms: Some(1),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("sync chat response");

    let RelayResponse::Chat {
        status: sync_status,
        results: sync_results,
        ..
    } = sync_response
    else {
        panic!("expected sync chat response");
    };
    assert_eq!(sync_status, agentmux::relay::ChatStatus::Success);
    assert!(sync_results.is_empty());

    let async_response = handle_request(
        RelayRequest::Chat {
            request_id: None,
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: Vec::new(),
            broadcast: true,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: Some(1),
            quiescence_timeout_ms: Some(1),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("async chat response");

    let RelayResponse::Chat {
        status: async_status,
        results: async_results,
        ..
    } = async_response
    else {
        panic!("expected async chat response");
    };
    assert_eq!(async_status, agentmux::relay::ChatStatus::Accepted);
    assert!(async_results.is_empty());
}

#[test]
fn look_rejects_cross_bundle_scope() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = handle_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            lines: Some(3),
            bundle_name: Some("other".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");
    assert_eq!(response.code, "validation_cross_bundle_unsupported");
}

#[test]
fn look_rejects_out_of_range_lines() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = handle_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            lines: Some(1001),
            bundle_name: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("look should fail");
    assert_eq!(response.code, "validation_invalid_lines");
}

#[test]
fn look_rejects_unknown_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = handle_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "missing".to_string(),
            lines: Some(5),
            bundle_name: None,
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

    let response = handle_request(
        RelayRequest::Look {
            requester_session: "bravo".to_string(),
            target_session: "bravo".to_string(),
            lines: Some(5),
            bundle_name: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("look should succeed");
    let RelayResponse::Look { snapshot_lines, .. } = response else {
        panic!("expected look response");
    };
    assert!(snapshot_lines.is_empty());
}

#[test]
fn look_denies_same_bundle_non_self_target_under_default_self_scope() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            lines: Some(3),
            bundle_name: None,
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
    assert_eq!(details["bundle_name"], "party");
    assert_eq!(details["target_session"], "bravo");
}

#[test]
fn request_rejects_missing_policy_artifact() {
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
"#,
        None,
    );
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("request should fail");
    assert_eq!(response.code, "validation_invalid_arguments");
}

#[test]
fn request_rejects_invalid_policy_artifact() {
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
"#,
        Some("not = [valid"),
    );
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("request should fail");
    assert_eq!(response.code, "validation_invalid_arguments");
}

#[test]
fn request_rejects_unknown_session_policy_reference() {
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
policy = "missing"
"#,
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "all:home"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = handle_request(
        RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("request should fail");
    assert_eq!(response.code, "validation_invalid_arguments");
}
