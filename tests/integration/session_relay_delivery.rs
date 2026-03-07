use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command as StdCommand,
    time::{Duration, Instant},
};

use tempfile::TempDir;
use tmuxmux::{
    relay::{ChatOutcome, ChatStatus, RelayRequest, RelayResponse, handle_request},
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tokio::{
    io::AsyncBufReadExt,
    process::{Child, Command},
    time::sleep,
};

fn tmux_available() -> bool {
    StdCommand::new("tmux")
        .arg("-V")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn tmux_command(socket: &Path, arguments: &[&str]) -> std::process::Output {
    StdCommand::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(arguments)
        .output()
        .expect("run tmux command")
}

fn wait_for_pane_contains(socket: &Path, target: &str, needle: &str, timeout: Duration) {
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

fn write_bundle_configuration(root: &Path, bundle_name: &str, sessions: &[&str]) -> PathBuf {
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
struct CoderSpec {
    id: String,
    initial_command: String,
    resume_command: String,
    prompt_regex: Option<String>,
    prompt_inspect_lines: Option<usize>,
    prompt_idle_column: Option<usize>,
}

#[derive(Clone)]
struct SessionSpec {
    id: String,
    name: Option<String>,
    directory: PathBuf,
    coder: String,
    coder_session_id: Option<String>,
}

fn write_bundle_configuration_members(
    root: &Path,
    bundle_name: &str,
    coders: &[CoderSpec],
    sessions: &[SessionSpec],
) -> PathBuf {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    let mut coders_toml = String::from("format-version = 1\n");
    for coder in coders {
        coders_toml.push_str(
            format!(
                "\n[[coders]]\nid = \"{}\"\ninitial-command = \"{}\"\nresume-command = \"{}\"\n",
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

    let mut bundle_toml = String::from("format-version = 1\n");
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
    config_root
}

fn spawn_session(socket: &Path, session_name: &str, shell_command: &str) {
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

#[test]
fn relay_chat_broadcast_delivers_to_all_other_configured_sessions() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(
        temporary.path(),
        bundle_name,
        &["alpha", "bravo", "charlie"],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "charlie", "exec sleep 45");

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-broadcast".to_string()),
            sender_session: "alpha".to_string(),
            message: "standup".to_string(),
            targets: Vec::new(),
            broadcast: true,
            quiet_window_ms: Some(50),
            delivery_timeout_ms: Some(1_000),
        },
        &config_root,
        bundle_name,
        &paths.tmux_socket,
    )
    .expect("broadcast should succeed");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };

    assert_eq!(status, ChatStatus::Success);
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|result| result.target_session != "alpha")
    );
    for result in results {
        assert_eq!(result.outcome, ChatOutcome::Delivered);
    }

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_reports_timeout_for_noisy_target_with_partial_status() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(
        temporary.path(),
        bundle_name,
        &["alpha", "bravo", "charlie"],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "charlie",
        "while :; do date +%s%N; sleep 0.01; done",
    );

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-partial".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string(), "charlie".to_string()],
            broadcast: false,
            quiet_window_ms: Some(80),
            delivery_timeout_ms: Some(350),
        },
        &config_root,
        bundle_name,
        &paths.tmux_socket,
    )
    .expect("targeted chat should return results");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };

    assert_eq!(status, ChatStatus::Partial);
    let bravo = results
        .iter()
        .find(|result| result.target_session == "bravo")
        .expect("bravo result");
    assert_eq!(bravo.outcome, ChatOutcome::Delivered);
    let charlie = results
        .iter()
        .find(|result| result.target_session == "charlie")
        .expect("charlie result");
    assert_eq!(charlie.outcome, ChatOutcome::Timeout);
    assert!(
        charlie
            .reason
            .as_ref()
            .is_some_and(|reason| reason.contains("timed out")),
        "timeout reason should describe quiescence timeout: {:?}",
        charlie.reason
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_delivers_when_prompt_readiness_template_matches() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("READY>".to_string()),
                prompt_inspect_lines: Some(8),
                prompt_idle_column: None,
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'booting\\n'; sleep 0.2; printf 'READY>\\n'; exec sleep 45",
    );

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-ready".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            delivery_timeout_ms: Some(2_000),
        },
        &config_root,
        bundle_name,
        &paths.tmux_socket,
    )
    .expect("delivery should complete");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };

    assert_eq!(status, ChatStatus::Success);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_times_out_when_prompt_readiness_never_matches() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("^›".to_string()),
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'idle\\n'; exec sleep 45",
    );

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-unready".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            delivery_timeout_ms: Some(350),
        },
        &config_root,
        bundle_name,
        &paths.tmux_socket,
    )
    .expect("delivery should complete");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };

    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Timeout);
    assert!(
        results[0]
            .reason
            .as_ref()
            .is_some_and(|reason| reason.contains("prompt readiness")),
        "expected prompt readiness timeout reason: {:?}",
        results[0].reason
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_delivers_when_prompt_idle_column_matches() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("(?m)^READY>".to_string()),
                prompt_inspect_lines: Some(3),
                prompt_idle_column: Some(6),
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "PS1='READY>'; export PS1; exec bash --noprofile --norc -i",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "READY>",
        Duration::from_millis(1_200),
    );

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-idle-match".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            delivery_timeout_ms: Some(1_000),
        },
        &config_root,
        bundle_name,
        &paths.tmux_socket,
    )
    .expect("delivery should complete");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_delivers_when_prompt_regex_requires_blank_separator_line() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("(?ms)^READY>.*\\n\\nstatus.*$".to_string()),
                prompt_inspect_lines: Some(3),
                prompt_idle_column: None,
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "PS1='READY>\\n\\nstatus '; export PS1; exec bash --noprofile --norc -i",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "status",
        Duration::from_millis(1_200),
    );

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-blank-line".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            delivery_timeout_ms: Some(1_000),
        },
        &config_root,
        bundle_name,
        &paths.tmux_socket,
    )
    .expect("delivery should complete");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_times_out_when_prompt_idle_column_does_not_match() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("(?m)^READY>".to_string()),
                prompt_inspect_lines: Some(3),
                prompt_idle_column: Some(6),
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "PS1='READY>'; export PS1; exec bash --noprofile --norc -i",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "READY>",
        Duration::from_millis(1_200),
    );
    let typed = tmux_command(
        &paths.tmux_socket,
        &["send-keys", "-t", "bravo", "--", "echo hi"],
    );
    assert!(
        typed.status.success(),
        "failed to prefill prompt input: {}",
        String::from_utf8_lossy(&typed.stderr)
    );

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-idle-mismatch".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            delivery_timeout_ms: Some(450),
        },
        &config_root,
        bundle_name,
        &paths.tmux_socket,
    )
    .expect("delivery should complete");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Timeout);
    assert!(
        results[0]
            .reason
            .as_ref()
            .is_some_and(|reason| reason.contains("prompt readiness")),
        "expected prompt readiness mismatch: {:?}",
        results[0].reason
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_startup_retries_transient_tmux_create_failures() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");

    let started = Instant::now();
    let mut child =
        spawn_relay_with_fake_tmux(bundle_name, &config_root, &state_root, &fake_tmux_script);
    wait_for_relay_socket(&relay_socket).await;
    let elapsed = started.elapsed();

    let stdout = drain_child_stdout(&mut child).await;
    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    assert!(
        stdout.contains("tmuxmux-relay listening"),
        "relay should report successful startup, stdout={stdout:?}"
    );
    let attempts = fs::read_to_string(&attempts_file)
        .expect("read attempts")
        .trim()
        .parse::<usize>()
        .expect("parse attempts");
    assert_eq!(attempts, 3, "relay should retry transient create failures");
    assert!(
        elapsed >= Duration::from_millis(50),
        "retry delays should be observable, elapsed={elapsed:?}"
    );
}

fn write_fake_tmux_script(script_path: &Path, attempts_file: &Path, log_file: &Path) {
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
      echo "no server running on /tmp/tmuxmux-fake" >&2
      exit 1
    fi
    format="${{args[2]-}}"
    while IFS= read -r session; do
      [[ -z "${{session}}" ]] && continue
      if [[ "${{format}}" == "#{{session_name}}\t#{{@tmuxmux_owned}}" ]]; then
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

fn spawn_relay_with_fake_tmux(
    bundle_name: &str,
    config_root: &Path,
    state_root: &Path,
    fake_tmux_script: &Path,
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tmuxmux-relay"));
    command
        .arg("--bundle")
        .arg(bundle_name)
        .arg("--config-directory")
        .arg(config_root)
        .arg("--state-directory")
        .arg(state_root)
        .env("TMUXMUX_TMUX_COMMAND", fake_tmux_script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command.spawn().expect("spawn relay")
}

async fn wait_for_relay_socket(socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if socket.exists() {
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for relay socket {}", socket.display());
}

async fn drain_child_stdout(child: &mut Child) -> String {
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
