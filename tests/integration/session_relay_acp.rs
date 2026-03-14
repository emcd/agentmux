use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use agentmux::relay::{
    ChatDeliveryMode, ChatOutcome, ChatStatus, RelayRequest, RelayResponse, handle_request,
};
use tempfile::TempDir;

fn write_acp_stub(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

log_file="${ACP_LOG_FILE:?}"
fail_load="${FAIL_LOAD:-0}"

while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log_file"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "${id}" ]; then
    continue
  fi
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}\n' "$id"
      ;;
    *'"method":"session/load"'*)
      if [ "$fail_load" = "1" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32001,"message":"load failed"}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
      fi
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-generated"}}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
      ;;
  esac
done
"#;
    fs::write(path, script).expect("write ACP stub");
    let mut permissions = fs::metadata(path).expect("stub metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod ACP stub");
}

fn write_configuration(
    root: &Path,
    fail_load: bool,
    target_session_id: Option<&str>,
) -> (PathBuf, PathBuf) {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");

    let script_path = root.join("acp_stub.sh");
    let log_path = root.join("acp_requests.log");
    write_acp_stub(&script_path);
    let command = format!(
        "ACP_LOG_FILE={} FAIL_LOAD={} {}",
        log_path.display(),
        if fail_load { "1" } else { "0" },
        script_path.display(),
    );

    let escaped_command = command.replace('\\', "\\\\").replace('"', "\\\"");
    let coders = format!(
        r#"format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "{escaped_command}"
"#
    );
    fs::write(config_root.join("coders.toml"), coders).expect("write coders");

    fs::write(
        config_root.join("policies.toml"),
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
    .expect("write policies");

    let mut bundle = format!(
        r#"format-version = 1

[[sessions]]
id = "alpha"
name = "alpha"
directory = "{}"
coder = "acp"

[[sessions]]
id = "bravo"
name = "bravo"
directory = "{}"
coder = "acp"
"#,
        root.display(),
        root.display()
    );
    if let Some(value) = target_session_id {
        bundle.push_str(format!("coder-session-id = \"{value}\"\n").as_str());
    }
    fs::write(bundles.join("party.toml"), bundle).expect("write bundle");
    (config_root, log_path)
}

fn dispatch_send(config_root: &Path, tmux_socket: &Path) -> RelayResponse {
    handle_request(
        RelayRequest::Chat {
            request_id: Some("req-acp".to_string()),
            sender_session: "alpha".to_string(),
            message: "status?".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: Some(1_000),
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay request should parse")
}

#[test]
fn acp_send_selects_session_new_without_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, log_path) = write_configuration(temporary.path(), false, None);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/new\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_send_selects_session_load_with_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, log_path) = write_configuration(temporary.path(), false, Some("sess-abc"));
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/new\""), "log={log}");
}

#[test]
fn acp_load_failure_does_not_fallback_to_session_new() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, log_path) = write_configuration(temporary.path(), true, Some("sess-abc"));
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Failed);
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/new\""), "log={log}");
}
