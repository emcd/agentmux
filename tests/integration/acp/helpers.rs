use agentmux::configuration::ConfigurationRoots;
use agentmux::runtime::paths::BundleRuntimePaths;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

pub(super) use agentmux::relay::ChoicesQueueEvent;
use agentmux::relay::{
    RelayRequest, RelayResponse, handle_request, subscribe_choices_queue_events,
    subscribe_worker_readiness,
};
pub(super) use agentmux::transports::WorkerReadinessState;
use serde_json::Value;
use tokio::sync::{broadcast, watch};

fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    tmux_socket: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    let runtime_directory = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    handle_request(request, configuration_roots, bundle_name, runtime_directory)
}

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
    /// Leave the turn in flight forever without spawning a helper process. A
    /// `sleep`-based delay would inherit the child's stdout, so killing the
    /// agent would leave the pipe open and no reader could observe EOF.
    pub(super) never_respond_to_prompt: bool,
    /// Make every agent from this one onward (counting from zero, across the
    /// whole bundle) hang inside `initialize`, by blocking on a fifo that has no
    /// writer until a test opens one.
    ///
    /// Holds a bootstrap in flight so a fence can land while it runs — the state
    /// in which the relay must not report the generation ceased. `Some(0)` hangs
    /// the very first agent, which is a target's *initial* bootstrap; `Some(1)`
    /// leaves the first agent healthy and hangs every respawn after it.
    pub(super) hang_initialize_from_agent: Option<usize>,
    pub(super) update_count: usize,
    pub(super) update_line_prefix: String,
    pub(super) update_after_response: bool,
    pub(super) update_delay_ms: u64,
    pub(super) load_replay_count: usize,
    pub(super) load_replay_line_prefix: String,
    pub(super) request_permission_on_prompt: bool,
    pub(super) disconnect_on_prompt: Option<String>,
    pub(super) configured_session_id: Option<String>,
    pub(super) coder_prime_timeout_ms: Option<u64>,
    pub(super) tool_call_on_prompt: bool,
    pub(super) tool_call_id: String,
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
            coder_prime_timeout_ms: None,
            tool_call_on_prompt: false,
            tool_call_id: "tc-stub-1".to_string(),
        }
    }
}

/// Where the ACP stub appends the pid of every child it spawns, so a test can
/// assert on the fate of the real process rather than on the relay's account of
/// it.
pub(super) fn acp_child_pid_path(root: &Path) -> PathBuf {
    root.join("acp_child_pids.txt")
}

pub(super) fn write_acp_stub(path: &Path) {
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

pub(super) fn as_json_boolean(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

pub(super) fn write_configuration(
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
    ];

    let mut env_toml = String::new();
    for (name, value) in &env_entries {
        let escaped_value = value.replace('\\', "\\\\").replace('"', "\\\"");
        env_toml.push_str(&format!(
            "\n[[coders.environment]]\nname = \"{name}\"\nvalue = \"{escaped_value}\"\n"
        ));
    }

    let command = script_path.display().to_string();
    let coder_timeout_line = options
        .coder_prime_timeout_ms
        .map(|value| format!("prime-timeout-ms = {value}\n"))
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

fn acp_send_request() -> RelayRequest {
    RelayRequest::Send {
        request_id: Some("req-acp".to_string()),
        requester_session: "alpha".to_string(),
        message: "status?".to_string(),
        targets: vec!["bravo@party".to_string()],
        broadcast: false,
        quiet_window_ms: Some(50),
        on_behalf_of: None,
    }
}

/// Starts the bundle, dispatches an async send to `bravo`, then blocks until
/// the persistent ACP worker has acted on the queued task -- its
/// `session/prompt` reached the stub, or the worker settled `unavailable` --
/// so callers can inspect post-delivery side effects deterministically.
pub(super) fn dispatch_send(config_root: &ConfigurationRoots, tmux_socket: &Path) -> RelayResponse {
    let root = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    let log_path = root.join("acp_requests.log");
    let baseline_prompts = count_logged_method(&log_path, "session/prompt");
    let response =
        dispatch_send_result(config_root, tmux_socket).expect("relay request should parse");
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if count_logged_method(&log_path, "session/prompt") > baseline_prompts
            || read_worker_state(root, "bravo").as_deref() == Some("unavailable")
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    response
}

/// Starts the bundle and dispatches an async send to `bravo`, returning the
/// immediate relay response without waiting for the worker to act.
pub(super) fn dispatch_send_result(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    startup_bundle(config_root, tmux_socket)?;
    dispatch_request(acp_send_request(), config_root, "party", tmux_socket)
}

pub(super) fn dispatch_send_without_startup_result(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    dispatch_request(acp_send_request(), config_root, "party", tmux_socket)
}

fn count_logged_method(log_path: &Path, method: &str) -> usize {
    fs::read_to_string(log_path)
        .map(|contents| {
            contents
                .matches(&format!("\"method\":\"{method}\""))
                .count()
        })
        .unwrap_or(0)
}

/// Polls the persistent ACP worker for `target_session` until it converges to
/// the expected readiness state, returning true on success within the timeout.
pub(super) fn wait_for_worker_state(
    root: &Path,
    target_session: &str,
    expected: &str,
    timeout: Duration,
) -> bool {
    wait_for_any_worker_state(root, target_session, &[expected], timeout)
}

/// Polls the persistent ACP worker for `target_session` until it converges to
/// any of the expected readiness states, returning true on success.
pub(super) fn wait_for_any_worker_state(
    root: &Path,
    target_session: &str,
    expected: &[&str],
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(state) = read_worker_state(root, target_session)
            && expected.iter().any(|candidate| state == *candidate)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Subscribes to readiness-state transitions for `bravo` under the default
/// test bundle (`party`). Callers must invoke this BEFORE dispatching the
/// action that triggers the transition; the watch::Receiver yields the
/// current value on first borrow and every subsequent published transition.
pub(super) fn subscribe_bravo_worker_state(
    root: &Path,
) -> watch::Receiver<Option<WorkerReadinessState>> {
    subscribe_worker_readiness("party", root, "bravo")
}

/// Subscribes to permission-queue mutation events for the runtime directory.
/// Subscriptions must be established BEFORE the action that publishes events;
/// the broadcast::Receiver only observes events that arrive after subscribe.
pub(super) fn subscribe_bravo_permission_queue(
    root: &Path,
) -> broadcast::Receiver<ChoicesQueueEvent> {
    subscribe_choices_queue_events(root)
}

/// Polls a watch::Receiver for the ACP worker's readiness state until it
/// matches `expected`. Uses `has_changed` + `borrow_and_update` so it can be
/// called from synchronous test bodies without a tokio runtime; the channel
/// is updated in-process by the producer, so each poll is a cheap atomic
/// read with no filesystem or RPC cost.
pub(super) fn await_acp_worker_state(
    receiver: &mut watch::Receiver<Option<WorkerReadinessState>>,
    expected: WorkerReadinessState,
    timeout: Duration,
) -> bool {
    await_acp_worker_any_state(receiver, &[expected], timeout)
}

pub(super) fn await_acp_worker_any_state(
    receiver: &mut watch::Receiver<Option<WorkerReadinessState>>,
    expected: &[WorkerReadinessState],
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(current) = *receiver.borrow_and_update()
            && expected.contains(&current)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Polls a broadcast::Receiver for the first permission-queue event matching
/// the predicate. Drops non-matching events; treats `Lagged` as a continue
/// (tests should subscribe before the action so lag is not expected).
pub(super) fn await_permission_event<F>(
    receiver: &mut broadcast::Receiver<ChoicesQueueEvent>,
    mut matcher: F,
    timeout: Duration,
) -> bool
where
    F: FnMut(&ChoicesQueueEvent) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                if matcher(&event) {
                    return true;
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return false;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(broadcast::error::TryRecvError::Closed) => return false,
        }
    }
}

/// Maximum wall-clock time `assert_acp_delivery_unavailable` will wait for a
/// failed-stage ACP worker to settle `Unavailable` before panicking. Sized to
/// align with `agentmux::acp::client::ACP_OPERATION_TIMEOUT` so the test
/// budget cannot be exhausted faster than the production bootstrap
/// legitimately can; the previous 10 s value undersized under the
/// `cargo nextest run --release` full-suite load profile (debug-mode full
/// suite and isolated runs in the same wall-time budget stayed clean, so
/// the constraint is load-profile-specific). On exhaustion the panic
/// message carries the observed wall time, last seen watch state, and
/// readiness-poll state so a future release-mode nextest flake can be
/// triaged from the panic alone rather than re-deriving context from
/// relay inscriptions.
pub(super) const ACP_FAIL_LOAD_SETTLE_BUDGET: Duration = Duration::from_secs(30);

/// Asserts that an ACP send to `bravo` does not succeed: either the relay
/// rejects the enqueue outright with `runtime_acp_worker_unavailable`, or it
/// accepts the async dispatch and the persistent worker settles `unavailable`.
///
/// Subscribes to the worker readiness channel BEFORE dispatching so the
/// terminal `Unavailable` transition cannot be missed if it fires before the
/// test code reaches the await. Uses [`ACP_FAIL_LOAD_SETTLE_BUDGET`] (30 s,
/// aligned with `ACP_OPERATION_TIMEOUT`) so the test outlasts any legitimate
/// path the production bootstrap can take; the test thread is parked on the
/// in-process watch channel, so the wider deadline costs nothing in the
/// happy case.
pub(super) fn assert_acp_delivery_unavailable(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) {
    let root = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    let mut receiver = subscribe_bravo_worker_state(root);
    match dispatch_send_result(config_root, tmux_socket) {
        Err(error) => assert_eq!(error.code, "runtime_acp_worker_unavailable"),
        Ok(response) => {
            let _result = send_result(response);
            let started = Instant::now();
            // The level first, the transition second, and the order matters.
            //
            // A watch fires only on transitions, and this receiver is subscribed
            // after the worker may already have reached its terminal state — in
            // which case there is nothing further to observe and waiting on the
            // watch burns the whole budget before failing. Reading the registry
            // is not the weaker check: it reads the same published state, just
            // without requiring that we were listening when it changed.
            //
            // Waiting on the transition alone made this assertion depend on the
            // send being refused synchronously, which depended in turn on the
            // worker being Unavailable at that exact instant rather than
            // mid-respawn — a coincidence of timing, not a property.
            //
            // The watch is still needed for the other ordering, where the worker
            // is mid-respawn at this point and settles shortly after.
            let settled = read_worker_state(root, "bravo").as_deref() == Some("unavailable")
                || await_acp_worker_state(
                    &mut receiver,
                    WorkerReadinessState::Unavailable,
                    ACP_FAIL_LOAD_SETTLE_BUDGET,
                );
            if !settled {
                let elapsed = started.elapsed();
                let last_state_label = (*receiver.borrow())
                    .map(|s| format!("{s:?}"))
                    .unwrap_or_else(|| "None (no transitions published)".to_string());
                let poll_state = read_worker_state(root, "bravo")
                    .unwrap_or_else(|| "None (worker not registered)".to_string());
                panic!(
                    "ACP worker did not settle unavailable within {ACP_FAIL_LOAD_SETTLE_BUDGET:?} \
                     after a failed startup stage; observed wall time {elapsed:?}, last watch \
                     state = {last_state_label}, readiness poll state = {poll_state}"
                );
            }
        }
    }
}

/// Qualifies a bare target id with the fixture bundle namespace, mirroring the
/// client-side fill-in the relay now requires. Already-qualified ids pass
/// through unchanged.
fn qualify_party_target(target_session: &str) -> String {
    if target_session.contains('@') {
        target_session.to_string()
    } else {
        format!("{target_session}@party")
    }
}

pub(super) fn dispatch_look(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
) -> RelayResponse {
    dispatch_look_with_offset(
        config_root,
        tmux_socket,
        requester_session,
        target_session,
        lines,
        None,
    )
}

pub(super) fn dispatch_look_with_offset(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
    offset: Option<usize>,
) -> RelayResponse {
    startup_bundle(config_root, tmux_socket).expect("relay startup should parse");
    dispatch_request(
        RelayRequest::Look {
            requester_session: requester_session.to_string(),
            target_session: qualify_party_target(target_session),
            lines,
            offset,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay look should parse")
}

pub(super) fn dispatch_look_without_startup(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
) -> RelayResponse {
    dispatch_request(
        RelayRequest::Look {
            requester_session: requester_session.to_string(),
            target_session: qualify_party_target(target_session),
            lines,
            offset: None,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay look should parse")
}

pub(super) fn dispatch_raww(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    text: &str,
    no_enter: bool,
) -> RelayResponse {
    startup_bundle(config_root, tmux_socket).expect("relay startup should parse");
    dispatch_request(
        RelayRequest::Raww {
            request_id: Some("req-acp-raww".to_string()),
            requester_session: requester_session.to_string(),
            target_session: qualify_party_target(target_session),
            text: text.to_string(),
            no_enter,
            on_behalf_of: None,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay raww should parse")
}

fn startup_bundle(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) -> Result<(), agentmux::relay::RelayError> {
    ensure_fast_respawn_for_tests();
    let runtime_directory = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    // Built rather than resolved: this fixture flattens the layout, putting the
    // socket directly in the temporary directory instead of under
    // `bundles/party/`. These tests exercise ACP worker lifecycle, not path
    // resolution, so the paths are stated to match the fixture and the state
    // root coincides with the runtime directory.
    let paths = BundleRuntimePaths {
        state_root: runtime_directory.to_path_buf(),
        bundle_name: "party".to_string(),
        runtime_directory: runtime_directory.to_path_buf(),
        tmux_socket: tmux_socket.to_path_buf(),
    };
    let _ = agentmux::relay::startup_bundle(config_root, &paths)?;
    Ok(())
}

static FAST_RESPAWN_INIT: OnceLock<()> = OnceLock::new();

// Shrinks the ACP respawn backoff cap so integration tests do not have to
// wait the full 1s initial backoff. Idempotent via OnceLock; the `unsafe`
// block is required by Rust 2024's `std::env::set_var` signature, and the
// OnceLock guard ensures this runs before any worker thread consults the
// variable on a respawn path.
fn ensure_fast_respawn_for_tests() {
    FAST_RESPAWN_INIT.get_or_init(|| unsafe {
        std::env::set_var("AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS", "50");
    });
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
    agentmux::relay::read_worker_readiness("party", root, target_session).map(ToString::to_string)
}

pub(super) fn send_result(response: RelayResponse) -> agentmux::relay::SendResult {
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    results.into_iter().next().expect("one result")
}
