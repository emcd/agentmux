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
            target_session: "missing@party".to_string(),
            text: "hello".to_string(),
            no_enter: false,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should fail");

    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn raww_rejects_relay_wide_target_as_unsupported_operation() {
    // A relay-wide (`@GLOBAL`) target resolves through routing; the rejection
    // is the capability gate on its configured session type (`ui` carries
    // `can_be_written = false`), reported with the target id and the failed
    // capability flag so the diagnostic is actionable.
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
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should fail");

    assert_eq!(response.code, "validation_unsupported_operation");
    let details = response.details.expect("capability details");
    assert_eq!(details["target_session"], "ui@GLOBAL");
    assert_eq!(details["can_be_written"], false);
}

#[test]
fn raww_rejects_reserved_namespace_target_as_unsupported_namespace() {
    // Reserved namespaces (`@EXTERNAL`/`@RELAY`) name no routable session at
    // all; their routing-stage rejection is unchanged by the capability gate.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: None,
            requester_session: "alpha".to_string(),
            target_session: "service@RELAY".to_string(),
            text: "hello".to_string(),
            no_enter: false,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("raww should fail");

    assert_eq!(response.code, "validation_unsupported_namespace");
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
            target_session: "bravo@party".to_string(),
            text,
            no_enter: false,
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
list = "home"
look = "self"
send = "home"
raww = "self"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: None,
            requester_session: "alpha".to_string(),
            target_session: "bravo@party".to_string(),
            text: "hello".to_string(),
            no_enter: false,
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
    // Raww now authorizes through the shared routing spine, which reports the
    // target with its canonical `<session>@<bundle>` identifier (as Send/Look do).
    assert_eq!(details["target_session"], "bravo@party");
}
