use tempfile::TempDir;
use tmuxmux::relay::{RelayRequest, RelayResponse, handle_request};

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
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders file");
    let body = r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "alpha"
display-name = "Alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "bravo"
name = "bravo"
display-name = "Bravo"
directory = "/tmp"
coder = "shell"
"#;
    std::fs::write(bundles.join(format!("{name}.toml")), body).expect("write bundle file");
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
            quiet_window_ms: None,
            delivery_timeout_ms: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("chat should fail");
    assert_eq!(response.code, "validation_unknown_recipient");
}
