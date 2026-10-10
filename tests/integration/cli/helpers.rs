use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use agentmux::envelope::ENVELOPE_SCHEMA_VERSION;
use agentmux::relay::RelayResponse;
use agentmux::runtime::paths::{BundleRuntimePaths, RelayRuntimePaths};
use serde_json::{Value, json};

pub(super) fn write_bundle_configuration(
    config_root: &Path,
    bundle_name: &str,
    groups: Option<&[&str]>,
    sessions: &[&str],
) {
    write_bundle_configuration_with_options(config_root, bundle_name, groups, sessions, None);
}

pub(super) fn write_bundle_configuration_with_options(
    config_root: &Path,
    bundle_name: &str,
    groups: Option<&[&str]>,
    sessions: &[&str],
    autostart: Option<bool>,
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
# The CLI operates this bundle as the relay-wide `user@GLOBAL` operator, whose
# home namespace is GLOBAL; cross-namespace list/send into a bundle requires
# all.
list = "all"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
choose = "home"
list = "all"
look = "all"
raww = "home"
send = "all"
updown = "home"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root.join("users.toml"),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
name = "Operator"
policy = "operator"

[sessions.ui]
"#,
    )
    .expect("write users config");
    let mut bundle = String::from("format-version = 1\n");
    if let Some(autostart) = autostart {
        bundle.push_str(format!("autostart = {autostart}\n").as_str());
    }
    if let Some(groups) = groups {
        let encoded = groups
            .iter()
            .map(|group| format!("\"{group}\""))
            .collect::<Vec<_>>()
            .join(", ");
        bundle.push_str(format!("groups = [{encoded}]\n").as_str());
    }
    for session in sessions {
        bundle.push_str(
            format!(
                "\n[[sessions]]\nid = \"{name}\"\nname = \"{name}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
                name = session
            )
            .as_str(),
        );
    }
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle,
    )
    .expect("write bundle config");
}

pub(super) fn write_bundle_configuration_with_member_directories(
    config_root: &Path,
    bundle_name: &str,
    groups: Option<&[&str]>,
    members: &[(&str, &Path)],
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
# The CLI operates this bundle as the relay-wide `user@GLOBAL` operator, whose
# home namespace is GLOBAL; cross-namespace list/send into a bundle requires
# all.
list = "all"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
choose = "home"
list = "all"
look = "all"
raww = "home"
send = "all"
updown = "home"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root.join("users.toml"),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
name = "Operator"
policy = "operator"

[sessions.ui]
"#,
    )
    .expect("write users config");
    let mut bundle = String::from("format-version = 1\n");
    if let Some(groups) = groups {
        let encoded = groups
            .iter()
            .map(|group| format!("\"{group}\""))
            .collect::<Vec<_>>()
            .join(", ");
        bundle.push_str(format!("groups = [{encoded}]\n").as_str());
    }
    for (session, directory) in members {
        bundle.push_str(
            format!(
                "\n[[sessions]]\nid = \"{name}\"\nname = \"{name}\"\ndirectory = \"{}\"\ncoder = \"default\"\n",
                directory.display(),
                name = session
            )
            .as_str(),
        );
    }
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle,
    )
    .expect("write bundle config");
}

/// Qualifies a bare user selector into canonical `session@GLOBAL` form.
fn qualify_global(id: &str) -> String {
    if id.ends_with("@GLOBAL") {
        id.to_string()
    } else {
        format!("{id}@GLOBAL")
    }
}

pub(super) fn write_tui_configuration(
    config_root: &Path,
    default_bundle: Option<&str>,
    default_session: Option<&str>,
    sessions: &[(&str, &str, Option<&str>)],
) {
    // default-bundle is a UI-surface default: it lives in ui.toml, not the
    // users.toml identity/policy file.
    if let Some(default_bundle) = default_bundle {
        fs::write(
            config_root.join("ui.toml"),
            format!("default-bundle = \"{default_bundle}\"\n"),
        )
        .expect("write ui config");
    }
    let mut body = String::new();
    if let Some(default_session) = default_session {
        body.push_str(
            format!(
                "default-session = \"{}\"\n",
                qualify_global(default_session)
            )
            .as_str(),
        );
    }
    for (id, policy_id, name) in sessions {
        body.push_str(
            format!(
                "\n[[sessions]]\nid = \"{}\"\npolicy = \"{policy_id}\"\n",
                qualify_global(id)
            )
            .as_str(),
        );
        if let Some(name) = name {
            body.push_str(format!("name = \"{name}\"\n").as_str());
        }
        body.push_str("\n[sessions.ui]\n");
    }
    fs::write(config_root.join("users.toml"), body).expect("write users config");
}

pub(super) fn parse_summary_json_line(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("find summary json line");
    serde_json::from_str(line).expect("parse summary json")
}

/// Path the fake tmux script records each invocation's full argument vector to,
/// `-S` included. Lets a test assert on what actually reached tmux rather than
/// on an intermediate the production code could stop using.
pub(super) fn fake_tmux_log_path(script_path: &Path) -> PathBuf {
    script_path.with_extension("log")
}

/// Where the fake tmux records the `PATH` it was invoked with.
///
/// Separate from the invocation log because that log is asserted line by line
/// elsewhere. This is what makes the client's inherited environment observable:
/// the fake tmux stands in the tmux client's position, so its `PATH` is the one a
/// tmux server it started -- and thence a pane -- would inherit.
pub(super) fn fake_tmux_search_path_file(script_path: &Path) -> PathBuf {
    script_path.with_extension("search-path")
}

/// Sidecar listing sessions the fake tmux creates but never returns a pane for.
///
/// Readiness is a pane lookup, so this is the deterministic way to produce a
/// session that exists yet is not ready — no dependence on how fast a real pane's
/// process exits.
pub(super) fn fake_tmux_unready_sessions_file(script_path: &Path) -> PathBuf {
    script_path.with_extension("unready")
}

/// Sidecar whose presence makes the fake tmux fail `has-session` with an error
/// that is not a missing-session error, standing in for a tmux state query that
/// fails for reasons no single session owns.
pub(super) fn fake_tmux_query_failure_file(script_path: &Path) -> PathBuf {
    script_path.with_extension("queryfail")
}

/// Marks `session_id` as created-but-never-ready for the fake tmux at
/// `script_path`.
pub(super) fn mark_fake_tmux_session_unready(script_path: &Path, session_id: &str) {
    let path = fake_tmux_unready_sessions_file(script_path);
    let mut existing = fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(session_id);
    existing.push('\n');
    fs::write(&path, existing).expect("write fake tmux unready sessions");
}

/// Makes every subsequent fake-tmux `has-session` query fail outright.
pub(super) fn fail_fake_tmux_state_queries(script_path: &Path) {
    fs::write(fake_tmux_query_failure_file(script_path), b"1")
        .expect("write fake tmux query failure flag");
}

/// Sidecar listing sessions the fake tmux refuses to create.
///
/// Real tmux is unhelpfully tolerant here — it will happily create a session
/// whose start directory does not exist — so a creation failure has to be
/// arranged rather than provoked.
pub(super) fn fake_tmux_create_failure_file(script_path: &Path) -> PathBuf {
    script_path.with_extension("createfail")
}

/// Makes the fake tmux refuse to create `session_id`.
pub(super) fn fail_fake_tmux_session_creation(script_path: &Path, session_id: &str) {
    let path = fake_tmux_create_failure_file(script_path);
    let mut existing = fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(session_id);
    existing.push('\n');
    fs::write(&path, existing).expect("write fake tmux create failure sessions");
}

pub(super) fn fake_tmux_kill_after_create_file(script_path: &Path) -> PathBuf {
    script_path.with_extension("kill-after-create")
}

pub(super) fn kill_fake_tmux_session_after_create(script_path: &Path, session_id: &str) {
    fs::write(
        fake_tmux_kill_after_create_file(script_path),
        format!("{session_id}\n"),
    )
    .expect("write fake tmux kill-after-create sentinel");
}

pub(super) fn fake_tmux_malformed_id_file(script_path: &Path) -> PathBuf {
    script_path.with_extension("malformed-id")
}

pub(super) fn fail_fake_tmux_session_id_report(script_path: &Path, session_id: &str) {
    fs::write(
        fake_tmux_malformed_id_file(script_path),
        format!("{session_id}\n"),
    )
    .expect("write fake tmux malformed-id sentinel");
}

/// Makes the fake tmux answer `new-session -P` for `session_id` with
/// `raw_output` verbatim (backslash escapes decoded), exercising malformed
/// id shapes — empty, non-numeric, multiline, trailing garbage — through
/// the CLI instead of a unit test.
pub(super) fn fail_fake_tmux_session_id_report_raw(
    script_path: &Path,
    session_id: &str,
    raw_output: &str,
) {
    let path = fake_tmux_malformed_id_file(script_path);
    let mut existing = fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(&format!("{session_id}={raw_output}\n"));
    fs::write(&path, existing).expect("write fake tmux malformed-id sentinel");
}

pub(super) fn read_fake_tmux_owned_sessions(script_path: &Path) -> Vec<String> {
    let content = fs::read_to_string(script_path.with_extension("owned")).unwrap_or_default();
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn write_fake_tmux_script(path: &Path) {
    let sessions_file = path.with_extension("sessions");
    let owned_file = path.with_extension("owned");
    let log_file = fake_tmux_log_path(path);
    let search_path_file = fake_tmux_search_path_file(path);
    let unready_file = fake_tmux_unready_sessions_file(path);
    let query_failure_file = fake_tmux_query_failure_file(path);
    let create_failure_file = fake_tmux_create_failure_file(path);
    let ids_file = path.with_extension("ids");
    let kill_after_create_file = fake_tmux_kill_after_create_file(path);
    let malformed_id_file = fake_tmux_malformed_id_file(path);
    let body = format!(
        r##"#!/usr/bin/env bash
set -euo pipefail

SESSIONS_FILE="{sessions}"
OWNED_FILE="{owned}"
LOG_FILE="{log}"
SEARCH_PATH_FILE="{search_path}"
UNREADY_FILE="{unready}"
QUERY_FAILURE_FILE="{query_failure}"
CREATE_FAILURE_FILE="{create_failure}"
IDS_FILE="{ids}"
KILL_AFTER_CREATE_FILE="{kill_after_create}"
MALFORMED_ID_FILE="{malformed_id}"
touch "${{SESSIONS_FILE}}" "${{OWNED_FILE}}" "${{LOG_FILE}}" "${{UNREADY_FILE}}" "${{CREATE_FAILURE_FILE}}"

printf "%s\n" "$*" >> "${{LOG_FILE}}"
printf "%s\n" "${{PATH-}}" > "${{SEARCH_PATH_FILE}}"

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
command_name="${{args[0]-}}"

case "${{command_name}}" in
  has-session)
    target="${{args[2]#=}}"
    if [[ -f "${{QUERY_FAILURE_FILE}}" ]]; then
      echo "permission denied" >&2
      exit 1
    fi
    if [[ -s "${{SESSIONS_FILE}}" ]] && grep -Fxq "${{target}}" "${{SESSIONS_FILE}}"; then
      exit 0
    fi
    echo "can't find session: ${{target}}" >&2
    exit 1
    ;;
  list-sessions)
    if [[ ! -s "${{SESSIONS_FILE}}" ]]; then
      echo "no server running on /tmp/agentmux-fake" >&2
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
  new-session)
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
    if [[ -s "${{CREATE_FAILURE_FILE}}" ]] && grep -Fxq "${{session_name}}" "${{CREATE_FAILURE_FILE}}"; then
      echo "permission denied" >&2
      exit 1
    fi
    printf "%s\n" "${{session_name}}" >> "${{SESSIONS_FILE}}"
    sort -u "${{SESSIONS_FILE}}" -o "${{SESSIONS_FILE}}"
    # A malformed-id entry is either a bare session name (fixed sentinel
    # output) or a `name=raw` pair whose raw bytes print verbatim with
    # backslash escapes decoded, so the harness can emit the full malformed
    # shape matrix through the CLI.
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
    grep -Fxv "${{session_name}}" "${{SESSIONS_FILE}}" > "${{SESSIONS_FILE}}.tmp" || true
    mv "${{SESSIONS_FILE}}.tmp" "${{SESSIONS_FILE}}"
    grep -Fxv "${{session_name}}" "${{OWNED_FILE}}" > "${{OWNED_FILE}}.tmp" || true
    mv "${{OWNED_FILE}}.tmp" "${{OWNED_FILE}}"
    ;;
  kill-server)
    if [[ ! -s "${{SESSIONS_FILE}}" ]]; then
      echo "no server running on /tmp/agentmux-fake" >&2
      exit 1
    fi
    : > "${{SESSIONS_FILE}}"
    : > "${{OWNED_FILE}}"
    ;;
  display-message)
    target_session="${{args[3]-}}"
    target_session="${{target_session#=}}"
    format_value="${{args[4]-}}"
    case "${{format_value}}" in
      '#{{pane_id}}')
        if [[ -s "${{UNREADY_FILE}}" ]] && grep -Fxq "${{target_session}}" "${{UNREADY_FILE}}"; then
          printf "\n"
        else
          printf "%%1\n"
        fi
        ;;
      '#{{session_name}} #{{pane_id}}')
        if [[ -s "${{UNREADY_FILE}}" ]] && grep -Fxq "${{target_session}}" "${{UNREADY_FILE}}"; then
          printf "\n"
        else
          printf "%s %%1\n" "${{target_session}}"
        fi
        ;;
      '#{{window_activity}}')
        printf "0\n"
        ;;
      '#{{pane_in_mode}}')
        printf "0\n"
        ;;
      '#{{cursor_x}}')
        printf "0\n"
        ;;
      *)
        printf "\n"
        ;;
    esac
    ;;
  *)
    :
    ;;
esac
"##,
        sessions = sessions_file.display(),
        owned = owned_file.display(),
        log = log_file.display(),
        search_path = search_path_file.display(),
        unready = unready_file.display(),
        query_failure = query_failure_file.display(),
        create_failure = create_failure_file.display(),
        ids = ids_file.display(),
        kill_after_create = kill_after_create_file.display(),
        malformed_id = malformed_id_file.display(),
    );
    fs::write(path, body).expect("write fake tmux script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("set fake tmux executable");
}

pub(super) fn shutdown_relay_if_present(state_root: &Path, bundle_name: &str) {
    let bundle_paths = BundleRuntimePaths::resolve(state_root, bundle_name).expect("bundle paths");
    let relay_paths = RelayRuntimePaths::resolve(state_root);
    let _ = bundle_paths;
    let Some(pid) = fs::read_to_string(&relay_paths.relay_lock_file)
        .ok()
        .and_then(|value| value.lines().next().map(str::to_string))
        .and_then(|value| value.trim().parse::<i32>().ok())
    else {
        return;
    };
    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !relay_paths.relay_socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn wait_for_relay_ready(state_root: &Path, bundle_name: &str) {
    let bundle_paths = BundleRuntimePaths::resolve(state_root, bundle_name).expect("bundle paths");
    let relay_paths = RelayRuntimePaths::resolve(state_root);
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        // The sentinel is published only after signal handlers are installed
        // and the accept loop is spawned. Pairing it with socket existence
        // closes the early-startup race (issues/relay/20).
        if relay_paths.relay_ready_sentinel.exists() && relay_paths.relay_socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let startup_failures =
        fs::read_to_string(bundle_paths.runtime_directory.join("startup_failures.json")).ok();
    let relay_lock = fs::read_to_string(&relay_paths.relay_lock_file).ok();
    panic!(
        "timed out waiting for relay socket {}; startup_failures={startup_failures:?}; relay_lock={relay_lock:?}",
        relay_paths.relay_socket.display(),
    );
}

/// Multibundle fake-relay fixture. Accepts up to `expected_calls` sequential
/// connections on `socket_path`, dispatching each response by the Hello
/// frame's `bundle_name`. Records every request envelope into the
/// per-bundle log if present in `request_logs`.
pub(super) fn spawn_fake_relay_for_bundles(
    socket_path: &Path,
    expected_calls: usize,
    responses: std::collections::HashMap<String, RelayResponse>,
    request_logs: std::collections::HashMap<String, Arc<Mutex<Vec<Value>>>>,
) -> thread::JoinHandle<()> {
    if socket_path.exists() {
        fs::remove_file(socket_path).expect("remove stale relay socket");
    }
    let parent = socket_path.parent().expect("relay socket parent");
    fs::create_dir_all(parent).expect("create relay socket parent");
    let listener = UnixListener::bind(socket_path).expect("bind fake relay socket");
    listener
        .set_nonblocking(true)
        .expect("set fake relay listener nonblocking");
    let socket_path = socket_path.to_path_buf();
    thread::spawn(move || {
        let mut served = 0usize;
        let deadline = Instant::now() + Duration::from_secs(5);
        while served < expected_calls && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _address)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("set accepted stream blocking");
                    let mut reader =
                        BufReader::new(stream.try_clone().expect("clone fake relay stream"));

                    let mut hello_line = String::new();
                    reader
                        .read_line(&mut hello_line)
                        .expect("read fake relay hello");
                    let hello: Value = serde_json::from_str(hello_line.trim_end())
                        .expect("decode fake relay hello");
                    assert_eq!(hello.get("frame").and_then(Value::as_str), Some("hello"));
                    let hello_ack = json!({
                        "frame": "hello_ack",
                        "schema_version": ENVELOPE_SCHEMA_VERSION,
                        "principal_id": hello.get("principal_id").cloned().unwrap_or(Value::Null),
                    });
                    let encoded_ack =
                        serde_json::to_string(&hello_ack).expect("encode fake relay hello_ack");
                    stream
                        .write_all(encoded_ack.as_bytes())
                        .expect("write fake relay hello_ack");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay hello_ack");

                    let mut request_line = String::new();
                    reader
                        .read_line(&mut request_line)
                        .expect("read fake relay request");
                    let envelope: Value = serde_json::from_str(request_line.trim_end())
                        .expect("decode fake relay request envelope");
                    let request = envelope
                        .get("request")
                        .cloned()
                        .expect("fake relay request envelope missing 'request' field");
                    let bundle_name = envelope
                        .get("namespace")
                        .and_then(Value::as_str)
                        .expect("request envelope namespace")
                        .to_string();
                    if let Some(log) = request_logs.get(&bundle_name) {
                        log.lock().expect("request log lock").push(request);
                    }

                    let response = responses
                        .get(&bundle_name)
                        .expect("fake relay missing response for bundle")
                        .clone();
                    let response_frame = json!({
                        "frame": "response",
                        "request_id": envelope.get("request_id").cloned().unwrap_or(Value::Null),
                        "response": response,
                    });
                    let encoded =
                        serde_json::to_string(&response_frame).expect("encode fake relay response");
                    stream
                        .write_all(encoded.as_bytes())
                        .expect("write fake relay response");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay response");
                    served += 1;
                }
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(source) => panic!("accept fake relay connection: {source}"),
            }
        }
        let _ = fs::remove_file(socket_path);
        assert_eq!(
            served, expected_calls,
            "fake relay did not serve all expected calls"
        );
    })
}

pub(super) fn spawn_fake_relay_once(
    socket_path: &Path,
    response: RelayResponse,
    request_log: Arc<Mutex<Vec<Value>>>,
) -> thread::JoinHandle<()> {
    if socket_path.exists() {
        fs::remove_file(socket_path).expect("remove stale relay socket");
    }
    let parent = socket_path.parent().expect("relay socket parent");
    fs::create_dir_all(parent).expect("create relay socket parent");
    let listener = UnixListener::bind(socket_path).expect("bind fake relay socket");
    listener
        .set_nonblocking(true)
        .expect("set fake relay listener nonblocking");
    let socket_path = socket_path.to_path_buf();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _address)) => {
                    // macOS propagates the listener's non-blocking flag to the
                    // accepted socket; reset to blocking before any reads.
                    stream
                        .set_nonblocking(false)
                        .expect("set accepted stream blocking");
                    let mut reader =
                        BufReader::new(stream.try_clone().expect("clone fake relay stream"));

                    let mut hello_line = String::new();
                    reader
                        .read_line(&mut hello_line)
                        .expect("read fake relay hello");
                    let hello: Value = serde_json::from_str(hello_line.trim_end())
                        .expect("decode fake relay hello");
                    assert_eq!(hello.get("frame").and_then(Value::as_str), Some("hello"));
                    let hello_ack = json!({
                        "frame": "hello_ack",
                        "schema_version": ENVELOPE_SCHEMA_VERSION,
                        "principal_id": hello.get("principal_id").cloned().unwrap_or(Value::Null),
                    });
                    let encoded_ack =
                        serde_json::to_string(&hello_ack).expect("encode fake relay hello_ack");
                    stream
                        .write_all(encoded_ack.as_bytes())
                        .expect("write fake relay hello_ack");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay hello_ack");

                    let mut request_line = String::new();
                    reader
                        .read_line(&mut request_line)
                        .expect("read fake relay request");
                    let envelope: Value = serde_json::from_str(request_line.trim_end())
                        .expect("decode fake relay request envelope");
                    let request = envelope
                        .get("request")
                        .cloned()
                        .expect("fake relay request envelope missing 'request' field");
                    request_log.lock().expect("request log lock").push(request);

                    let response_frame = json!({
                        "frame": "response",
                        "request_id": envelope.get("request_id").cloned().unwrap_or(Value::Null),
                        "response": response,
                    });
                    let encoded =
                        serde_json::to_string(&response_frame).expect("encode fake relay response");
                    stream
                        .write_all(encoded.as_bytes())
                        .expect("write fake relay response");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay response");
                    let _ = fs::remove_file(socket_path);
                    return;
                }
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(source) => panic!("accept fake relay connection: {source}"),
            }
        }
        panic!("timed out waiting for fake relay request");
    })
}
