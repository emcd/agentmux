use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
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
    fail_new: bool,
    fail_prompt: bool,
    load_capability: bool,
    prompt_capability: bool,
    stop_reason: String,
    prompt_delay_sec: u64,
    update_count: usize,
    configured_session_id: Option<String>,
    coder_turn_timeout_ms: Option<u64>,
}

impl Default for AcpStubOptions {
    fn default() -> Self {
        Self {
            fail_initialize: false,
            fail_load: false,
            fail_new: false,
            fail_prompt: false,
            load_capability: true,
            prompt_capability: true,
            stop_reason: "end_turn".to_string(),
            prompt_delay_sec: 0,
            update_count: 0,
            configured_session_id: None,
            coder_turn_timeout_ms: None,
        }
    }
}

fn write_acp_stub(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

log_file="${ACP_LOG_FILE:?}"
fail_initialize="${FAIL_INITIALIZE:-0}"
fail_load="${FAIL_LOAD:-0}"
fail_new="${FAIL_NEW:-0}"
fail_prompt="${FAIL_PROMPT:-0}"
load_capability="${LOAD_CAPABILITY:-true}"
prompt_capability="${PROMPT_CAPABILITY:-true}"
stop_reason="${STOP_REASON:-end_turn}"
prompt_delay_sec="${PROMPT_DELAY_SEC:-0}"
update_count="${UPDATE_COUNT:-0}"
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
      if [ "$fail_new" = "1" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32002,"message":"new failed"}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"%s"}}\n' "$id" "$new_session_id"
      fi
      ;;
    *'"method":"session/prompt"'*)
      if [ "$fail_prompt" = "1" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32003,"message":"prompt failed"}}\n' "$id"
        continue
      fi
      prompt_session_id=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
      if [ -z "$prompt_session_id" ]; then
        prompt_session_id="$new_session_id"
      fi
      count=1
      while [ "$count" -le "$update_count" ]; do
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":[{"type":"text","text":"ACP-LINE-%s"}]}}\n' \
          "$prompt_session_id" "$count"
        count=$((count + 1))
      done
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
        "ACP_LOG_FILE={} FAIL_INITIALIZE={} FAIL_LOAD={} FAIL_NEW={} FAIL_PROMPT={} LOAD_CAPABILITY={} PROMPT_CAPABILITY={} STOP_REASON={} PROMPT_DELAY_SEC={} UPDATE_COUNT={} NEW_SESSION_ID=sess-generated {}",
        log_path.display(),
        if options.fail_initialize { "1" } else { "0" },
        if options.fail_load { "1" } else { "0" },
        if options.fail_new { "1" } else { "0" },
        if options.fail_prompt { "1" } else { "0" },
        as_json_boolean(options.load_capability),
        as_json_boolean(options.prompt_capability),
        options.stop_reason,
        options.prompt_delay_sec,
        options.update_count,
        script_path.display(),
    );

    let escaped_command = command.replace('\\', "\\\\").replace('"', "\\\"");
    let coder_timeout_line = options
        .coder_turn_timeout_ms
        .map(|value| format!("turn-timeout-ms = {value}\n"))
        .unwrap_or_default();
    let coders = format!(
        r#"format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "{escaped_command}"
{coder_timeout_line}
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
    acp_turn_timeout_ms: Option<u64>,
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
            quiescence_timeout_ms: None,
            acp_turn_timeout_ms,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay request should parse")
}

fn dispatch_look(
    config_root: &Path,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
) -> RelayResponse {
    handle_request(
        RelayRequest::Look {
            requester_session: requester_session.to_string(),
            target_session: target_session.to_string(),
            lines,
            bundle_name: None,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay look should parse")
}

fn read_request_log(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read ACP request log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("parse ACP request JSON line"))
        .collect()
}

fn request_by_method<'a>(requests: &'a [Value], method: &str) -> &'a Value {
    requests
        .iter()
        .find(|request| request.get("method").and_then(Value::as_str) == Some(method))
        .unwrap_or_else(|| panic!("missing ACP request for method '{method}'"))
}

fn persisted_state_path(root: &Path, target_session: &str) -> PathBuf {
    root.join("sessions")
        .join(target_session)
        .join("state.json")
}

fn read_worker_state(root: &Path, target_session: &str) -> Option<String> {
    let path = persisted_state_path(root, target_session);
    if !path.exists() {
        return None;
    }
    let value: Value =
        serde_json::from_str(fs::read_to_string(path).expect("read state json").as_str())
            .expect("parse state json");
    value
        .get("worker_state")
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

    let state_path = persisted_state_path(temporary.path(), "bravo");
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
fn acp_initialize_request_uses_protocol_version_integer_and_client_version() {
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

    let requests = read_request_log(log_path.as_path());
    let initialize = request_by_method(requests.as_slice(), "initialize");
    let params = initialize.get("params").expect("initialize params object");

    assert_eq!(params["protocolVersion"], 1);
    assert_eq!(params["clientInfo"]["name"], "agentmux-relay");
    assert!(
        params["clientInfo"]["version"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(params["clientInfo"].get("title").is_none());
    assert_eq!(params["clientCapabilities"]["terminal"], false);
    assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], false);
    assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], false);
}

#[test]
fn acp_session_setup_requests_include_mcp_servers_array() {
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

    let requests = read_request_log(log_path.as_path());
    let session_new = request_by_method(requests.as_slice(), "session/new");
    assert_eq!(
        session_new["params"]["mcpServers"],
        Value::Array(Vec::new())
    );

    let options = AcpStubOptions {
        configured_session_id: Some("sess-configured".to_string()),
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

    let requests = read_request_log(log_path.as_path());
    let session_load = request_by_method(requests.as_slice(), "session/load");
    assert_eq!(
        session_load["params"]["mcpServers"],
        Value::Array(Vec::new())
    );
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
    assert_eq!(
        result.reason_code.as_deref(),
        Some("runtime_acp_session_load_failed")
    );
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/new\""), "log={log}");
}

#[test]
fn acp_new_failure_returns_runtime_stage_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_new: true,
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
        Some("runtime_acp_session_new_failed")
    );
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
fn acp_prompt_failure_returns_runtime_stage_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_prompt: true,
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
        Some("runtime_acp_prompt_failed")
    );
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
fn acp_turn_timeout_uses_coder_default_when_request_override_is_absent() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_turn_timeout_ms: Some(120),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), None);
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Timeout);
    assert_eq!(result.reason_code.as_deref(), Some("acp_turn_timeout"));
}

#[test]
fn acp_turn_timeout_request_override_takes_precedence_over_coder_default() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_turn_timeout_ms: Some(5_000),
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
fn acp_successful_terminal_stop_reason_marks_accepted_in_progress_phase() {
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
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
}

#[test]
fn acp_first_activity_acceptance_prevents_late_turn_timeout_failure() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), Some(100));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert_eq!(result.reason_code, None);
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
}

#[test]
fn acp_worker_state_transitions_busy_then_available() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let config_root_for_thread = config_root.clone();
    let tmux_socket_for_thread = tmux_socket.clone();
    let handle = thread::spawn(move || {
        dispatch_send(
            &config_root_for_thread,
            &tmux_socket_for_thread,
            Some(2_000),
        )
    });

    let deadline = Instant::now() + Duration::from_millis(800);
    let mut observed_busy = false;
    while Instant::now() < deadline {
        if read_worker_state(temporary.path(), "bravo").as_deref() == Some("busy") {
            observed_busy = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        observed_busy,
        "expected worker_state=busy before completion"
    );

    let response = handle.join().expect("join send thread");
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert_eq!(
        read_worker_state(temporary.path(), "bravo").as_deref(),
        Some("available")
    );
}

#[test]
fn acp_worker_state_transitions_to_unavailable_on_prompt_failure() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_prompt: true,
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
        Some("runtime_acp_prompt_failed")
    );
    assert_eq!(
        read_worker_state(temporary.path(), "bravo").as_deref(),
        Some("unavailable")
    );
}

#[test]
fn acp_look_returns_oldest_to_newest_session_update_lines() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 3,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(3));
    let RelayResponse::Look { snapshot_lines, .. } = look else {
        panic!("expected look response");
    };
    assert_eq!(
        snapshot_lines,
        vec!["ACP-LINE-1", "ACP-LINE-2", "ACP-LINE-3"]
    );
}

#[test]
fn acp_look_enforces_bounded_retention_and_tail_selection() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 1_105,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket, Some(2_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(1_000));
    let RelayResponse::Look { snapshot_lines, .. } = look else {
        panic!("expected look response");
    };
    assert_eq!(snapshot_lines.len(), 1_000);
    assert_eq!(
        snapshot_lines.first().map(String::as_str),
        Some("ACP-LINE-106")
    );
    assert_eq!(
        snapshot_lines.last().map(String::as_str),
        Some("ACP-LINE-1105")
    );

    let tail = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(5));
    let RelayResponse::Look {
        snapshot_lines: tail_lines,
        ..
    } = tail
    else {
        panic!("expected look response");
    };
    assert_eq!(
        tail_lines,
        vec![
            "ACP-LINE-1101".to_string(),
            "ACP-LINE-1102".to_string(),
            "ACP-LINE-1103".to_string(),
            "ACP-LINE-1104".to_string(),
            "ACP-LINE-1105".to_string(),
        ]
    );
}

#[test]
fn acp_look_returns_empty_snapshot_when_no_updates_exist() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(5));
    let RelayResponse::Look { snapshot_lines, .. } = look else {
        panic!("expected look response");
    };
    assert!(snapshot_lines.is_empty());
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
