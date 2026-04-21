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
    wait_for_pane_contains, write_bundle_configuration, write_bundle_configuration_members,
};

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &std::path::Path,
    bundle_name: &str,
    runtime_directory: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_root, bundle_name, runtime_directory)
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
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "charlie", "exec sleep 45");

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-broadcast".to_string()),
            sender_session: "alpha".to_string(),
            message: "standup".to_string(),
            targets: Vec::new(),
            broadcast: true,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: Some(1_000),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "charlie",
        "while :; do date +%s%N; sleep 0.01; done",
    );

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-partial".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string(), "charlie".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(80),
            quiescence_timeout_ms: Some(350),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
fn relay_chat_times_out_when_activity_changes_despite_stable_visible_text() {
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
                prompt_regex: Some("(?m)^READY>$".to_string()),
                prompt_inspect_lines: Some(1),
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
        "while :; do printf '\\rREADY>'; sleep 0.02; done",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "READY>",
        Duration::from_millis(1_200),
    );
    let window_activity_probe = tmux_command(
        &paths.tmux_socket,
        &["display-message", "-p", "-t", "bravo", "#{window_activity}"],
    );
    if !window_activity_probe.status.success()
        || String::from_utf8_lossy(&window_activity_probe.stdout)
            .trim()
            .is_empty()
    {
        eprintln!("skipping activity test because tmux lacks #{{window_activity}}");
        let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
        return;
    }

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-stable-text-busy".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(1_200),
            quiescence_timeout_ms: Some(2_500),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
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
            .is_some_and(|reason| reason.contains("quiescence")),
        "expected quiescence timeout reason: {:?}",
        results[0].reason
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}
