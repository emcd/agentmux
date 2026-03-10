use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use agentmux::relay::RelayResponse;
use agentmux::runtime::{
    bootstrap::acquire_relay_runtime_lock,
    paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn host_relay_requires_selector_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay"])
        .output()
        .expect("run agentmux host relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid argument <bundle-id>|--group: missing selector"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_conflicting_bundle_and_group_selectors() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "alpha", "--group", "dev"])
        .output()
        .expect("run agentmux host relay with conflicting selectors");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_selectors"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_all_flag_in_group_mvp() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--all"])
        .output()
        .expect("run agentmux host relay --all");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_arguments"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_invalid_uppercase_group_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--group", "DEV"])
        .output()
        .expect("run agentmux host relay --group DEV");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_group_name"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_unknown_custom_group() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["a"]);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--group",
            "nightly",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux host relay --group nightly");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_group"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_group_mode_reports_partial_lock_held_summary() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["a"]);
    write_bundle_configuration(&config_root, "bravo", Some(&["dev"]), &["b"]);

    let bravo_paths = BundleRuntimePaths::resolve(&state_root, "bravo").expect("bravo paths");
    ensure_bundle_runtime_directory(&bravo_paths).expect("ensure bravo runtime directory");
    let _bravo_lock = acquire_relay_runtime_lock(&bravo_paths).expect("acquire bravo lock");

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--group",
            "dev",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux host relay --group dev");

    assert!(output.status.success(), "command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("mode=bundle_group group=dev hosted=1 skipped=1 failed=0 hosted_any=true"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        stdout.contains("bundle=bravo outcome=skipped reason_code=lock_held"),
        "unexpected stdout: {stdout}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
}

#[test]
fn host_relay_group_mode_returns_non_zero_when_zero_hosted() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["a"]);

    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");
    let _alpha_lock = acquire_relay_runtime_lock(&alpha_paths).expect("acquire alpha lock");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--group",
            "dev",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux host relay --group dev");

    assert!(!output.status.success(), "command should fail");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("mode=bundle_group group=dev hosted=0 skipped=1 failed=0 hosted_any=false"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        stderr.contains("validation_no_hosted_bundles"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_group_all_selects_all_configured_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["a"]);
    write_bundle_configuration(&config_root, "bravo", Some(&["ops"]), &["b"]);

    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    let bravo_paths = BundleRuntimePaths::resolve(&state_root, "bravo").expect("bravo paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");
    ensure_bundle_runtime_directory(&bravo_paths).expect("ensure bravo runtime directory");
    let _alpha_lock = acquire_relay_runtime_lock(&alpha_paths).expect("acquire alpha lock");
    let _bravo_lock = acquire_relay_runtime_lock(&bravo_paths).expect("acquire bravo lock");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--group",
            "ALL",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux host relay --group ALL");

    assert!(!output.status.success(), "command should fail");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("mode=bundle_group group=ALL hosted=0 skipped=2 failed=0 hosted_any=false"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        stdout.contains("bundle=alpha outcome=skipped reason_code=lock_held"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        stdout.contains("bundle=bravo outcome=skipped reason_code=lock_held"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn host_relay_single_bundle_summary_json_omits_group_name() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["a"]);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "alpha",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux host relay alpha");

    let relay_socket = state_root.join("bundles").join("alpha").join("relay.sock");
    wait_for_relay_socket(&relay_socket);
    thread::sleep(Duration::from_millis(100));
    shutdown_relay_if_present(&state_root, "alpha");

    let output = child.wait_with_output().expect("wait for relay process");
    assert!(
        output.status.success(),
        "relay should exit cleanly, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{') && line.contains("\"host_mode\""))
        .expect("find startup summary json line");
    let payload: Value = serde_json::from_str(summary_line).expect("parse summary payload");
    let payload_object = payload.as_object().expect("summary payload object");
    assert_eq!(
        payload_object
            .get("host_mode")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "single_bundle"
    );
    assert!(
        !payload_object.contains_key("group_name"),
        "group_name should be omitted in single-bundle mode payload: {payload_object:?}"
    );
}

#[test]
fn send_rejects_missing_message_input() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo"])
        .stdin(Stdio::null())
        .output()
        .expect("run agentmux send without message");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_missing_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_conflicting_flag_and_piped_message_sources() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo", "--message", "hello"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin
            .write_all(b"hello from stdin")
            .expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_returns_canonical_json_payload() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "bravo"]);

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Look {
            schema_version: "1".to_string(),
            bundle_name: "agentmux".to_string(),
            requester_session: "tui".to_string(),
            target_session: "bravo".to_string(),
            captured_at: "2026-03-08T00:00:00Z".to_string(),
            snapshot_lines: vec!["LOOK-A".to_string(), "LOOK-B".to_string()],
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "look",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode look json payload");
    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle_name"], "agentmux");
    assert_eq!(payload["requester_session"], "tui");
    assert_eq!(payload["target_session"], "bravo");
    assert_eq!(payload["captured_at"], "2026-03-08T00:00:00Z");
    assert_eq!(
        payload["snapshot_lines"],
        serde_json::json!(["LOOK-A", "LOOK-B"])
    );

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "look");
    assert_eq!(requests[0]["requester_session"], "tui");
    assert_eq!(requests[0]["target_session"], "bravo");
}

#[test]
fn look_rejects_cross_bundle_request_in_mvp() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "look",
            "bravo",
            "--bundle",
            "other-bundle",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look --bundle");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_cross_bundle_unsupported"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_rejects_invalid_lines_bounds() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["look", "bravo", "--lines", "0"])
        .output()
        .expect("run agentmux look --lines 0");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_lines"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn unified_host_help_output_includes_relay_and_mcp_modes() {
    let relay = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--help"])
        .output()
        .expect("run agentmux host relay --help");
    assert!(relay.status.success(), "relay help should succeed");
    let relay_stdout = String::from_utf8_lossy(&relay.stdout);
    assert!(
        relay_stdout.contains("Usage: agentmux host relay"),
        "unexpected relay help output: {relay_stdout}"
    );
    assert!(
        relay_stdout.contains("--group GROUP"),
        "unexpected relay help output: {relay_stdout}"
    );

    let mcp = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "mcp", "--help"])
        .output()
        .expect("run agentmux host mcp --help");
    assert!(mcp.status.success(), "mcp help should succeed");
    let mcp_stdout = String::from_utf8_lossy(&mcp.stdout);
    assert!(
        mcp_stdout.contains("Usage: agentmux host mcp"),
        "unexpected mcp help output: {mcp_stdout}"
    );
}

fn write_bundle_configuration(
    config_root: &Path,
    bundle_name: &str,
    groups: Option<&[&str]>,
    sessions: &[&str],
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    let mut bundle = String::from("format-version = 1\n");
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

fn write_fake_tmux_script(path: &Path) {
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

fn shutdown_relay_if_present(state_root: &Path, bundle_name: &str) {
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

fn wait_for_relay_socket(socket_path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if socket_path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for relay socket {}",
        socket_path.display()
    );
}

fn spawn_fake_relay_once(
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
