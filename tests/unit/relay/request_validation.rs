use super::*;

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
    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("alpha".to_string()),
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
    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("alpha".to_string()),
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
policy = "missing"
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
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("request should fail");
    assert_eq!(response.code, "validation_invalid_arguments");
}

#[test]
fn raww_policy_rejects_invalid_scope_value() {
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
raww = "all:everything"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("policy validation should fail");

    assert_eq!(response.code, "validation_invalid_policy_scope");
}

#[test]
fn choose_policy_rejects_unknown_scope_value() {
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
choose = "everywhere"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("policy validation should fail");

    assert_eq!(response.code, "validation_invalid_policy_scope");
}

// The policies file is authoritative: every control accepts the full
// none/self/home/all ladder. The fixture pins each control to a value the
// retired per-control caps used to reject (choose/updown at all, send at
// self, new/change at self), so a parse-time rejection would resurface here.
#[test]
fn policy_controls_accept_full_scope_ladder() {
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
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "none"
list = "all"
look = "none"
send = "self"
raww = "none"
choose = "all"
updown = "all"

[policies.controls.new]
peer = "self"

[policies.controls.change]
psk = "self"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::List {
            requester_session: Some("alpha".to_string()),
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("dispatch should accept every configured scope value");

    assert!(matches!(response, RelayResponse::List { .. }));
}
