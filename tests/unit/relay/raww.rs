use super::*;

#[test]
fn raww_rejects_unknown_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: None,
            requester_session: "alpha".to_string(),
            target_session: "missing".to_string(),
            text: "hello".to_string(),
            no_enter: false,
            bundle_name: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should fail");

    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn raww_rejects_cross_bundle_selector() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: None,
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            text: "hello".to_string(),
            no_enter: false,
            bundle_name: Some("other".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should fail");

    assert_eq!(response.code, "validation_cross_bundle_unsupported");
}

#[test]
fn raww_rejects_ui_target_class() {
    // Raww writes raw text to a tmux pane; a global UI session is rejected as
    // an unsupported target class.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration_with_session_id(&config_root, "default", "ui@GLOBAL");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: None,
            requester_session: "alpha".to_string(),
            target_session: "ui@GLOBAL".to_string(),
            text: "hello".to_string(),
            no_enter: false,
            bundle_name: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should fail");

    assert_eq!(response.code, "validation_invalid_params");
    let details = response.details.expect("details");
    assert_eq!(details["target_class"], "ui");
}

#[test]
fn raww_rejects_oversized_text() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let text = "x".repeat(32 * 1024 + 1);

    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: None,
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            text,
            no_enter: false,
            bundle_name: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should fail");

    assert_eq!(response.code, "validation_invalid_params");
}

#[test]
fn raww_denial_uses_raww_write_capability() {
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
id = "bravo"
directory = "/tmp"
coder = "shell"
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
raww = "self"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: None,
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            text: "hello".to_string(),
            no_enter: false,
            bundle_name: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should be denied");

    assert_eq!(response.code, "authorization_forbidden");
    let details = response.details.expect("authorization details");
    assert_eq!(details["capability"], "raww.write");
    assert_eq!(details["requester_session"], "alpha");
    assert_eq!(details["target_session"], "bravo");
}
