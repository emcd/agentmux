use agentmux::configuration::ConfigurationRoots;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command as StdCommand,
    time::{Duration, Instant},
};

use tokio::{
    io::AsyncBufReadExt,
    process::{Child, Command},
    time::sleep,
};

pub(crate) fn tmux_available() -> bool {
    StdCommand::new("tmux")
        .arg("-V")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub(crate) fn tmux_command(socket: &Path, arguments: &[&str]) -> std::process::Output {
    StdCommand::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(arguments)
        .output()
        .expect("run tmux command")
}

pub(crate) struct TmuxServerGuard {
    socket: PathBuf,
}

impl TmuxServerGuard {
    pub(crate) fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl Drop for TmuxServerGuard {
    fn drop(&mut self) {
        let _ = StdCommand::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(["kill-server"])
            .output();
    }
}

pub(crate) fn wait_for_pane_contains(socket: &Path, target: &str, needle: &str, timeout: Duration) {
    let started = Instant::now();
    loop {
        let output = tmux_command(socket, &["capture-pane", "-p", "-t", target, "-S", "-40"]);
        assert!(
            output.status.success(),
            "failed to capture pane for {target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let snapshot = String::from_utf8_lossy(&output.stdout);
        if snapshot.contains(needle) {
            return;
        }
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for '{needle}' in {target} pane, snapshot={snapshot:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn capture_pane(socket: &Path, target: &str, lines: &str) -> String {
    let output = tmux_command(socket, &["capture-pane", "-p", "-t", target, "-S", lines]);
    assert!(
        output.status.success(),
        "failed to capture pane for {target}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn write_bundle_configuration(
    root: &Path,
    bundle_name: &str,
    sessions: &[&str],
) -> ConfigurationRoots {
    let coders = vec![CoderSpec {
        id: "default".to_string(),
        initial_command: "sh -lc 'exec sleep 45'".to_string(),
        resume_command: "sh -lc 'exec sleep 45'".to_string(),
        prompt_regex: None,
        prompt_inspect_lines: None,
        prompt_idle_column: None,
    }];
    let session_specs = sessions
        .iter()
        .map(|session| SessionSpec {
            id: (*session).to_string(),
            name: Some((*session).to_string()),
            directory: PathBuf::from("/tmp"),
            coder: "default".to_string(),
            coder_session_id: None,
        })
        .collect::<Vec<_>>();
    write_bundle_configuration_members(root, bundle_name, &coders, &session_specs)
}

#[derive(Clone)]
pub(crate) struct CoderSpec {
    pub(crate) id: String,
    pub(crate) initial_command: String,
    pub(crate) resume_command: String,
    pub(crate) prompt_regex: Option<String>,
    pub(crate) prompt_inspect_lines: Option<usize>,
    pub(crate) prompt_idle_column: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct SessionSpec {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) directory: PathBuf,
    pub(crate) coder: String,
    pub(crate) coder_session_id: Option<String>,
}

pub(crate) fn write_bundle_configuration_members(
    root: &Path,
    bundle_name: &str,
    coders: &[CoderSpec],
    sessions: &[SessionSpec],
) -> ConfigurationRoots {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    let mut coders_toml = String::from("format-version = 1\n");
    for coder in coders {
        coders_toml.push_str(
            format!(
                "\n[[coders]]\nid = \"{}\"\n[coders.tmux]\ninitial-command = \"{}\"\nresume-command = \"{}\"\n",
                coder.id, coder.initial_command, coder.resume_command
            )
            .as_str(),
        );
        if let Some(prompt_regex) = coder.prompt_regex.as_deref() {
            coders_toml.push_str(format!("prompt-regex = \"{}\"\n", prompt_regex).as_str());
        }
        if let Some(lines) = coder.prompt_inspect_lines {
            coders_toml.push_str(format!("prompt-inspect-lines = {lines}\n").as_str());
        }
        if let Some(column) = coder.prompt_idle_column {
            coders_toml.push_str(format!("prompt-idle-column = {column}\n").as_str());
        }
    }
    fs::write(config_root.join("coders.toml"), coders_toml).expect("write coders config");
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
    .expect("write policies config");
    fs::write(config_root.join("users.toml"), "").expect("write users config");

    let mut bundle_toml = String::from("format-version = 1\nautostart = true\n");
    for session in sessions {
        bundle_toml.push_str(format!("\n[[sessions]]\nid = \"{}\"\n", session.id).as_str());
        if let Some(name) = session.name.as_deref() {
            bundle_toml.push_str(format!("name = \"{}\"\n", name).as_str());
        }
        bundle_toml.push_str(
            format!(
                "directory = \"{}\"\ncoder = \"{}\"\n",
                session.directory.display(),
                session.coder
            )
            .as_str(),
        );
        if let Some(coder_session_id) = session.coder_session_id.as_deref() {
            bundle_toml.push_str(format!("coder-session-id = \"{}\"\n", coder_session_id).as_str());
        }
    }
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml)
        .expect("write bundle config");
    ConfigurationRoots::single(config_root)
}

/// Writes a bundle with one tmux coder session and one bare `pubsub`-marker
/// session, reusing the standard coders/policies/users config. Used to prove a
/// configured Pubsub member is constructed as the forward-declared stub transport
/// and yields a not-implemented outcome rather than being misrouted to tmux.
pub(crate) fn write_bundle_with_pubsub_member(
    root: &Path,
    bundle_name: &str,
    tmux_session: &str,
    pubsub_session: &str,
) -> ConfigurationRoots {
    let coders = vec![CoderSpec {
        id: "default".to_string(),
        initial_command: "sh -lc 'exec sleep 45'".to_string(),
        resume_command: "sh -lc 'exec sleep 45'".to_string(),
        prompt_regex: None,
        prompt_inspect_lines: None,
        prompt_idle_column: None,
    }];
    let sessions = vec![SessionSpec {
        id: tmux_session.to_string(),
        name: Some(tmux_session.to_string()),
        directory: PathBuf::from("/tmp"),
        coder: "default".to_string(),
        coder_session_id: None,
    }];
    // Reuse the helper to lay down coders/policies/users; then re-write the bundle
    // file to add the bare pubsub session alongside the tmux one.
    let config_root = write_bundle_configuration_members(root, bundle_name, &coders, &sessions);
    let mut bundle_toml = String::from("format-version = 1\nautostart = true\n");
    bundle_toml.push_str(
        format!(
            "\n[[sessions]]\nid = \"{tmux_session}\"\nname = \"{tmux_session}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
        )
        .as_str(),
    );
    bundle_toml.push_str(
        format!(
            "\n[[sessions]]\nid = \"{pubsub_session}\"\ndirectory = \"/tmp\"\n[sessions.pubsub]\n",
        )
        .as_str(),
    );
    fs::write(
        config_root
            .base_layer()
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle_toml,
    )
    .expect("write bundle config with pubsub member");
    config_root
}

/// Writes a single-session tmux bundle whose bundle file declares the given
/// top-level `environment` entries, reusing the standard coders/policies/users
/// config. Used to prove the merged environment reaches the tmux
/// session-creation call as `new-session -e KEY=VALUE` flags.
pub(crate) fn write_bundle_configuration_with_environment(
    root: &Path,
    bundle_name: &str,
    session: &str,
    environment: &[(&str, &str)],
) -> ConfigurationRoots {
    let coders = vec![CoderSpec {
        id: "default".to_string(),
        initial_command: "sh -lc 'exec sleep 45'".to_string(),
        resume_command: "sh -lc 'exec sleep 45'".to_string(),
        prompt_regex: None,
        prompt_inspect_lines: None,
        prompt_idle_column: None,
    }];
    let sessions = vec![SessionSpec {
        id: session.to_string(),
        name: Some(session.to_string()),
        directory: PathBuf::from("/tmp"),
        coder: "default".to_string(),
        coder_session_id: None,
    }];
    let config_root = write_bundle_configuration_members(root, bundle_name, &coders, &sessions);

    let mut bundle_toml = String::from("format-version = 1\nautostart = true\n");
    for (name, value) in environment {
        bundle_toml.push_str(
            format!("\n[[environment]]\nname = \"{name}\"\nvalue = \"{value}\"\n").as_str(),
        );
    }
    bundle_toml.push_str(
        format!(
            "\n[[sessions]]\nid = \"{session}\"\nname = \"{session}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
        )
        .as_str(),
    );
    fs::write(
        config_root
            .base_layer()
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle_toml,
    )
    .expect("write bundle config with environment");
    config_root
}

/// Writes a bundle whose sole coder is a stdio ACP coder backed by a stub that
/// answers `initialize`/`session/new` but deliberately never completes a
/// `session/prompt` turn, so a send to an ACP session leaves a turn in flight.
/// The stub appends every spawned child's PID to `pid_file` and logs each
/// received JSON-RPC line to `log_file`. Stub configuration rides in through the
/// coder-level `[[coders.environment]]` surface (the same channel the ACP
/// integration suite uses), which the relay applies to the spawned child.
///
/// Used to reproduce the in-flight-ACP-turn shutdown condition for
/// `issues/relay/49`: relay shutdown must reap the delivery thread's bounded
/// prompt-completion wait within a bounded window rather than pinning the
/// process until SIGKILL.
pub(crate) fn write_acp_hang_bundle_configuration(
    root: &Path,
    bundle_name: &str,
    sessions: &[&str],
    stub_path: &Path,
    log_file: &Path,
    pid_file: &Path,
) -> ConfigurationRoots {
    write_acp_hang_stub(stub_path);
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");

    let coders = format!(
        r#"format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "{command}"

[[coders.environment]]
name = "ACP_LOG_FILE"
value = "{log}"

[[coders.environment]]
name = "ACP_PID_FILE"
value = "{pid}"
"#,
        command = stub_path.display(),
        log = log_file.display(),
        pid = pid_file.display(),
    );
    fs::write(config_root.join("coders.toml"), coders).expect("write coders config");

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
    .expect("write policies config");
    fs::write(config_root.join("users.toml"), "").expect("write users config");

    let mut bundle_toml = String::from("format-version = 1\nautostart = true\n");
    for session in sessions {
        bundle_toml.push_str(
            format!(
                "\n[[sessions]]\nid = \"{session}\"\nname = \"{session}\"\ndirectory = \"/tmp\"\ncoder = \"acp\"\n",
            )
            .as_str(),
        );
    }
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml)
        .expect("write bundle config");
    ConfigurationRoots::single(config_root)
}

/// Writes a minimal stdio ACP agent stub that hangs on `session/prompt`.
///
/// It answers `initialize`, `session/new`, and `session/load` so worker
/// bootstrap succeeds and the worker reaches `Available`, but on
/// `session/prompt` it emits no `stopReason` result and loops back to read.
/// The turn therefore stays in flight and the child stays alive (blocked on
/// read) until relay shutdown kills it or closes its stdin (EOF ends the loop).
fn write_acp_hang_stub(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

log_file="${ACP_LOG_FILE:?}"
pid_file="${ACP_PID_FILE:?}"
printf '%s\n' "$$" >> "$pid_file"

while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log_file"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  [ -z "$id" ] && continue
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptSession":true}}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-hang"}}\n' "$id"
      ;;
    *'"method":"session/load"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      : # never respond -- leave the turn in flight
      ;;
  esac
done
"#;
    fs::write(path, script).expect("write ACP hang stub");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("set ACP hang stub executable");
}

pub(crate) fn spawn_session(socket: &Path, session_name: &str, shell_command: &str) {
    let output = tmux_command(
        socket,
        &[
            "new-session",
            "-d",
            "-s",
            session_name,
            "sh",
            "-lc",
            shell_command,
        ],
    );
    assert!(
        output.status.success(),
        "failed to create session {session_name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn write_fake_tmux_script(script_path: &Path, attempts_file: &Path, log_file: &Path) {
    let body = format!(
        r##"#!/usr/bin/env bash
set -euo pipefail

ATTEMPTS_FILE="{attempts}"
LOG_FILE="{log}"
SESSIONS_FILE="${{ATTEMPTS_FILE}}.sessions"
OWNED_FILE="${{ATTEMPTS_FILE}}.owned"

mkdir -p "$(dirname "${{ATTEMPTS_FILE}}")"
touch "${{ATTEMPTS_FILE}}" "${{LOG_FILE}}"

args=("$@")
if [[ "${{#args[@]}}" -ge 2 && "${{args[0]}}" == "-S" ]]; then
  args=("${{args[@]:2}}")
fi
if [[ "${{#args[@]}}" -eq 0 ]]; then
  exit 1
fi
command_name="${{args[0]}}"
printf "%s %s\n" "$(date +%s%3N)" "${{args[*]}}" >> "${{LOG_FILE}}"

case "${{command_name}}" in
  has-session)
    target="${{args[2]#=}}"
    if [[ -s "${{SESSIONS_FILE}}" ]] && grep -Fxq "${{target}}" "${{SESSIONS_FILE}}"; then
      exit 0
    fi
    echo "can't find session: ${{target}}" >&2
    exit 1
    ;;
  list-sessions)
    if [[ ! -s "${{SESSIONS_FILE}}" ]]; then
      EMPTY_LIST_ERROR_MODE="${{FAKE_TMUX_EMPTY_LIST_ERROR_MODE:-no_server_running}}"
      if [[ "${{EMPTY_LIST_ERROR_MODE}}" == "server_exited_unexpectedly" ]]; then
        echo "server exited unexpectedly" >&2
      else
        echo "no server running on /tmp/agentmux-fake" >&2
      fi
      exit 1
    fi
    format="${{args[2]-}}"
    owned_format=$'#{{session_name}}\t#{{@agentmux_owned}}'
    while IFS= read -r session; do
      [[ -z "${{session}}" ]] && continue
      if [[ "${{format}}" == "${{owned_format}}" || "${{format}}" == "#{{session_name}}\\t#{{@agentmux_owned}}" ]]; then
        marker=""
        if [[ -s "${{OWNED_FILE}}" ]] && grep -Fxq "${{session}}" "${{OWNED_FILE}}"; then
          marker="1"
        fi
        printf "%s\t%s\n" "${{session}}" "${{marker}}"
      else
        printf "%s\n" "${{session}}"
      fi
    done < "${{SESSIONS_FILE}}"
    ;;
  display-message)
    format="${{args[4]-}}"
    case "${{format}}" in
      '#{{pane_id}}')
        printf "%%1\n"
        ;;
      '#{{window_activity}}')
        printf "1\n"
        ;;
      '#{{pane_in_mode}}')
        printf "%s\n" "${{FAKE_TMUX_PANE_IN_MODE:-0}}"
        ;;
      '#{{cursor_x}}')
        printf "0\n"
        ;;
      *)
        printf "\n"
        ;;
    esac
    ;;
  capture-pane)
    CAPTURE_MODE="${{FAKE_TMUX_CAPTURE_MODE:-incremental}}"
    if [[ "${{CAPTURE_MODE}}" == "stable" ]]; then
      printf "frame-stable\n"
    else
      CAPTURE_FILE="${{ATTEMPTS_FILE}}.capture"
      capture_count="$(cat "${{CAPTURE_FILE}}" 2>/dev/null || true)"
      if [[ -z "${{capture_count}}" ]]; then capture_count=0; fi
      capture_count="$((capture_count + 1))"
      printf "%s" "${{capture_count}}" > "${{CAPTURE_FILE}}"
      printf "frame-%s\n" "${{capture_count}}"
    fi
    ;;
  send-keys)
    :
    ;;
  load-buffer)
    buffer_name=""
    i=0
    while [[ $i -lt ${{#args[@]}} ]]; do
      if [[ "${{args[$i]}}" == "-b" ]]; then
        buffer_name="${{args[$((i+1))]}}"
        break
      fi
      i=$((i+1))
    done
    cat - > "${{LOG_FILE}}.buffer.${{buffer_name}}"
    ;;
  paste-buffer)
    :
    ;;
  new-session)
    count="$(cat "${{ATTEMPTS_FILE}}" 2>/dev/null || true)"
    if [[ -z "${{count}}" ]]; then count=0; fi
    count="$((count + 1))"
    printf "%s" "${{count}}" > "${{ATTEMPTS_FILE}}"
    if [[ "${{count}}" -le 2 ]]; then
      echo "failed to connect to server" >&2
      exit 1
    fi
    session_name="${{args[3]}}"
    printf "%s\n" "${{session_name}}" >> "${{SESSIONS_FILE}}"
    sort -u "${{SESSIONS_FILE}}" -o "${{SESSIONS_FILE}}"
    ;;
  set-option)
    session_name="${{args[2]#=}}"
    printf "%s\n" "${{session_name}}" >> "${{OWNED_FILE}}"
    sort -u "${{OWNED_FILE}}" -o "${{OWNED_FILE}}"
    ;;
  kill-session)
    session_name="${{args[2]#=}}"
    if [[ -f "${{SESSIONS_FILE}}" ]]; then
      grep -Fxv "${{session_name}}" "${{SESSIONS_FILE}}" > "${{SESSIONS_FILE}}.tmp" || true
      mv "${{SESSIONS_FILE}}.tmp" "${{SESSIONS_FILE}}"
    fi
    if [[ -f "${{OWNED_FILE}}" ]]; then
      grep -Fxv "${{session_name}}" "${{OWNED_FILE}}" > "${{OWNED_FILE}}.tmp" || true
      mv "${{OWNED_FILE}}.tmp" "${{OWNED_FILE}}"
    fi
    ;;
  kill-server)
    : > "${{SESSIONS_FILE}}"
    : > "${{OWNED_FILE}}"
    ;;
  *)
    echo "unsupported fake tmux command: ${{command_name}}" >&2
    exit 2
    ;;
esac
"##,
        attempts = attempts_file.display(),
        log = log_file.display(),
    );
    fs::write(script_path, body).expect("write fake tmux script");
    fs::set_permissions(script_path, fs::Permissions::from_mode(0o755))
        .expect("set fake tmux script executable");
}

pub(crate) fn spawn_relay_with_fake_tmux(
    bundle_name: &str,
    config_root: &ConfigurationRoots,
    state_root: &Path,
    inscriptions_root: &Path,
    fake_tmux_script: &Path,
) -> Child {
    spawn_relay_with_fake_tmux_and_env(
        bundle_name,
        config_root,
        state_root,
        inscriptions_root,
        fake_tmux_script,
        &[],
    )
}

pub(crate) fn spawn_relay_with_fake_tmux_and_env(
    _bundle_name: &str,
    config_root: &ConfigurationRoots,
    state_root: &Path,
    inscriptions_root: &Path,
    fake_tmux_script: &Path,
    environment: &[(&str, &str)],
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
    command.arg("host").arg("relay");
    // One occurrence per layer, matching how the flag is declared.
    for layer in config_root.layers() {
        command.arg("--configuration-directory").arg(layer);
    }
    command
        .arg("--state-directory")
        .arg(state_root)
        .arg("--inscriptions-directory")
        .arg(inscriptions_root)
        .env("AGENTMUX_TMUX_COMMAND", fake_tmux_script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // A tokio `Child` does not kill on drop by default, so a test that
        // panicked before reaching its explicit shutdown left its relay running
        // against a temp directory nothing would ever clean up. The panic path is
        // exactly when cleanup matters most, and it is the one path that cannot
        // be written out longhand at each call site.
        .kill_on_drop(true);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.spawn().expect("spawn relay")
}

// Waits on the relay's published readiness sentinel (relay.ready), which the
// host writes only after SIGINT/SIGTERM handlers are installed AND every
// per-bundle accept loop has been spawned. Socket-existence on its own is not
// a sound readiness signal: the kernel queues connections from `bind`, well
// before the relay is actually serving (and on macOS, before signal handlers
// are installed -- see issues/relay/20).
//
// Callers pass the relay socket path; the sentinel lives alongside it in the
// bundle runtime directory.
pub(crate) async fn wait_for_relay_ready(socket: &Path) {
    let sentinel = socket
        .parent()
        .expect("relay socket has parent directory")
        .join("relay.ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if sentinel.exists() && socket.exists() {
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "timed out waiting for relay ready sentinel {}",
        sentinel.display()
    );
}

pub(crate) async fn drain_child_stdout(child: &mut Child) -> String {
    let mut output = String::new();
    if let Some(stdout) = child.stdout.as_mut() {
        let mut reader = tokio::io::BufReader::new(stdout);
        let _ = reader.read_line(&mut output).await;
    }
    if output.is_empty()
        && let Some(stderr) = child.stderr.as_mut()
    {
        let mut reader = tokio::io::BufReader::new(stderr);
        let _ = reader.read_line(&mut output).await;
    }
    output
}
