use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    time::{Duration, Instant},
};

use tokio::{
    io::AsyncBufReadExt,
    process::{Child, Command},
    time::sleep,
};

use agentmux::configuration::ConfigurationRoots;

use crate::support::process::{RELAY_GRACE_PERIOD, WAIT_POLL_INTERVAL};

pub(crate) fn write_fake_tmux_script(
    script_path: &Path,
    attempts_file: &Path,
    log_file: &Path,
) {
    let body = format!(
        r##"#!/usr/bin/env bash
set -euo pipefail

ATTEMPTS_FILE="{attempts}"
LOG_FILE="{log}"
SESSIONS_FILE="${{ATTEMPTS_FILE}}.sessions"
OWNED_FILE="${{ATTEMPTS_FILE}}.owned"
IDS_FILE="${{ATTEMPTS_FILE}}.ids"
KILL_AFTER_CREATE_FILE="${{ATTEMPTS_FILE}}.kill-after-create"
MALFORMED_ID_FILE="${{ATTEMPTS_FILE}}.malformed-id"

mkdir -p "$(dirname "${{ATTEMPTS_FILE}}")"
touch "${{ATTEMPTS_FILE}}" "${{LOG_FILE}}"

# Creation-time session-id bookkeeping. Each successful `new-session`
# mints one `$N` handle and remembers which name it belongs to, the way
# real tmux assigns immutable session ids at creation.
allocate_session_id() {{
  touch "${{IDS_FILE}}"
  local count
  count="$(wc -l < "${{IDS_FILE}}" | tr -d ' ')"
  local id='$'"${{count}}"
  printf "%s %s\n" "${{id}}" "$1" >> "${{IDS_FILE}}"
  printf "%s\n" "${{id}}"
}}

lookup_session_id() {{
  [[ -f "${{IDS_FILE}}" ]] || return 0
  while IFS=' ' read -r id name; do
    if [[ "${{id}}" == "$1" ]]; then
      printf "%s\n" "${{name}}"
      return 0
    fi
  done < "${{IDS_FILE}}"
  return 0
}}

session_present() {{
  [[ -s "${{SESSIONS_FILE}}" ]] && grep -Fxq "$1" "${{SESSIONS_FILE}}"
}}

# Models real tmux 3.4 name resolution for a missing shorter name: it falls
# back to an existing longer name with the request as a prefix.
prefix_match_session() {{
  [[ -s "${{SESSIONS_FILE}}" ]] || return 0
  while IFS= read -r session; do
    [[ -z "${{session}}" ]] && continue
    if [[ "${{session}}" == "$1"* && "${{session}}" != "$1" ]]; then
      printf "%s\n" "${{session}}"
      return 0
    fi
  done < "${{SESSIONS_FILE}}"
  return 0
}}

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
    target="${{args[3]-}}"
    target="${{target#=}}"
    case "${{format}}" in
      '#{{pane_id}}')
        printf "%%1\n"
        ;;
      '#{{session_name}} #{{pane_id}}')
        printf "%s %%1\n" "${{target}}"
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
    session_name=""
    print_session_id=0
    i=0
    while [[ $i -lt ${{#args[@]}} ]]; do
      case "${{args[$i]}}" in
        -s)
          session_name="${{args[$((i+1))]-}}"
          i=$((i+2))
          continue
          ;;
        -F)
          if [[ "${{args[$((i+1))]-}}" == '#{{session_id}}' ]]; then
            print_session_id=1
          fi
          i=$((i+2))
          continue
          ;;
      esac
      i=$((i+1))
    done
    printf "%s\n" "${{session_name}}" >> "${{SESSIONS_FILE}}"
    sort -u "${{SESSIONS_FILE}}" -o "${{SESSIONS_FILE}}"
    malformed_kind=""
    malformed_text=""
    if [[ -f "${{MALFORMED_ID_FILE}}" ]]; then
      malformed_entry="$(grep -E "^${{session_name}}=" "${{MALFORMED_ID_FILE}}" | tail -n 1 || true)"
      if [[ -n "${{malformed_entry}}" ]]; then
        malformed_kind="pair"
        malformed_text="${{malformed_entry#*=}}"
      elif grep -Fxq "${{session_name}}" "${{MALFORMED_ID_FILE}}"; then
        malformed_kind="fixed"
      fi
    fi
    case "${{malformed_kind}}" in
      pair) printf '%b\n' "${{malformed_text}}" ;;
      fixed) printf "not-a-session-id\n" ;;
      *)
        if [[ "${{print_session_id}}" == "1" ]]; then
          allocate_session_id "${{session_name}}"
        fi
        ;;
    esac
    if [[ -f "${{KILL_AFTER_CREATE_FILE}}" ]] && grep -Fxq "${{session_name}}" "${{KILL_AFTER_CREATE_FILE}}"; then
      grep -Fxv "${{session_name}}" "${{SESSIONS_FILE}}" > "${{SESSIONS_FILE}}.tmp" || true
      mv "${{SESSIONS_FILE}}.tmp" "${{SESSIONS_FILE}}"
    fi
    ;;
  set-option)
    # Models real tmux 3.4 target resolution: an exact session id (`$N`)
    # resolves through the creation-time mapping and errors when the session
    # is gone; a session name resolves exactly, then by prefix fallback to an
    # existing longer name, else errors. A `=`-prefixed target is rejected
    # outright — real tmux refuses it — so a reversion to `=name` cannot
    # falsely pass through normalization here.
    if [[ "${{args[2]-}}" == =* ]]; then
      echo "no such session: ${{args[2]}}" >&2
      exit 1
    fi
    target="${{args[2]#=}}"
    target="${{target%:}}"
    if [[ "${{target}}" == '$'* ]]; then
      session_name="$(lookup_session_id "${{target}}")"
      if [[ -z "${{session_name}}" ]] || ! session_present "${{session_name}}"; then
        echo "no such session: ${{target}}" >&2
        exit 1
      fi
    elif session_present "${{target}}"; then
      session_name="${{target}}"
    else
      session_name="$(prefix_match_session "${{target}}")"
      if [[ -z "${{session_name}}" ]]; then
        echo "can't find session: ${{target}}" >&2
        exit 1
      fi
    fi
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

/// RAII guard for a tokio-spawned `agentmux host relay` child.
///
/// Mirrors `crate::support::process::RelayChildGuard` for the `tokio::process::Child`
/// path used by `spawn_relay_with_fake_tmux`. A bare tokio `Child` with
/// `kill_on_drop(true)` would send SIGKILL on drop, bypassing the relay's
/// generation fence that reaps its ACP-worker grandchildren and orphaning them a
/// second time. This guard sends SIGTERM first and gives the relay a bounded
/// window to exit gracefully before escalating to SIGKILL.
pub(crate) struct RelayChildGuard {
    child: Option<Child>,
}

impl RelayChildGuard {
    pub(crate) fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    pub(crate) fn start_kill(&mut self) -> std::io::Result<()> {
        match self.child.as_mut() {
            Some(child) => child.start_kill(),
            None => Ok(()),
        }
    }

    pub(crate) async fn wait(
        &mut self,
    ) -> std::io::Result<std::process::ExitStatus> {
        // Safety: `self.child` is Some while this method is called; it is never
        // taken concurrently because `&mut self` is exclusive.
        let child = self
            .child
            .as_mut()
            .expect("RelayChildGuard::wait on empty guard");
        child.wait().await
    }
}

impl std::ops::Deref for RelayChildGuard {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        self.child
            .as_ref()
            .expect("RelayChildGuard deref on empty guard")
    }
}

impl std::ops::DerefMut for RelayChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.child
            .as_mut()
            .expect("RelayChildGuard deref_mut on empty guard")
    }
}

impl Drop for RelayChildGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {},
            Err(_) => return,
        }
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        // Bounded graceful window — uses the same fence as the std guard.
        let deadline = Instant::now() + RELAY_GRACE_PERIOD;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(WAIT_POLL_INTERVAL);
                },
                Err(_) => return,
            }
        }
        let _ = child.start_kill();
        // `start_kill` is SIGKILL but does not reap; poll until reaped so the
        // pid cannot be recycled. Bounding this wait prevents Drop from hanging
        // indefinitely if the child ignores SIGKILL (should not happen).
        let reap_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= reap_deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                },
                Err(_) => break,
            }
        }
    }
}

pub(crate) fn spawn_relay_with_fake_tmux(
    bundle_name: &str,
    config_root: &ConfigurationRoots,
    state_root: &Path,
    inscriptions_root: &Path,
    fake_tmux_script: &Path,
) -> RelayChildGuard {
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
) -> RelayChildGuard {
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
        .stderr(std::process::Stdio::piped());
    for (name, value) in environment {
        command.env(name, value);
    }
    RelayChildGuard::new(command.spawn().expect("spawn relay"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    /// Tokio guard must also send SIGTERM first. Mirrors the std guard test
    /// but exercises the `tokio::process::Child` path whose `Drop` is the one
    /// that previously did `kill_on_drop(true)` (SIGKILL).
    #[tokio::test]
    async fn tokio_relay_child_guard_sends_sigterm_before_sigkill_and_reaps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("sigterm-marker-tokio");
        assert!(!marker.exists());

        let child = Command::new("sh")
            .arg("-c")
            .arg(r#"trap 'touch "$1"; kill $! 2>/dev/null; exit 0' TERM; sleep 30 & wait"#)
            .arg("sh")
            .arg(&marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn trap child");
        // Allow handler installation before Drop sends TERM.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let pid = child.id().expect("tokio child has pid");

        let start = Instant::now();
        {
            let guard = RelayChildGuard::new(child);
            drop(guard);
        }
        let elapsed = start.elapsed();

        let deadline = Instant::now() + Duration::from_millis(500);
        while !marker.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            marker.exists(),
            "SIGTERM trap did not create marker at {}; tokio guard may have sent SIGKILL directly",
            marker.display()
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "Drop should be quick on SIGTERM; elapsed={elapsed:?}"
        );
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        assert_ne!(
            rc, 0,
            "pid {pid} should be reaped after tokio guard Drop; kill -0 succeeded"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "expected ESRCH after reap"
        );
    }
}
