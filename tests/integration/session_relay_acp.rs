use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use agentmux::relay::{
    ChatDeliveryMode, ChatOutcome, ChatStatus, RelayRequest, RelayResponse, handle_request,
};
use serde_json::Value;
use tempfile::TempDir;

#[derive(Clone, Debug)]
struct AcpStubOptions {
    fail_initialize: bool,
    fail_load: bool,
    load_capability: bool,
    prompt_capability: bool,
    stop_reason: String,
    prompt_delay_sec: u64,
    configured_session_id: Option<String>,
}

impl Default for AcpStubOptions {
    fn default() -> Self {
        Self {
            fail_initialize: false,
            fail_load: false,
            load_capability: true,
            prompt_capability: true,
            stop_reason: "end_turn".to_string(),
            prompt_delay_sec: 0,
            configured_session_id: None,
        }
    }
}

fn write_acp_stub(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

log_file="${ACP_LOG_FILE:?}"
fail_initialize="${FAIL_INITIALIZE:-0}"
fail_load="${FAIL_LOAD:-0}"
load_capability="${LOAD_CAPABILITY:-true}"
prompt_capability="${PROMPT_CAPABILITY:-true}"
stop_reason="${STOP_REASON:-end_turn}"
prompt_delay_sec="${PROMPT_DELAY_SEC:-0}"
new_session_id="${NEW_SESSION_ID:-sess-generated}"

while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log_file"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "${id}" ]; then
    continue
  fi
  case "$line" in
    *'"method":"initialize"'*)
      if [ "$fail_initialize" = "1" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"initialize failed"}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":%s,"promptSession":%s}}}\n' \
          "$id" "$load_capability" "$prompt_capability"
      fi
      ;;
    *'"method":"session/load"'*)
      if [ "$fail_load" = "1" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32001,"message":"load failed"}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
      fi
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"%s"}}\n' "$id" "$new_session_id"
      ;;
    *'"method":"session/prompt"'*)
      if [ "$prompt_delay_sec" != "0" ]; then
        sleep "$prompt_delay_sec"
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"%s"}}\n' "$id" "$stop_reason"
      ;;
  esac
done
"#;
    fs::write(path, script).expect("write ACP stub");
    let mut permissions = fs::metadata(path).expect("stub metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod ACP stub");
}

fn as_json_boolean(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn write_configuration(root: &Path, options: &AcpStubOptions) -> (PathBuf, PathBuf) {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");

    let script_path = root.join("acp_stub.sh");
    let log_path = root.join("acp_requests.log");
    write_acp_stub(&script_path);
    let command = format!(
        "ACP_LOG_FILE={} FAIL_INITIALIZE={} FAIL_LOAD={} LOAD_CAPABILITY={} PROMPT_CAPABILITY={} STOP_REASON={} PROMPT_DELAY_SEC={} NEW_SESSION_ID=sess-generated {}",
        log_path.display(),
        if options.fail_initialize { "1" } else { "0" },
        if options.fail_load { "1" } else { "0" },
        as_json_boolean(options.load_capability),
        as_json_boolean(options.prompt_capability),
        options.stop_reason,
        options.prompt_delay_sec,
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
    if let Some(value) = options.configured_session_id.as_deref() {
        bundle.push_str(format!("coder-session-id = \"{value}\"\n").as_str());
    }
    fs::write(bundles.join("party.toml"), bundle).expect("write bundle");
    (config_root, log_path)
}

fn dispatch_send(
    config_root: &Path,
    tmux_socket: &Path,
    quiescence_timeout_ms: Option<u64>,
) -> RelayResponse {
    handle_request(
        RelayRequest::Chat {
            request_id: Some("req-acp".to_string()),
            sender_session: "alpha".to_string(),
            message: "status?".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay request should parse")
}

fn chat_result(response: RelayResponse) -> (ChatStatus, agentmux::relay::ChatResult) {
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    (status, results.into_iter().next().expect("one result"))
}

#[test]
fn acp_send_selects_session_new_without_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let state_path = temporary
        .path()
        .join("sessions")
        .join("bravo")
        .join("state.json");
    assert!(
        state_path.is_file(),
        "missing state file: {}",
        state_path.display()
    );
    let persisted: Value = serde_json::from_str(
        fs::read_to_string(&state_path)
            .expect("read state file")
            .as_str(),
    )
    .expect("parse state json");
    assert_eq!(persisted["schema_version"], 1);
    assert_eq!(persisted["acp_session_id"], "sess-generated");

    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/new\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_send_uses_persisted_session_id_when_config_id_is_absent() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let first = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let second = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (first_status, first_result) = chat_result(first);
    let (second_status, second_result) = chat_result(second);
    assert_eq!(first_status, ChatStatus::Success);
    assert_eq!(second_status, ChatStatus::Success);
    assert_eq!(first_result.outcome, ChatOutcome::Delivered);
    assert_eq!(second_result.outcome, ChatOutcome::Delivered);

    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert_eq!(
        log.matches("\"method\":\"session/new\"").count(),
        1,
        "log={log}"
    );
    assert_eq!(
        log.matches("\"method\":\"session/load\"").count(),
        1,
        "log={log}"
    );
}

#[test]
fn acp_send_selects_session_load_with_configured_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/new\""), "log={log}");
}

#[test]
fn acp_load_failure_does_not_fallback_to_session_new() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_load: true,
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/new\""), "log={log}");
}

#[test]
fn acp_missing_load_capability_returns_canonical_failure_code_and_details() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        load_capability: false,
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(
        result.reason_code.as_deref(),
        Some("validation_missing_acp_capability")
    );
    let details = result.details.expect("capability details");
    assert_eq!(details["required_capability"], "session/load");
    assert_eq!(details["target_session"], "bravo");
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(!log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_missing_prompt_capability_returns_canonical_failure_code_and_details() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_capability: false,
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(
        result.reason_code.as_deref(),
        Some("validation_missing_acp_capability")
    );
    let details = result.details.expect("capability details");
    assert_eq!(details["required_capability"], "session/prompt");
    assert_eq!(details["target_session"], "bravo");
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(!log.contains("\"method\":\"session/prompt\""), "log={log}");
}

#[test]
fn acp_initialize_failure_returns_canonical_runtime_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_initialize: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(
        result.reason_code.as_deref(),
        Some("runtime_acp_initialize_failed")
    );
    let details = result.details.expect("initialize details");
    assert_eq!(details["target_session"], "bravo");
}

#[test]
fn acp_cancelled_stop_reason_maps_to_failed_reason_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        stop_reason: "cancelled".to_string(),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(result.reason_code.as_deref(), Some("acp_stop_cancelled"));
}

#[test]
fn acp_turn_timeout_maps_to_timeout_outcome_and_reason_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), Some(100));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Timeout);
    assert_eq!(result.reason_code.as_deref(), Some("acp_turn_timeout"));
}

#[test]
fn acp_successful_terminal_stop_reason_has_no_reason_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert_eq!(result.reason_code, None);
    assert_eq!(result.reason, None);
    assert_eq!(result.details, None);
}

#[test]
fn acp_result_serialization_preserves_optional_reason_fields() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        stop_reason: "cancelled".to_string(),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    let encoded = serde_json::to_value(results).expect("serialize results");
    let Value::Array(results) = encoded else {
        panic!("expected array");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["reason_code"], "acp_stop_cancelled");
}
