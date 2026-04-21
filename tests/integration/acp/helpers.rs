use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub(super) use agentmux::relay::ChatDeliveryMode;
use agentmux::relay::{ChatStatus, RelayRequest, RelayResponse, handle_request};
use serde_json::Value;

#[derive(Clone, Debug)]
pub(super) struct AcpStubOptions {
    pub(super) fail_initialize: bool,
    pub(super) fail_load: bool,
    pub(super) fail_new: bool,
    pub(super) fail_prompt: bool,
    pub(super) load_capability: bool,
    pub(super) prompt_capability: bool,
    pub(super) stop_reason: String,
    pub(super) prompt_delay_sec: u64,
    pub(super) update_count: usize,
    pub(super) update_line_prefix: String,
    pub(super) update_after_response: bool,
    pub(super) update_delay_ms: u64,
    pub(super) load_replay_count: usize,
    pub(super) load_replay_line_prefix: String,
    pub(super) request_permission_on_prompt: bool,
    pub(super) disconnect_on_prompt: Option<String>,
    pub(super) configured_session_id: Option<String>,
    pub(super) coder_turn_timeout_ms: Option<u64>,
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
            update_line_prefix: "ACP".to_string(),
            update_after_response: false,
            update_delay_ms: 0,
            load_replay_count: 0,
            load_replay_line_prefix: "ACP-LOAD".to_string(),
            request_permission_on_prompt: false,
            disconnect_on_prompt: None,
            configured_session_id: None,
            coder_turn_timeout_ms: None,
        }
    }
}

pub(super) fn write_acp_stub(path: &Path) {
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
update_line_prefix="${UPDATE_LINE_PREFIX:-ACP}"
update_after_response="${UPDATE_AFTER_RESPONSE:-0}"
update_delay_ms="${UPDATE_DELAY_MS:-0}"
load_replay_count="${LOAD_REPLAY_COUNT:-0}"
load_replay_line_prefix="${LOAD_REPLAY_LINE_PREFIX:-ACP-LOAD}"
new_session_id="${NEW_SESSION_ID:-sess-generated}"
disconnect_on_prompt="${DISCONNECT_ON_PROMPT:-none}"
request_permission_on_prompt="${REQUEST_PERMISSION_ON_PROMPT:-0}"

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
        count=1
        while [ "$count" -le "$load_replay_count" ]; do
          printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":[{"type":"text","text":"%s-LINE-%s"}]}}\n' \
            "$new_session_id" "$load_replay_line_prefix" "$count"
          count=$((count + 1))
        done
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
      if [ "$disconnect_on_prompt" = "before_activity" ]; then
        exit 0
      fi
      prompt_session_id=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
      if [ -z "$prompt_session_id" ]; then
        prompt_session_id="$new_session_id"
      fi
      if [ "$request_permission_on_prompt" = "1" ]; then
        printf '{"jsonrpc":"2.0","method":"session/request_permission","params":{"sessionId":"%s","kind":"exec","description":"need permission"}}\n' \
          "$prompt_session_id"
      fi
      emit_updates() {
        count=1
        while [ "$count" -le "$update_count" ]; do
          printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":[{"type":"text","text":"%s-LINE-%s"}]}}\n' \
            "$prompt_session_id" "$update_line_prefix" "$count"
          count=$((count + 1))
        done
      }
      if [ "$update_after_response" != "1" ]; then
        emit_updates
      fi
      if [ "$disconnect_on_prompt" = "after_activity" ]; then
        exit 0
      fi
      if [ "$prompt_delay_sec" != "0" ]; then
        sleep "$prompt_delay_sec"
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"%s"}}\n' "$id" "$stop_reason"
      if [ "$update_after_response" = "1" ]; then
        if [ "$update_delay_ms" != "0" ]; then
          delay_sec=$(awk -v ms="$update_delay_ms" 'BEGIN { printf "%.3f", ms / 1000 }')
          sleep "$delay_sec"
        fi
        emit_updates
      fi
      ;;
  esac
done
"#;
    fs::write(path, script).expect("write ACP stub");
    let mut permissions = fs::metadata(path).expect("stub metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod ACP stub");
}

pub(super) fn as_json_boolean(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

pub(super) fn write_configuration(root: &Path, options: &AcpStubOptions) -> (PathBuf, PathBuf) {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");

    let script_path = root.join("acp_stub.sh");
    let log_path = root.join("acp_requests.log");
    write_acp_stub(&script_path);

    let env_entries: Vec<(&str, String)> = vec![
        ("ACP_LOG_FILE", log_path.display().to_string()),
        (
            "FAIL_INITIALIZE",
            if options.fail_initialize { "1" } else { "0" }.to_string(),
        ),
        (
            "FAIL_LOAD",
            if options.fail_load { "1" } else { "0" }.to_string(),
        ),
        (
            "FAIL_NEW",
            if options.fail_new { "1" } else { "0" }.to_string(),
        ),
        (
            "FAIL_PROMPT",
            if options.fail_prompt { "1" } else { "0" }.to_string(),
        ),
        (
            "DISCONNECT_ON_PROMPT",
            options
                .disconnect_on_prompt
                .as_deref()
                .unwrap_or("none")
                .to_string(),
        ),
        (
            "REQUEST_PERMISSION_ON_PROMPT",
            if options.request_permission_on_prompt {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        (
            "LOAD_CAPABILITY",
            as_json_boolean(options.load_capability).to_string(),
        ),
        (
            "PROMPT_CAPABILITY",
            as_json_boolean(options.prompt_capability).to_string(),
        ),
        ("STOP_REASON", options.stop_reason.clone()),
        ("PROMPT_DELAY_SEC", options.prompt_delay_sec.to_string()),
        ("UPDATE_COUNT", options.update_count.to_string()),
        ("UPDATE_LINE_PREFIX", options.update_line_prefix.clone()),
        (
            "UPDATE_AFTER_RESPONSE",
            if options.update_after_response {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        ("UPDATE_DELAY_MS", options.update_delay_ms.to_string()),
        ("LOAD_REPLAY_COUNT", options.load_replay_count.to_string()),
        (
            "LOAD_REPLAY_LINE_PREFIX",
            options.load_replay_line_prefix.clone(),
        ),
        ("NEW_SESSION_ID", "sess-generated".to_string()),
    ];

    let mut env_toml = String::new();
    for (name, value) in &env_entries {
        let escaped_value = value.replace('\\', "\\\\").replace('"', "\\\"");
        env_toml.push_str(&format!(
            "\n[[coders.acp.environment]]\nname = \"{name}\"\nvalue = \"{escaped_value}\"\n"
        ));
    }

    let command = script_path.display().to_string();
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
command = "{command}"
{coder_timeout_line}{env_toml}"#
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

pub(super) fn dispatch_send(
    config_root: &Path,
    tmux_socket: &Path,
    acp_turn_timeout_ms: Option<u64>,
) -> RelayResponse {
    dispatch_send_with_mode(
        config_root,
        tmux_socket,
        acp_turn_timeout_ms,
        ChatDeliveryMode::Sync,
    )
}

pub(super) fn dispatch_send_with_mode(
    config_root: &Path,
    tmux_socket: &Path,
    acp_turn_timeout_ms: Option<u64>,
    delivery_mode: ChatDeliveryMode,
) -> RelayResponse {
    dispatch_send_with_mode_result(config_root, tmux_socket, acp_turn_timeout_ms, delivery_mode)
        .expect("relay request should parse")
}

pub(super) fn dispatch_send_with_mode_result(
    config_root: &Path,
    tmux_socket: &Path,
    acp_turn_timeout_ms: Option<u64>,
    delivery_mode: ChatDeliveryMode,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    startup_bundle(config_root, tmux_socket)?;
    handle_request(
        RelayRequest::Chat {
            request_id: Some("req-acp".to_string()),
            sender_session: "alpha".to_string(),
            message: "status?".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: None,
            acp_turn_timeout_ms,
        },
        config_root,
        "party",
        tmux_socket,
    )
}

pub(super) fn dispatch_send_without_startup_result(
    config_root: &Path,
    tmux_socket: &Path,
    acp_turn_timeout_ms: Option<u64>,
    delivery_mode: ChatDeliveryMode,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(
        RelayRequest::Chat {
            request_id: Some("req-acp".to_string()),
            sender_session: "alpha".to_string(),
            message: "status?".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: None,
            acp_turn_timeout_ms,
        },
        config_root,
        "party",
        tmux_socket,
    )
}

pub(super) fn dispatch_look(
    config_root: &Path,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
) -> RelayResponse {
    startup_bundle(config_root, tmux_socket).expect("relay startup should parse");
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

pub(super) fn dispatch_look_without_startup(
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

fn startup_bundle(
    config_root: &Path,
    tmux_socket: &Path,
) -> Result<(), agentmux::relay::RelayError> {
    let _ = agentmux::relay::startup_bundle(config_root, "party", tmux_socket)?;
    Ok(())
}

pub(super) fn read_request_log(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read ACP request log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("parse ACP request JSON line"))
        .collect()
}

pub(super) fn request_by_method<'a>(requests: &'a [Value], method: &str) -> &'a Value {
    requests
        .iter()
        .find(|request| request.get("method").and_then(Value::as_str) == Some(method))
        .unwrap_or_else(|| panic!("missing ACP request for method '{method}'"))
}

pub(super) fn request_count_by_method(requests: &[Value], method: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.get("method").and_then(Value::as_str) == Some(method))
        .count()
}

pub(super) fn persisted_state_path(root: &Path, target_session: &str) -> PathBuf {
    root.join("sessions")
        .join(target_session)
        .join("state.json")
}

pub(super) fn read_worker_state(root: &Path, target_session: &str) -> Option<String> {
    let path = persisted_state_path(root, target_session);
    if !path.exists() {
        return None;
    }
    let raw = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let value: Value = match serde_json::from_str(raw.as_str()) {
        Ok(value) => value,
        Err(_) => return None,
    };
    value
        .get("worker_state")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub(super) fn chat_result(response: RelayResponse) -> (ChatStatus, agentmux::relay::ChatResult) {
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    (status, results.into_iter().next().expect("one result"))
}
