use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use agentmux::relay::RelayResponse;
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::Value;

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
find = "self"
list = "all:home"
look = "self"
send = "all:home"
"#,
    )
    .expect("write policies config");
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
find = "self"
list = "all:home"
look = "self"
send = "all:home"
"#,
    )
    .expect("write policies config");
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

pub(super) fn write_tui_configuration(
    config_root: &Path,
    default_bundle: Option<&str>,
    default_session: Option<&str>,
    sessions: &[(&str, &str, Option<&str>)],
) {
    let mut body = String::new();
    if let Some(default_bundle) = default_bundle {
        body.push_str(format!("default-bundle = \"{default_bundle}\"\n").as_str());
    }
    if let Some(default_session) = default_session {
        body.push_str(format!("default-session = \"{default_session}\"\n").as_str());
    }
    for (id, policy_id, name) in sessions {
        body.push_str(
            format!("\n[[sessions]]\nid = \"{id}\"\npolicy = \"{policy_id}\"\n").as_str(),
        );
        if let Some(name) = name {
            body.push_str(format!("name = \"{name}\"\n").as_str());
        }
    }
    fs::write(config_root.join("tui.toml"), body).expect("write tui config");
}

pub(super) fn parse_summary_json_line(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("find summary json line");
    serde_json::from_str(line).expect("parse summary json")
}

pub(super) fn write_fake_tmux_script(path: &Path) {
    let sessions_file = path.with_extension("sessions");
    let owned_file = path.with_extension("owned");
    let body = format!(
        r##"#!/usr/bin/env bash
set -euo pipefail

SESSIONS_FILE="{sessions}"
OWNED_FILE="{owned}"
touch "${{SESSIONS_FILE}}" "${{OWNED_FILE}}"

args=("$@")
if [[ "${{#args[@]}}" -ge 2 && "${{args[0]}}" == "-S" ]]; then
  args=("${{args[@]:2}}")
fi
command_name="${{args[0]-}}"

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
  *)
    :
    ;;
esac
"##,
        sessions = sessions_file.display(),
        owned = owned_file.display(),
    );
    fs::write(path, body).expect("write fake tmux script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("set fake tmux executable");
}

pub(super) fn shutdown_relay_if_present(state_root: &Path, bundle_name: &str) {
    let paths = BundleRuntimePaths::resolve(state_root, bundle_name).expect("bundle paths");
    let Some(pid) = fs::read_to_string(&paths.relay_lock_file)
        .ok()
        .and_then(|value| value.lines().next().map(str::to_string))
        .and_then(|value| value.trim().parse::<i32>().ok())
    else {
        return;
    };
    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !paths.relay_socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn wait_for_relay_socket(state_root: &Path, bundle_name: &str) {
    let paths = BundleRuntimePaths::resolve(state_root, bundle_name).expect("bundle paths");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if paths.relay_socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for relay socket {}",
        paths.relay_socket.display()
    );
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
                    let mut request_line = String::new();
                    let mut reader =
                        BufReader::new(stream.try_clone().expect("clone fake relay stream"));
                    reader
                        .read_line(&mut request_line)
                        .expect("read fake relay request");
                    let request: Value =
                        serde_json::from_str(request_line.trim_end()).expect("decode request");
                    request_log.lock().expect("request log lock").push(request);
                    let encoded =
                        serde_json::to_string(&response).expect("encode fake relay response");
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
