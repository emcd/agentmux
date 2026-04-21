use std::time::Duration;

use agentmux::{
    relay::{
        ChatDeliveryMode, ChatOutcome, ChatStatus, RelayRequest, RelayResponse, handle_request,
    },
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    TmuxServerGuard, capture_pane, spawn_session, tmux_available, tmux_command,
    wait_for_pane_contains, write_bundle_configuration,
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
fn relay_chat_async_processes_repeated_target_messages_in_fifo_order() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");

    let first_marker = "FIFO-ONE-MARKER";
    let second_marker = "FIFO-TWO-MARKER";

    let first = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-fifo-1".to_string()),
            sender_session: "alpha".to_string(),
            message: first_marker.to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: Some(70),
            quiescence_timeout_ms: None,
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("first async send should be accepted");
    let RelayResponse::Chat {
        status: first_status,
        results: first_results,
        ..
    } = first
    else {
        panic!("expected chat response");
    };
    assert_eq!(first_status, ChatStatus::Accepted);
    assert_eq!(first_results.len(), 1);
    assert_eq!(first_results[0].outcome, ChatOutcome::Queued);

    let second = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-fifo-2".to_string()),
            sender_session: "alpha".to_string(),
            message: second_marker.to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: Some(70),
            quiescence_timeout_ms: None,
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("second async send should be accepted");
    let RelayResponse::Chat {
        status: second_status,
        results: second_results,
        ..
    } = second
    else {
        panic!("expected chat response");
    };
    assert_eq!(second_status, ChatStatus::Accepted);
    assert_eq!(second_results.len(), 1);
    assert_eq!(second_results[0].outcome, ChatOutcome::Queued);

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        first_marker,
        Duration::from_millis(2_000),
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        second_marker,
        Duration::from_millis(2_000),
    );

    let snapshot = capture_pane(&paths.tmux_socket, "bravo", "-200");
    let first_index = snapshot
        .find(first_marker)
        .expect("first marker should exist in pane");
    let second_index = snapshot
        .find(second_marker)
        .expect("second marker should exist in pane");
    assert!(
        first_index < second_index,
        "expected FIFO marker order, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_async_without_timeout_waits_for_late_quiescence() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "i=0; while [ \"$i\" -lt 30 ]; do printf '\\rWORK-%02d' \"$i\"; i=$((i+1)); sleep 0.02; done; printf '\\nIDLE\\n'; exec sleep 45",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "WORK-",
        Duration::from_millis(1_200),
    );

    let marker = "ASYNC-LATE-QUIESCENCE-MARKER";
    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-async-default".to_string()),
            sender_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: Some(120),
            quiescence_timeout_ms: None,
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Accepted);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Queued);

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        marker,
        Duration::from_millis(3_000),
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_chat_async_timeout_override_stops_wait_before_late_quiescence() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "i=0; while [ \"$i\" -lt 80 ]; do printf '\\rWORK-%02d' \"$i\"; i=$((i+1)); sleep 0.02; done; printf '\\nIDLE\\n'; exec sleep 45",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "WORK-",
        Duration::from_millis(1_200),
    );

    let marker = "ASYNC-TIMEOUT-OVERRIDE-MARKER";
    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-async-timeout".to_string()),
            sender_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: Some(120),
            quiescence_timeout_ms: Some(350),
            acp_turn_timeout_ms: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");

    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Accepted);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Queued);

    std::thread::sleep(Duration::from_millis(2_100));
    let snapshot = capture_pane(&paths.tmux_socket, "bravo", "-200");
    assert!(
        !snapshot.contains(marker),
        "marker should not be delivered after async timeout override, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}
