use std::{path::PathBuf, time::Duration};

use agentmux::{
    relay::{
        ChatDeliveryMode, ChatOutcome, ChatStatus, RelayRequest, RelayResponse, handle_request,
    },
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    CoderSpec, SessionSpec, TmuxServerGuard, spawn_session, tmux_available, tmux_command,
    wait_for_pane_contains, write_bundle_configuration_members,
};

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &std::path::Path,
    bundle_name: &str,
    runtime_directory: &std::path::Path,
    tmux_socket: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(
        request,
        configuration_root,
        bundle_name,
        runtime_directory,
        tmux_socket,
    )
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
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'booting\\n'; sleep 0.2; printf 'READY>\\n'; exec sleep 45",
    );

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-ready".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: Some(2_000),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'idle\\n'; exec sleep 45",
    );

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-unready".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: Some(350),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

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

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-idle-match".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(70),
            quiescence_timeout_ms: Some(1_000),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

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

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-blank-line".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(70),
            quiescence_timeout_ms: Some(1_000),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

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

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-idle-mismatch".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(70),
            quiescence_timeout_ms: Some(450),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
