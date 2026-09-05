use agentmux::configuration::ConfigurationRoots;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub(in crate::acp) struct AcpStubOptions {
    pub(in crate::acp) fail_initialize: bool,
    pub(in crate::acp) fail_load: bool,
    pub(in crate::acp) fail_new: bool,
    pub(in crate::acp) fail_prompt: bool,
    pub(in crate::acp) load_capability: bool,
    pub(in crate::acp) prompt_capability: bool,
    pub(in crate::acp) stop_reason: String,
    pub(in crate::acp) prompt_delay_sec: u64,
    /// Leave the turn in flight forever without spawning a helper process. A
    /// `sleep`-based delay would inherit the child's stdout, so killing the
    /// agent would leave the pipe open and no reader could observe EOF.
    pub(in crate::acp) never_respond_to_prompt: bool,
    /// Make every agent from this one onward (counting from zero, across the
    /// whole bundle) hang inside `initialize`, by blocking on a fifo that has no
    /// writer until a test opens one.
    ///
    /// Holds a bootstrap in flight so a fence can land while it runs — the state
    /// in which the relay must not report the generation ceased. `Some(0)` hangs
    /// the very first agent, which is a target's *initial* bootstrap; `Some(1)`
    /// leaves the first agent healthy and hangs every respawn after it.
    pub(in crate::acp) hang_initialize_from_agent: Option<usize>,
    pub(in crate::acp) update_count: usize,
    pub(in crate::acp) update_line_prefix: String,
    pub(in crate::acp) update_after_response: bool,
    pub(in crate::acp) update_delay_ms: u64,
    pub(in crate::acp) load_replay_count: usize,
    pub(in crate::acp) load_replay_line_prefix: String,
    pub(in crate::acp) request_permission_on_prompt: bool,
    pub(in crate::acp) disconnect_on_prompt: Option<String>,
    pub(in crate::acp) configured_session_id: Option<String>,
    pub(in crate::acp) tool_call_on_prompt: bool,
    pub(in crate::acp) tool_call_id: String,
    /// Stop draining stdin once `session/new` has been answered, while staying
    /// alive and holding the pipe open.
    ///
    /// This is the only state that parks the relay's own executor mid-write: the
    /// agent is healthy, the target is reachable, and the relay is blocked in a
    /// `write_line_to_stdin` whose pipe buffer has filled. Every other stall the
    /// stub can produce is the agent being slow to *answer*, which resolves at the
    /// framed write and so never reaches the execution watchdog at all.
    ///
    /// Blocks on a fifo rather than sleeping, for the reason the initialize hang
    /// does: a `sleep` subprocess inherits the agent's stdout, so killing the
    /// agent would leave that pipe open and the relay's reader would never observe
    /// EOF — turning a positive fence verdict into a negative one for a reason
    /// belonging entirely to the harness.
    pub(in crate::acp) stop_reading_stdin_after_new: bool,
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
            never_respond_to_prompt: false,
            hang_initialize_from_agent: None,
            update_count: 0,
            update_line_prefix: "ACP".to_string(),
            update_after_response: false,
            update_delay_ms: 0,
            load_replay_count: 0,
            load_replay_line_prefix: "ACP-LOAD".to_string(),
            request_permission_on_prompt: false,
            disconnect_on_prompt: None,
            configured_session_id: None,
            tool_call_on_prompt: false,
            tool_call_id: "tc-stub-1".to_string(),
            stop_reading_stdin_after_new: false,
        }
    }
}

/// Where the ACP stub appends the pid of every child it spawns, so a test can
/// assert on the fate of the real process rather than on the relay's account of
/// it.
pub(in crate::acp) fn acp_child_pid_path(root: &Path) -> PathBuf {
    root.join("acp_child_pids.txt")
}

pub(in crate::acp) fn write_acp_stub(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

log_file="${ACP_LOG_FILE:?}"
pid_file="${ACP_PID_FILE:-}"
session="${AGENTMUX_SESSION:-unknown}"
# How many agents THIS target has already started. Counted per session, not
# across the file: every member of the bundle appends here, so a global count
# makes "the second agent" mean whichever member happened to start second, and
# a member's *initial* bootstrap can be mistaken for a respawn.
prior_agents=0
if [ -n "$pid_file" ]; then
  if [ -f "$pid_file" ]; then
    prior_agents=$(grep -c "^${session} " "$pid_file" || true)
  fi
  printf '%s %s\n' "$session" "$$" >> "$pid_file"
fi
fail_initialize="${FAIL_INITIALIZE:-0}"
fail_load="${FAIL_LOAD:-0}"
fail_new="${FAIL_NEW:-0}"
fail_prompt="${FAIL_PROMPT:-0}"
load_capability="${LOAD_CAPABILITY:-true}"
prompt_capability="${PROMPT_CAPABILITY:-true}"
stop_reason="${STOP_REASON:-end_turn}"
prompt_delay_sec="${PROMPT_DELAY_SEC:-0}"
never_respond_to_prompt="${NEVER_RESPOND_TO_PROMPT:-0}"
hang_initialize_fifo="${ACP_HANG_INITIALIZE_FIFO:-}"
hang_initialize_from_agent="${ACP_HANG_INITIALIZE_FROM_AGENT:-}"
update_count="${UPDATE_COUNT:-0}"
update_line_prefix="${UPDATE_LINE_PREFIX:-ACP}"
update_after_response="${UPDATE_AFTER_RESPONSE:-0}"
update_delay_ms="${UPDATE_DELAY_MS:-0}"
load_replay_count="${LOAD_REPLAY_COUNT:-0}"
load_replay_line_prefix="${LOAD_REPLAY_LINE_PREFIX:-ACP-LOAD}"
new_session_id="${NEW_SESSION_ID:-sess-generated}"
disconnect_on_prompt="${DISCONNECT_ON_PROMPT:-none}"
request_permission_on_prompt="${REQUEST_PERMISSION_ON_PROMPT:-0}"
tool_call_on_prompt="${TOOL_CALL_ON_PROMPT:-0}"
tool_call_id="${TOOL_CALL_ID:-tc-stub-1}"
stop_reading_stdin_after_new="${STOP_READING_STDIN_AFTER_NEW:-0}"
stop_reading_fifo="${ACP_STOP_READING_FIFO:-}"

while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log_file"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "${id}" ]; then
    continue
  fi
  case "$line" in
    *'"method":"initialize"'*)
      if [ -n "$hang_initialize_from_agent" ] && [ "$prior_agents" -ge "$hang_initialize_from_agent" ]; then
        [ -p "$hang_initialize_fifo" ] || mkfifo "$hang_initialize_fifo"
        read hung < "$hang_initialize_fifo" || true
      fi
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
          printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":[{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"%s-LINE-%s"}}]}}\n' \
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
      if [ "$stop_reading_stdin_after_new" = "1" ]; then
        # Alive, healthy, and no longer draining stdin. Blocking in-process on a
        # fifo with no writer, so nothing inherits stdout and killing this shell
        # closes the pipe the relay's reader is watching.
        [ -p "$stop_reading_fifo" ] || mkfifo "$stop_reading_fifo"
        read stopped < "$stop_reading_fifo" || true
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
      if [ "$never_respond_to_prompt" = "1" ]; then
        continue
      fi
      prompt_session_id=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
      if [ -z "$prompt_session_id" ]; then
        prompt_session_id="$new_session_id"
      fi
      if [ "$request_permission_on_prompt" = "1" ]; then
        choice_request_id=$((id + 1000000))
        printf '{"jsonrpc":"2.0","id":%s,"method":"session/request_permission","params":{"sessionId":"%s","kind":"exec","description":"need permission","options":[{"optionId":"allow","name":"Allow","kind":"allow"}]}}\n' \
          "$choice_request_id" "$prompt_session_id"
      fi
      emit_updates() {
        count=1
        while [ "$count" -le "$update_count" ]; do
          printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":[{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"%s-LINE-%s"}}]}}\n' \
            "$prompt_session_id" "$update_line_prefix" "$count"
          count=$((count + 1))
        done
      }
      emit_tool_call_lifecycle() {
        # Emit a single tool_call notification followed by its terminal
        # tool_call_update. The reader-thread parser mutates the
        # Pending Invocation entry in place on the update, so the buffer
        # ends up holding exactly one Invocation entry per call_id
        # (not two). Used by the
        # `replace-pending-completed-tool-call-in-place` proposal tests.
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":[{"sessionUpdate":"tool_call","toolCallId":"%s","title":"stub-tool","kind":"exec"}]}}\n' \
          "$prompt_session_id" "$tool_call_id"
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":[{"sessionUpdate":"tool_call_update","toolCallId":"%s","status":"completed","result":{"ok":true}}]}}\n' \
          "$prompt_session_id" "$tool_call_id"
      }
      if [ "$update_after_response" != "1" ]; then
        if [ "$tool_call_on_prompt" = "1" ]; then
          emit_tool_call_lifecycle
        else
          emit_updates
        fi
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

pub(in crate::acp) fn as_json_boolean(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

pub(in crate::acp) fn write_configuration(
    root: &Path,
    options: &AcpStubOptions,
) -> (ConfigurationRoots, PathBuf) {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");

    let script_path = root.join("acp_stub.sh");
    let log_path = root.join("acp_requests.log");
    write_acp_stub(&script_path);

    let env_entries: Vec<(&str, String)> = vec![
        ("ACP_LOG_FILE", log_path.display().to_string()),
        (
            "ACP_PID_FILE",
            acp_child_pid_path(root).display().to_string(),
        ),
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
        (
            "ACP_HANG_INITIALIZE_FIFO",
            root.join("acp_hang_initialize.fifo").display().to_string(),
        ),
        (
            "ACP_HANG_INITIALIZE_FROM_AGENT",
            options
                .hang_initialize_from_agent
                .map(|from| from.to_string())
                .unwrap_or_default(),
        ),
        (
            "NEVER_RESPOND_TO_PROMPT",
            if options.never_respond_to_prompt {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
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
        (
            "TOOL_CALL_ON_PROMPT",
            if options.tool_call_on_prompt {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        ("TOOL_CALL_ID", options.tool_call_id.clone()),
        (
            "STOP_READING_STDIN_AFTER_NEW",
            if options.stop_reading_stdin_after_new {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        (
            "ACP_STOP_READING_FIFO",
            root.join("acp_stop_reading.fifo").display().to_string(),
        ),
    ];

    let mut env_toml = String::new();
    for (name, value) in &env_entries {
        let escaped_value = value.replace('\\', "\\\\").replace('"', "\\\"");
        env_toml.push_str(&format!(
            "\n[[coders.environment]]\nname = \"{name}\"\nvalue = \"{escaped_value}\"\n"
        ));
    }

    let command = script_path.display().to_string();
    let coders = format!(
        r#"format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "{command}"
{env_toml}"#
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
list = "home"
look = "self"
raww = "home"
send = "home"
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
    (ConfigurationRoots::single(config_root), log_path)
}

pub(super) static FAST_RESPAWN_INIT: OnceLock<()> = OnceLock::new();
pub(in crate::acp) fn ensure_fast_respawn_for_tests() {
    FAST_RESPAWN_INIT.get_or_init(|| unsafe {
        std::env::set_var("AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS", "50");
    });
}
